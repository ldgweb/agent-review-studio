import type { FindingReviewState, ReviewerFinding } from '../../types/domain';

// A hint from the fixed checks (a possible secret, leftover debug code…),
// as one quiet line under the code it is about: send it to the agent, or
// dismiss it. No scores, no panels.

export function FindingHint({
  finding,
  state,
  onSetState,
}: {
  finding: ReviewerFinding;
  state?: FindingReviewState;
  onSetState: (id: string, state?: FindingReviewState) => void;
}) {
  const sent = state === 'correction';
  if (state === 'dismissed') {
    return (
      <div className="hint dismissed">
        <span>{finding.title} · dismissed</span>
        <button className="btn-mini" onClick={() => onSetState(finding.id, undefined)}>Undo</button>
      </div>
    );
  }
  return (
    <div className={`hint sev-${finding.severity}${sent ? ' sent' : ''}`}>
      <span className="hint-text">
        <strong>{finding.title}</strong>
        {finding.body && <span className="faint"> · {finding.body}</span>}
      </span>
      <span className="hint-actions">
        <button
          className={`btn-mini accent${sent ? ' on' : ''}`}
          aria-pressed={sent}
          title={sent ? 'In your review for the agent. Click to take it out.' : 'Add this to your review for the agent'}
          onClick={() => onSetState(finding.id, sent ? undefined : 'correction')}
        >
          {sent ? '✓ In your review' : 'Send to agent'}
        </button>
        {!sent && (
          <button className="btn-mini" onClick={() => onSetState(finding.id, 'dismissed')}>
            Dismiss
          </button>
        )}
      </span>
    </div>
  );
}
