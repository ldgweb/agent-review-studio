import type { FileChange, RiskDimension } from '../../../types/domain';
import type { DiffViewMode } from '../../../components/DiffView';
import {
  IconAlert,
  IconChevronLeft,
  IconChevronRight,
  IconColumns,
  IconFocus,
  IconGitBranch,
  IconWrapText,
} from '../../../components/icons';
import { fileName } from '../../../lib/format';
import { dirOf } from '../../../lib/triage';

// The diff bar — 36px of chrome between the reviewer and the code. It owns
// WHAT you are looking at and HOW: file identity, navigation, view
// preferences. The verdict lives on the decision bar at the bottom —
// decisions down, preferences up.

const riskDotColor: Record<RiskDimension, string> = {
  security: 'var(--risk-critical)',
  database: 'var(--risk-high)',
  performance: 'var(--risk-medium)',
  maintainability: 'var(--accent)',
};

interface DiffBarProps {
  file: FileChange;
  onPrev: () => void;
  onNext: () => void;
  zen: boolean;
  onToggleZen: () => void;
  /** Session lives in a git work tree — the source badge can say so */
  gitRepo?: boolean;
  /** Open this file in the editor it was written in (Cursor / VS Code) */
  onOpenInEditor?: () => void;
  /** Diff font size controls (also on + / − keys) */
  fontSize: number;
  onFontSize: (delta: number) => void;
  onFontSizeReset: () => void;
  /** Long-line wrapping and unified/side-by-side (persisted preferences) */
  wrap: boolean;
  onToggleWrap: () => void;
  viewMode: DiffViewMode;
  onToggleView: () => void;
}

function SourceBadge({ file, gitRepo }: { file: FileChange; gitRepo?: boolean }) {
  if (file.drift === 'committed') {
    return (
      <span className="badge badge-neutral" style={{ fontSize: 10, padding: '0 6px' }}
        title="The change was committed after the session; the working tree shows no further diff">
        <IconGitBranch size={10} /> committed
      </span>
    );
  }
  if (file.drift === 'missing') {
    return (
      <span className="badge badge-warn" style={{ fontSize: 10, padding: '0 6px' }}
        title="The transcript modified this file but the working tree shows no change — reverted or overwritten">
        <IconAlert size={10} /> not in working tree
      </span>
    );
  }
  if (file.diffSource === 'git') {
    return (
      <span className="badge badge-ok" style={{ fontSize: 10, padding: '0 6px' }}
        title="Current working-tree diff — may include edits made after the session">
        <IconGitBranch size={10} /> working tree
      </span>
    );
  }
  if (gitRepo && file.diffSource === 'transcript') {
    return <span className="faint" style={{ fontSize: 10, flexShrink: 0 }}>transcript</span>;
  }
  return null;
}

export function DiffBar({
  file,
  onPrev,
  onNext,
  zen,
  onToggleZen,
  gitRepo,
  onOpenInEditor,
  fontSize,
  onFontSize,
  onFontSizeReset,
  wrap,
  onToggleWrap,
  viewMode,
  onToggleView,
}: DiffBarProps) {
  const dir = dirOf(file.path);
  return (
    <div className="diff-bar-wrap">
      <div className="diff-bar">
        <button className="btn btn-sm" title="Previous file (k)" onClick={onPrev}><IconChevronLeft /></button>
        <button className="btn btn-sm" title="Next file (j)" onClick={onNext}><IconChevronRight /></button>
        <span className="diff-file" title={file.path}>
          {dir && <span className="faint">{dir}/</span>}
          <strong>{fileName(file.path)}</strong>
        </span>
        {file.changeType === 'created' && (
          <span className="badge badge-ok" style={{ fontSize: 10, padding: '0 6px' }}>new</span>
        )}
        <span className="mono faint" style={{ fontSize: 'var(--fs-xs)', flexShrink: 0 }}>
          <span style={{ color: 'var(--diff-add-text)' }}>+{file.linesAdded}</span>{' '}
          <span style={{ color: 'var(--diff-del-text)' }}>−{file.linesRemoved}</span>
        </span>
        <span style={{ display: 'inline-flex', gap: 3, flexShrink: 0 }}>
          {file.riskFlags.map((r) => (
            <span key={r} className="dot" style={{ background: riskDotColor[r], width: 6, height: 6 }} title={`${r} risk`} />
          ))}
        </span>
        <SourceBadge file={file} gitRepo={gitRepo} />
        <span className="topbar-spacer" style={{ flex: 1 }} />
        <span className="diff-actions">
          {onOpenInEditor && (
            <button className="btn btn-sm" title="Open this file in your editor at the focused line" onClick={onOpenInEditor}>
              Editor ↗
            </button>
          )}
          <span className="diff-sep" />
          <button className="btn btn-sm" title="Smaller code font (−)" onClick={() => onFontSize(-1)}>
            A<span style={{ fontSize: 9 }}>−</span>
          </button>
          <button
            className="btn btn-sm mono"
            style={{ minWidth: 34, color: 'var(--text-2)' }}
            title="Code font size — click to reset"
            onClick={onFontSizeReset}
          >
            {fontSize}
          </button>
          <button className="btn btn-sm" title="Larger code font (+)" onClick={() => onFontSize(1)}>
            A<span style={{ fontSize: 9 }}>+</span>
          </button>
          <span className="diff-sep" />
          <button
            className={`btn btn-sm${wrap ? ' active' : ''}`}
            disabled={viewMode === 'split'}
            title={viewMode === 'split' ? 'Side-by-side always wraps' : wrap ? 'Long lines wrap — click for horizontal scrolling' : 'Long lines scroll horizontally — click to wrap'}
            onClick={onToggleWrap}
          >
            <IconWrapText />
          </button>
          <button
            className={`btn btn-sm${viewMode === 'split' ? ' active' : ''}`}
            title={viewMode === 'split' ? 'Back to the unified diff' : 'Side-by-side diff'}
            onClick={onToggleView}
          >
            <IconColumns />
          </button>
          <span className="diff-sep" />
          <button className={`btn btn-sm${zen ? ' active' : ''}`} title="Zen mode — code only (z)" onClick={onToggleZen}>
            <IconFocus />
          </button>
        </span>
      </div>
    </div>
  );
}
