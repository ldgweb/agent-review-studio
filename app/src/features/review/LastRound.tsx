import type { ReviewComment } from '../../types/review';
import type { RoundItem } from '../../lib/review';

// Above the file's diff: what became of earlier comments. From the last
// review, the comments the agent has not dealt with yet and the note you
// left on the file. From this review, comments whose code changed since:
// they no longer sit on any line, so they are out of the review.

interface LastRoundProps {
  /** The last review's comments on this file, not marked done */
  items: RoundItem[];
  /** The last review's note on this file */
  note?: string;
  /** This review's comments on code that is gone */
  outdated: ReviewComment[];
  onResolve: (id: string) => void;
  onDelete: (id: string) => void;
}

/** A comment can be only a suggested change */
const said = (c: ReviewComment) => (c.text ? `“${c.text}”` : 'Suggested change');

export function LastRound({ items, note, outdated, onResolve, onDelete }: LastRoundProps) {
  if (items.length === 0 && !note && outdated.length === 0) return null;
  return (
    <div className="last-round">
      {(items.length > 0 || note) && (
        <>
          <div className="last-round-title">From your last review</div>
          {note && (
            <div className="last-round-item">
              <span className="last-round-text">“{note}”</span>
              <span className="faint">your note on this file</span>
            </div>
          )}
          {items.map(({ comment, status }) => (
            <div key={comment.id} className="last-round-item">
              <span className="last-round-text">{said(comment)}</span>
              <span className="faint">
                {comment.line ? `line ${comment.line} · ` : ''}
                {status === 'changed' ? 'the agent changed that code' : 'not changed yet: it goes into the next review'}
              </span>
              <button className="btn-mini" onClick={() => onResolve(comment.id)}>
                Mark done
              </button>
            </div>
          ))}
        </>
      )}
      {outdated.length > 0 && (
        <>
          <div className="last-round-title">The code under these comments changed</div>
          {outdated.map((c) => (
            <div key={c.id} className="last-round-item">
              <span className="last-round-text">{said(c)}</span>
              <span className="faint">
                {c.line ? `was at line ${c.line} · ` : ''}no longer in the review
              </span>
              <button className="btn-mini" onClick={() => onDelete(c.id)}>
                Delete
              </button>
            </div>
          ))}
        </>
      )}
    </div>
  );
}
