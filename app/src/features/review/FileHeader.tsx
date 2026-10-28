import { useRef, useState } from 'react';
import type { FileVerdictView, ReviewFile, Verdict } from '../../types/review';
import type { DiffViewMode } from '../../components/DiffView';
import { IconArrowRight, IconCheck, IconFile, IconMore, IconX } from '../../components/icons';
import { fileName } from '../../lib/format';
import { changeKind, dirOf } from '../../lib/review';
import { useDismiss } from '../../state/useDismiss';

// The one toolbar of the review, above the code. On the left, the file and
// how much changed. On the right, every action in the order you use them:
// decide this file, then move on (to the next file, then to copying the
// review for the agent). View preferences sit behind "⋯": set once, not
// read every time.

export type DiffScope = 'all' | 'since';

/** Where the review goes after this file */
export type NextStep =
  | { kind: 'next'; onNext: () => void }
  | { kind: 'copy'; label: string; busy: boolean; onCopy: () => void }
  | { kind: 'done' }
  | { kind: 'none' };

interface FileHeaderProps {
  file: ReviewFile;
  verdict: FileVerdictView;
  onVerdict: (v: Verdict) => void;
  scope: DiffScope;
  onScope: (scope: DiffScope) => void;
  next: NextStep;
  /** Items in the review for the agent so far */
  reviewItems: number;
  reviewOpen: boolean;
  onToggleReview: () => void;
  viewMode: DiffViewMode;
  onToggleView: () => void;
  wrap: boolean;
  onToggleWrap: () => void;
  fontSize: number;
  onFontSize: (delta: number) => void;
  /** See the whole file, inside ARS */
  onViewFile?: () => void;
  editorLabel: string;
  onOpenInEditor?: () => void;
  onShortcuts: () => void;
}

type ViewProps = Pick<
  FileHeaderProps,
  'viewMode' | 'onToggleView' | 'wrap' | 'onToggleWrap' | 'fontSize' | 'onFontSize' | 'editorLabel' | 'onOpenInEditor' | 'onShortcuts'
>;

export function FileHeader({
  file,
  verdict,
  onVerdict,
  scope,
  onScope,
  next,
  reviewItems,
  reviewOpen,
  onToggleReview,
  onViewFile,
  ...view
}: FileHeaderProps) {
  const dir = dirOf(file.path);
  const canCompare = file.since === 'changed' && Boolean(file.sinceHunks);
  const kind = changeKind(file);
  return (
    <div className="file-head">
      <span className="file-head-path" title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}>
        {dir && <span className="faint">{dir}/</span>}
        <strong>{fileName(file.path)}</strong>
      </span>
      {kind && <span className="file-head-kind">{kind}</span>}
      <span className="file-head-size">
        {file.linesAdded > 0 && <span className="n-add">+{file.linesAdded}</span>}
        {file.linesRemoved > 0 && <span className="n-del">−{file.linesRemoved}</span>}
      </span>
      {onViewFile && (
        <button className="btn-mini" title="See the whole file, as it is now (f)" onClick={onViewFile}>
          <IconFile size={12} /> Whole file
        </button>
      )}
      {canCompare && (
        <span className="scope-toggle" title="Compare with the code as it was when you last copied the review">
          <button className={scope === 'since' ? 'on' : ''} onClick={() => onScope('since')}>Since last review</button>
          <button className={scope === 'all' ? 'on' : ''} onClick={() => onScope('all')}>All changes</button>
        </span>
      )}
      <span style={{ flex: 1 }} />
      <span className="file-verdict" role="group" aria-label="Decision on this file">
        <button
          className={`fv ok${verdict === 'approved' ? ' on' : ''}`}
          aria-pressed={verdict === 'approved'}
          title="Approve the changes you have not decided yet (a)"
          onClick={() => onVerdict('approved')}
        >
          <IconCheck size={14} /> {verdict === 'approved' ? 'Approved' : 'Approve'}
        </button>
        <button
          className={`fv err${verdict === 'rejected' ? ' on' : ''}`}
          aria-pressed={verdict === 'rejected'}
          title="Reject the file (r): the agent will be asked to undo it"
          onClick={() => onVerdict('rejected')}
        >
          <IconX size={14} /> {verdict === 'rejected' ? 'Rejected' : 'Reject'}
        </button>
      </span>
      <span className="toolbar-sep" aria-hidden="true" />
      {reviewItems > 0 && (
        <button
          className={`btn-tool${reviewOpen ? ' on' : ''}`}
          aria-expanded={reviewOpen}
          title="What the agent will get: read it, or edit it"
          onClick={onToggleReview}
        >
          Your review <span className="count">{reviewItems}</span>
        </button>
      )}
      {next.kind === 'next' && (
        <button className="btn btn-primary btn-tool-primary" title="The next file waiting for a decision (n)" onClick={next.onNext}>
          Next file <IconArrowRight size={14} />
        </button>
      )}
      {next.kind === 'copy' && (
        <button
          className="btn btn-primary btn-tool-primary"
          disabled={next.busy}
          title="Copy the review to paste into the agent. Next time you see only what changes from here."
          onClick={next.onCopy}
        >
          {next.label} <IconArrowRight size={14} />
        </button>
      )}
      {next.kind === 'done' && (
        <span className="done-chip" title="Nothing to send back: commit with your Git tools">
          <IconCheck size={13} /> All approved
        </span>
      )}
      <ViewMenu {...view} />
    </div>
  );
}

function ViewMenu({
  viewMode,
  onToggleView,
  wrap,
  onToggleWrap,
  fontSize,
  onFontSize,
  editorLabel,
  onOpenInEditor,
  onShortcuts,
}: ViewProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(open, () => setOpen(false), ref);
  return (
    <div className="menu-anchor" ref={ref}>
      <button
        className={`icon-btn${open ? ' active' : ''}`}
        title="View options"
        aria-label="View options"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <IconMore size={16} />
      </button>
      {open && (
        <div className="menu">
          <button aria-pressed={viewMode === 'split'} onClick={onToggleView}>
            <span className="menu-check">{viewMode === 'split' ? '✓' : ''}</span> Side by side
          </button>
          <button aria-pressed={wrap || viewMode === 'split'} disabled={viewMode === 'split'} onClick={onToggleWrap}>
            <span className="menu-check">{wrap || viewMode === 'split' ? '✓' : ''}</span> Wrap long lines
          </button>
          <div className="menu-row">
            <span className="menu-check" /> Text size
            <span style={{ flex: 1 }} />
            <button className="menu-step" title="Smaller (−)" aria-label="Smaller text" onClick={() => onFontSize(-1)}>−</button>
            <span className="mono">{fontSize}</span>
            <button className="menu-step" title="Larger (+)" aria-label="Larger text" onClick={() => onFontSize(1)}>+</button>
          </div>
          <div className="menu-sep" />
          {onOpenInEditor && (
            <button
              onClick={() => {
                setOpen(false);
                onOpenInEditor();
              }}
            >
              <span className="menu-check" /> Open in {editorLabel}
            </button>
          )}
          <button
            onClick={() => {
              setOpen(false);
              onShortcuts();
            }}
          >
            <span className="menu-check" /> Keyboard shortcuts <span style={{ flex: 1 }} /> <kbd className="kbd">?</kbd>
          </button>
        </div>
      )}
    </div>
  );
}
