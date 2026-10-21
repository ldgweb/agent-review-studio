import { useMemo, useState } from 'react';
import type {
  AgentSession,
  FindingKind,
  FindingReviewState,
  FindingSeverity,
  ReviewerFinding,
} from '../../../types/domain';
import { SeverityPill } from '../../../components/ui';
import { IconCheck, IconRotateCcw, IconX } from '../../../components/icons';
import { findingCounts, findingKindLabel, severityRank, sortFindings } from '../../../lib/triage';

// Findings panel: severity-descending sort with potential-bug
// first, kind/severity counter chips that filter, and per-finding actions
// that feed the approval state and the correction draft.

interface FindingCardProps {
  finding: ReviewerFinding;
  state?: FindingReviewState;
  onSetState: (findingId: string, state?: FindingReviewState) => void;
  /** Present when the finding maps to a changed file */
  onJump?: () => void;
  /** Skip title/path/line row — the diff line above the card already says it */
  compact?: boolean;
}

export function FindingCard({ finding, state, onSetState, onJump, compact }: FindingCardProps) {
  const toggle = (next: FindingReviewState) => onSetState(finding.id, state === next ? undefined : next);
  return (
    <div
      className={`banner finding-row${state === 'dismissed' ? ' dismissed' : ''}${state === 'accepted' ? ' accepted' : ''}`}
      style={{
        background: 'var(--bg-1)',
        borderColor: 'var(--border-1)',
        padding: 'var(--sp-3) var(--sp-4)',
      }}
    >
      <SeverityPill severity={finding.severity} />
      <span style={{ flex: 1, minWidth: 0 }}>
        <span className="flex flex-wrap" style={{ gap: 'var(--sp-2)' }}>
          <strong>{finding.title}</strong>
          <span className="badge badge-neutral" style={{ fontSize: 10, padding: '1px 6px' }}>
            {findingKindLabel(finding.kind)}
          </span>
          {finding.source === 'custom' && (
            <span
              className="badge badge-accent"
              style={{ fontSize: 10, padding: '1px 6px' }}
              title="From your custom review checks (Settings → Review checks, or the repo's .ars/rules.json)"
            >
              custom check
            </span>
          )}
          {!compact && finding.filePath && (
            <span className="faint mono" style={{ fontSize: 'var(--fs-xs)' }}>
              {finding.filePath}
              {finding.line ? `:${finding.line}` : ''}
            </span>
          )}
          {state === 'accepted' && (
            <span className="badge badge-ok" style={{ fontSize: 10, padding: '1px 6px' }}>
              <IconCheck size={9} /> accepted
            </span>
          )}
          {state === 'correction' && (
            <span className="badge badge-warn" style={{ fontSize: 10, padding: '1px 6px' }}>
              <IconRotateCcw size={9} /> in correction draft
            </span>
          )}
        </span>
        <div className="muted" style={{ fontSize: 'var(--fs-sm)', marginTop: 2 }}>{finding.body}</div>
        {finding.suggestedFix && (
          <code
            style={{
              display: 'block',
              marginTop: 6,
              padding: '4px 8px',
              background: 'var(--bg-0)',
              borderRadius: 'var(--radius-sm)',
              whiteSpace: 'pre-wrap',
            }}
          >
            {finding.suggestedFix}
          </code>
        )}
        <div className="finding-actions mt-2">
          {onJump && (
            <button className="btn btn-sm" title="Open this file in the diff" onClick={onJump}>
              {finding.line ? `Jump to line ${finding.line}` : 'Jump to file'}
            </button>
          )}
          <button
            className={`btn btn-sm btn-warn${state === 'correction' ? ' active' : ''}`}
            title="Include this finding in the correction request for the agent"
            onClick={() => toggle('correction')}
          >
            <IconRotateCcw size={11} /> Request correction
          </button>
          <button
            className={`btn btn-sm btn-ok${state === 'accepted' ? ' active' : ''}`}
            title="Mark as valid and acknowledged"
            onClick={() => toggle('accepted')}
          >
            <IconCheck size={11} /> Accept
          </button>
          <button
            className={`btn btn-sm${state === 'dismissed' ? ' active' : ''}`}
            title="Not relevant — hide from the panel"
            onClick={() => toggle('dismissed')}
          >
            <IconX size={11} /> Dismiss
          </button>
        </div>
      </span>
    </div>
  );
}

