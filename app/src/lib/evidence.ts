import type { AgentSession, FileChange } from '../types/domain';

// Process evidence from the timeline: did the tests re-run after a file's
// last edit? Heuristic (timeline order), so callers should present it as a
// process signal, not ground truth.

export type VerificationStatus = 'verified' | 'unverified' | 'no-signal';

/**
 * Did any successful test run happen after the file's last edit?
 * Uses timeline order (chronological); 'no-signal' when the session has no
 * test runs or the file's edits were not captured as events.
 */
export function verificationStatus(session: AgentSession, file: FileChange): VerificationStatus {
  const timeline = session.timeline;
  let lastEdit = -1;
  let lastOkTest = -1;
  for (let i = 0; i < timeline.length; i++) {
    const e = timeline[i];
    if (e.kind === 'file-edit' && e.filePath === file.path) lastEdit = i;
    if (e.kind === 'test-run' && e.exitCode === 0) lastOkTest = i;
  }
  if (lastEdit === -1 || !timeline.some((e) => e.kind === 'test-run')) return 'no-signal';
  return lastOkTest > lastEdit ? 'verified' : 'unverified';
}

/** Files edited after the last successful test run — "changed but never re-tested" */
export function unverifiedFileIds(session: AgentSession): Set<string> {
  const ids = new Set<string>();
  for (const f of session.files) {
    if (verificationStatus(session, f) === 'unverified') ids.add(f.id);
  }
  return ids;
}
