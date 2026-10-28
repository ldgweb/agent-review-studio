import { Fragment, memo, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent as ReactKeyboardEvent, type MutableRefObject, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react';
import type { DiffLine, LineComment } from '../types/domain';
import { extToLang, highlightFile, highlightLine, renderHast, type HastNode } from '../lib/highlight';
import { alignRun, changedRanges, type IntraRange } from '../lib/intraline';
import { hunkSection } from '../lib/review';
import { scrollFlash } from '../lib/scroll';
import { useDismiss } from '../state/useDismiss';
import { usePref } from '../state/usePref';
import { IconComment, IconPencil } from './icons';

// The code-reading surface, modeled on today's best diff viewers (GitHub,
// VS Code, JetBrains): unified or side-by-side (with a draggable divider),
// long lines wrap by default (no-wrap scrolls horizontally with sticky
// gutters instead of clipping). Syntax colors come from the whole file, as
// in an editor; removed and added lines are paired by similarity and every
// changed word is marked; moved code links to where it came from. Hover any
// line to comment, or to suggest the code you want instead. Findings render
// inline at their line; the unchanged context around hunks expands from the
// working tree. Hunks are windowed with content-visibility so huge diffs
// stay smooth, and oversized hunks render behind a "show more" expander.

const MAX_RENDER_LINES = 400;
/** Context lines revealed per expander click (GitHub-style) */
const EXPAND_STEP = 20;
/** Code font size (px): the default, and the range the view menu allows */
export const DEFAULT_DIFF_FONT = 13;
export const MIN_DIFF_FONT = 10;
export const MAX_DIFF_FONT = 22;
/** Line height as a multiple of the font size (matches the CSS) */
export const LINE_HEIGHT = 1.6;
/** Side-by-side divider: where it starts, and how far it can go (% of width) */
const SPLIT_DEFAULT = 50;
const SPLIT_MIN = 25;
const SPLIT_MAX = 75;

export type DiffViewMode = 'unified' | 'split';

type ViewLine = DiffLine & { moved?: boolean; movedPeer?: { path: string; line: number } };
/** A hunk; with a content id, what is open in it survives the file changing */
type ViewHunk = { id?: string; header: string; lines: ViewLine[] };
/** Whole-file tokens for a line, when its file was highlighted */
type TokensOf = (line: DiffLine) => HastNode[] | undefined;
/** The open comment editor: its line, and whether it opened on the code */
interface Editing {
  anchor: string;
  suggest: boolean;
}
type OnEdit = (anchor: string | null, suggest?: boolean) => void;
/** What is typed in the open editor, kept by the diff so it survives the
 * editor moving when the agent rewrites the file */
type Draft = { anchor: string; text: string; suggestion?: string };
type DraftRef = MutableRefObject<Draft | null>;

/** A hunk's key for what is open in it: its content id, else its place */
const hunkKey = (h: ViewHunk, hi: number) => h.id ?? `#${hi}`;

/** Hover actions in the gutter, side by side: suggest the code you want
 * instead (not on a removed line), and comment */
function LineActions({ anchor, kind, onEdit }: { anchor: string; kind: DiffLine['kind']; onEdit: OnEdit }) {
  return (
    <span className="line-tools">
      {kind !== 'del' && (
        <button
          className="line-tool suggest"
          title="Suggest code: write this line the way you want it"
          aria-label="Suggest code for this line"
          onClick={() => onEdit(anchor, true)}
        >
          <IconPencil size={12} />
        </button>
      )}
      <button className="line-tool comment" title="Comment on this line" aria-label="Comment on this line" onClick={() => onEdit(anchor)}>
        <IconComment size={12} />
      </button>
    </span>
  );
}

/** What the viewer needs of a file: identity, kind and hunks. */
export interface DiffFile {
  id: string;
  path: string;
  language: string;
  changeType: string;
  linesAdded: number;
  hunks: ViewHunk[];
}

interface DiffViewProps {
  file: DiffFile;
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
  /** Loads the file's lines as they are now: the context around hunks expands */
  onLoadFileLines?: () => Promise<string[] | null>;
  /** Loads HEAD's lines of the file: removed lines get whole-file colors */
  onLoadBaseLines?: () => Promise<string[] | null>;
  /** focusLine is a HEAD line (a removed line), not a new one */
  focusOld?: boolean;
  /** A moved block's link was clicked: the other half's file and line
   * (old: a removed line, numbered as in HEAD) */
  onMovedPeer?: (path: string, line: number, old: boolean) => void;
  /** A diff line was clicked (its new-file line number) */
  onLineClick?: (line: number) => void;
  /** Replaces the plain "@@" header of hunk `hi` (origin, verdicts, noise) */
  renderHunkHeader?: (hi: number) => ReactNode;
  /** Hunks shown as their header only (noise folded away) */
  collapsedHunks?: ReadonlySet<number>;
  /** The hunk the keyboard is on */
  focusedHunk?: number;
  /** Open the comment editor on a hunk's first changed line. A new nonce
   * is a new request; the one the view mounts with is not replayed. */
  commentRequest?: { hi: number; nonce: number };
}

/** Writing to the agent about one line: words, or the code you want there
 * instead (GitHub's suggested change), edited in place in the diff's own
 * font, right under the line. The agent gets that code verbatim. */
function CommentEditor({
  initial,
  initialSuggestion,
  draft,
  request,
  lineText,
  lineNo,
  canSuggest,
  lang,
  onSave,
  onCancel,
}: {
  initial: string;
  initialSuggestion?: string;
  draft: DraftRef;
  /** The latest click that opened it: on the code (Suggest) or on the words
   * (Comment). Each click is a new object, so a second click on the same
   * line switches the editor rather than being ignored. */
  request: Editing;
  lineText: string;
  lineNo?: number;
  canSuggest: boolean;
  lang: string | null;
  onSave: (text: string, suggestion?: string) => void;
  onCancel: () => void;
}) {
  // What was typed before the editor moved, else what is saved
  const kept = draft.current?.anchor === request.anchor ? draft.current : null;
  const [text, setText] = useState(kept ? kept.text : initial);
  const [suggestion, setSuggestion] = useState<string | undefined>(
    kept ? kept.suggestion : (initialSuggestion ?? (request.suggest && canSuggest ? lineText : undefined)),
  );
  useEffect(() => {
    draft.current = { anchor: request.anchor, text, suggestion };
  }, [draft, request.anchor, text, suggestion]);
  useDismiss(true, onCancel);
  const suggests = suggestion !== undefined && suggestion !== lineText;
  const ready = Boolean(text.trim()) || suggests;
  const save = () => {
    if (ready) onSave(text.trim(), suggests ? suggestion : undefined);
  };
  const keys = (e: ReactKeyboardEvent) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) save();
  };
  const codeRef = useRef<HTMLTextAreaElement>(null);
  const textRef = useRef<HTMLTextAreaElement>(null);
  // Every click lands somewhere useful: Suggest opens the code (keeping what
  // is written), Comment goes to the words (dropping an untouched
  // suggestion). The caret moves there too, so Esc and Ctrl+Enter work.
  useEffect(() => {
    const onCode = request.suggest && canSuggest;
    if (onCode) setSuggestion((s) => s ?? lineText);
    else setSuggestion((s) => (s === lineText ? undefined : s));
    const frame = requestAnimationFrame(() => {
      const el = onCode ? codeRef.current : textRef.current;
      el?.focus();
      if (el) el.setSelectionRange(el.value.length, el.value.length);
    });
    return () => cancelAnimationFrame(frame);
  }, [request]);
  return (
    <div className="note editing">
      {suggestion !== undefined && (
        <div className="sugg">
          <div className="sugg-label">
            Suggested change{lineNo ? ` for line ${lineNo}` : ''} <span className="faint">· leave it empty to remove the line</span>
          </div>
          <div className="sugg-line del">
            <span className="sugg-sign">−</span>
            <span className="sugg-code">{highlightLine(lineText, lang)}</span>
          </div>
          <div className="sugg-line add editing">
            <span className="sugg-sign">+</span>
            <textarea
              ref={codeRef}
              className="sugg-input"
              spellCheck={false}
              rows={Math.max(1, suggestion.split('\n').length)}
              value={suggestion}
              onChange={(e) => setSuggestion(e.target.value)}
              onKeyDown={keys}
              aria-label="The code to put instead of this line"
            />
          </div>
        </div>
      )}
      <textarea
        ref={textRef}
        className="note-input"
        rows={2}
        placeholder={
          suggestion !== undefined ? 'Why (optional)' : 'What should the agent change here? End with “?” to ask a question.'
        }
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={keys}
      />
      <div className="note-bar">
        {canSuggest && (
          <button
            className="btn-mini"
            title="Write the code you want instead: the agent gets it verbatim"
            onClick={() => setSuggestion((s) => (s === undefined ? lineText : undefined))}
          >
            <IconPencil size={11} /> {suggestion === undefined ? 'Suggest code' : 'Remove the suggestion'}
          </button>
        )}
        <span style={{ flex: 1 }} />
        <button className="btn btn-sm" onClick={onCancel}>Cancel</button>
        <button className="btn btn-sm btn-primary" disabled={!ready} onClick={save} title="Ctrl+Enter">
          Save
        </button>
      </div>
    </div>
  );
}

