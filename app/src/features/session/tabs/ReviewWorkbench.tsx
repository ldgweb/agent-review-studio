import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useOutletContext, useSearchParams } from 'react-router-dom';
import type { SessionContext } from '../SessionLayout';
import type {
  AgentSession,
  FileApproval,
  FileChange,
  FindingReviewState,
  LineComment,
  ReviewerFinding,
} from '../../../types/domain';
import { DEFAULT_DIFF_FONT, DiffView, type DiffViewMode } from '../../../components/DiffView';
import { CorrectionExport } from '../../../components/CorrectionExport';
import { StatusPill } from '../../../components/ui';
import {
  IconCheck,
  IconComment,
  IconGitBranch,
  IconRotateCcw,
  IconSparkle,
  IconX,
} from '../../../components/icons';
import { apiFetch, errorMessage } from '../../../lib/api';
import { openExternal } from '../../../lib/desktop';
import { statusMeta } from '../../../lib/risk';
import { computeDecision, DRAFT_NOTE_KEY, useApproval } from '../../../state/approvalStore';
import { groupFiles, riskCoverage, sortFindings, type GroupMode } from '../../../lib/triage';
import { unverifiedFileIds } from '../../../lib/evidence';
import { FileNavigator } from '../review/FileNavigator';
import { ColResizer } from '../review/ColResizer';
import { ContextRail, RailStrip, RAIL_TABS, type RailTab } from '../review/ContextRail';
import { DiffBar } from '../review/DiffBar';
import { NoticeLine, type Notice } from '../review/NoticeLine';
import { FindingCard } from '../review/FindingsPanel';
import { ShortcutOverlay } from '../review/ShortcutOverlay';
import { useReviewShortcuts } from '../review/useReviewShortcuts';

// The review workbench — the code owns the window. Chrome is docked:
// 40px topbar (route-aware, in App), 36px diff bar, 52px decision bar;
// the diff is the only vertical scroller in the center. Left rail answers
// "what should I look at first?" (risk-first triage), the context rail
// answers everything else (findings, git, session read, timeline). One
// notice at a time, never a banner stack. Fully keyboard-drivable;
// z = zen (code only).
//
// The review is a guided flow — Understand → Review → Decide → Apply —
// surfaced by the stage stepper on the decision bar and ONE primary action
// that follows the stage: walk the pending files, then finalize, then the
// outcome's consequence (commit approved / export corrections / revert).

/** Where the reviewer is in the natural flow of one session. */
type FlowStage = 'understand' | 'review' | 'decide' | 'apply';

const FLOW_STAGES: Array<{ key: FlowStage; label: string; hint: string }> = [
  { key: 'understand', label: 'Understand', hint: 'What the agent was asked, planned and reports — the Session tab' },
  { key: 'review', label: 'Review', hint: 'Walk the files riskiest-first and give each a verdict (a / c / r)' },
  { key: 'decide', label: 'Decide', hint: 'Every file has a verdict — finalize to record the outcome' },
  { key: 'apply', label: 'Apply', hint: 'Make it count: commit approved files, export corrections, or revert' },
];

function FlowSteps({ stage, onStep }: { stage: FlowStage; onStep: (s: FlowStage) => void }) {
  const idx = FLOW_STAGES.findIndex((s) => s.key === stage);
  return (
    <div className="flow-steps" aria-label="Review flow">
      {FLOW_STAGES.map((s, i) => (
        <button
          key={s.key}
          className={`flow-step${i === idx ? ' current' : i < idx ? ' done' : ''}`}
          title={s.hint}
          onClick={() => onStep(s.key)}
        >
          <span className="flow-step-n">{i < idx ? '✓' : i + 1}</span>
          <span className="flow-step-label">{s.label}</span>
        </button>
      ))}
    </div>
  );
}

const RAIL_PREF_KEY = 'ars:context-rail';

/** Debounce for note autosave — each save POSTs the session's full state */
const NOTE_SAVE_DELAY_MS = 500;

/** A note textarea that keeps its text locally and saves it debounced, on
 * blur and on unmount — never one full-state POST per keystroke. Picks up
 * the stored value whenever no local edit is waiting. */
function NoteField({
  value,
  onSave,
  focusToken = 0,
  onFocused,
  ...rest
}: {
  value: string;
  onSave: (text: string) => void;
  /** Non-zero: focus the field (once per token) */
  focusToken?: number;
  onFocused?: () => void;
  className: string;
  rows: number;
  placeholder?: string;
  'aria-label': string;
}) {
  const [text, setText] = useState(value);
  const dirty = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const textRef = useRef(text);
  textRef.current = text;
  const onSaveRef = useRef(onSave);
  onSaveRef.current = onSave;
  const ref = useRef<HTMLTextAreaElement>(null);

  const flush = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    if (!dirty.current) return;
    dirty.current = false;
    onSaveRef.current(textRef.current);
  }, []);

  useEffect(() => {
    if (!dirty.current) setText(value);
  }, [value]);

  useEffect(() => flush, [flush]);

  useEffect(() => {
    if (!focusToken) return;
    ref.current?.focus();
    onFocused?.();
    // Once per token: onFocused is the owner's "request handled" callback
  }, [focusToken]);

  return (
    <textarea
      {...rest}
      ref={ref}
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        dirty.current = true;
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(flush, NOTE_SAVE_DELAY_MS);
      }}
      onBlur={flush}
    />
  );
}

