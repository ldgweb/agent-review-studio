import { Fragment, memo, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react';
import type { DiffHunk, DiffLine, FileChange, LineComment } from '../types/domain';
import { extToLang, highlightLine } from '../lib/highlight';
import { changedRanges, type IntraRange } from '../lib/intraline';
import { scrollFlash } from '../lib/scroll';

// The code-reading surface, modeled on today's best diff viewers (GitHub /
// VS Code): unified or side-by-side (with a draggable divider), long lines
// wrap by default (no-wrap scrolls horizontally with sticky gutters instead
// of clipping), word-level changed segments are highlighted inside del/add
// pairs, hover any line to leave a comment, findings render inline at their
// line. Git-enriched files can expand the unchanged context around hunks,
// GitHub-style, straight from the working tree. Hunks are windowed with
// content-visibility so huge diffs stay smooth, and oversized hunks render
// behind a "show more" expander.

const MAX_RENDER_LINES = 400;
/** Context lines revealed per expander click (GitHub-style) */
const EXPAND_STEP = 20;
/** Default diff font size (px) — the user can adjust it from the diff bar */
export const DEFAULT_DIFF_FONT = 13;

export type DiffViewMode = 'unified' | 'split';

interface DiffViewProps {
  file: FileChange;
  comments?: LineComment[];
  onSaveComment?: (comment: LineComment) => void;
  onDeleteComment?: (anchor: string) => void;
  /** 1-based new-file line to scroll to and flash (e.g. from a finding) */
  focusLine?: number;
  /** Called once the focus scroll has been performed (or was impossible) */
  onFocusHandled?: () => void;
  /** newLine → node rendered under that line (inline finding annotations) */
  lineAnnotations?: Map<number, ReactNode>;
  /** Code font size in px (persisted preference) */
  fontSize?: number;
  /** Unified (default) or side-by-side */
  viewMode?: DiffViewMode;
  /** Wrap long lines (default); off = horizontal scroll, never clipped */
  wrap?: boolean;
  /** Loads the file's working-tree lines — enables expanding hunk context */
  onLoadFileLines?: () => Promise<string[] | null>;
  /** A diff line was clicked (its new-file line number) */
  onLineClick?: (line: number) => void;
}

function CommentEditor({
  initial,
  onSave,
  onCancel,
}: {
  initial: string;
  onSave: (text: string) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState(initial);
  return (
    <div className="comment-editor">
      <textarea
        className="textarea"
        autoFocus
        rows={2}
        placeholder="Leave a comment for the agent — e.g. “add rate limiting here” or “why not reuse UserService?”"
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && text.trim()) onSave(text.trim());
          if (e.key === 'Escape') onCancel();
        }}
      />
      <div className="flex" style={{ justifyContent: 'flex-end', marginTop: 6 }}>
        <button className="btn btn-sm" onClick={onCancel}>Cancel</button>
        <button className="btn btn-sm btn-primary" disabled={!text.trim()} onClick={() => onSave(text.trim())}>
          Comment
        </button>
      </div>
    </div>
  );
}

/**
 * Comments anchor positionally ("hunkIndex:lineIndex"). When the hunks change
 * shape after the comment was left (e.g. diffs re-enriched), re-resolve the
 * anchor through the comment's recorded line number.
 */
function resolveAnchor(file: FileChange, comment: LineComment): string {
  const [hi, li] = comment.anchor.split(':').map(Number);
  const at = file.hunks[hi]?.lines[li];
  if (!comment.line || (at && (at.newLine ?? at.oldLine) === comment.line)) return comment.anchor;
  for (let h = 0; h < file.hunks.length; h++) {
    const idx = file.hunks[h].lines.findIndex((l) => l.newLine === comment.line);
    if (idx !== -1) return `${h}:${idx}`;
  }
  return comment.anchor;
}

const EMPTY_COMMENTS: ReadonlyMap<string, LineComment> = new Map();
const EMPTY_RANGES: ReadonlyMap<number, IntraRange> = new Map();

/** The line text with the changed word-level segment marked behind it: a
 * transparent copy sits under the tokenized code so the highlight never
 * fights the syntax colors. */