/** A suggested change as the agent will get it: the line, then what replaces it */
function SuggestionView({ lineText, suggestion, lang }: { lineText: string; suggestion: string; lang: string | null }) {
  return (
    <div className="sugg">
      <div className="sugg-label">Suggested change</div>
      <div className="sugg-line del">
        <span className="sugg-sign">−</span>
        <span className="sugg-code">{highlightLine(lineText, lang)}</span>
      </div>
      {suggestion === '' ? (
        <div className="sugg-line removed">
          <span className="sugg-sign" />
          <span className="sugg-code faint">the line is removed</span>
        </div>
      ) : (
        suggestion.split('\n').map((l, i) => (
          <div key={i} className="sugg-line add">
            <span className="sugg-sign">+</span>
            <span className="sugg-code">{highlightLine(l, lang)}</span>
          </div>
        ))
      )}
    </div>
  );
}

const EMPTY_COMMENTS: ReadonlyMap<string, LineComment> = new Map();
const EMPTY_RANGES: ReadonlyMap<number, IntraRange[]> = new Map();

/** What every line needs to render, shared by the whole file */
interface LineRender {
  lang: string | null;
  tokensOf: TokensOf;
  filePath: string;
  onMovedPeer?: (path: string, line: number, old: boolean) => void;
}

