import type { TimelineEvent } from '../types/domain';
import type { EditStep } from '../types/review';

// The story of one change, not of the whole session: the recorded edits
// that wrote its lines (the core lists them), then what the agent ran
// right after the last one that concerns it: the first test run, and the
// commands and errors that name the file. Pure, so it can be tested.

/** Steps after the change listed at most */
const MAX_AFTER = 3;

/** What the agent did after a change's last edit that concerns it */
export function afterEdit(timeline: TimelineEvent[], last: EditStep, path: string): TimelineEvent[] {
  let from = last.eventId ? timeline.findIndex((e) => e.id === last.eventId) : -1;
  if (from < 0) {
    // No event for the edit: start at the first step recorded after it
    const at = Date.parse(last.timestamp);
    if (Number.isNaN(at)) return [];
    from = timeline.findIndex((e) => Date.parse(e.timestamp) > at) - 1;
    if (from < -1) return [];
  }
  const name = path.split('/').pop() ?? path;
  // A short name ("a.ts") would match too much by itself
  const needles = name.length >= 5 && name !== path ? [path, name] : [path];
  const names = (text?: string) => text !== undefined && needles.some((n) => text.includes(n));
  const out: TimelineEvent[] = [];
  for (const e of timeline.slice(from + 1)) {
    if (out.length >= MAX_AFTER) break;
    if (e.kind === 'test-run') {
      out.push(e);
      // The first test run after the change is what verified it
      break;
    }
    if (e.kind === 'command' && names(e.command)) out.push(e);
    else if (e.kind === 'error' && (names(e.title) || names(e.detail))) out.push(e);
  }
  return out;
}

/** A test run "passed" or "failed (exit 2)", a command "exit 0"; without
 * an exit code nothing counts as a pass */
export function outcomeOf(e: TimelineEvent): { text: string; tone: 'ok' | 'err' | 'faint' } | null {
  if (e.kind === 'error') return null;
  if (e.exitCode === undefined) return { text: 'no exit code', tone: 'faint' };
  if (e.kind !== 'test-run') return { text: `exit ${e.exitCode}`, tone: e.exitCode === 0 ? 'faint' : 'err' };
  return e.exitCode === 0 ? { text: 'passed', tone: 'ok' } : { text: `failed (exit ${e.exitCode})`, tone: 'err' };
}
