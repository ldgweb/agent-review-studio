import { useNavigate } from 'react-router-dom';
import type { AgentSession, SessionStatus } from '../../types/domain';
import { agentColor } from '../../components/ui';
import { maxRiskLevel, riskColor, statusMeta } from '../../lib/risk';
import { riskCoverage } from '../../lib/triage';
import { useApproval } from '../../state/approvalStore';
import { formatDateTime, formatRelative } from '../../lib/format';
import { IconArchive, IconRestore } from '../../components/icons';

// The inbox as a review queue: one calm row per session, ordered by the
// side panel's sort. Risk-first at a glance — a left stripe and risk-dimension
// chips surface what needs attention; status, size and local-check counts sit on
// the right. Selection feeds archive.

/** Row tone per status (labels come from statusMeta) */
const STATUS_TONE: Record<SessionStatus, string> = {
  'needs-review': 'review',
  'in-review': 'neutral',
  'corrections-requested': 'warn',
  approved: 'ok',
  'partially-approved': 'neutral',
  rejected: 'err',
};

const NOTABLE = new Set(['medium', 'high', 'critical']);

function initials(label: string): string {
  const words = label.trim().split(/\s+/);
  const chars = words.length > 1 ? words[0][0] + words[1][0] : label.slice(0, 2);
  return chars.toUpperCase();
}

function SessionRow({
  session,
  status,
  selected,
  archivedView,
  onToggleSelect,
  onArchive,
  onOpen,
}: {
  session: AgentSession;
  status: SessionStatus;
  selected: boolean;
  archivedView: boolean;
  onToggleSelect: () => void;
  onArchive: () => void;
  onOpen: () => void;
}) {
  const { state, saveError } = useApproval(session.sessionId, session.reviewRevision, session.contentHash);
  const decided = session.files.filter((f) => (state.fileStates[f.id] ?? 'pending') !== 'pending').length;
  const coverage = riskCoverage(session.files, session, state.fileStates);
  const risk = maxRiskLevel(session);
  const meta = statusMeta[status] ?? statusMeta['needs-review'];
  const tone = STATUS_TONE[status] ?? 'review';
  const localChecks = session.findings.filter((f) => f.source === 'local').length;
  const notableRisks = session.risks
    .filter((r) => NOTABLE.has(r.level))
    .sort((a, b) => b.score - a.score)
    .slice(0, 3);
  const stripe = risk === 'critical' ? ' crit' : risk === 'high' || risk === 'medium' ? ' attn' : '';

  return (
    <div
      className={`sl-row${selected ? ' selected' : ''}${stripe}`}
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        // Keys on the checkbox / archive button inside the row are theirs
        if (e.target !== e.currentTarget) return;
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onOpen();
        }
      }}
    >
      <div className="sl-lead">
        <input
          type="checkbox"
          className="sl-check"
          checked={selected}
          title="Select for archive"
          aria-label={`Select ${session.declaredGoal}`}
          onClick={(e) => e.stopPropagation()}
          onChange={onToggleSelect}
        />
        <span className="sl-avatar" style={{ background: agentColor(session.agent) }} title={session.agentLabel}>
          {initials(session.agentLabel)}
        </span>
        <span className="sl-agent">{session.agentLabel}</span>
      </div>

      <div className="sl-goal">
        <div className="sl-title">{session.declaredGoal}</div>
        <div className="sl-meta">
          <span className="mono">{session.projectName}</span>
          <span className="sl-dot" />
          <span className="mono">{session.branch}</span>
          <span className="sl-dot" />
          <span>{session.files.length} file{session.files.length === 1 ? '' : 's'}</span>
          {localChecks > 0 && (
            <>
              <span className="sl-dot" />
              <span className="sl-local">{localChecks} local check{localChecks === 1 ? '' : 's'}</span>
            </>
          )}
          <span className="sl-dot" />
          <span title={formatDateTime(session.timestamp)}>{formatRelative(session.timestamp)}</span>
        </div>
      </div>

      <div className="sl-rail">
        <div className="sl-risks">
          {notableRisks.length > 0 ? (
            notableRisks.map((r) => (
              <span
                key={r.dimension}
                className="sl-rk"
                style={{ color: riskColor(r.level), background: `color-mix(in srgb, ${riskColor(r.level)} 12%, transparent)` }}
                title={`${r.dimension}: ${r.level} (${r.score})`}
              >
                {r.dimension}
              </span>
            ))
          ) : (
            <span className="sl-rk low">no risk flags</span>
          )}
        </div>
        <span className="sl-counts mono tnum">
          <span className="add">+{session.stats.linesAdded}</span>
          <span className="del">−{session.stats.linesRemoved}</span>
        </span>
        {decided > 0 && (
          <span className="sl-cov" title={`Risk-weighted review coverage: ${coverage.pct}%`}>
            <span className="sl-cov-fill" style={{ width: `${coverage.pct}%` }} />
          </span>
        )}
        {saveError && (
          <span className="sl-status err" title={saveError}>Not saved</span>
        )}
        <span className={`sl-status ${tone}`}>{meta.label}</span>
        <button
          className="sl-arch"
          title={archivedView ? 'Restore to the queue' : 'Archive (hide from the queue)'}
          aria-label={archivedView ? 'Restore to the queue' : 'Archive'}
          onClick={(e) => {
            e.stopPropagation();
            onArchive();
          }}
        >
          {archivedView ? <IconRestore size={13} /> : <IconArchive size={13} />}
        </button>
      </div>
    </div>
  );
}

export function SessionList({
  sessions,
  statusOf,
  selectedIds,
  onToggleSelect,
  onArchive,
  archivedView,
}: {
  sessions: AgentSession[];
  statusOf: (s: AgentSession) => SessionStatus;
  selectedIds: Set<string>;
  onToggleSelect: (id: string) => void;
  onArchive: (id: string) => void;
  archivedView: boolean;
}) {
  const navigate = useNavigate();

  return (
    <div className="session-list">
      {sessions.map((s) => (
        <SessionRow
          key={s.sessionId}
          session={s}
          status={statusOf(s)}
          selected={selectedIds.has(s.sessionId)}
          archivedView={archivedView}
          onToggleSelect={() => onToggleSelect(s.sessionId)}
          onArchive={() => onArchive(s.sessionId)}
          onOpen={() => navigate(`/sessions/${encodeURIComponent(s.sessionId)}/review`)}
        />
      ))}
    </div>
  );
}