/** The line text with its changed word-level segments marked behind it: a
 * transparent copy sits under the tokenized code so the marks never fight
 * the syntax colors. */
function CodeText({ line, ranges, rc }: { line: ViewLine; ranges?: IntraRange[]; rc: LineRender }) {
  const { text, kind } = line;
  const tokens = rc.tokensOf(line);
  const peer = line.movedPeer;
  const underlay: ReactNode[] = [];
  if (ranges?.length) {
    let pos = 0;
    ranges.forEach((r, i) => {
      if (r.start > pos) underlay.push(text.slice(pos, r.start));
      underlay.push(
        <span key={i} className={`seg-${kind}`}>
          {text.slice(r.start, r.end)}
        </span>,
      );
      pos = r.end;
    });
    if (pos < text.length) underlay.push(text.slice(pos));
  }
  return (
    <>
      {underlay.length > 0 && (
        <span className="seg-underlay" aria-hidden="true">
          {underlay}
        </span>
      )}
      {tokens ? renderHast(tokens) : highlightLine(text, rc.lang)}
      {peer && rc.onMovedPeer && (
        <button
          className="moved-peer"
          title={kind === 'add' ? 'This block was removed there: open it' : 'This block was added there: open it'}
          onClick={(e) => {
            e.stopPropagation();
            rc.onMovedPeer?.(peer.path, peer.line, kind === 'add');
          }}
        >
          {kind === 'add' ? 'moved from' : 'moved to'} {peer.path === rc.filePath ? `line ${peer.line}` : `${peer.path}:${peer.line}`}
        </button>
      )}
    </>
  );
}

