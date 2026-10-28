import type { RefObject } from 'react';
import type { FileVerdictView, ProjectReview, ReviewFile, ReviewState } from '../../types/review';
import { fileName } from '../../lib/format';
import { attentionReasons, changeKind, dirOf, fileVerdict, type FileGroup } from '../../lib/review';

// The file list: one line per file with what you decided, its name and how
// big it is. What deserves a closer look comes first; the reasons are in
// the tooltip, not on screen. A dot marks a file the agent changed since
// the review you last copied.

/** Files past this count get a filter field */
const FILTER_FROM = 20;

const verdictLabel: Record<FileVerdictView, string> = {
  pending: 'Not reviewed yet',
  approved: 'Approved',
  rejected: 'Rejected: the agent will undo it',
  changes: 'Changes requested',
  mixed: 'Partly approved',
};

export function VerdictMark({ verdict }: { verdict: FileVerdictView }) {
  return <span className={`vmark ${verdict}`} role="img" aria-label={verdictLabel[verdict]} />;
}

interface FileListProps {
  review: ProjectReview;
  state: ReviewState;
  groups: FileGroup[];
  selectedId?: string;
  onSelect: (fileId: string) => void;
  reviewed: number;
  query: string;
  onQuery: (q: string) => void;
  filterRef: RefObject<HTMLInputElement>;
  onToggleSession: () => void;
  sessionOpen: boolean;
  /** A right-click on a file: its menu, at that point of the window */
  onFileMenu: (file: ReviewFile, x: number, y: number) => void;
}

export function FileList({
  review,
  state,
  groups,
  selectedId,
  onSelect,
  reviewed,
  query,
  onQuery,
  filterRef,
  onToggleSession,
  sessionOpen,
  onFileMenu,
}: FileListProps) {
  const total = review.files.length;
  const sessions = review.sessions;
  const requests = sessions.reduce((n, s) => n + Math.max(1, s.turns.length), 0);
  return (
    <aside className="files">
      <div className="files-progress" title={`${reviewed} of ${total} files have a verdict on every change`}>
        <span>
          <strong>{reviewed}</strong> of {total} files reviewed
        </span>
        <div className="progress-line">
          <i style={{ width: `${total ? (reviewed / total) * 100 : 100}%` }} />
        </div>
      </div>

      {total > FILTER_FROM && (
        <input
          ref={filterRef}
          className="files-filter"
          aria-label="Filter files"
          placeholder="Filter files  /"
          value={query}
          onChange={(e) => onQuery(e.target.value)}
        />
      )}

      <div className="files-scroll">
        {groups.map((group) => (
          <div key={group.key} className="files-group">
            {(groups.length > 1 || group.key !== 'routine') && <div className="files-label">{group.label}</div>}
            {group.files.map((f) => (
              <FileRow
                key={f.id}
                file={f}
                verdict={fileVerdict(state, f)}
                reasons={attentionReasons(f, review)}
                active={f.id === selectedId}
                onSelect={() => onSelect(f.id)}
                onMenu={(x, y) => onFileMenu(f, x, y)}
              />
            ))}
          </div>
        ))}
        {groups.length === 0 && <div className="files-empty">No files match.</div>}
      </div>

      <div className="files-foot">
        {sessions.length > 0 ? (
          <button
            className={`files-session${sessionOpen ? ' active' : ''}`}
            aria-expanded={sessionOpen}
            title="What you asked the agent, what it reported, what it did"
            onClick={onToggleSession}
          >
            {sessions[0].agentLabel}
            {sessions.length > 1 ? ` + ${sessions.length - 1} more` : ''} · {requests} request{requests === 1 ? '' : 's'}
          </button>
        ) : (
          <span className="files-nosession">
            No agent session found
            {review.disabledSources.length > 0 && <> · {review.disabledSources.join(', ')} is off in Settings</>}
          </span>
        )}
      </div>
    </aside>
  );
}

function FileRow({
  file,
  verdict,
  reasons,
  active,
  onSelect,
  onMenu,
}: {
  file: ReviewFile;
  verdict: FileVerdictView;
  reasons: string[];
  active: boolean;
  onSelect: () => void;
  onMenu: (x: number, y: number) => void;
}) {
  const dir = dirOf(file.path);
  const kind = file.changeType === 'renamed' ? `renamed from ${file.oldPath}` : changeKind(file);
  const title = [file.path, kind, verdictLabel[verdict], ...reasons].filter(Boolean).join('\n');
  return (
    <button
      className={`file-row${active ? ' active' : ''}${verdict !== 'pending' ? ' decided' : ''}`}
      title={title}
      onClick={onSelect}
      onContextMenu={(e) => {
        e.preventDefault();
        // From the keyboard (Shift+F10) there is no pointer: open under the row
        const fromPointer = e.clientX !== 0 || e.clientY !== 0;
        const row = e.currentTarget.getBoundingClientRect();
        onMenu(fromPointer ? e.clientX : row.left + 24, fromPointer ? e.clientY : row.bottom);
      }}
    >
      <VerdictMark verdict={verdict} />
      <span className="file-row-name">
        <span className={file.changeType === 'deleted' ? 'struck' : undefined}>{fileName(file.path)}</span>
        {dir && <span className="file-row-dir">{dir}</span>}
      </span>
      {file.since === 'changed' && <span className="since-dot" aria-label="changed since your last review" />}
      <span className="file-row-size">
        {file.changeType === 'created' ? <span className="n-add">new</span> : null}
        {file.changeType !== 'created' && file.linesAdded > 0 && <span className="n-add">+{file.linesAdded}</span>}
        {file.linesRemoved > 0 && <span className="n-del">−{file.linesRemoved}</span>}
      </span>
    </button>
  );
}
