import { useState } from 'react';
import type { EditStep, ReviewHunk, SessionContext, Verdict } from '../../types/review';
import { IconAlert, IconCheck, IconX } from '../../components/icons';
import { formatTime } from '../../lib/format';
import { afterEdit, outcomeOf } from '../../lib/history';
import { hunkRange, hunkSection } from '../../lib/review';

// The line above every change: where it is, why the agent made it (one
// line; a click opens, right under it, the request and the change step by
// step: the edits that wrote it and what the agent ran after),
// and the decision on this one change. A comment on any of its lines asks
// for a change, so there are only two buttons. A file with a single change
// has none here: the file's own buttons decide it.

interface HunkHeaderProps {
  hunk: ReviewHunk;
  /** The file the change is in */
  path: string;
  verdict?: Verdict;
  /** The project's sessions: the steps of each change come from them */
  sessions: SessionContext[];
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

/** This change, step by step: each recorded edit that wrote its lines with
 * the agent's words before it, then what the agent ran right after */
function ChangeStory({ steps, total, sessions, path }: { steps: EditStep[]; total: number; sessions: SessionContext[]; path: string }) {
  const last = steps[steps.length - 1];
  const session = (id: string) => sessions.find((s) => s.sessionId === id);
  const after = afterEdit(session(last.sessionId)?.timeline ?? [], last, path);
  return (
    <>
      <div className="why-label">How this change came about</div>
      <ol className="change-story">
        {steps.map((st, i) => (
          <li key={st.eventId ?? `${st.sessionId}-${i}`}>
            <span className="story-time">{formatTime(st.timestamp)}</span>
            <span className="story-body">
              <span className="story-what">
                {session(st.sessionId)?.agentLabel ?? 'The agent'} wrote {steps.length > 1 ? `${st.lines} of ${total} lines` : 'it'}
                {st.rationale ? ', after saying:' : ''}
              </span>
              {st.rationale && <span className="story-said">{st.rationale}</span>}
            </span>
          </li>
        ))}
        {after.map((e) => {
          const outcome = outcomeOf(e);
          return (
            <li key={e.id} className={e.kind === 'error' ? 'is-error' : undefined}>
              <span className="story-time">{formatTime(e.timestamp)}</span>
              <span className="story-body">
                <span className="story-what">
                  {e.kind === 'test-run' ? 'Ran the tests' : e.kind === 'error' ? 'Error' : 'Ran'}
                  {outcome && <span className={`story-outcome ${outcome.tone}`}>{outcome.text}</span>}
                </span>
                <code className="story-cmd">{e.command ?? e.title}</code>
              </span>
            </li>
          );
        })}
        {after.every((e) => e.kind !== 'test-run') && (
          <li>
            <span className="story-time" />
            <span className="story-body faint">No test run recorded after this change.</span>
          </li>
        )}
      </ol>
    </>
  );
}

export function HunkHeader({
  hunk,
  path,
  verdict,
  sessions,
  collapsed,
  onToggleCollapsed,
  groupSize,
  onVerdict,
  onGroupVerdict,
  showVerdict,
}: HunkHeaderProps) {
  const [open, setOpen] = useState(false);
  const hasSessions = sessions.length > 0;
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
            title={canOpen ? (open ? 'Hide' : 'Why this change: your request, the agent’s words, and what it ran after') : undefined}
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
              {o.steps && <ChangeStory steps={o.steps} total={o.total} sessions={sessions} path={path} />}
              {o.kind === 'partial' && (
                <p className="faint">
                  {o.matched} of {o.total} changed lines come from the edits above; the others were changed afterwards, outside the
                  recorded edits.
                </p>
              )}
            </>
          )}
        </div>
      )}
    </>
  );
}
