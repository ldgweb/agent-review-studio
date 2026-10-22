import { useState } from 'react';
import type { AgentSession, RiskDimension } from '../../../types/domain';
import { StatTile } from '../../../components/ui';
import { riskColor } from '../../../lib/risk';
import { formatDuration } from '../../../lib/format';
import { RiskVerdictBar } from './RiskVerdictBar';

// Session tab of the context rail: what happened
// and how risky it is — contextual reading next to the code, not a separate
// destination.

interface SessionPanelProps {
  session: AgentSession;
  rejectedCategories: RiskDimension[];
  onRejectCategory: (dimension: RiskDimension, fileIds: string[]) => void;
  onClearCategory: (dimension: RiskDimension, fileIds: string[]) => void;
}

export function SessionPanel({
  session,
  rejectedCategories,
  onRejectCategory,
  onClearCategory,
}: SessionPanelProps) {
  const [promptExpanded, setPromptExpanded] = useState(false);

  return (
    <div className="stack" style={{ gap: 'var(--sp-3)' }}>
      {/* The prompt — the anchor of the whole review */}
      <div className="card prompt-card" style={{ padding: 'var(--sp-4)' }}>
        <div className="section-label">The prompt</div>
        <div style={{ fontSize: 'var(--fs-md)', fontWeight: 600, letterSpacing: '-0.01em', margin: '2px 0 var(--sp-2)' }}>
          {session.declaredGoal}
        </div>
        <p className="mono" style={{ fontSize: 'var(--fs-sm)', color: 'var(--text-1)', whiteSpace: 'pre-wrap' }}>
          “{promptExpanded || session.originalPrompt.length <= 260
            ? session.originalPrompt
            : `${session.originalPrompt.slice(0, 259)}…`}”
        </p>
        {session.originalPrompt.length > 260 && (
          <button className="chip mt-2" onClick={() => setPromptExpanded(!promptExpanded)}>
            {promptExpanded ? 'Collapse prompt' : `Show full prompt (${session.originalPrompt.length} chars)`}
          </button>
        )}
        <div className="mt-3">
          <div className="section-label">Agent self-report</div>
          <p className="muted" style={{ fontSize: 'var(--fs-sm)' }}>{session.outcome}</p>
        </div>
      </div>

      {/* What the agent planned for that prompt */}
      {session.initialPlan.length > 0 && (
        <div className="card" style={{ padding: 'var(--sp-4)' }}>
          <div className="card-title">Plan the agent declared</div>
          <ol style={{ paddingLeft: 20 }} className="muted mt-2">
            {session.initialPlan.map((step, i) => (
              <li key={i} style={{ padding: '2px 0', fontSize: 'var(--fs-sm)' }}>{step}</li>
            ))}
          </ol>
        </div>
      )}

      {/* Summary + stats */}
      <div className="card" style={{ padding: 'var(--sp-4)' }}>
        <div className="card-title">Summary</div>
        <p style={{ fontSize: 'var(--fs-sm)' }}>{session.summary.text}</p>
        <div className="chip-row mt-3">
          {session.summary.impactedAreas.map((a) => (
            <span key={a} className="badge badge-neutral">{a}</span>
          ))}
        </div>
      </div>

      <div className="stat-row">
        <StatTile value={session.stats.filesModified} label="Files changed" />
        <StatTile
          value={
            <>
              <span style={{ color: 'var(--diff-add-text)' }}>+{session.stats.linesAdded}</span>{' '}
              <span style={{ color: 'var(--diff-del-text)' }}>−{session.stats.linesRemoved}</span>
            </>
          }
          label="Lines"
        />
        <StatTile value={session.stats.commandsRun} label="Commands" />
        <StatTile value={session.stats.testsRun} label="Test commands recorded" />
        <StatTile value={formatDuration(session.stats.durationMinutes)} label="Duration" />
      </div>

      <div className="card" style={{ padding: 'var(--sp-4)' }}>
        <div className="card-title">Review hints (local heuristics)</div>
        <div className="stack" style={{ gap: 'var(--sp-2)' }}>
          {session.risks.map((r) => (
            <div key={r.dimension}>
              <div style={{ color: riskColor(r.level) }}>{r.dimension}: {r.level}</div>
              <div className="faint" style={{ fontSize: 'var(--fs-xs)', marginTop: 1 }}>
                {r.rationale}
              </div>
            </div>
          ))}
        </div>
        {session.summary.riskWarnings.length > 0 && (
          <div className="stack mt-3" style={{ gap: 'var(--sp-2)' }}>
            {session.summary.riskWarnings.map((w, i) => (
              <div key={i} className="banner banner-warn" style={{ padding: 'var(--sp-2) var(--sp-3)' }}>
                <span>⚠</span>
                <span style={{ fontSize: 'var(--fs-sm)' }}>{w}</span>
              </div>
            ))}
          </div>
        )}
      </div>

      <RiskVerdictBar
        session={session}
        rejectedCategories={rejectedCategories}
        onRejectCategory={onRejectCategory}
        onClearCategory={onClearCategory}
      />

    </div>
  );
}
