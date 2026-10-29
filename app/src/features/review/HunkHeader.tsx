import { useState } from 'react';
import type { ReviewHunk, Verdict } from '../../types/review';
import { IconAlert, IconCheck, IconX } from '../../components/icons';
import { hunkRange, hunkSection } from '../../lib/review';

// The line above every change: where it is, why the agent made it (one
// line; a click opens the request and the agent's words right under it),
// and the decision on this one change. A comment on any of its lines asks
// for a change, so there are only two buttons. A file with a single change
// has none here: the file's own buttons decide it.

interface HunkHeaderProps {
  hunk: ReviewHunk;
  verdict?: Verdict;
  hasSessions: boolean;
  collapsed: boolean;
  onToggleCollapsed?: () => void;
  /** Places the same change appears, when more than one */
  groupSize?: number;
  onVerdict: (v: Verdict) => void;
  onGroupVerdict?: (v: Verdict) => void;
  /** Off when the file's buttons already decide this one change */
  showVerdict: boolean;
}

function firstLine(text: string, max: number): string {
  const line = text.split('\n').find((l) => l.trim()) ?? '';
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

export function HunkHeader({
  hunk,
  verdict,
  hasSessions,
  collapsed,
  onToggleCollapsed,
  groupSize,
  onVerdict,
  onGroupVerdict,
  showVerdict,
}: HunkHeaderProps) {
  const [open, setOpen] = useState(false);
  const section = hunkSection(hunk.header);
  const range = hunkRange(hunk);
  const o = hunk.origin;
  const why =
    o.kind === 'none'
      ? hasSessions
        ? 'No recorded agent edit'
        : ''
      : `${o.kind === 'partial' ? `Partly ${o.agentLabel ?? 'the agent'}` : (o.agentLabel ?? 'Agent')}${
          o.rationale ? `: “${firstLine(o.rationale, 160)}”` : o.prompt ? `: request “${firstLine(o.prompt, 120)}”` : ''
        }`;
  const canOpen = o.kind !== 'none' ? Boolean(o.prompt || o.rationale) : hasSessions;
  return (
    <>
      <div className={`hunk-head${verdict ? ` is-${verdict}` : ''}`}>
        <span className="hunk-where" title={hunk.header}>
          {range}
          {section && <span className="hunk-section">{section}</span>}
        </span>
        {why && (
          <button
            className={`hunk-why${o.kind === 'none' ? ' unrecorded' : ''}${open ? ' open' : ''}`}
            disabled={!canOpen}
            onClick={() => setOpen(!open)}
            title={canOpen ? (open ? 'Hide' : 'Why this change: the request and the agent’s words') : undefined}
          >
            {o.kind === 'none' && <IconAlert size={11} />} {why}
          </button>
        )}
        {hunk.noise && onToggleCollapsed && (
          <button className="hunk-chip" onClick={onToggleCollapsed}>
            {hunk.noise === 'moved' ? 'moved code' : 'whitespace only'} · {collapsed ? 'show' : 'hide'}
          </button>
        )}
        {!hunk.noise && collapsed && onToggleCollapsed && (
          <button className="hunk-chip" onClick={onToggleCollapsed}>show</button>
        )}
        {groupSize && groupSize > 1 && onGroupVerdict && (
          <span className="hunk-group" title="The same change, made in several places">
            same change ×{groupSize}
            <button className="btn-mini" onClick={() => onGroupVerdict('approved')}>Approve all</button>
          </span>
        )}
        <span style={{ flex: 1 }} />
        {verdict === 'rejected' && <span className="hunk-state err">the agent will undo it</span>}
        {verdict === 'changes' && <span className="hunk-state warn">change requested</span>}
        {showVerdict && (
          <span className="hunk-verdict" role="group" aria-label="Verdict on this change">
            <button
              className={`hv ok${verdict === 'approved' ? ' on' : ''}`}
              aria-pressed={verdict === 'approved'}
              aria-label="Approve this change"
              title="Approve this change (⇧A)"
              onClick={() => onVerdict('approved')}
            >
              <IconCheck size={13} />
            </button>
            <button
              className={`hv err${verdict === 'rejected' ? ' on' : ''}`}
              aria-pressed={verdict === 'rejected'}
              aria-label="Reject this change"
              title="Reject this change (⇧R): the agent will be asked to undo it"
              onClick={() => onVerdict('rejected')}
            >
              <IconX size={13} />
            </button>
          </span>
        )}
      </div>
      {open && (
        <div className="hunk-why-detail">
          {o.kind === 'none' ? (
            <>
              <p>
                None of the edits recorded in the agents’ transcripts wrote these lines: a shell command, a script, another tool
                or a person changed them. Read them with that in mind.
              </p>
              {o.commandHint && (
                <>
                  <div className="why-label">A command that named this file</div>
                  <pre className="why-code">{o.commandHint.command}</pre>
                </>
              )}
            </>
          ) : (
            <>
              {o.prompt && (
                <>
                  <div className="why-label">You asked{o.turn !== undefined ? ` (request ${o.turn + 1})` : ''}</div>
                  <p className="why-prompt">{o.prompt}</p>
                </>
              )}
              {o.rationale && (
                <>
                  <div className="why-label">{o.agentLabel ?? 'The agent'}, right before this edit</div>
                  <p>{o.rationale}</p>
                </>
              )}
              {o.kind === 'partial' && (
                <p className="faint">
                  {o.matched} of {o.total} changed lines come from this edit; the others were changed afterwards, outside the recorded
                  edits.
                </p>
              )}
            </>
          )}
        </div>
      )}
    </>
  );
}
