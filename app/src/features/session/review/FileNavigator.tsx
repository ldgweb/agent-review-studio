import type { RefObject } from 'react';
import type { AgentSession, FileApproval, FileChange, RiskDimension } from '../../../types/domain';
import { IconComment, IconDiamond, IconFlag } from '../../../components/icons';
import { fileApprovalMeta } from '../../../lib/risk';
import { fileName } from '../../../lib/format';
import {
  describeWeight,
  fileWeight,
  findingsForFile,
  needsAttention,
  type FileGroup,
  type GroupMode,
  type RiskCoverage,
} from '../../../lib/triage';

// The file navigator — answers "what should I look at first?".
// Grouping modes turn the flat file list into a triage surface: by risk
// (needs attention vs routine), by system area, or by directory.

const riskDotColor: Record<RiskDimension, string> = {
  security: 'var(--risk-critical)',
  database: 'var(--risk-high)',
  performance: 'var(--risk-medium)',
  maintainability: 'var(--accent)',
};

const GROUP_MODES: Array<{ mode: GroupMode; label: string }> = [
  { mode: 'risk', label: 'Risk' },
  { mode: 'dir', label: 'Dir' },
  { mode: 'area', label: 'Area' },
];

interface FileNavigatorProps {
  session: AgentSession;
  groups: FileGroup[];
  groupMode: GroupMode;
  onGroupMode: (mode: GroupMode) => void;
  selectedId?: string;
  onSelect: (fileId: string) => void;
  approvalOf: (fileId: string) => FileApproval;
  commentsCountOf: (fileId: string) => number;
  coverage: RiskCoverage;
  query: string;
  onQuery: (query: string) => void;
  onlyPending: boolean;
  onOnlyPending: (value: boolean) => void;
  pendingCount: number;
  unverifiedIds: ReadonlySet<string>;
  filterRef: RefObject<HTMLInputElement>;
}

export function FileNavigator({
  session,
  groups,
  groupMode,
  onGroupMode,
  selectedId,
  onSelect,
  approvalOf,
  commentsCountOf,
  coverage,
  query,
  onQuery,
  onlyPending,
  onOnlyPending,
  pendingCount,
  unverifiedIds,
  filterRef,
}: FileNavigatorProps) {
  const totalFiles = session.files.length;
  const visibleCount = groups.reduce((sum, g) => sum + g.files.length, 0);

  const modeAvailable = (mode: GroupMode): boolean => {
    if (mode === 'area') return session.systemAreas.length > 0;
    return true;
  };

  return (
    <aside className="wb-sidebar">
      <div style={{ padding: '4px 10px 0' }}>
        <div className="spread" style={{ gap: 'var(--sp-2)' }}>
          <span className="section-label" style={{ marginBottom: 0 }}>Risk coverage</span>
          <span className="mono" style={{ fontSize: 'var(--fs-xs)', color: 'var(--text-1)' }}>
            {coverage.pct}%
          </span>
        </div>
        <div
          className="coverage-track mt-2"
          title={`Weighted by risk: deciding a risky file counts more (${coverage.done}/${coverage.total} weight decided)`}
        >
          <div className="coverage-fill" style={{ width: `${coverage.pct}%` }} />
        </div>
      </div>

      <div className="segmented mt-3" style={{ width: '100%' }}>
        {GROUP_MODES.map(({ mode, label }) => (
          <button
            key={mode}
            className={groupMode === mode ? 'active' : ''}
            style={{ flex: 1, padding: '4px 0' }}
            disabled={!modeAvailable(mode)}
            title={
              !modeAvailable(mode)
                ? 'No system areas detected in this session'
                : `Group files by ${label.toLowerCase()}`
            }
            onClick={() => onGroupMode(mode)}
          >
            {label}
          </button>
        ))}
      </div>

      {totalFiles > 6 && (
        <div className="stack" style={{ gap: 'var(--sp-2)', padding: '8px 4px', marginBottom: 'var(--sp-1)' }}>
          <input
            ref={filterRef}
            className="textarea"
            style={{ minHeight: 0, padding: '6px 10px', fontFamily: 'var(--font-mono)', fontSize: 'var(--fs-sm)' }}
            placeholder="Filter files…  ( / )"
            value={query}
            onChange={(e) => onQuery(e.target.value)}
          />
          <button className={`chip${onlyPending ? ' active' : ''}`} onClick={() => onOnlyPending(!onlyPending)}>
            {onlyPending ? '● ' : ''}Only undecided ({pendingCount})
          </button>
        </div>
      )}

      {visibleCount === 0 && (
        <div className="faint" style={{ padding: '8px 10px', fontSize: 'var(--fs-sm)' }}>No files match.</div>
      )}

      {groups.map((group) => {
        // No bulk approve here on purpose — every file earns its own verdict
        const added = group.files.reduce((sum, f) => sum + f.linesAdded, 0);
        const removed = group.files.reduce((sum, f) => sum + f.linesRemoved, 0);
        return (
          <div key={group.key}>
            <div className="nav-group-header">
              <span className="group-label" title={group.label}>{group.label}</span>
              <span className="mono" style={{ flexShrink: 0 }}>
                {group.files.length} · <span style={{ color: 'var(--diff-add-text)' }}>+{added}</span>{' '}
                <span style={{ color: 'var(--diff-del-text)' }}>−{removed}</span>
              </span>
              {group.meta && <span style={{ flexShrink: 0, fontStyle: 'italic' }}>{group.meta}</span>}
            </div>
            {group.files.map((f) => (
              <FileNavItem
                key={f.id}
                session={session}
                file={f}
                active={f.id === selectedId}
                state={approvalOf(f.id)}
                comments={commentsCountOf(f.id)}
                attention={needsAttention(f, session, unverifiedIds)}
                unverified={unverifiedIds.has(f.id)}
                showWeight={groupMode === 'risk' && group.key === 'attention'}
                onSelect={() => onSelect(f.id)}
              />
            ))}
          </div>
        );
      })}
    </aside>
  );
}

