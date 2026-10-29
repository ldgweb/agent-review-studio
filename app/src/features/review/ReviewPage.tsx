import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useSearchParams } from 'react-router-dom';
import type { FindingReviewState, LineComment } from '../../types/domain';
import type { ProjectReview, ReviewComment, ReviewFile, ReviewState, Verdict } from '../../types/review';
import { DEFAULT_DIFF_FONT, DiffView, MAX_DIFF_FONT, MIN_DIFF_FONT, type DiffFile, type DiffViewMode } from '../../components/DiffView';
import { NoteField } from '../../components/NoteField';
import { errorMessage } from '../../lib/api';
import { copyText } from '../../lib/clipboard';
import { openExternal } from '../../lib/desktop';
import { EDITORS, editorLink, isEditorId, type EditorId } from '../../lib/editors';
import { formatAgo, fullPath } from '../../lib/format';
import {
  commentLineIndex,
  commentOnHunk,
  countItems,
  countVerdicts,
  fileVerdict,
  findingsOf,
  groupFiles,
  hunkVerdict,
  outdatedComments,
  previousRoundItems,
  setFileVerdict,
  setFindingState,
  setGroupVerdict,
  setHunkVerdict,
  setNote,
  setRoundCommentResolved,
  sortFindings,
  uncommentHunk,
} from '../../lib/review';
import { useProjectReview } from '../../state/useProjectReview';
import { usePref } from '../../state/usePref';
import { FileHeader, type DiffScope, type NextStep } from './FileHeader';
import { FileList } from './FileList';
import { FileMenu, type FileActions } from './FileMenu';
import { FileViewer } from './FileViewer';
import { FindingHint } from './FindingHint';
import { HunkHeader } from './HunkHeader';
import { LastRound } from './LastRound';
import { NoticeLine, type Notice } from './NoticeLine';
import { ReviewDrawer } from './ReviewDrawer';
import { NothingToReview, ReviewProblem } from './ReviewStatus';
import { SessionPanel } from './SessionPanel';
import { ShortcutOverlay } from './ShortcutOverlay';
import { useFolds } from './useFolds';
import { useReviewShortcuts } from './useReviewShortcuts';

// The project review, reduced to what a reviewer does. Pick a file on the
// left and read its changes in the middle, each with the agent's reason
// above it. Decide: approve or reject a change or the whole file, comment
// on any line, or write the code you want instead. Every action sits in
// one toolbar above the code, in the order you use them: decide the file,
// go to the next one, then copy the review for the agent. The agent's
// session opens on request; nothing else is on screen.

export function ReviewPage() {
  const [params] = useSearchParams();
  const path = params.get('path') ?? '';
  if (!path) {
    return (
      <main className="page">
        <div className="banner banner-err" role="alert">No project given.</div>
      </main>
    );
  }
  // Another project is a fresh page: nothing of the last one lingers
  return <ProjectReviewPage key={path} path={path} />;
}

function ProjectReviewPage({ path }: { path: string }) {
  const api = useProjectReview(path);
  if (!api.review || !api.state) {
    return (
      <main className="page">
        {api.error ? (
          <ReviewProblem path={path} error={api.error} onRetry={() => void api.refresh()} />
        ) : (
          <div className="banner banner-info" role="status">
            <span className="spinner" aria-hidden="true" /> Reading the changes in <span className="mono">{path}</span>…
          </div>
        )}
      </main>
    );
  }
  if (api.review.files.length === 0) return <NothingToReview review={api.review} state={api.state} />;
  return <Workbench api={api} review={api.review} state={api.state} />;
}

type Api = ReturnType<typeof useProjectReview>;

