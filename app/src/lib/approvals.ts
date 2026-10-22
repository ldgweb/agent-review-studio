import type { AgentSession, ApprovalState, FileApproval, FindingReviewState, LineComment, RiskDimension, SessionDecision, SessionStatus } from '../types/domain';

export type Store = Record<string, ApprovalState>;

/** correctionNotes key holding the reviewer-edited correction draft */
export const DRAFT_NOTE_KEY = '__draft';

export function emptyState(sessionId: string): ApprovalState {
  return {
    sessionId,
    fileStates: {},
    correctionNotes: {},
    lineComments: {},
    findingStates: {},
    rejectedCategories: [],
  };
}

export type Action = (
  | { type: 'set-files'; sessionId: string; fileIds: string[]; state: FileApproval }
  | { type: 'set-note'; sessionId: string; key: string; note: string }
  | { type: 'set-line-comment'; sessionId: string; fileId: string; comment: LineComment }
  | { type: 'remove-line-comment'; sessionId: string; fileId: string; anchor: string }
  | { type: 'set-finding-state'; sessionId: string; findingId: string; state?: FindingReviewState }
  | { type: 'reject-category'; sessionId: string; dimension: RiskDimension; fileIds: string[] }
  | { type: 'clear-category'; sessionId: string; dimension: RiskDimension; fileIds: string[] }
  | { type: 'finalize'; sessionId: string; decision: SessionDecision }
  | { type: 'hydrate'; approvals: Store }
  | { type: 'reset-all' }) & { reviewRevision?: string; contentHash?: string };

export function withSession(store: Store, sessionId: string): ApprovalState {
  const s = store[sessionId] ?? emptyState(sessionId);
  // Fill fields missing from older cached states
  if (s.lineComments && s.findingStates) return s;
  return { ...s, lineComments: s.lineComments ?? {}, findingStates: s.findingStates ?? {} };
}

export function reducer(store: Store, action: Action): Store {
  if (action.type === 'reset-all') return {};
  // Server state wins over the localStorage cache; local-only sessions survive
  if (action.type === 'hydrate') return { ...store, ...action.approvals };
  const previous = withSession(store, action.sessionId);
  const s = previous.reviewRevision === action.reviewRevision ? previous : emptyState(action.sessionId);
  // A finalized decision rests on verdicts, line comments, finding stances and
  // category verdicts: changing any of them clears it. Free-text notes do not,
  // so the correction request can still be edited after finalizing.
  let next: ApprovalState;
  switch (action.type) {
    case 'set-files': {
      const fileStates = { ...s.fileStates };
      for (const id of action.fileIds) fileStates[id] = action.state;
      next = { ...s, fileStates, sessionDecision: undefined };
      break;
    }
    case 'set-note':
      next = { ...s, correctionNotes: { ...s.correctionNotes, [action.key]: action.note } };
      break;
    case 'set-line-comment': {
      const existing = (s.lineComments[action.fileId] ?? []).filter((c) => c.anchor !== action.comment.anchor);
      next = {
        ...s,
        lineComments: { ...s.lineComments, [action.fileId]: [...existing, action.comment] },
        sessionDecision: undefined,
      };
      break;
    }
    case 'remove-line-comment':
      next = {
        ...s,
        lineComments: {
          ...s.lineComments,
          [action.fileId]: (s.lineComments[action.fileId] ?? []).filter((c) => c.anchor !== action.anchor),
        },
        sessionDecision: undefined,
      };
      break;
    case 'set-finding-state': {
      const findingStates = { ...s.findingStates };
      if (action.state) findingStates[action.findingId] = action.state;
      else delete findingStates[action.findingId];
      next = { ...s, findingStates, sessionDecision: undefined };
      break;
    }
    case 'reject-category': {
      const fileStates = { ...s.fileStates };
      for (const id of action.fileIds) fileStates[id] = 'rejected';
      const rejectedCategories = s.rejectedCategories.includes(action.dimension)
        ? s.rejectedCategories
        : [...s.rejectedCategories, action.dimension];
      next = { ...s, fileStates, rejectedCategories, sessionDecision: undefined };
      break;
    }
    case 'clear-category': {
      const fileStates = { ...s.fileStates };
      // Undo resets every rejected file in the category's set, including any
      // the user rejected individually before the category verdict.
      for (const id of action.fileIds) {
        if (fileStates[id] === 'rejected') fileStates[id] = 'pending';
      }
      next = {
        ...s,
        fileStates,
        rejectedCategories: s.rejectedCategories.filter((d) => d !== action.dimension),
        sessionDecision: undefined,
      };
      break;
    }
    case 'finalize':
      next = { ...s, sessionDecision: action.decision };
      break;
  }
  return { ...store, [action.sessionId]: { ...next, reviewRevision: action.reviewRevision, contentHash: action.contentHash } };
}

/** Status shown in the UI. A stored review applies only when it was saved
 * with a revision for this exact transcript (contentHash); then its recorded
 * decision wins, and any file verdict already means "in review". Otherwise
 * the session's base status shows. */
export function effectiveStatus(
  session: Pick<AgentSession, 'sessionId' | 'status' | 'contentHash'>,
  approval: ApprovalState = emptyState(session.sessionId),
): SessionStatus {
  if (!approval.reviewRevision || approval.contentHash !== session.contentHash) return session.status;
  if (approval.sessionDecision) return approval.sessionDecision;
  if (Object.values(approval.fileStates).some((v) => v !== 'pending')) return 'in-review';
  return session.status;
}

/** Compute the outcome that finalizing would produce right now; `decision`
 * is null when there is nothing to decide (no files). */
export function computeDecision(
  fileIds: string[],
  approval: ApprovalState,
): { decision: SessionDecision | null; approved: number; rejected: number; corrections: number; pending: number } {
  let approved = 0;
  let rejected = 0;
  let corrections = 0;
  let pending = 0;
  for (const id of fileIds) {
    const s = approval.fileStates[id] ?? 'pending';
    if (s === 'approved') approved++;
    else if (s === 'rejected') rejected++;
    else if (s === 'correction-requested') corrections++;
    else pending++;
  }
  let decision: SessionDecision | null;
  if (fileIds.length === 0) decision = null;
  else if (corrections > 0) decision = 'corrections-requested';
  else if (rejected === fileIds.length) decision = 'rejected';
  else if (rejected > 0 || pending > 0) decision = 'partially-approved';
  else decision = 'approved';
  return { decision, approved, rejected, corrections, pending };
}