function CodeText({
  text,
  lang,
  range,
  kind,
}: {
  text: string;
  lang: string | null;
  range?: IntraRange;
  kind: DiffLine['kind'];
}) {
  return (
    <>
      {range && (
        <span className="seg-underlay" aria-hidden="true">
          {text.slice(0, range.start)}
          <span className={`seg-${kind}`}>{text.slice(range.start, range.end)}</span>
          {text.slice(range.end)}
        </span>
      )}
      {highlightLine(text, lang)}
    </>
  );
}

interface SplitSide {
  line: DiffLine;
  li: number;
}

interface SplitRow {
  left?: SplitSide;
  right?: SplitSide;
}

/** Pair lines for the side-by-side view: context spans both sides, each
 * del-run is aligned with the add-run that follows it (GitHub-style). */
function buildSplitRows(lines: DiffLine[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let i = 0;
  while (i < lines.length) {
    if (lines[i].kind === 'context') {
      rows.push({ left: { line: lines[i], li: i }, right: { line: lines[i], li: i } });
      i++;
      continue;
    }
    const delStart = i;
    while (i < lines.length && lines[i].kind === 'del') i++;
    const addStart = i;
    while (i < lines.length && lines[i].kind === 'add') i++;
    const dels = addStart - delStart;
    const adds = i - addStart;
    for (let k = 0; k < Math.max(dels, adds); k++) {
      rows.push({
        left: k < dels ? { line: lines[delStart + k], li: delStart + k } : undefined,
        right: k < adds ? { line: lines[addStart + k], li: addStart + k } : undefined,
      });
    }
  }
  return rows;
}

function SplitCell({
  side,
  num,
  id,
  lang,
  ranges,
  canComment,
  commented,
  onEdit,
  hi,
}: {
  side?: SplitSide;
  num?: number;
  id?: string;
  lang: string | null;
  ranges: ReadonlyMap<number, IntraRange>;
  canComment: boolean;
  commented: boolean;
  onEdit: (anchor: string) => void;
  hi: number;
}) {
  if (!side) return <div className="split-side none" />;
  const { line, li } = side;
  return (
    <div id={id} className={`split-side ${line.kind}${commented ? ' commented' : ''}`} data-line={line.newLine}>
      <span className="ln">{num ?? ''}</span>
      <span className="sign">
        {canComment && (
          <button className="add-comment" title="Comment on this line" onClick={() => onEdit(`${hi}:${li}`)}>
            +
          </button>
        )}
        <span className="sign-char">{line.kind === 'add' ? '+' : line.kind === 'del' ? '−' : ''}</span>
      </span>
      <span className="code">
        <CodeText text={line.text} lang={lang} range={ranges.get(li)} kind={line.kind} />
      </span>
    </div>
  );
}

/** Expanded lines pulled from the working tree, in either view mode.
 * Usually unchanged context; for a created file they are additions (the
 * whole file is new) and render green with an empty old side. */
function ContextLines({ lines, split, lang }: { lines: DiffLine[]; split: boolean; lang: string | null }) {
  if (lines.length === 0) return null;
  if (split) {
    return (
      <>
        {lines.map((l, i) => (
          <div key={i} className="split-row">
            {l.kind === 'add' ? (
              <div className="split-side none" />
            ) : (
              <div className="split-side context">
                <span className="ln">{l.oldLine ?? ''}</span>
                <span className="sign" />
                <span className="code">
                  <CodeText text={l.text} lang={lang} kind="context" />
                </span>
              </div>
            )}
            <div className={`split-side ${l.kind}`}>
              <span className="ln">{l.newLine ?? ''}</span>
              <span className="sign">
                <span className="sign-char">{l.kind === 'add' ? '+' : ''}</span>
              </span>
              <span className="code">
                <CodeText text={l.text} lang={lang} kind={l.kind} />
              </span>
            </div>
          </div>
        ))}
      </>
    );
  }
  return (
    <>
      {lines.map((l, i) => (
        <div key={i} className={`diff-line ${l.kind}`}>
          <span className="ln">{l.oldLine ?? ''}</span>
          <span className="ln">{l.newLine ?? ''}</span>
          <span className="sign">
            <span className="sign-char">{l.kind === 'add' ? '+' : ''}</span>
          </span>
          <span className="code">
            <CodeText text={l.text} lang={lang} kind={l.kind} />
          </span>
        </div>
      ))}
    </>
  );
}

/** Hunk header that can pull more context in from the working tree */
function ExpanderBar({
  dir,
  label,
  remaining,
  onExpand,
}: {
  dir: 'up' | 'down';
  label?: string;
  /** null = unknown until the file loads */
  remaining: number | null;
  onExpand: () => void;
}) {
  const step = remaining === null ? EXPAND_STEP : Math.min(EXPAND_STEP, remaining);
  return (
    <div className="diff-hunk-header expandable">
      <span className="sticky-x">
        <button
          className="ctx-expand"
          title={dir === 'up' ? 'Show more lines above this hunk' : 'Show more lines below'}
          onClick={onExpand}
        >
          {dir === 'up' ? '↑' : '↓'} {step}
        </button>
        {label && <span className="ctx-label">{label}</span>}
      </span>
    </div>
  );
}

interface HunkViewProps {
  hunk: DiffHunk;
  hi: number;
  fileId: string;
  lang: string | null;
  showHeader: boolean;
  canComment: boolean;
  expanded: boolean;
  onExpand: (hi: number) => void;
  /** resolved anchor → comment, only anchors within this hunk */
  comments: ReadonlyMap<string, LineComment>;
  /** The open comment editor's anchor — only when it is in this hunk */
  editingAnchor: string | null;
  onEdit: (anchor: string | null) => void;
  onSaveComment?: (comment: LineComment) => void;
  onDeleteComment?: (anchor: string) => void;
  lineAnnotations?: Map<number, ReactNode>;
  /** Estimated rendered line height for contain-intrinsic-size */
  lineHeightPx: number;
  viewMode: DiffViewMode;
  /** line index → changed word-level segment (paired del/add lines) */
  ranges: ReadonlyMap<number, IntraRange>;
}

const HunkView = memo(function HunkView({
  hunk,
  hi,
  fileId,
  lang,
  showHeader,
  canComment,
  expanded,
  onExpand,
  comments,
  editingAnchor,
  onEdit,
  onSaveComment,
  onDeleteComment,
  lineAnnotations,
  lineHeightPx,
  viewMode,
  ranges,
}: HunkViewProps) {
  const visibleLines = useMemo(
    () => (expanded ? hunk.lines : hunk.lines.slice(0, MAX_RENDER_LINES)),
    [expanded, hunk.lines],
  );
  const hidden = hunk.lines.length - visibleLines.length;
  const splitRows = useMemo(
    () => (viewMode === 'split' ? buildSplitRows(visibleLines) : []),
    [viewMode, visibleLines],
  );
  // Offscreen hunks skip layout and paint entirely; scrollIntoView still
  // forces them to render when a deep link lands inside one. The width
  // estimate must stay empty: in no-wrap mode the diff is max-content wide
  // and a height-sized width estimate would fake thousands of pixels.
  const style: CSSProperties = {
    contentVisibility: 'auto',
    containIntrinsicWidth: 'auto none',
    containIntrinsicHeight: `auto ${(viewMode === 'split' ? splitRows.length : visibleLines.length) * lineHeightPx}px`,
  };

  // A saved comment (bubble or editor) renders full-width under its row, in
  // both view modes.
  const commentBlock = (anchor: string, lineNo?: number): ReactNode => {
    const existing = comments.get(anchor);
    if (existing && editingAnchor !== anchor) {
      return (
        <div className="comment-row">
          <div className="comment-bubble">
            <div className="spread">
              <span className="comment-author">Your comment{existing.line ? ` · line ${existing.line}` : ''}</span>
              <span className="flex" style={{ gap: 4 }}>
                <button className="btn btn-sm" onClick={() => onEdit(anchor)}>Edit</button>
                <button className="btn btn-sm" onClick={() => onDeleteComment?.(existing.anchor)}>Delete</button>
              </span>
            </div>
            <div className="mt-2" style={{ whiteSpace: 'pre-wrap' }}>{existing.text}</div>
          </div>
        </div>
      );
    }
    if (editingAnchor === anchor) {
      return (
        <div className="comment-row">
          <CommentEditor
            initial={existing?.text ?? ''}
            onCancel={() => onEdit(null)}
            onSave={(text) => {
              // Editing an existing comment must keep its original anchor
              // (the store key), even when it renders re-resolved.
              onSaveComment?.({ anchor: existing?.anchor ?? anchor, line: existing?.line ?? lineNo, text });
              onEdit(null);
            }}
          />
        </div>
      );
    }
    return null;
  };

  return (
    <div style={style}>
      {showHeader && (
        <div className="diff-hunk-header">
          <span className="sticky-x">{hunk.header}</span>
        </div>
      )}

      {viewMode === 'unified' &&
        visibleLines.map((line, li) => {
          const anchor = `${hi}:${li}`;
          const lineNo = line.newLine ?? line.oldLine;
          const annotation = line.newLine != null ? lineAnnotations?.get(line.newLine) : undefined;
          return (
            <div key={li}>
              <div
                id={`dl-${fileId}-${hi}-${li}`}
                className={`diff-line ${line.kind}${comments.has(anchor) ? ' commented' : ''}`}
                data-line={line.newLine}
              >
                <span className="ln">{line.oldLine ?? ''}</span>
                <span className="ln">{line.newLine ?? ''}</span>
                <span className="sign">
                  {canComment && (
                    <button className="add-comment" title="Comment on this line" onClick={() => onEdit(anchor)}>
                      +
                    </button>
                  )}
                  <span className="sign-char">{line.kind === 'add' ? '+' : line.kind === 'del' ? '−' : ''}</span>
                </span>
                <span className="code">
                  <CodeText text={line.text} lang={lang} range={ranges.get(li)} kind={line.kind} />
                </span>
              </div>
              {annotation && <div className="annotation-row">{annotation}</div>}
              {commentBlock(anchor, lineNo)}
            </div>
          );
        })}

      {viewMode === 'split' &&
        splitRows.map((row, ri) => {
          const leftAnchor = row.left ? `${hi}:${row.left.li}` : null;
          const rightAnchor = row.right ? `${hi}:${row.right.li}` : null;
          const annotation = row.right?.line.newLine != null ? lineAnnotations?.get(row.right.line.newLine) : undefined;
          return (
            <div key={ri}>
              <div className="split-row">
                <SplitCell
                  side={row.left}
                  num={row.left?.line.oldLine}
                  // context rows share one li — the right cell owns the id then
                  id={row.left && row.left.li !== row.right?.li ? `dl-${fileId}-${hi}-${row.left.li}` : undefined}
                  lang={lang}
                  ranges={ranges}
                  canComment={canComment}
                  commented={leftAnchor ? comments.has(leftAnchor) : false}
                  onEdit={onEdit}
                  hi={hi}
                />
                <SplitCell
                  side={row.right}
                  num={row.right?.line.newLine}
                  id={row.right ? `dl-${fileId}-${hi}-${row.right.li}` : undefined}
                  lang={lang}
                  ranges={ranges}
                  canComment={canComment}
                  commented={rightAnchor ? comments.has(rightAnchor) : false}
                  onEdit={onEdit}
                  hi={hi}
                />
              </div>
              {annotation && <div className="annotation-row">{annotation}</div>}
              {leftAnchor && leftAnchor !== rightAnchor && commentBlock(leftAnchor, row.left!.line.oldLine)}
              {rightAnchor && commentBlock(rightAnchor, row.right!.line.newLine ?? row.right!.line.oldLine)}
            </div>
          );
        })}

      {hidden > 0 && (
        <button className="diff-expander" onClick={() => onExpand(hi)}>
          <span className="sticky-x">↓ Show {hidden} more line{hidden > 1 ? 's' : ''}</span>
        </button>
      )}
    </div>
  );
});

export function DiffView({
  file,
  comments = [],
  onSaveComment,
  onDeleteComment,
  focusLine,
  onFocusHandled,
  lineAnnotations,
  fontSize = DEFAULT_DIFF_FONT,
  viewMode = 'unified',
  wrap = true,
  onLoadFileLines,
  onLineClick,
}: DiffViewProps) {
  const [editingAnchor, setEditingAnchor] = useState<string | null>(null);
  const [expandedHunks, setExpandedHunks] = useState<Set<number>>(new Set());
  const expandHunk = useCallback((index: number) => setExpandedHunks((prev) => new Set(prev).add(index)), []);
  const editingHunk = editingAnchor === null ? null : Number(editingAnchor.split(':')[0]);
  const canComment = Boolean(onSaveComment);
  const lang = useMemo(() => extToLang(file.language), [file.language]);

  const hunkRanges = useMemo(() => file.hunks.map((h) => changedRanges(h)), [file]);

  // --- Context expansion: unchanged lines pulled from the working tree ---
  const [fileLines, setFileLines] = useState<string[] | null>(null);
  const [expandFailed, setExpandFailed] = useState(false);
  const [expandAbove, setExpandAbove] = useState<Map<number, number>>(new Map());
  const [expandBelow, setExpandBelow] = useState(0);
  const linesPromise = useRef<Promise<string[] | null> | null>(null);
  const canExpand = Boolean(onLoadFileLines) && !expandFailed && file.hunks.length > 0;

  // Per hunk: its new-file line span and the old↔new offset accumulated by
  // the hunks before it (old = new − deltaBefore for surrounding context)
  const { hunkMeta, totalDelta } = useMemo(() => {
    let running = 0;
    const metas = file.hunks.map((h) => {
      let startNew: number | null = null;
      let endNew: number | null = null;
      let delta = 0;
      for (const l of h.lines) {
        if (l.newLine != null) {
          if (startNew === null || l.newLine < startNew) startNew = l.newLine;
          if (endNew === null || l.newLine > endNew) endNew = l.newLine;
        }
        if (l.kind === 'add') delta++;
        else if (l.kind === 'del') delta--;
      }
      const meta = { startNew, endNew, deltaBefore: running };
      running += delta;
      return meta;
    });
    return { hunkMeta: metas, totalDelta: running };
  }, [file]);

  const gapAbove = (hi: number): number => {
    const start = hunkMeta[hi].startNew;
    if (start == null) return 0;
    let prevEnd = 0;
    for (let j = hi - 1; j >= 0; j--) {
      const end = hunkMeta[j].endNew;
      if (end != null) {
        prevEnd = end;
        break;
      }
    }
    return Math.max(0, start - prevEnd - 1);
  };

  const ensureLines = async (): Promise<string[] | null> => {
    if (fileLines) return fileLines;
    if (!onLoadFileLines) return null;
    if (!linesPromise.current) linesPromise.current = onLoadFileLines();
    const lines = await linesPromise.current;
    if (lines && lines.length > 0) {
      setFileLines(lines);
      return lines;
    }
    setExpandFailed(true);
    return null;
  };
  const expandUp = async (hi: number, gap: number) => {
    if (!(await ensureLines())) return;
    setExpandAbove((prev) => {
      const next = new Map(prev);
      next.set(hi, Math.min(gap, (prev.get(hi) ?? 0) + EXPAND_STEP));
      return next;
    });
  };
  const expandDown = async () => {
    if (!(await ensureLines())) return;
    setExpandBelow((prev) => prev + EXPAND_STEP);
  };

  const contextBefore = (hi: number): DiffLine[] => {
    const revealed = expandAbove.get(hi) ?? 0;
    const start = hunkMeta[hi].startNew;
    if (!fileLines || revealed === 0 || start == null) return [];
    const out: DiffLine[] = [];
    for (let n = Math.max(1, start - revealed); n < start; n++) {
      const text = fileLines[n - 1];
      if (text === undefined) continue;
      out.push({ kind: 'context', text, newLine: n, oldLine: n - hunkMeta[hi].deltaBefore });
    }
    return out;
  };

  const lastEndNew = (() => {
    for (let j = hunkMeta.length - 1; j >= 0; j--) {
      if (hunkMeta[j].endNew != null) return hunkMeta[j].endNew;
    }
    return null;
  })();
  // A trailing newline yields a phantom empty last line — don't offer it
  const effectiveLen = fileLines ? (fileLines[fileLines.length - 1] === '' ? fileLines.length - 1 : fileLines.length) : 0;
  // In a created file every working-tree line is an addition, not context —
  // expanding past a truncated new-file hunk must stay green
  const createdFile = file.changeType === 'created';
  const tailLines: DiffLine[] = [];
  if (fileLines && lastEndNew != null && expandBelow > 0) {
    for (let n = lastEndNew + 1; n <= Math.min(effectiveLen, lastEndNew + expandBelow); n++) {
      const text = fileLines[n - 1];
      if (text === undefined) break;
      tailLines.push(
        createdFile
          ? { kind: 'add', text, newLine: n }
          : { kind: 'context', text, newLine: n, oldLine: n - totalDelta },
      );
    }
  }
  const tailRemaining = fileLines && lastEndNew != null ? Math.max(0, effectiveLen - lastEndNew - tailLines.length) : null;
  // For a created file the hunk carries the whole tree content up to the
  // truncation cap — when it already reaches linesAdded (the full count)
  // there is nothing below, so don't offer a dead ↓ expander pre-load
  const tailPossible = !(createdFile && lastEndNew != null && lastEndNew >= file.linesAdded);

  // --- Side-by-side divider: draggable left/right ratio, persisted ---
  const [splitPos, setSplitPos] = useState(() => {
    const v = Number(localStorage.getItem('ars:sxs-split'));
    return Number.isFinite(v) && v >= 25 && v <= 75 ? v : 50;
  });
  const diffRef = useRef<HTMLDivElement>(null);
  const dragDivider = (e: ReactPointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const el = diffRef.current;
    if (!el) return;
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const rect = el.getBoundingClientRect();
      setSplitPos(Math.min(75, Math.max(25, ((ev.clientX - rect.left) / rect.width) * 100)));
    };
    const up = () => {
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
      handle.removeEventListener('pointercancel', up);
      setSplitPos((v) => {
        localStorage.setItem('ars:sxs-split', String(Math.round(v * 10) / 10));
        return v;
      });
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
    handle.addEventListener('pointercancel', up);
  };

  const commentsByHunk = useMemo(() => {
    const byHunk = new Map<number, Map<string, LineComment>>();
    for (const c of comments) {
      const anchor = resolveAnchor(file, c);
      const hi = Number(anchor.split(':')[0]);
      if (!byHunk.has(hi)) byHunk.set(hi, new Map());
      byHunk.get(hi)!.set(anchor, c);
    }
    return byHunk;
  }, [comments, file]);

  const onFocusHandledRef = useRef(onFocusHandled);
  onFocusHandledRef.current = onFocusHandled;

  useEffect(() => {
    if (!focusLine) return;
    let exact: { hi: number; li: number } | null = null;
    let nearest: { hi: number; li: number; line: number } | null = null;
    for (let hi = 0; hi < file.hunks.length; hi++) {
      const lines = file.hunks[hi].lines;
      for (let li = 0; li < lines.length; li++) {
        const l = lines[li];
        if (l.newLine === focusLine && !exact) exact = { hi, li };
        if (l.newLine != null && l.newLine <= focusLine && (!nearest || l.newLine > nearest.line)) {
          nearest = { hi, li, line: l.newLine };
        }
      }
    }
    const target: { hi: number; li: number } | null = exact ?? nearest;
    if (!target) {
      onFocusHandledRef.current?.();
      return;
    }
    if (target.li >= MAX_RENDER_LINES) {
      setExpandedHunks((prev) => new Set(prev).add(target.hi));
    }
    // Two frames: let an expanded hunk render before scrolling into it.
    // Cancel both on cleanup so a quick file change cannot run this file's
    // onFocusHandled (clearing the next file's ?line).
    let inner = 0;
    const outer = requestAnimationFrame(() => {
      inner = requestAnimationFrame(() => {
        scrollFlash(`dl-${file.id}-${target.hi}-${target.li}`);
        onFocusHandledRef.current?.();
      });
    });
    return () => {
      cancelAnimationFrame(outer);
      cancelAnimationFrame(inner);
    };
  }, [focusLine, file]);

  if (file.hunks.length === 0) {
    return (
      <div className="diff-empty">
        {file.changeType === 'created' ? (
          <>
            <strong>New file</strong> — its content was not captured in the transcript.
            <div className="faint mt-2">You can still decide on it, or open it in your editor.</div>
          </>
        ) : (
          <>
            <strong>No diff captured</strong> for this change.
            <div className="faint mt-2">The transcript recorded the edit but not its patch content.</div>
          </>
        )}
      </div>
    );
  }

  const lineHeightPx = Math.round(fontSize * 1.6);
  // Two independent horizontal scrollers would fight each other — the
  // side-by-side view always wraps.
  const effectiveWrap = wrap || viewMode === 'split';

  return (
    <div
      ref={diffRef}
      className={`diff${viewMode === 'split' ? ' sxs' : ''}${effectiveWrap ? ' wrap' : ' nowrap'}`}
      style={{ fontSize, ...(viewMode === 'split' ? ({ '--sxs-split': `${splitPos}%` } as CSSProperties) : {}) }}
      onClick={
        onLineClick
          ? (e) => {
              const row = (e.target as HTMLElement).closest<HTMLElement>('[data-line]');
              const line = Number(row?.dataset.line);
              if (line > 0) onLineClick(line);
            }
          : undefined
      }
    >
      {viewMode === 'split' && (
        <div
          className="sxs-divider"
          style={{ left: `${splitPos}%` }}
          title="Drag to resize the panes — double-click to reset"
          onPointerDown={dragDivider}
          onDoubleClick={() => {
            setSplitPos(50);
            localStorage.setItem('ars:sxs-split', '50');
          }}
        />
      )}
      {file.changeType === 'created' && (
        <div className="diff-hunk-header" style={{ color: 'var(--ok)', background: 'var(--ok-dim)' }}>
          <span className="sticky-x">New file — {file.linesAdded} lines</span>
        </div>
      )}
      {file.hunks.map((hunk, hi) => {
        const meta = hunkMeta[hi];
        const revealed = expandAbove.get(hi) ?? 0;
        const gap = gapAbove(hi);
        const remaining = Math.max(0, gap - revealed);
        const wantsHeader = !(file.changeType === 'created' && file.hunks.length === 1);
        const expandableHunk = canExpand && meta.startNew != null;
        return (
          <Fragment key={hi}>
            {expandableHunk && remaining > 0 && (
              <ExpanderBar dir="up" label={hunk.header} remaining={remaining} onExpand={() => expandUp(hi, gap)} />
            )}
            {expandableHunk && remaining === 0 && revealed === 0 && wantsHeader && (
              <div className="diff-hunk-header">
                <span className="sticky-x">{hunk.header}</span>
              </div>
            )}
            <ContextLines lines={contextBefore(hi)} split={viewMode === 'split'} lang={lang} />
            <HunkView
              hunk={hunk}
              hi={hi}
              fileId={file.id}
              lang={lang}
              showHeader={!expandableHunk && wantsHeader}
              canComment={canComment}
              expanded={expandedHunks.has(hi)}
              onExpand={expandHunk}
              comments={commentsByHunk.get(hi) ?? EMPTY_COMMENTS}
              editingAnchor={editingHunk === hi ? editingAnchor : null}
              onEdit={setEditingAnchor}
              onSaveComment={onSaveComment}
              onDeleteComment={onDeleteComment}
              lineAnnotations={lineAnnotations}
              lineHeightPx={lineHeightPx}
              viewMode={viewMode}
              ranges={hunkRanges[hi] ?? EMPTY_RANGES}
            />
          </Fragment>
        );
      })}
      {canExpand && lastEndNew != null && tailPossible && (
        <>
          <ContextLines lines={tailLines} split={viewMode === 'split'} lang={lang} />
          {(tailRemaining === null || tailRemaining > 0) && (
            <ExpanderBar dir="down" remaining={tailRemaining} onExpand={expandDown} />
          )}
        </>
      )}
    </div>
  );
}