const now = () => new Date().toISOString();
const newCommentId = () => `c-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
const clampFont = (size: number) => Math.min(MAX_DIFF_FONT, Math.max(MIN_DIFF_FONT, size));

function Workbench({ api, review, state }: { api: Api; review: ProjectReview; state: ReviewState }) {
  const [params, setParams] = useSearchParams();
  const files = review.files;

  const [query, setQuery] = useState('');
  const [sessionOpen, setSessionOpen] = useState(false);
  const [overlayOpen, setOverlayOpen] = useState(false);
  const [reviewOpen, setReviewOpen] = useState(false);
  const [viewerOpen, setViewerOpen] = useState(false);
  const [fileMenu, setFileMenu] = useState<{ file: ReviewFile; x: number; y: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const copying = useRef(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [focusedHunk, setFocusedHunk] = useState(0);
  const [scope, setScope] = useState<DiffScope>('all');
  const [noteFocus, setNoteFocus] = useState(0);
  const [lastLine, setLastLine] = useState<number | undefined>(undefined);
  const [commentRequest, setCommentRequest] = useState<{ hi: number; nonce: number } | undefined>(undefined);
  const filterRef = useRef<HTMLInputElement>(null);

  const [diffFont, setDiffFont] = usePref<number>('ars:diff-font', DEFAULT_DIFF_FONT, (v) => clampFont(Number(v)) === Number(v));
  const [diffWrap, setDiffWrap] = usePref<boolean>('ars:diff-wrap-on', true, (v) => v === 'true' || v === 'false');
  const [diffMode, setDiffMode] = usePref<DiffViewMode>('ars:diff-view', 'unified', (v) => v === 'unified' || v === 'split');
  const [editor, setEditor] = usePref<EditorId>('ars:editor', review.sessions[0]?.agent === 'cursor' ? 'cursor' : 'vscode', isEditorId);

  const mergeParams = (mutate: (p: URLSearchParams) => void) => {
    const next = new URLSearchParams(params);
    mutate(next);
    setParams(next, { replace: true });
  };

  const visible = useMemo(() => files.filter((f) => f.path.toLowerCase().includes(query.toLowerCase())), [files, query]);
  const groups = useMemo(() => groupFiles(visible, review), [visible, review]);
  const flatOrder = useMemo(() => groups.flatMap((g) => g.files), [groups]);

  const selectedId = params.get('file') ?? flatOrder[0]?.id ?? files[0].id;
  const selected: ReviewFile = files.find((f) => f.id === selectedId) ?? flatOrder[0] ?? files[0];
  const lineParam = params.get('line');
  const focusLine = lineParam ? Number(lineParam) : undefined;
  const focusOld = params.get('old') === '1';

  // A new file starts on its first change, in the scope that matters: only
  // what changed since the review you last copied, when that is what is new.
  const hasSince = Boolean(selected.sinceHunks);
  useEffect(() => {
    setFocusedHunk(0);
    setScope(selected.since === 'changed' && hasSince ? 'since' : 'all');
    setLastLine(undefined);
    // Only another file (or another relation to the last round) resets the
    // view, not every refresh while the agent keeps working.
  }, [selected.id, selected.since, hasSince]);

  const selectFile = (id: string) =>
    mergeParams((p) => {
      p.set('file', id);
      p.delete('line');
      p.delete('old');
    });
  const jumpToFile = (fileId: string, line?: number, old = false) =>
    mergeParams((p) => {
      p.set('file', fileId);
      if (line) p.set('line', String(line));
      else p.delete('line');
      if (old) p.set('old', '1');
      else p.delete('old');
    });

  const counts = useMemo(() => countVerdicts(state, files), [state, files]);
  const pkg = api.pkg ?? review.package;
  const reviewText = state.draft || pkg.text;
  const agentLabel = review.sessions[0]?.agentLabel ?? 'the agent';

  const nextUndecidedFrom = (afterId: string): ReviewFile | undefined => {
    const idx = flatOrder.findIndex((f) => f.id === afterId);
    const ordered = idx === -1 ? flatOrder : [...flatOrder.slice(idx + 1), ...flatOrder.slice(0, idx)];
    return ordered.find((f) => fileVerdict(state, f) === 'pending');
  };
  const stepFile = (delta: number) => {
    if (!flatOrder.length) return;
    const idx = flatOrder.findIndex((f) => f.id === selected.id);
    const next = flatOrder[(idx + delta + flatOrder.length) % flatOrder.length];
    if (next) selectFile(next.id);
  };
  const goNext = () => {
    const target = nextUndecidedFrom(selected.id);
    if (target) selectFile(target.id);
  };

  const notify = (tone: Notice['tone'], text: string) => setNotice({ key: `n-${Date.now()}`, tone, text, onDismiss: () => setNotice(null) });

  const copyReview = async () => {
    if (copying.current) return;
    copying.current = true;
    setBusy(true);
    try {
      if (!(await copyText(reviewText))) {
        notify('warn', 'The review could not be copied. Save it as .md, then try again.');
        return;
      }
      const res = await api.handback(reviewText, state.draft ? countItems(state.draft) : pkg.items);
      if (!res.ok) {
        notify('warn', `The review is on the clipboard, but the next round could not start: ${res.error}`);
        return;
      }
      setReviewOpen(false);
      notify('info', `Review copied. Paste it into ${agentLabel}. Next time you see only what changes from here.`);
    } finally {
      copying.current = false;
      setBusy(false);
    }
  };

  // One next step at a time: another file waiting, else copying the review
  const nextStep: NextStep = (() => {
    const other = nextUndecidedFrom(selected.id);
    if (other && other.id !== selected.id) return { kind: 'next', onNext: goNext };
    if (counts.pending > 0) return { kind: 'none' };
    if (pkg.items > 0) return { kind: 'copy', label: `Copy review for ${agentLabel}`, busy, onCopy: () => void copyReview() };
    return { kind: 'done' };
  })();

  const decideFile = (file: ReviewFile, verdict: Verdict) => {
    const before = fileVerdict(state, file);
    api.update((s) => setFileVerdict(s, file, verdict, now()));
    // The natural flow: a decided file hands over to the next one waiting
    if (before === 'pending') {
      const target = nextUndecidedFrom(file.id);
      if (target) selectFile(target.id);
    }
  };
  const decideHunk = (hi: number, verdict: Verdict) => {
    const h = selected.hunks[hi];
    if (!h) return;
    api.update((s) => setHunkVerdict(s, selected, h.id, verdict, now()));
    setFocusedHunk(Math.min(hi + 1, selected.hunks.length - 1));
  };

  // --- the selected file in the diff -----------------------------------------
  const folds = useFolds(selected);

  const fileComments: Array<LineComment & { id: string }> = useMemo(() => {
    const out: Array<LineComment & { id: string }> = [];
    for (const c of state.comments) {
      if (c.path !== selected.path) continue;
      const hi = selected.hunks.findIndex((h) => h.id === c.hunkId);
      if (hi < 0) continue;
      const li = commentLineIndex(selected.hunks[hi], c);
      if (li < 0) continue;
      // The line number as the file is now: the code above may have moved it
      const at = selected.hunks[hi].lines[li];
      out.push({ id: c.id, anchor: `${hi}:${li}`, line: at.newLine ?? at.oldLine, text: c.text, suggestion: c.suggestion });
    }
    return out;
  }, [selected, state.comments]);

  const { update } = api;
  const saveComment = useCallback(
    (c: LineComment) => {
      const [hi, li] = c.anchor.split(':').map(Number);
      const hunk = selected.hunks[hi];
      const line = hunk?.lines[li];
      if (!hunk || !line) return;
      const existing = state.comments.find((x) => x.hunkId === hunk.id && commentLineIndex(hunk, x) === li);
      const comment: ReviewComment = {
        id: existing?.id ?? newCommentId(),
        path: selected.path,
        hunkId: hunk.id,
        li,
        kind: line.kind,
        lineText: line.text,
        line: line.newLine ?? line.oldLine,
        text: c.text,
        ...(c.suggestion !== undefined ? { suggestion: c.suggestion } : {}),
        at: now(),
      };
      update((s) => commentOnHunk(s, comment, comment.at));
      if (comment.line) setLastLine(comment.line);
    },
    [selected, state.comments, update],
  );
  const deleteComment = useCallback(
    (anchor: string) => {
      const c = fileComments.find((x) => x.anchor === anchor);
      if (c) update((s) => uncommentHunk(s, c.id));
    },
    [fileComments, update],
  );

  const { fileLines, readFile } = api;
  const loadFileLines = useCallback(() => fileLines(selected.path), [selected, fileLines]);
  const loadBaseLines = useCallback(() => fileLines(selected.oldPath ?? selected.path, true), [selected, fileLines]);
  const loadWholeFile = () =>
    selected.changeType === 'deleted' ? readFile(selected.oldPath ?? selected.path, true) : readFile(selected.path);

  // The other half of a moved block is usually folded as noise: open it
  const openMovedPeer = (path: string, line: number, old: boolean) => {
    const target = files.find((f) => f.path === path);
    if (!target) return;
    const hunk = target.hunks.find((h) => h.lines.some((l) => (old ? l.kind !== 'add' && l.oldLine === line : l.newLine === line)));
    if (hunk) folds.unfold(hunk.id);
    jumpToFile(target.id, line, old);
  };

  const groupSize = useMemo(() => new Map(review.groups.map((g) => [g.id, g.hunkIds.length])), [review.groups]);

  const renderHunkHeader = (hi: number) => {
    const h = selected.hunks[hi];
    return (
      <HunkHeader
        hunk={h}
        verdict={hunkVerdict(state, h.id)}
        hasSessions={review.sessions.length > 0}
        collapsed={folds.collapsed.has(hi)}
        onToggleCollapsed={() => folds.toggle(hi)}
        groupSize={h.group ? groupSize.get(h.group) : undefined}
        onVerdict={(verdict) => decideHunk(hi, verdict)}
        showVerdict={selected.hunks.length > 1}
        onGroupVerdict={h.group ? (verdict) => update((s) => setGroupVerdict(s, review, h.group!, verdict, now())) : undefined}
      />
    );
  };

  // Hints at a line render under that line; the others sit above the diff
  const findingsHere = useMemo(() => sortFindings(findingsOf(review, selected)), [review, selected]);
  const inlineable = useMemo(() => {
    const set = new Set<number>();
    for (const h of selected.hunks) for (const l of h.lines) if (l.newLine != null) set.add(l.newLine);
    return set;
  }, [selected]);
  const setFinding = useCallback((id: string, st?: FindingReviewState) => update((s) => setFindingState(s, id, st)), [update]);
  const lineAnnotations = useMemo(() => {
    const map = new Map<number, ReactNode>();
    for (const f of findingsHere) {
      if (f.line == null || !inlineable.has(f.line)) continue;
      map.set(
        f.line,
        <>
          {map.get(f.line)}
          <FindingHint key={f.id} finding={f} state={state.findings[f.id]} onSetState={setFinding} />
        </>,
      );
    }
    return map;
  }, [findingsHere, inlineable, state.findings, setFinding]);
  const unanchored = findingsHere.filter((f) => f.line == null || !inlineable.has(f.line));

  // What became of earlier comments on this file
  const earlier = useMemo(() => {
    const round = state.rounds[state.rounds.length - 1];
    // After a commit, what the agent did since that review is history: only
    // the comments still waiting on untouched code stay
    const current = Boolean(round) && (round.head ?? null) === review.project.head;
    return {
      items: previousRoundItems(review, state).filter(
        (i) => i.comment.path === selected.path && (i.status === 'open' || (i.status === 'changed' && current)),
      ),
      note: current ? round?.notes[selected.path] : undefined,
      outdated: outdatedComments(state, files).filter((c) => c.path === selected.path),
    };
  }, [review, state, files, selected.path]);

  // Memoized: the diff reloads its file whenever this object changes
  const sinceFile: DiffFile | null = useMemo(
    () =>
      scope === 'since' && selected.sinceHunks
        ? {
            id: `${selected.id}-since`,
            path: selected.path,
            language: selected.language,
            changeType: 'modified',
            linesAdded: selected.linesAdded,
            hunks: selected.sinceHunks,
          }
        : null,
    [selected, scope],
  );

  const openFileInEditor = (path: string, line?: number) =>
    openExternal(editorLink(editor, review.project.root, path, line)).catch((e: unknown) =>
      notify('warn', `Could not open ${EDITORS[editor].label}: ${errorMessage(e)}`),
    );
  const openInEditor = (line = lastLine) => openFileInEditor(selected.path, line);

  const fileActions: FileActions = {
    editorLabel: EDITORS[editor].label,
    reveal: (file) =>
      void api.reveal(file.path).then((r) => {
        if (!r.ok) notify('warn', `Could not show ${file.path}: ${r.error}`);
      }),
    openInEditor: (file) => void openFileInEditor(file.path),
    copyPath: (file, relative) => {
      const text = relative ? file.path : fullPath(review.project.root, file.path);
      void copyText(text).then((ok) => notify(ok ? 'info' : 'warn', ok ? `Copied: ${text}` : 'The path could not be copied.'));
    },
  };

  // The hunk keys act on the full diff, not on "since last review"
  const hunkKeys = scope === 'all' && selected.hunks.length > 0;
  const stepHunk = (delta: number) => {
    if (!hunkKeys) return;
    const next = Math.max(0, Math.min(selected.hunks.length - 1, focusedHunk + delta));
    setFocusedHunk(next);
    document.getElementById(`hk-${selected.id}-${next}`)?.scrollIntoView({ block: 'start', behavior: 'smooth' });
  };

  useReviewShortcuts({
    nextFile: () => stepFile(1),
    prevFile: () => stepFile(-1),
    nextUndecided: goNext,
    approve: () => decideFile(selected, 'approved'),
    reject: () => decideFile(selected, 'rejected'),
    noteFile: () => setNoteFocus((n) => n + 1),
    nextHunk: () => stepHunk(1),
    prevHunk: () => stepHunk(-1),
    approveHunk: () => hunkKeys && decideHunk(focusedHunk, 'approved'),
    rejectHunk: () => hunkKeys && decideHunk(focusedHunk, 'rejected'),
    commentHunk: () => hunkKeys && setCommentRequest((r) => ({ hi: focusedHunk, nonce: (r?.nonce ?? 0) + 1 })),
    viewFile: () => !selected.binary && setViewerOpen(true),
    fontUp: () => setDiffFont(clampFont(diffFont + 1)),
    fontDown: () => setDiffFont(clampFont(diffFont - 1)),
    focusFilter: () => filterRef.current?.focus(),
    toggleOverlay: () => setOverlayOpen((v) => !v),
    suspended: overlayOpen || viewerOpen || fileMenu !== null,
  });

  return (
    <div className="review">
      <FileList
        review={review}
        state={state}
        groups={groups}
        selectedId={selected.id}
        onSelect={selectFile}
        reviewed={files.length - counts.pending}
        query={query}
        onQuery={setQuery}
        filterRef={filterRef}
        onToggleSession={() => setSessionOpen(!sessionOpen)}
        sessionOpen={sessionOpen}
        onFileMenu={(file, x, y) => setFileMenu({ file, x, y })}
      />

      <section className="review-main">
        {api.saveError && (
          <div className="banner banner-err" role="alert">
            <span style={{ flex: 1 }}>Review not saved: {api.saveError}</span>
          </div>
        )}
        {review.warnings.map((w) => (
          <div key={w} className="banner banner-warn" role="status">
            {w}
          </div>
        ))}
        <NoticeLine notice={notice} />

        <FileHeader
          file={selected}
          verdict={fileVerdict(state, selected)}
          onVerdict={(v) => decideFile(selected, v)}
          scope={scope}
          onScope={setScope}
          next={nextStep}
          reviewItems={pkg.items}
          reviewOpen={reviewOpen}
          onToggleReview={() => setReviewOpen((v) => !v)}
          viewMode={diffMode}
          onToggleView={() => setDiffMode(diffMode === 'split' ? 'unified' : 'split')}
          wrap={diffWrap}
          onToggleWrap={() => setDiffWrap(!diffWrap)}
          fontSize={diffFont}
          onFontSize={(d) => setDiffFont(clampFont(diffFont + d))}
          onViewFile={!selected.binary ? () => setViewerOpen(true) : undefined}
          editorLabel={EDITORS[editor].label}
          onOpenInEditor={selected.changeType !== 'deleted' ? () => void openInEditor() : undefined}
          onShortcuts={() => setOverlayOpen(true)}
        />

        <div className="diff-scroll">
          <LastRound
            items={earlier.items}
            note={earlier.note}
            outdated={earlier.outdated}
            onResolve={(id) => update((s) => setRoundCommentResolved(s, id, true))}
            onDelete={(id) => update((s) => uncommentHunk(s, id))}
          />
          {selected.generated && <div className="diff-note-line">Generated file ({selected.generated}): its changes are folded.</div>}
          {selected.binary && (
            <div className="diff-empty">
              <strong>Binary file.</strong> Decide on it as a whole, or open it in your editor.
            </div>
          )}
          {selected.modeChange && selected.hunks.length === 0 && (
            <div className="diff-empty">
              <strong>Mode change</strong> {selected.modeChange}
            </div>
          )}
          {selected.changeType === 'renamed' && selected.hunks.length === 0 && (
            <div className="diff-empty">
              <strong>Renamed</strong> from <span className="mono">{selected.oldPath}</span>, content unchanged.
            </div>
          )}
          {sinceFile && (
            <div className="diff-note-line">
              Only what the agent changed since your last review{review.sinceReview ? ` (${formatAgo(review.sinceReview.at)})` : ''}.{' '}
              <button className="btn-mini" onClick={() => setScope('all')}>
                Show all changes
              </button>
            </div>
          )}
          {!sinceFile && unanchored.map((f) => <FindingHint key={f.id} finding={f} state={state.findings[f.id]} onSetState={setFinding} />)}
          {sinceFile && <DiffView key={sinceFile.id} file={sinceFile} fontSize={diffFont} viewMode={diffMode} wrap={diffWrap} onLoadFileLines={loadFileLines} />}
          {!sinceFile && selected.hunks.length > 0 && (
            <DiffView
              key={selected.id}
              file={selected}
              comments={fileComments}
              onSaveComment={saveComment}
              onDeleteComment={deleteComment}
              focusLine={focusLine}
              focusOld={focusOld}
              onFocusHandled={() =>
                mergeParams((p) => {
                  p.delete('line');
                  p.delete('old');
                })
              }
              onMovedPeer={openMovedPeer}
              onLineClick={setLastLine}
              lineAnnotations={lineAnnotations}
              fontSize={diffFont}
              viewMode={diffMode}
              wrap={diffWrap}
              onLoadFileLines={selected.changeType !== 'deleted' ? loadFileLines : undefined}
              onLoadBaseLines={selected.changeType !== 'created' && !selected.untracked ? loadBaseLines : undefined}
              renderHunkHeader={renderHunkHeader}
              collapsedHunks={folds.collapsed}
              focusedHunk={focusedHunk}
              commentRequest={commentRequest}
            />
          )}
          <div className="file-note">
            <NoteField
              key={selected.path}
              className="file-note-input"
              rows={2}
              aria-label={`Note for the agent about ${selected.path}`}
              placeholder="Anything else the agent should change in this file?  (c)"
              value={state.notes[selected.path] ?? ''}
              onSave={(text) => update((s) => setNote(s, selected.path, text))}
              focusToken={noteFocus}
            />
          </div>
        </div>

        {reviewOpen && pkg.items > 0 && (
          <ReviewDrawer
            agentLabel={agentLabel}
            projectName={review.project.name}
            text={reviewText}
            generated={pkg.text}
            items={state.draft ? countItems(state.draft) : pkg.items}
            busy={busy}
            onEdit={(t) => update((s) => ({ ...s, draft: t || undefined }))}
            onCopy={() => void copyReview()}
            onClose={() => setReviewOpen(false)}
          />
        )}
      </section>

      {viewerOpen && (
        <FileViewer
          key={`${selected.id}:${selected.fingerprint}`}
          file={selected}
          load={loadWholeFile}
          fontSize={diffFont}
          focusLine={lastLine}
          editor={editor}
          onEditor={setEditor}
          onOpenInEditor={selected.changeType !== 'deleted' ? (line) => void openInEditor(line) : undefined}
          onClose={() => setViewerOpen(false)}
        />
      )}
      {sessionOpen && review.sessions.length > 0 && <SessionPanel review={review} onClose={() => setSessionOpen(false)} />}
      {overlayOpen && <ShortcutOverlay onClose={() => setOverlayOpen(false)} />}
      {fileMenu && <FileMenu {...fileMenu} actions={fileActions} onClose={() => setFileMenu(null)} />}
    </div>
  );
}