function FileNavItem({
  session,
  file,
  active,
  state,
  comments,
  attention,
  unverified,
  showWeight,
  onSelect,
}: {
  session: AgentSession;
  file: FileChange;
  active: boolean;
  state: FileApproval;
  comments: number;
  attention: boolean;
  unverified: boolean;
  showWeight: boolean;
  onSelect: () => void;
}) {
  const findings = findingsForFile(session, file).length;
  const weight = fileWeight(file, session);
  return (
    <button
      className={`file-nav-item${active ? ' active' : ''}${attention ? ' attention' : ''}`}
      title={showWeight ? `Weight ${weight.total} — ${describeWeight(weight)}` : undefined}
      onClick={onSelect}
    >
      <div className="flex" style={{ gap: 8 }}>
        <span className="dot" style={{ background: fileApprovalMeta[state].color }} title={fileApprovalMeta[state].label} />
        <span
          className="mono"
          style={{
            fontSize: 'var(--fs-sm)',
            fontWeight: active ? 600 : 400,
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap',
          }}
        >
          {fileName(file.path)}
        </span>
        {file.changeType === 'created' && (
          <span className="badge badge-ok" style={{ fontSize: 10, padding: '1px 6px' }}>new</span>
        )}
        <span style={{ marginLeft: 'auto', display: 'inline-flex', gap: 3, flexShrink: 0, alignItems: 'center' }}>
          {attention && (
            <span title="Needs attention" style={{ color: 'var(--risk-high)' }}><IconFlag size={9} /></span>
          )}
          {file.riskFlags.map((r) => (
            <span key={r} className="dot" style={{ background: riskDotColor[r], width: 6, height: 6 }} title={`${r} risk`} />
          ))}
        </span>
      </div>
      <div className="faint mono" style={{ fontSize: 'var(--fs-xs)', marginLeft: 15 }}>
        <span style={{ color: 'var(--diff-add-text)' }}>+{file.linesAdded}</span>{' '}
        <span style={{ color: 'var(--diff-del-text)' }}>−{file.linesRemoved}</span>
        {findings > 0 && (
          <span style={{ color: 'var(--risk-high)' }}> · {findings} finding{findings > 1 ? 's' : ''}</span>
        )}
        {comments > 0 && (
          <span style={{ color: 'var(--accent)' }}> · {comments} <IconComment size={9} /></span>
        )}
        {unverified && (
          <span title="Edited after the last successful test run"> · <IconDiamond size={9} /> not re-tested</span>
        )}
      </div>
    </button>
  );
}