interface SplitSide {
  line: ViewLine;
  li: number;
}

interface SplitRow {
  left?: SplitSide;
  right?: SplitSide;
}

/** Rows for the side-by-side view: context spans both sides; each run of
 * removed lines meets the added run after it line by line, paired by
 * similarity (an edited line sits next to its new version even when lines
 * were inserted above it). */
function buildSplitRows(lines: ViewLine[]): SplitRow[] {
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
    const dels = lines.slice(delStart, addStart).map((l) => l.text);
    const adds = lines.slice(addStart, i).map((l) => l.text);
    for (const row of alignRun(dels, adds)) {
      rows.push({
        left: row.del !== null ? { line: lines[delStart + row.del], li: delStart + row.del } : undefined,
        right: row.add !== null ? { line: lines[addStart + row.add], li: addStart + row.add } : undefined,
      });
    }
  }
  return rows;
}

function SplitCell({
  side,
  num,
  id,
  rc,
  ranges,
  canComment,
  commented,
  onEdit,
  hi,
}: {
  side?: SplitSide;
  num?: number;
  id?: string;
  rc: LineRender;
  ranges: ReadonlyMap<number, IntraRange[]>;
  canComment: boolean;
  commented: boolean;
  onEdit: OnEdit;
  hi: number;
}) {
  if (!side) return <div className="split-side none" />;
  const { line, li } = side;
  return (
    <div id={id} className={`split-side ${line.kind}${line.moved ? ' moved' : ''}${commented ? ' commented' : ''}`} data-line={line.newLine}>
      <span className="ln">{num ?? ''}</span>
      <span className="sign">
        {canComment && <LineActions anchor={`${hi}:${li}`} kind={line.kind} onEdit={onEdit} />}
        <span className="sign-char">{line.kind === 'add' ? '+' : line.kind === 'del' ? '−' : ''}</span>
      </span>
      <span className="code">
        <CodeText line={line} ranges={ranges.get(li)} rc={rc} />
      </span>
    </div>
  );
}

/** Expanded lines pulled from the working tree, in either view mode.
 * Usually unchanged context; for a created file they are additions (the
 * whole file is new) and render green with an empty old side. */
function ContextLines({ lines, split, rc }: { lines: DiffLine[]; split: boolean; rc: LineRender }) {
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
                  <CodeText line={l} rc={rc} />
                </span>
              </div>
            )}
            <div className={`split-side ${l.kind}`}>
              <span className="ln">{l.newLine ?? ''}</span>
              <span className="sign">
                <span className="sign-char">{l.kind === 'add' ? '+' : ''}</span>
              </span>
              <span className="code">
                <CodeText line={l} rc={rc} />
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
            <CodeText line={l} rc={rc} />
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
          title={dir === 'up' ? 'Show more lines above' : 'Show more lines below'}
          onClick={onExpand}
        >
          {dir === 'up' ? '↑' : '↓'} {step} more line{step === 1 ? '' : 's'}
        </button>
        {label && <span className="ctx-label">{label}</span>}
      </span>
    </div>
  );
}

interface HunkViewProps {
  hunk: ViewHunk;
  focused: boolean;
  hi: number;
  fileId: string;
  rc: LineRender;
  showHeader: boolean;
  canComment: boolean;
  /** Every line shown, past MAX_RENDER_LINES */
  expanded: boolean;
  onExpand: (key: string) => void;
  /** This hunk's comments, by anchor */
  comments: ReadonlyMap<string, LineComment>;
  /** The open comment editor, only when it is in this hunk */
  editing: Editing | null;
  onEdit: OnEdit;
  draft: DraftRef;
  onSaveComment?: (comment: LineComment) => void;
  onDeleteComment?: (anchor: string) => void;
  lineAnnotations?: Map<number, ReactNode>;
  /** Estimated rendered line height for contain-intrinsic-size */
  lineHeightPx: number;
  viewMode: DiffViewMode;
  /** line index → changed word-level segments (paired del/add lines) */
  ranges: ReadonlyMap<number, IntraRange[]>;
}

