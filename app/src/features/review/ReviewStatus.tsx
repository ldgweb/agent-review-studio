import { useState } from 'react';
import { Link } from 'react-router-dom';
import type { ProjectReview, ReviewState } from '../../types/review';
import { IconCheck } from '../../components/icons';
import { copyText } from '../../lib/clipboard';
import { formatAgo } from '../../lib/format';
import { countItems } from '../../lib/review';

// The two pages shown instead of a review: why it cannot open, and that
// there is nothing to review.

/** Why a review cannot open, in words, with what to do about it */
export function ReviewProblem({ path, error, onRetry }: { path: string; error: string; onRetry: () => void }) {
  const gone = error.includes('no longer exists');
  const noGit = error.includes('not inside a Git repository');
  return (
    <div className="review-problem" role="alert">
      <h1>{gone ? 'This folder no longer exists' : noGit ? 'No Git repository here' : 'The changes could not be read'}</h1>
      <p className="mono faint">{path}</p>
      {gone && <p>It was moved or deleted. You can hide it from the projects list.</p>}
      {noGit && (
        <p>
          ARS reviews what is not committed yet, and it reads that from Git. Start a repository in this folder (for example with{' '}
          <span className="mono">git init</span> and a first commit), then open it again.
        </p>
      )}
      {!gone && !noGit && <p>{error}</p>}
      <div className="all-clear-actions">
        {!gone && (
          <button className="btn-tool" onClick={onRetry}>
            Try again
          </button>
        )}
        <Link className="btn btn-primary btn-tool-primary" to="/">
          All projects
        </Link>
      </div>
    </div>
  );
}

/** Nothing uncommitted: say so plainly, show the page is still listening,
 * and keep the last review one click away. */
export function NothingToReview({ review, state }: { review: ProjectReview; state: ReviewState }) {
  const round = state.rounds[state.rounds.length - 1];
  const asked = round ? (round.items ?? countItems(round.text)) : 0;
  const [copied, setCopied] = useState<'' | 'ok' | 'failed'>('');
  return (
    <main className="page">
      <div className="all-clear">
        <span className="all-clear-mark" aria-hidden="true">
          <IconCheck size={28} />
        </span>
        <h1>Everything in {review.project.name} is committed</h1>
        <p>There is nothing to review. When your agent changes the code, the review opens here by itself.</p>
        <span className="waiting" role="status">
          <i aria-hidden="true" /> Waiting for changes
        </span>
        {round && (
          <div className="all-clear-round">
            <span>
              Last review copied {formatAgo(round.at)}
              {asked > 0 ? `, with ${asked} item${asked === 1 ? '' : 's'} for the agent` : ''}.
            </span>
            <button
              className="btn-mini"
              title="Copy the review you gave the agent last time"
              onClick={() => void copyText(round.text).then((ok) => setCopied(ok ? 'ok' : 'failed'))}
            >
              {copied === 'ok' ? '✓ Copied' : copied === 'failed' ? 'Could not copy' : 'Copy it again'}
            </button>
          </div>
        )}
        <div className="all-clear-actions">
          <Link className="btn-tool" to="/">
            All projects
          </Link>
        </div>
      </div>
    </main>
  );
}