const NO_COMMENTS: LineComment[] = [];

export function ReviewWorkbench() {
  const { session, refreshDetail } = useOutletContext<SessionContext>();
  // Remount per session: filter, zen and notice state must not leak when
  // navigating between sessions (e.g. via the inbox)
  return <WorkbenchInner key={session.sessionId} session={session} refreshDetail={refreshDetail} />;
}

function WorkbenchInner({ session, refreshDetail }: { session: AgentSession; refreshDetail: () => void }) {
  const [params, setParams] = useSearchParams();
  const approval = useApproval(session.sessionId, session.reviewRevision, session.contentHash);
  const { state } = approval;
  const [query, setQuery] = useState('');
  const [onlyPending, setOnlyPending] = useState(false);
  const [railOpen, setRailOpen] = useState(() => localStorage.getItem(RAIL_PREF_KEY) !== 'closed');
  const [zen, setZen] = useState(false);
  const [overlayOpen, setOverlayOpen] = useState(false);
  const [reopened, setReopened] = useState(false);
  const [draftOpen, setDraftOpen] = useState(false);
  const [gitBusy, setGitBusy] = useState(false);
  const [diffFont, setDiffFont] = useState(() => {
    const stored = Number(localStorage.getItem('ars:diff-font'));
    return Number.isFinite(stored) && stored >= 10 && stored <= 22 ? stored : DEFAULT_DIFF_FONT;
  });
  const changeDiffFont = (delta: number) =>
    setDiffFont((prev) => {
      const next = Math.min(22, Math.max(10, prev + delta));
      localStorage.setItem('ars:diff-font', String(next));
      return next;
    });
  const resetDiffFont = () => {
    localStorage.removeItem('ars:diff-font');
    setDiffFont(DEFAULT_DIFF_FONT);
  };
  const [diffWrap, setDiffWrap] = useState(() => localStorage.getItem('ars:diff-wrap') !== 'off');
  const toggleDiffWrap = () =>
    setDiffWrap((prev) => {
      const next = !prev;
      localStorage.setItem('ars:diff-wrap', next ? 'on' : 'off');
      return next;
    });
  const [diffMode, setDiffMode] = useState<DiffViewMode>(() =>
    localStorage.getItem('ars:diff-view') === 'split' ? 'split' : 'unified',
  );
  const toggleDiffMode = () =>
    setDiffMode((prev) => {
      const next: DiffViewMode = prev === 'split' ? 'unified' : 'split';
      localStorage.setItem('ars:diff-view', next);
      return next;
    });
  const [gitNotice, setGitNotice] = useState<Notice | null>(null);
  /** Pending "focus the note for this file" request (the c shortcut) */
  const [noteFocus, setNoteFocus] = useState<{ fileId: string; token: number } | null>(null);
  /** Last line the reviewer jumped to or clicked — survives ?line= being cleared */
  const [lastFocusedLine, setLastFocusedLine] = useState<number | undefined>(undefined);
  const filterRef = useRef<HTMLInputElement>(null);

  const files = session.files;

  const rawGroup = params.get('group');
  // Risk-first is the default review order: needsAttention() and fileWeight()
  // run on heuristic risk flags, critical areas and the "changed but never
  // re-tested" signal.
  const groupMode: GroupMode =
    rawGroup === 'dir' || rawGroup === 'risk' || rawGroup === 'area' ? rawGroup : 'risk';

  const rawRail = params.get('rail');
  // Default to the Session tab: the agent's story (prompt, plan, self-report)
  // is where a review naturally starts — the code comes second.
  const railTab: RailTab = (RAIL_TABS as string[]).includes(rawRail ?? '') ? (rawRail as RailTab) : 'session';

  const selectedId = params.get('file') ?? files[0]?.id;
  const selected = files.find((f) => f.id === selectedId) ?? files[0];
  const lineParam = params.get('line');
  const focusLine = lineParam ? Number(lineParam) : undefined;

  // Merge into the existing search params — a plain setParams({file}) would
  // wipe ?group=, ?rail= and friends.
  const mergeParams = (mutate: (p: URLSearchParams) => void) => {
    const next = new URLSearchParams(params);
    mutate(next);
    setParams(next);
  };
  const selectFile = (id: string) => {
    if (id !== selected?.id) setLastFocusedLine(undefined);
    mergeParams((p) => {
      p.set('file', id);
      p.delete('line');
    });
  };
  const setGroupMode = (mode: GroupMode) => mergeParams((p) => p.set('group', mode));
  const setRailTab = (tab: RailTab) => {
    setRailOpen(true);
    mergeParams((p) => p.set('rail', tab));
  };
  const jumpToFile = (fileId: string, line?: number) => {
    setLastFocusedLine(line);
    mergeParams((p) => {
      p.set('file', fileId);
      if (line) p.set('line', String(line));
      else p.delete('line');
    });
  };
  const clearFocusLine = () => mergeParams((p) => p.delete('line'));
  // Deep links (?line= in the URL on load) count as a jump too
  useEffect(() => {
    if (focusLine) setLastFocusedLine(focusLine);
  }, [focusLine]);

  const approvalOf = (fileId: string): FileApproval => state.fileStates[fileId] ?? 'pending';
  /** Verdict the core has confirmed at this revision (commit / revert check this) */
  const savedApprovalOf = (fileId: string): FileApproval => approval.saved.fileStates[fileId] ?? 'pending';
  const commentsOf = (fileId: string) => state.lineComments[fileId] ?? NO_COMMENTS;

  // Stable callbacks (with DiffView's memoized hunks) keep unrelated renders —
  // e.g. typing in the file filter — from re-tokenizing the whole diff.
  const approvalRef = useRef(approval);
  approvalRef.current = approval;
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  const setFindingState = useCallback(
    (findingId: string, st?: FindingReviewState) => approvalRef.current.setFindingState(findingId, st),
    [],
  );
  const saveComment = useCallback((c: LineComment) => {
    const sel = selectedRef.current;
    if (!sel) return;
    if (c.line) setLastFocusedLine(c.line);
    void approvalRef.current.setLineComment(sel.id, c);
  }, []);
  const deleteComment = useCallback((anchor: string) => {
    const sel = selectedRef.current;
    if (sel) void approvalRef.current.removeLineComment(sel.id, anchor);
  }, []);
  // Working-tree lines for "expand context" — only meaningful when the hunks
  // themselves come from the working tree
  const loadFileLines = useCallback(async (): Promise<string[] | null> => {
    const sel = selectedRef.current;
    if (!sel) return null;
    try {
      const res = await apiFetch<{ lines?: string[] }>('/api/git/file', {
        method: 'POST',
        body: { sessionId: session.sessionId, path: sel.path },
      });
      return res.lines ?? null;
    } catch {
      // DiffView turns null into "context unavailable" (expanders disappear)
      return null;
    }
  }, [session.sessionId]);

  const outcome = useMemo(
    () => computeDecision(files.map((f) => f.id), state),
    [files, state],
  );
  const coverage = useMemo(() => riskCoverage(files, session, state.fileStates), [files, session, state.fileStates]);
  const unverified = useMemo(() => unverifiedFileIds(session), [session]);

  // Commit only what is approved on screen AND saved at this revision
  const approvedPaths = files
    .filter((f) => approvalOf(f.id) === 'approved' && savedApprovalOf(f.id) === 'approved')
    .map((f) => f.path);

  // Where the reviewer stands in the flow — drives the stepper and the ONE
  // primary action on the decision bar.
  const decidedCount = files.length - outcome.pending;
  const stage: FlowStage = state.sessionDecision
    ? 'apply'
    : outcome.pending === 0
      ? 'decide'
      : decidedCount > 0
        ? 'review'
        : 'understand';

  const visible = useMemo(
    () =>
      files.filter(
        (f) =>
          f.path.toLowerCase().includes(query.toLowerCase()) &&
          (!onlyPending || (state.fileStates[f.id] ?? 'pending') === 'pending'),
      ),
    [files, query, onlyPending, state.fileStates],
  );
  const groups = useMemo(
    () => groupFiles(visible, session, groupMode, unverified),
    [visible, session, groupMode, unverified],
  );
  // Keyboard order = visual order: j/k walk the grouped list as rendered
  const flatOrder = useMemo(() => groups.flatMap((g) => g.files), [groups]);


  // A recorded decision is cleared by the store on any later change — make
  // that visible instead of silent.
  const prevDecision = useRef(state.sessionDecision);
  useEffect(() => {
    if (prevDecision.current && !state.sessionDecision) setReopened(true);
    if (state.sessionDecision) setReopened(false);
    prevDecision.current = state.sessionDecision;
  }, [state.sessionDecision]);

  const nextUndecidedFrom = (after: string): FileChange | undefined => {
    const idx = flatOrder.findIndex((f) => f.id === after);
    const ordered = idx === -1 ? flatOrder : [...flatOrder.slice(idx + 1), ...flatOrder.slice(0, idx)];
    return ordered.find((f) => approvalOf(f.id) === 'pending');
  };

  const decide = (fileId: string, decision: FileApproval) => {
    const current = approvalOf(fileId);
    const next = current === decision ? 'pending' : decision;
    void approval.setFiles([fileId], next);
    // Natural flow: after a decision, move to the next file still waiting
    if (next === 'approved' || next === 'rejected') {
      const target = nextUndecidedFrom(fileId);
      if (target) selectFile(target.id);
    }
  };

  const stepFile = (delta: number) => {
    if (flatOrder.length === 0) return;
    const idx = flatOrder.findIndex((f) => f.id === selected?.id);
    const next = flatOrder[(idx + delta + flatOrder.length) % flatOrder.length];
    if (next) selectFile(next.id);
  };

  const toggleRail = () => {
    setRailOpen((prev) => {
      const next = !prev;
      localStorage.setItem(RAIL_PREF_KEY, next ? 'open' : 'closed');
      return next;
    });
  };

  // Jump into the editor the code actually lives in (Cursor for Cursor
  // sessions, VS Code otherwise) at the focused line.
  const openInEditor = () => {
    if (!selected || !session.projectPath) return;
    const abs = `${session.projectPath}/${selected.path}`.replace(/\\/g, '/');
    const scheme = session.agent === 'cursor' ? 'cursor' : 'vscode';
    openExternal(`${scheme}://file/${abs}${lastFocusedLine ? `:${lastFocusedLine}` : ''}`).catch((e: unknown) =>
      setGitNotice({
        key: `editor-err-${Date.now()}`,
        tone: 'warn',
        icon: <IconGitBranch size={13} />,
        text: `Could not open the editor: ${errorMessage(e)}`,
        onDismiss: () => setGitNotice(null),
      }),
    );
  };

  const onFinalize = () => {
    if (!outcome.decision) return;
    // Repo policy gate: an approving finalize is blocked when a risk score
    // meets its .ars/policy.json threshold (reject/corrections still allowed)
    const thresholds = session.policy?.blockFinalizeAboveRisk;
    if (thresholds && (outcome.decision === 'approved' || outcome.decision === 'partially-approved')) {
      const violation = session.risks.find((r) => {
        const limit = thresholds[r.dimension];
        return limit !== undefined && r.score >= limit;
      });
      if (violation) {
        window.alert(
          `Blocked by .ars/policy.json: ${violation.dimension} risk is ${violation.score} (threshold ${thresholds[violation.dimension]}). ` +
            'Request corrections or reject instead.',
        );
        return;
      }
    }
    if (outcome.pending > 0) {
      const label = statusMeta[outcome.decision].label;
      const ok = window.confirm(
        `${outcome.pending} file${outcome.pending > 1 ? 's are' : ' is'} still undecided — finalize as “${label}”?`,
      );
      if (!ok) return;
    }
    void approval.finalize(outcome.decision);
  };

  // The guided flow: jump to whatever the next natural act is.
  const goNextPending = () => {
    if (!selected) return;
    if (approvalOf(selected.id) === 'pending') return; // already on it — decide here
    const target = nextUndecidedFrom(selected.id);
    if (target) selectFile(target.id);
  };
  const goToStage = (target: FlowStage) => {
    if (target === 'understand') setRailTab('session');
    else if (target === 'review') goNextPending();
    else if (target === 'decide' && outcome.pending === 0 && !state.sessionDecision) onFinalize();
    else if (target === 'apply' && state.sessionDecision) {
      if (outcome.decision === 'corrections-requested') setDraftOpen(true);
      else setRailTab('git');
    }
  };

  useReviewShortcuts({
    nextFile: () => stepFile(1),
    prevFile: () => stepFile(-1),
    nextUndecided: () => {
      if (!selected) return;
      const target = nextUndecidedFrom(selected.id);
      if (target) selectFile(target.id);
    },
    approve: () => selected && decide(selected.id, 'approved'),
    reject: () => selected && decide(selected.id, 'rejected'),
    requestChanges: () => {
      if (!selected) return;
      if (approvalOf(selected.id) !== 'correction-requested') {
        void approval.setFiles([selected.id], 'correction-requested');
      }
      // The note field focuses itself once it is rendered for this file
      setNoteFocus((prev) => ({ fileId: selected.id, token: (prev?.token ?? 0) + 1 }));
    },
    toggleRail,
    toggleZen: () => setZen((v) => !v),
    fontUp: () => changeDiffFont(1),
    fontDown: () => changeDiffFont(-1),
    focusFilter: () => filterRef.current?.focus(),
    toggleOverlay: () => setOverlayOpen((v) => !v),
    closeOverlay: () => setOverlayOpen(false),
    suspended: overlayOpen,
  });

  const findingsFor = (file: FileChange | undefined) =>
    file ? sortFindings(session.findings.filter((f) => f.filePath === file.path)) : [];

  // Line-anchored findings render inline in the diff; the rest fold into a
  // compact row above it so the code stays above the fold.
  const selectedFindings = findingsFor(selected);
  const inlineableLines = useMemo(() => {
    const set = new Set<number>();
    for (const h of selected?.hunks ?? []) {
      for (const l of h.lines) if (l.newLine != null) set.add(l.newLine);
    }
    return set;
  }, [selected]);
  const unanchoredFindings = selectedFindings.filter((f) => f.line == null || !inlineableLines.has(f.line));
  const lineAnnotations = useMemo(() => {
    const byLine = new Map<number, ReviewerFinding[]>();
    for (const f of sortFindings(session.findings)) {
      if (f.filePath !== selected?.path || f.line == null || !inlineableLines.has(f.line)) continue;
      byLine.set(f.line, [...(byLine.get(f.line) ?? []), f]);
    }
    const map = new Map<number, ReactNode>();
    for (const [line, group] of byLine) {
      map.set(
        line,
        <div className="stack" style={{ gap: 'var(--sp-2)' }}>
          {group.map((f) => (
            <FindingCard key={f.id} compact finding={f} state={state.findingStates[f.id]} onSetState={setFindingState} />
          ))}
        </div>,
      );
    }
    return map;
  }, [session.findings, selected, inlineableLines, state.findingStates, setFindingState]);

  // Correction request draft, assembled from everything the reviewer flagged
  const draftLines: string[] = [];
  for (const f of files) {
    for (const c of commentsOf(f.id)) draftLines.push(`• ${f.path}${c.line ? `:${c.line}` : ''} — ${c.text}`);
    if (approvalOf(f.id) === 'correction-requested') {
      const note = state.correctionNotes[f.id]?.trim();
      draftLines.push(`• ${f.path} — ${note || 'please revise this change'}`);
    }
  }
  for (const f of sortFindings(session.findings)) {
    if (state.findingStates[f.id] === 'correction') {
      draftLines.push(
        `• ${f.filePath ?? 'session'}${f.line ? `:${f.line}` : ''} — [${f.kind}] ${f.title}${f.suggestedFix ? ` — suggested: ${f.suggestedFix}` : ''}`,
      );
    }
  }
  const totalComments = files.reduce((sum, f) => sum + commentsOf(f.id).length, 0);
  const generatedDraft = `Please address the following before resubmitting:\n${draftLines.join('\n')}`;
  // An empty stored draft means "follow the flagged items"
  const editedDraft = state.correctionNotes[DRAFT_NOTE_KEY] || undefined;
  const draftText = editedDraft ?? generatedDraft;

  if (files.length === 0) {
    return (
      <main className="page">
        <div className="banner banner-info"><span>ℹ</span><span>This session changed no files.</span></div>
      </main>
    );
  }

  // Git actions — the review's consequence in the repo. Revert runs only
  // after the reject verdict is SAVED at this revision (the core re-checks).
  const doRevert = async () => {
    if (!selected) return;
    const target = selected;
    const ok = window.confirm(
      `Revert ${target.path} to HEAD?\nA backup patch is saved to ~/.agent-review-studio/patches first.`,
    );
    if (!ok) return;
    const notify = (tone: Notice['tone'], text: string) =>
      setGitNotice({
        key: `git-revert-${Date.now()}`,
        tone,
        icon: <IconGitBranch size={13} />,
        text,
        onDismiss: () => setGitNotice(null),
      });
    setGitBusy(true);
    try {
      if (savedApprovalOf(target.id) !== 'rejected') {
        const saved = await approval.retrySave();
        if (!saved.ok) {
          notify('warn', `Nothing reverted: the reject decision could not be saved (${saved.error}).`);
          return;
        }
      }
      const res = await apiFetch<{ ok?: boolean; patch?: string; error?: string }>('/api/git/revert', {
        method: 'POST',
        body: { sessionId: session.sessionId, reviewRevision: session.reviewRevision, paths: [target.path] },
        // A partial failure still reports where the backup patch went
        allowError: true,
      });
      if (res.error || !res.ok) {
        notify('warn', `${res.error ?? 'Revert failed.'}${res.patch ? ` — backup: ${res.patch}` : ''}`);
        return;
      }
      notify('info', `Reverted ${target.path} — backup: ${res.patch}`);
      refreshDetail();
    } catch (e) {
      notify('warn', errorMessage(e));
    } finally {
      setGitBusy(false);
    }
  };

  // One notice at a time, priority: git outcome > reopened
  const notices: Notice[] = [];
  if (gitNotice) notices.push(gitNotice);
  if (reopened) {
    notices.push({
      key: 'reopened',
      tone: 'warn',
      icon: <IconRotateCcw size={13} />,
      text: 'Review reopened — the recorded decision was cleared because the review changed. Finalize again when done.',
      onDismiss: () => setReopened(false),
    });
  }

  return (
    <div className={`workbench${zen ? ' zen' : ''}`}>
      <FileNavigator
        session={session}
        groups={groups}
        groupMode={groupMode}
        onGroupMode={setGroupMode}
        selectedId={selected?.id}
        onSelect={selectFile}
        approvalOf={approvalOf}
        commentsCountOf={(id) => commentsOf(id).length}
        coverage={coverage}
        query={query}
        onQuery={setQuery}
        onlyPending={onlyPending}
        onOnlyPending={setOnlyPending}
        pendingCount={outcome.pending}
        unverifiedIds={unverified}
        filterRef={filterRef}
      />

      <ColResizer varNames={['--nav-w']} storageKey="ars:nav-width" min={200} max={560} edge="right" />

      {/* The code column: status banners + notice → diff bar → diff → drawer / note strip → decision bar */}
      <section className="wb-main">
        {approval.saveError && (
          <div className="banner banner-err" role="alert">
            <span style={{ flex: 1 }}>
              Review not saved: {approval.saveError} — the decisions on screen are not recorded yet; commit uses
              saved approvals only.
            </span>
            <button className="btn btn-sm" disabled={approval.pending > 0} onClick={() => void approval.retrySave()}>
              Retry save
            </button>
          </div>
        )}
        {!approval.saveError && approval.error && <div className="banner banner-err" role="alert">{approval.error}</div>}
        {approval.stale && <div className="banner banner-warn" role="status">The session or project files changed. Previous decisions no longer apply; review this version again.</div>}
        {approval.pending > 0 && <div className="faint" role="status">Saving review…</div>}
        <NoticeLine notices={notices} />

        {selected && (
          <DiffBar
            file={selected}
            onPrev={() => stepFile(-1)}
            onNext={() => stepFile(1)}
            zen={zen}
            onToggleZen={() => setZen((v) => !v)}
            gitRepo={session.gitRepo}
            onOpenInEditor={session.projectPath ? openInEditor : undefined}
            fontSize={diffFont}
            onFontSize={changeDiffFont}
            onFontSizeReset={resetDiffFont}
            wrap={diffWrap}
            onToggleWrap={toggleDiffWrap}
            viewMode={diffMode}
            onToggleView={toggleDiffMode}
          />
        )}

        <div className="diff-scroll">
          {unanchoredFindings.length > 0 && (
            <details className="card" style={{ padding: 'var(--sp-2) var(--sp-3)' }}>
              <summary style={{ cursor: 'pointer', fontWeight: 600, fontSize: 'var(--fs-sm)' }}>
                <IconSparkle size={12} /> {unanchoredFindings.length} finding{unanchoredFindings.length > 1 ? 's' : ''} on this file
                {selectedFindings.length > unanchoredFindings.length ? ' (others shown inline in the diff)' : ''}
              </summary>
              <div className="stack mt-2" style={{ gap: 'var(--sp-2)' }}>
                {unanchoredFindings.map((f) => (
                  <FindingCard
                    key={f.id}
                    finding={f}
                    state={state.findingStates[f.id]}
                    onSetState={setFindingState}
                  />
                ))}
              </div>
            </details>
          )}

          {selected && (
            <DiffView
              // Remount per file: expansion / expander state must not leak
              // from one file's diff into another's
              key={selected.id}
              file={selected}
              comments={commentsOf(selected.id)}
              onSaveComment={saveComment}
              onDeleteComment={deleteComment}
              focusLine={focusLine}
              onFocusHandled={clearFocusLine}
              onLineClick={setLastFocusedLine}
              lineAnnotations={lineAnnotations}
              fontSize={diffFont}
              viewMode={diffMode}
              wrap={diffWrap}
              onLoadFileLines={selected.diffSource === 'git' ? loadFileLines : undefined}
            />
          )}
        </div>

        {/* Correction draft — a drawer over the code, opened from the bar */}
        {draftOpen && draftLines.length > 0 && (
          <div className="correction-drawer">
            <div className="spread">
              <strong style={{ fontSize: 'var(--fs-sm)' }}>
                Correction request for the agent ({draftLines.length} item{draftLines.length > 1 ? 's' : ''})
              </strong>
              <button className="btn btn-sm" onClick={() => setDraftOpen(false)}>✕</button>
            </div>
            <NoteField
              className="textarea mt-2"
              rows={Math.min(8, draftLines.length + 2)}
              aria-label="Correction request for the agent"
              value={draftText}
              onSave={(text) => void approval.setNote(DRAFT_NOTE_KEY, text === generatedDraft ? '' : text)}
            />
            {editedDraft !== undefined && editedDraft !== generatedDraft && (
              <button
                className="linklike mt-2"
                title="Replace your edited draft with one built from the current comments, notes and findings"
                onClick={() => void approval.setNote(DRAFT_NOTE_KEY, '')}
              >
                Regenerate from flagged items
              </button>
            )}
            <CorrectionExport session={session} prompt={draftText} />
          </div>
        )}

        {/* Note strip — opens over the decision bar while the verdict below
            is "Changes", so the ask travels with the request */}
        {selected && approvalOf(selected.id) === 'correction-requested' && (
          <div className="diff-note">
            <NoteField
              // Remount per file: a pending edit flushes to the file it belongs to
              key={selected.id}
              className="textarea"
              rows={2}
              aria-label={`Change request for ${selected.path}`}
              placeholder="What should the agent change in this file? (line comments are included automatically)"
              value={state.correctionNotes[selected.id] ?? ''}
              onSave={(text) => void approval.setNote(selected.id, text)}
              focusToken={noteFocus?.fileId === selected.id ? noteFocus.token : 0}
              onFocused={() => setNoteFocus(null)}
            />
          </div>
        )}

        {/* Decision bar — one row pinned to the window bottom.
            Left: where you are (flow stepper + progress). Right: the verdict
            on THIS file, then ONE primary action for the current stage.
            Decisions live down here; view preferences live on the diff bar. */}
        <div className="wb-bottombar">
          <div className="bb-row">
            <FlowSteps stage={stage} onStep={goToStage} />
            <div className="coverage-track bb-coverage" title={`Risk-weighted review coverage: ${coverage.pct}%`}>
              <div className="coverage-fill" style={{ width: `${coverage.pct}%` }} />
            </div>
            <span
              className="mono bb-counts flex"
              style={{ gap: 6 }}
              title={`${outcome.approved} approved · ${outcome.rejected} rejected · ${outcome.corrections} to revise · ${outcome.pending} pending`}
            >
              <span style={{ color: 'var(--ok)' }}>{outcome.approved}<IconCheck size={10} /></span>
              <span style={{ color: 'var(--err)' }}>{outcome.rejected}<IconX size={10} /></span>
              <span style={{ color: 'var(--risk-medium)' }}>{outcome.corrections}<IconRotateCcw size={10} /></span>
              <span className="faint">{outcome.pending}·</span>
            </span>
            {state.rejectedCategories.map((d) => (
              <span key={d} className="badge badge-err" title={`Every ${d}-flagged file was rejected as a category`}>
                {d} <IconX size={10} />
              </span>
            ))}
            {session.policy && (
              <span
                className={`badge ${session.policy.error ? 'badge-err' : 'badge-accent'}`}
                title={
                  session.policy.error ??
                  `.ars/policy.json is active${session.policy.blockFinalizeAboveRisk ? ` · finalize thresholds: ${Object.entries(session.policy.blockFinalizeAboveRisk).map(([k, v]) => `${k}≥${v}`).join(', ')}` : ''}`
                }
              >
                policy
              </span>
            )}
            {totalComments > 0 && (
              <span className="badge badge-accent" title={`${totalComments} line comment${totalComments > 1 ? 's' : ''}`}>
                {totalComments} <IconComment size={10} />
              </span>
            )}
            {state.sessionDecision && (
              <span className="flex" style={{ gap: 6, flexShrink: 0 }}>
                <span style={{ color: 'var(--ok)' }}><IconCheck size={11} /></span>
                <StatusPill status={state.sessionDecision} />
              </span>
            )}
            {approval.saveError && (
              <span className="badge badge-err" title={approval.saveError}>not saved</span>
            )}
            <span style={{ flex: 1 }} />
            <button
              className="btn btn-sm"
              style={{ border: 'none', background: 'transparent', color: 'var(--text-2)' }}
              title="Keyboard shortcuts"
              onClick={() => setOverlayOpen(true)}
            >
              <kbd className="kbd">?</kbd>
            </button>

            {/* Secondaries: quiet, never competing with the CTA. Deliberately
                NO bulk approve and NO early finalize — this is a review
                studio: every file gets an explicit verdict (a / c / r make it
                fast), and Finalize appears when the review is complete. */}
            {draftLines.length > 0 && !(stage === 'apply' && outcome.decision === 'corrections-requested') && (
              <button
                className={`btn btn-sm btn-warn${draftOpen ? ' active' : ''}`}
                title="Correction request assembled from comments, notes and findings"
                onClick={() => setDraftOpen((v) => !v)}
              >
                <IconRotateCcw size={12} /> Corrections ({draftLines.length})
              </button>
            )}
            {stage === 'apply' && (
              <button className="btn btn-sm" title="Change the recorded outcome" onClick={onFinalize}>
                Re-finalize
              </button>
            )}

            {/* The verdict on the file being read — the review's core act,
                so it sits on the decision bar, labeled, in fixed order */}
            {selected && (
              <span className="verdict-group">
                <button
                  className={`btn btn-sm btn-ok${approvalOf(selected.id) === 'approved' ? ' active' : ''}`}
                  title="Approve this file (a)"
                  onClick={() => decide(selected.id, 'approved')}
                >
                  <IconCheck /> Approve
                </button>
                <button
                  className={`btn btn-sm btn-warn${approvalOf(selected.id) === 'correction-requested' ? ' active' : ''}`}
                  title="Request changes (c) — a note field opens above"
                  onClick={() => decide(selected.id, 'correction-requested')}
                >
                  <IconRotateCcw /> Changes
                </button>
                <button
                  className={`btn btn-sm btn-danger${approvalOf(selected.id) === 'rejected' ? ' active' : ''}`}
                  title="Reject this file (r)"
                  onClick={() => decide(selected.id, 'rejected')}
                >
                  <IconX /> Reject
                </button>
              </span>
            )}
            {selected && approvalOf(selected.id) === 'rejected' && session.gitRepo && selected.drift !== 'missing' && (
              <button
                className="btn btn-sm btn-danger"
                disabled={gitBusy || approval.pending > 0}
                title={
                  approval.pending > 0
                    ? 'Saving the reject decision…'
                    : 'Restore this file to HEAD — the reject decision is saved first, and a backup patch is saved to ~/.agent-review-studio/patches'
                }
                onClick={() => void doRevert()}
              >
                <IconGitBranch size={12} /> Revert…
              </button>
            )}

            {/* THE primary action of the current stage */}
            {(stage === 'understand' || stage === 'review') && (
              <button
                className="btn btn-sm btn-primary"
                title="Go to the next file waiting for your verdict (n)"
                onClick={goNextPending}
              >
                Review next · {outcome.pending} left
              </button>
            )}
            {stage === 'decide' && (
              <button className="btn btn-sm btn-primary" title="Every file has a verdict — record the outcome" onClick={onFinalize}>
                <IconCheck size={12} /> Finalize{outcome.decision ? ` · ${statusMeta[outcome.decision].label}` : ''}
              </button>
            )}
            {stage === 'apply' && outcome.decision === 'corrections-requested' && draftLines.length > 0 && (
              <button
                className={`btn btn-sm btn-primary${draftOpen ? ' active' : ''}`}
                title="Open the correction request to copy or save it for the agent"
                onClick={() => setDraftOpen(true)}
              >
                <IconRotateCcw size={12} /> Export corrections ({draftLines.length})
              </button>
            )}
            {stage === 'apply' && outcome.decision !== 'corrections-requested' && session.gitRepo && outcome.approved > 0 && (
              <button
                className={`btn btn-sm btn-primary${railTab === 'git' ? ' active' : ''}`}
                disabled={gitBusy}
                title="Open Git: branch, commit the approved files, push, open a PR"
                onClick={() => setRailTab('git')}
              >
                <IconGitBranch size={12} /> Commit approved ({outcome.approved})
              </button>
            )}
            {stage === 'apply' && outcome.decision !== 'corrections-requested' && session.gitRepo && outcome.approved === 0 && (
              <button
                className={`btn btn-sm btn-primary${railTab === 'git' ? ' active' : ''}`}
                title="Open Git: branch state, change summary and commit graph"
                onClick={() => setRailTab('git')}
              >
                <IconGitBranch size={12} /> Open Git
              </button>
            )}
            {stage === 'apply' &&
              !session.gitRepo &&
              !(outcome.decision === 'corrections-requested' && draftLines.length > 0) && (
                <span className="faint" style={{ fontSize: 'var(--fs-xs)', flexShrink: 0 }}>
                  Done — decision recorded.
                </span>
              )}
          </div>
        </div>
      </section>

      {railOpen && !zen && (
        <ColResizer varNames={['--rail-w', '--rail-wide-w']} storageKey="ars:rail-width" min={240} max={760} edge="left" />
      )}
      {railOpen && !zen && (
        <ContextRail
          session={session}
          tab={railTab}
          onTab={setRailTab}
          findingStates={state.findingStates}
          onSetFindingState={setFindingState}
          onJumpToFile={jumpToFile}
          rejectedCategories={state.rejectedCategories}
          onRejectCategory={(d, ids) => void approval.rejectCategory(d, ids)}
          onClearCategory={(d, ids) => void approval.clearCategory(d, ids)}
          approvedPaths={approvedPaths}
          onGitChanged={refreshDetail}
          onClose={toggleRail}
        />
      )}

      {!railOpen && !zen && <RailStrip session={session} current={railTab} onOpen={setRailTab} />}

      {overlayOpen && <ShortcutOverlay onClose={() => setOverlayOpen(false)} />}
    </div>
  );
}