interface FindingsPanelProps {
  session: AgentSession;
  findingStates: Record<string, FindingReviewState>;
  onSetFindingState: (findingId: string, state?: FindingReviewState) => void;
  onJumpToFile: (fileId: string, line?: number) => void;
}

export function FindingsPanel({ session, findingStates, onSetFindingState, onJumpToFile }: FindingsPanelProps) {
  const sorted = useMemo(() => sortFindings(session.findings), [session.findings]);
  const counts = useMemo(() => findingCounts(session.findings), [session.findings]);
  const [kindFilter, setKindFilter] = useState<Set<FindingKind>>(new Set());
  const [severityFilter, setSeverityFilter] = useState<Set<FindingSeverity>>(new Set());
  const [showDismissed, setShowDismissed] = useState(false);

  if (sorted.length === 0) return null;

  const toggleSet = <T,>(set: Set<T>, value: T, apply: (next: Set<T>) => void) => {
    const next = new Set(set);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    apply(next);
  };

  const dismissedCount = sorted.filter((f) => findingStates[f.id] === 'dismissed').length;
  const visible = sorted.filter((f) => {
    if (!showDismissed && findingStates[f.id] === 'dismissed') return false;
    if (kindFilter.size > 0 && !kindFilter.has(f.kind)) return false;
    if (severityFilter.size > 0 && !severityFilter.has(f.severity)) return false;
    return true;
  });

  const fileIdByPath = new Map(session.files.map((f) => [f.path, f.id]));

  return (
    <div className="card" style={{ padding: 'var(--sp-3) var(--sp-4)' }}>
      <div style={{ fontWeight: 600 }}>
        Findings ({sorted.length}){' '}
        <span className="faint" style={{ fontWeight: 400, fontSize: 'var(--fs-xs)' }}>local checks</span>
      </div>
      <div className="chip-row mt-3">
        {(Object.entries(counts.byKind) as Array<[FindingKind, number]>).map(([kind, n]) => (
          <button
            key={kind}
            className={`chip${kindFilter.has(kind) ? ' active' : ''}`}
            onClick={() => toggleSet(kindFilter, kind, setKindFilter)}
          >
            {n} {findingKindLabel(kind)}
          </button>
        ))}
        {(Object.entries(counts.bySeverity) as Array<[FindingSeverity, number]>)
          .sort((a, b) => severityRank(b[0]) - severityRank(a[0]))
          .map(([severity, n]) => (
            <button
              key={severity}
              className={`chip${severityFilter.has(severity) ? ' active' : ''}`}
              onClick={() => toggleSet(severityFilter, severity, setSeverityFilter)}
            >
              {n} {severity}
            </button>
          ))}
        {dismissedCount > 0 && (
          <button className={`chip${showDismissed ? ' active' : ''}`} onClick={() => setShowDismissed(!showDismissed)}>
            {dismissedCount} dismissed
          </button>
        )}
      </div>
      <div className="stack mt-3" style={{ gap: 'var(--sp-2)' }}>
        {visible.length === 0 && (
          <div className="faint" style={{ fontSize: 'var(--fs-sm)' }}>No findings match the filters.</div>
        )}
        {visible.map((f) => {
          const targetFileId = f.filePath ? fileIdByPath.get(f.filePath) : undefined;
          return (
            <FindingCard
              key={f.id}
              finding={f}
              state={findingStates[f.id]}
              onSetState={onSetFindingState}
              onJump={targetFileId ? () => onJumpToFile(targetFileId, f.line) : undefined}
            />
          );
        })}
      </div>
    </div>
  );
}
