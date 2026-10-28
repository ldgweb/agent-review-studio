import { useMemo, useState } from 'react';
import type { ProjectReview, SessionContext } from '../../types/review';
import { IconX } from '../../components/icons';
import { formatDateTime, formatRelative } from '../../lib/format';
import { TimelinePanel } from './TimelinePanel';

// The agent's side of the story, on request: what you asked, what it
// reported, and (folded) what it ran. Closed by default: the reason for
// each change already sits above that change in the diff.

function Expandable({ text, max = 320, className }: { text: string; max?: number; className?: string }) {
  const [open, setOpen] = useState(false);
  if (text.length <= max) return <p className={className}>{text}</p>;
  return (
    <>
      <p className={className}>{open ? text : `${text.slice(0, max - 1)}…`}</p>
      <button className="linklike" onClick={() => setOpen(!open)}>{open ? 'Show less' : 'Show all'}</button>
    </>
  );
}

function SessionBlock({ review, session }: { review: ProjectReview; session: SessionContext }) {
  const perTurn = useMemo(() => {
    const counts = new Map<number, number>();
    for (const f of review.files) {
      for (const h of f.hunks) {
        if (h.origin.sessionId === session.sessionId && h.origin.turn !== undefined) {
          counts.set(h.origin.turn, (counts.get(h.origin.turn) ?? 0) + 1);
        }
      }
    }
    return counts;
  }, [review, session.sessionId]);
  const report = session.outcome.startsWith('No final self-report') ? '' : session.outcome;
  const turns = session.turns.length ? session.turns : [{ index: 0, prompt: session.originalPrompt, timestamp: session.timestamp }];
  return (
    <section className="session-block">
      <div className="session-title">
        <strong>{session.agentLabel}</strong>
        <span className="faint" title={formatDateTime(session.lastActivity)}>{formatRelative(session.lastActivity)}</span>
      </div>
      <div className="why-label">You asked</div>
      <ol className="session-turns">
        {turns.map((t) => (
          <li key={t.index}>
            <Expandable text={t.prompt} max={240} />
            {perTurn.get(t.index) ? (
              <span className="faint">
                {perTurn.get(t.index)} change{perTurn.get(t.index) === 1 ? '' : 's'} in this review
              </span>
            ) : null}
          </li>
        ))}
      </ol>
      {report && (
        <>
          <div className="why-label">What it reported</div>
          <Expandable text={report} />
        </>
      )}
      {session.timeline.length > 0 && (
        <details className="session-timeline">
          <summary>What it did ({session.timeline.length} steps)</summary>
          <TimelinePanel timeline={session.timeline} />
        </details>
      )}
    </section>
  );
}

export function SessionPanel({ review, onClose }: { review: ProjectReview; onClose: () => void }) {
  return (
    <aside className="session-panel">
      <div className="session-panel-head">
        <span>Agent session</span>
        <button className="icon-btn" title="Close" aria-label="Close" onClick={onClose}>
          <IconX size={14} />
        </button>
      </div>
      <div className="session-panel-body">
        {review.sessions.map((s) => (
          <SessionBlock key={s.sessionId} review={review} session={s} />
        ))}
      </div>
    </aside>
  );
}
