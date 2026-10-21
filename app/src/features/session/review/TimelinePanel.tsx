import { useState, type ReactNode } from 'react';
import type { TimelineEventKind, AgentSession } from '../../../types/domain';
import { formatTime } from '../../../lib/format';

// Timeline tab of the context rail: what the
// agent did, step by step, with kind filters and expandable detail.

const kindMeta: Record<TimelineEventKind, { label: string; color: string }> = {
  plan: { label: 'Plan', color: 'var(--accent)' },
  analysis: { label: 'Analysis', color: 'var(--accent)' },
  'file-read': { label: 'Read', color: 'var(--text-2)' },
  'file-edit': { label: 'Edit', color: '#9a8cf0' },
  command: { label: 'Command', color: 'var(--risk-medium)' },
  'test-run': { label: 'Test', color: 'var(--ok)' },
  error: { label: 'Error', color: 'var(--err)' },
  decision: { label: 'Decision', color: 'var(--risk-high)' },
  note: { label: 'Note', color: 'var(--text-1)' },
};

const filterOrder: TimelineEventKind[] = [
  'plan',
  'analysis',
  'file-read',
  'file-edit',
  'command',
  'test-run',
  'error',
  'decision',
  'note',
];

interface TimelinePanelProps {
  session: AgentSession;
}

export function TimelinePanel({ session }: TimelinePanelProps) {
  const [activeKinds, setActiveKinds] = useState<Set<TimelineEventKind>>(new Set());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  const presentKinds = filterOrder.filter((k) => session.timeline.some((e) => e.kind === k));
  const events =
    activeKinds.size === 0 ? session.timeline : session.timeline.filter((e) => activeKinds.has(e.kind));

  const toggleKind = (k: TimelineEventKind) => {
    const next = new Set(activeKinds);
    if (next.has(k)) next.delete(k);
    else next.add(k);
    setActiveKinds(next);
  };

  const toggleExpanded = (id: string) => {
    const next = new Set(expanded);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setExpanded(next);
  };

  return (
    <div>
      <div className="chip-row" style={{ marginBottom: 'var(--sp-3)' }}>
        {presentKinds.map((k) => (
          <button
            key={k}
            className={`chip${activeKinds.has(k) ? ' active' : ''}`}
            onClick={() => toggleKind(k)}
          >
            <span className="dot" style={{ background: kindMeta[k].color, marginRight: 5 }} />
            {kindMeta[k].label} ({session.timeline.filter((e) => e.kind === k).length})
          </button>
        ))}
        {activeKinds.size > 0 && (
          <button className="chip" onClick={() => setActiveKinds(new Set())}>Clear filters</button>
        )}
      </div>

      <div className="timeline">
        {events.map((e) => {
          const meta = kindMeta[e.kind];
          const hasDetail = Boolean(e.detail);
          const isOpen = expanded.has(e.id);
          const head: ReactNode = (
            <>
              <span className="timeline-time">{formatTime(e.timestamp)}</span>
              <span className="badge badge-neutral" style={{ minWidth: 60, justifyContent: 'center' }}>
                {meta.label}
              </span>
              <span style={{ color: e.kind === 'error' ? 'var(--err)' : undefined, fontSize: 'var(--fs-sm)' }}>
                {e.title}
                {e.exitCode !== undefined && (
                  <span className="mono faint" style={{ marginLeft: 8 }}>
                    exit {e.exitCode}
                  </span>
                )}
              </span>
            </>
          );
          return (
            <div key={e.id} className="timeline-event">
              <span className="timeline-dot" style={{ background: meta.color }} />
              {hasDetail ? (
                <button
                  type="button"
                  className="timeline-head expandable"
                  aria-expanded={isOpen}
                  aria-controls={isOpen ? `event-detail-${e.id}` : undefined}
                  onClick={() => toggleExpanded(e.id)}
                >
                  {head}
                  <span className="faint" aria-hidden="true">{isOpen ? '▾' : '▸'}</span>
                </button>
              ) : (
                <div className="timeline-head">{head}</div>
              )}
              {hasDetail && isOpen && <div id={`event-detail-${e.id}`} className="timeline-detail">{e.detail}</div>}
            </div>
          );
        })}
      </div>
    </div>
  );
}