const HunkView = memo(function HunkView({
  hunk,
  focused,
  hi,
  fileId,
  rc,
  showHeader,
  canComment,
  expanded,
  onExpand,
  comments,
  editing,
  onEdit,
  draft,
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

  // A comment (or the editor) renders right under its line, across the
  // code column, in both view modes: what you ask sits where you asked it.
  const commentBlock = (anchor: string, lineNo?: number): ReactNode => {
    const existing = comments.get(anchor);
    const line = hunk.lines[Number(anchor.split(':')[1])];
    const isEditing = editing?.anchor === anchor;
    if (existing && !isEditing) {
      return (
        <div className="note-row">
          <div className="note">
            <div className="note-head">
              <span className="note-who">You{existing.line ? ` · line ${existing.line}` : ''}</span>
              <span className="note-actions">
                <button className="btn-mini" onClick={() => onEdit(anchor, existing.suggestion !== undefined)}>Edit</button>
                <button className="btn-mini" onClick={() => onDeleteComment?.(existing.anchor)}>Delete</button>
              </span>
            </div>
            {existing.suggestion !== undefined && line && (
              <SuggestionView lineText={line.text} suggestion={existing.suggestion} lang={rc.lang} />
            )}
            {existing.text && <div className="note-text">{existing.text}</div>}
          </div>
        </div>
      );
    }
    if (isEditing) {
      return (
        <div className="note-row">
          <CommentEditor
            initial={existing?.text ?? ''}
            initialSuggestion={existing?.suggestion}
            draft={draft}
            request={editing}
            lineText={line?.text ?? ''}
            lineNo={lineNo}
            canSuggest={Boolean(line) && line.kind !== 'del'}
            lang={rc.lang}
            onCancel={() => onEdit(null)}
            onSave={(text, suggestion) => {
              onSaveComment?.({ anchor, line: lineNo, text, suggestion });
              onEdit(null);
            }}
          />
        </div>
      );
    }
    return null;
  };

  return (
    <div style={style} className={focused ? 'hunk-body focused' : 'hunk-body'}>
      {showHeader && (
        <div className="diff-hunk-header">
          <span className="sticky-x">{hunkSection(hunk.header) || '⋯'}</span>
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
                className={`diff-line ${line.kind}${line.moved ? ' moved' : ''}${comments.has(anchor) ? ' commented' : ''}`}
                data-line={line.newLine}
              >
                <span className="ln">{line.oldLine ?? ''}</span>
                <span className="ln">{line.newLine ?? ''}</span>
                <span className="sign">
                  {canComment && <LineActions anchor={anchor} kind={line.kind} onEdit={onEdit} />}
                  <span className="sign-char">{line.kind === 'add' ? '+' : line.kind === 'del' ? '−' : ''}</span>
                </span>
                <span className="code">
                  <CodeText line={line} ranges={ranges.get(li)} rc={rc} />
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
                  // context rows share one li: the right cell owns the id then
                  id={row.left && row.left.li !== row.right?.li ? `dl-${fileId}-${hi}-${row.left.li}` : undefined}
                  rc={rc}
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
                  rc={rc}
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
        <button className="diff-expander" onClick={() => onExpand(hunkKey(hunk, hi))}>
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
  onLoadBaseLines,
  focusOld,
  onMovedPeer,
  onLineClick,
  renderHunkHeader,
  collapsedHunks,
  focusedHunk,
  commentRequest,
}: DiffViewProps) {
  const [editing, setEditing] = useState<Editing | null>(null);
  const draft = useRef<Draft | null>(null);
  const onEdit = useCallback<OnEdit>((anchor, suggest = false) => {
    if (!anchor) draft.current = null;
    setEditing(anchor ? { anchor, suggest } : null);
  }, []);

  // The agent can rewrite the file while you type. The editor follows its
  // line (same hunk, same text) and closes only when that line is gone.
  const shown = useRef(file);
  useEffect(() => {
    const before = shown.current;
    shown.current = file;
    if (before === file) return;
    setEditing((e) => {
      if (!e) return e;
      const [hi, li] = e.anchor.split(':').map(Number);
      const hunk = before.hunks[hi];
      const line = hunk?.lines[li];
      const nhi = hunk ? file.hunks.findIndex((h, i) => hunkKey(h, i) === hunkKey(hunk, hi)) : -1;
      const nli = line && nhi >= 0 ? file.hunks[nhi].lines.findIndex((l) => l.kind === line.kind && l.text === line.text) : -1;
      if (nli < 0) {
        draft.current = null;
        return null;
      }
      const anchor = `${nhi}:${nli}`;
      if (draft.current) draft.current = { ...draft.current, anchor };
      return { ...e, anchor };
    });
  }, [file]);

  const seenRequest = useRef(commentRequest?.nonce);
  useEffect(() => {
    if (!commentRequest || commentRequest.nonce === seenRequest.current || !onSaveComment) return;
    seenRequest.current = commentRequest.nonce;
    const lines = file.hunks[commentRequest.hi]?.lines ?? [];
    const li = Math.max(0, lines.findIndex((l) => l.kind !== 'context'));
    onEdit(`${commentRequest.hi}:${li}`);
    requestAnimationFrame(() =>
      document.getElementById(`dl-${file.id}-${commentRequest.hi}-${li}`)?.scrollIntoView({ block: 'center', behavior: 'smooth' }),
    );
    // Only a new request opens the editor, on the file shown now
  }, [commentRequest?.nonce]);

  const [expandedHunks, setExpandedHunks] = useState<Set<string>>(new Set());
  const expandHunk = useCallback((key: string) => setExpandedHunks((prev) => new Set(prev).add(key)), []);
  const editingHunk = editing === null ? null : Number(editing.anchor.split(':')[0]);
  const canComment = Boolean(onSaveComment);
  const lang = useMemo(() => extToLang(file.language), [file.language]);

  const hunkRanges = useMemo(() => file.hunks.map((h) => changedRanges(h)), [file]);

  // --- Context expansion: unchanged lines pulled from the working tree ---
  const [fileLines, setFileLines] = useState<string[] | null>(null);
  const [expandFailed, setExpandFailed] = useState(false);
  /** Lines revealed above each hunk, by hunk key */
  const [expandAbove, setExpandAbove] = useState<Map<string, number>>(new Map());
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

  // --- Whole-file syntax colors: the file now, and HEAD's version for the
  // removed lines. Reloaded whenever the file's changes do (a new `file`).
  const [baseLines, setBaseLines] = useState<string[] | null>(null);
  useEffect(() => {
    linesPromise.current = null;
    setFileLines(null);
    setBaseLines(null);
    setExpandFailed(false);
    if (!lang) return;
    let live = true;
    if (onLoadFileLines && file.changeType !== 'deleted') {
      const pending = onLoadFileLines();
      linesPromise.current = pending;
      void pending.then((lines) => {
        if (live && lines?.length) setFileLines(lines);
      });
    }
    if (onLoadBaseLines && file.changeType !== 'created') {
      void onLoadBaseLines().then((lines) => {
        if (live && lines?.length) setBaseLines(lines);
      });
    }
    return () => {
      live = false;
    };
    // The loaders are read when the file changes, not followed
  }, [file, lang]);
  const newTokens = useMemo(() => (fileLines ? highlightFile(fileLines, lang) : null), [fileLines, lang]);
  const oldTokens = useMemo(() => (baseLines ? highlightFile(baseLines, lang) : null), [baseLines, lang]);
  const tokensOf = useCallback<TokensOf>(
    (l) => {
      // A line takes the file's tokens only while its text is that line's
      if (l.kind === 'del') {
        return l.oldLine != null && baseLines?.[l.oldLine - 1] === l.text ? oldTokens?.[l.oldLine - 1] : undefined;
      }
      return l.newLine != null && fileLines?.[l.newLine - 1] === l.text ? newTokens?.[l.newLine - 1] : undefined;
    },
    [fileLines, baseLines, newTokens, oldTokens],
  );
  const movedRef = useRef(onMovedPeer);
  movedRef.current = onMovedPeer;
  const hasMovedLinks = Boolean(onMovedPeer);
  const rc = useMemo<LineRender>(
    () => ({
      lang,
      tokensOf,
      filePath: file.path,
      onMovedPeer: hasMovedLinks ? (path, line, old) => movedRef.current?.(path, line, old) : undefined,
    }),
    [lang, tokensOf, file.path, hasMovedLinks],
  );
  const expandUp = async (key: string, gap: number) => {
    if (!(await ensureLines())) return;
    setExpandAbove((prev) => {
      const next = new Map(prev);
      next.set(key, Math.min(gap, (prev.get(key) ?? 0) + EXPAND_STEP));
      return next;
    });
  };
  const expandDown = async () => {
    if (!(await ensureLines())) return;
    setExpandBelow((prev) => prev + EXPAND_STEP);
  };

  // The gap above a hunk can shrink when the file changes: never reveal
  // more than it holds now
  const revealedAbove = (hi: number) => Math.min(expandAbove.get(hunkKey(file.hunks[hi], hi)) ?? 0, gapAbove(hi));
  const contextBefore = (hi: number): DiffLine[] => {
    const revealed = revealedAbove(hi);
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
  // A trailing newline yields a phantom empty last line: don't offer it
  const effectiveLen = fileLines ? (fileLines[fileLines.length - 1] === '' ? fileLines.length - 1 : fileLines.length) : 0;
  // In a created file every line is an addition, not context: expanding
  // past a truncated new-file hunk must stay green
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
  // A created file's hunk carries the whole file up to the truncation cap.
  // When it already reaches linesAdded (the full count) there is nothing
  // below, so no ↓ expander is offered before the file loads.
  const tailPossible = !(createdFile && lastEndNew != null && lastEndNew >= file.linesAdded);

  // --- Side-by-side divider: draggable left/right ratio, persisted ---
  const [splitPos, setSplitPos] = usePref<number>('ars:sxs-split', SPLIT_DEFAULT, (v) => Number(v) >= SPLIT_MIN && Number(v) <= SPLIT_MAX);
  const diffRef = useRef<HTMLDivElement>(null);
  const dragDivider = (e: ReactPointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const el = diffRef.current;
    if (!el) return;
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const rect = el.getBoundingClientRect();
      const pos = Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, ((ev.clientX - rect.left) / rect.width) * 100));
      setSplitPos(Math.round(pos * 10) / 10);
    };
    const up = () => {
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
      handle.removeEventListener('pointercancel', up);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
    handle.addEventListener('pointercancel', up);
  };

  const commentsByHunk = useMemo(() => {
    const byHunk = new Map<number, Map<string, LineComment>>();
    for (const c of comments) {
      const hi = Number(c.anchor.split(':')[0]);
      if (!byHunk.has(hi)) byHunk.set(hi, new Map());
      byHunk.get(hi)!.set(c.anchor, c);
    }
    return byHunk;
  }, [comments]);

  const onFocusHandledRef = useRef(onFocusHandled);
  onFocusHandledRef.current = onFocusHandled;

  useEffect(() => {
    if (!focusLine) return;
    let exact: { hi: number; li: number } | null = null;
    let nearest: { hi: number; li: number; line: number } | null = null;
    const numberOf = (l: DiffLine) => (focusOld ? (l.kind === 'add' ? undefined : l.oldLine) : l.newLine);
    for (let hi = 0; hi < file.hunks.length; hi++) {
      const lines = file.hunks[hi].lines;
      for (let li = 0; li < lines.length; li++) {
        const n = numberOf(lines[li]);
        if (n === focusLine && !exact) exact = { hi, li };
        if (n != null && n <= focusLine && (!nearest || n > nearest.line)) nearest = { hi, li, line: n };
      }
    }
    const target: { hi: number; li: number } | null = exact ?? nearest;
    if (!target) {
      onFocusHandledRef.current?.();
      return;
    }
    if (target.li >= MAX_RENDER_LINES) {
      setExpandedHunks((prev) => new Set(prev).add(hunkKey(file.hunks[target.hi], target.hi)));
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
  }, [focusLine, focusOld, file]);

  if (file.hunks.length === 0) {
    return (
      <div className="diff-empty">
        <strong>No line changes</strong> to show here.
      </div>
    );
  }

  const lineHeightPx = Math.round(fontSize * LINE_HEIGHT);
  // Two horizontal scrollers would fight each other, so the side-by-side
  // view always wraps.
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
          title="Drag to resize. Double-click to reset."
          onPointerDown={dragDivider}
          onDoubleClick={() => setSplitPos(SPLIT_DEFAULT)}
        />
      )}
      {file.hunks.map((hunk, hi) => {
        const meta = hunkMeta[hi];
        const key = hunkKey(hunk, hi);
        const revealed = revealedAbove(hi);
        const gap = gapAbove(hi);
        const remaining = Math.max(0, gap - revealed);
        const custom = Boolean(renderHunkHeader);
        const wantsHeader = !custom && !(file.changeType === 'created' && file.hunks.length === 1);
        const expandableHunk = canExpand && meta.startNew != null;
        const collapsed = collapsedHunks?.has(hi) ?? false;
        return (
          <Fragment key={key}>
            {custom && (
              <div id={`hk-${file.id}-${hi}`} className={`hunk-head-slot${focusedHunk === hi ? ' focused' : ''}`}>
                {renderHunkHeader?.(hi)}
              </div>
            )}
            {!collapsed && expandableHunk && remaining > 0 && (
              <ExpanderBar dir="up" label={custom ? undefined : hunkSection(hunk.header)} remaining={remaining} onExpand={() => expandUp(key, gap)} />
            )}
            {!collapsed && expandableHunk && remaining === 0 && revealed === 0 && wantsHeader && (
              <div className="diff-hunk-header">
                <span className="sticky-x">{hunkSection(hunk.header) || '⋯'}</span>
              </div>
            )}
            {!collapsed && <ContextLines lines={contextBefore(hi)} split={viewMode === 'split'} rc={rc} />}
            {!collapsed && (
            <HunkView
              hunk={hunk}
              focused={focusedHunk === hi}
              hi={hi}
              fileId={file.id}
              rc={rc}
              showHeader={!expandableHunk && wantsHeader}
              canComment={canComment}
              expanded={expandedHunks.has(key)}
              onExpand={expandHunk}
              comments={commentsByHunk.get(hi) ?? EMPTY_COMMENTS}
              editing={editingHunk === hi ? editing : null}
              onEdit={onEdit}
              draft={draft}
              onSaveComment={onSaveComment}
              onDeleteComment={onDeleteComment}
              lineAnnotations={lineAnnotations}
              lineHeightPx={lineHeightPx}
              viewMode={viewMode}
              ranges={hunkRanges[hi] ?? EMPTY_RANGES}
            />
            )}
          </Fragment>
        );
      })}
      {canExpand && lastEndNew != null && tailPossible && (
        <>
          <ContextLines lines={tailLines} split={viewMode === 'split'} rc={rc} />
          {(tailRemaining === null || tailRemaining > 0) && (
            <ExpanderBar dir="down" remaining={tailRemaining} onExpand={expandDown} />
          )}
        </>
      )}
    </div>
  );
}
