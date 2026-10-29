import type { FindingReviewState, ReviewerFinding } from '../types/domain';
import type {
  FileVerdictView,
  ProjectReview,
  ReviewComment,
  ReviewFile,
  ReviewHunk,
  ReviewState,
  Verdict,
} from '../types/review';

// The project review's rules, as pure functions over the review the core
// built and the reviewer's state. Decisions bind to content: a hunk verdict
// to the hunk's content id, a file verdict (files without hunks) to the
// file's fingerprint, a comment to the hunk it sits on. Whatever changes
// elsewhere, a decision on unchanged code holds; a decision on changed code
// is gone.

// --- verdicts --------------------------------------------------------------

export function hunkVerdict(state: ReviewState, hunkId: string): Verdict | undefined {
  return state.hunks[hunkId]?.verdict;
}

/** A file's verdicts taken together: pending while any change is undecided */
export function fileVerdict(state: ReviewState, file: ReviewFile): FileVerdictView {
  if (file.hunks.length === 0) {
    const v = state.files[file.path];
    return v && v.fingerprint === file.fingerprint ? v.verdict : 'pending';
  }
  const verdicts = file.hunks.map((h) => state.hunks[h.id]?.verdict);
  if (verdicts.some((v) => !v)) return 'pending';
  return verdicts.every((v) => v === verdicts[0]) ? (verdicts[0] as Verdict) : 'mixed';
}

function fileProgress(state: ReviewState, file: ReviewFile): { decided: number; total: number } {
  if (file.hunks.length === 0) return { decided: fileVerdict(state, file) === 'pending' ? 0 : 1, total: 1 };
  return { decided: file.hunks.filter((h) => state.hunks[h.id]).length, total: file.hunks.length };
}

function withHunks(state: ReviewState, hunks: ReviewState['hunks']): ReviewState {
  return { ...state, hunks };
}

/** A verdict for a whole file (the a and r keys). It goes to the changes
 * still undecided; with none left it goes to all of them. Pressing the
 * verdict every change already has clears them all. */
export function setFileVerdict(state: ReviewState, file: ReviewFile, verdict: Verdict, at: string): ReviewState {
  if (file.hunks.length === 0) {
    const files = { ...state.files };
    if (fileVerdict(state, file) === verdict) delete files[file.path];
    else files[file.path] = { verdict, fingerprint: file.fingerprint, at };
    return { ...state, files };
  }
  const hunks = { ...state.hunks };
  const pending = file.hunks.filter((h) => !hunks[h.id]);
  if (pending.length > 0) {
    for (const h of pending) hunks[h.id] = { verdict, path: file.path, at };
  } else if (file.hunks.every((h) => hunks[h.id]?.verdict === verdict)) {
    for (const h of file.hunks) delete hunks[h.id];
  } else {
    for (const h of file.hunks) hunks[h.id] = { verdict, path: file.path, at };
  }
  return withHunks(state, hunks);
}

/** One change's verdict; the verdict it already has toggles it off. */
export function setHunkVerdict(state: ReviewState, file: ReviewFile, hunkId: string, verdict: Verdict, at: string): ReviewState {
  const hunks = { ...state.hunks };
  if (hunks[hunkId]?.verdict === verdict) delete hunks[hunkId];
  else hunks[hunkId] = { verdict, path: file.path, at };
  return withHunks(state, hunks);
}

/** The same verdict for every place a repeated change appears. */
export function setGroupVerdict(state: ReviewState, review: ProjectReview, groupId: string, verdict: Verdict, at: string): ReviewState {
  const hunks = { ...state.hunks };
  for (const f of review.files) {
    for (const h of f.hunks) if (h.group === groupId) hunks[h.id] = { verdict, path: f.path, at };
  }
  return withHunks(state, hunks);
}

export function setNote(state: ReviewState, path: string, text: string): ReviewState {
  const notes = { ...state.notes };
  if (text.trim()) notes[path] = text;
  else delete notes[path];
  return { ...state, notes };
}

function upsertComment(state: ReviewState, comment: ReviewComment): ReviewState {
  const rest = state.comments.filter((c) => c.id !== comment.id);
  return { ...state, comments: [...rest, comment] };
}

function removeComment(state: ReviewState, id: string): ReviewState {
  return { ...state, comments: state.comments.filter((c) => c.id !== id) };
}

/** A comment asks for a change: the undecided change it sits on becomes
 * "changes requested" (a decided one keeps its verdict). */
export function commentOnHunk(state: ReviewState, comment: ReviewComment, at: string): ReviewState {
  const next = upsertComment(state, comment);
  if (next.hunks[comment.hunkId]) return next;
  return withHunks(next, { ...next.hunks, [comment.hunkId]: { verdict: 'changes', path: comment.path, at } });
}

/** Removing a change's last comment takes back the "changes requested" it made. */
export function uncommentHunk(state: ReviewState, id: string): ReviewState {
  const gone = state.comments.find((c) => c.id === id);
  const next = removeComment(state, id);
  if (!gone || next.hunks[gone.hunkId]?.verdict !== 'changes' || next.comments.some((c) => c.hunkId === gone.hunkId)) return next;
  const hunks = { ...next.hunks };
  delete hunks[gone.hunkId];
  return withHunks(next, hunks);
}

export function setFindingState(state: ReviewState, id: string, value?: FindingReviewState): ReviewState {
  const findings = { ...state.findings };
  if (value) findings[id] = value;
  else delete findings[id];
  return { ...state, findings };
}

/** A previous round's comment marked dealt with (or reopened). */
export function setRoundCommentResolved(state: ReviewState, id: string, resolved: boolean): ReviewState {
  const rounds = state.rounds.map((r, i) =>
    i === state.rounds.length - 1
      ? { ...r, comments: r.comments.map((c) => (c.id === id ? { ...c, resolved } : c)) }
      : r,
  );
  return { ...state, rounds };
}

// --- what exists now --------------------------------------------------------

function liveHunks(files: ReviewFile[]): Map<string, { file: ReviewFile; hunk: ReviewHunk; hi: number }> {
  const map = new Map<string, { file: ReviewFile; hunk: ReviewHunk; hi: number }>();
  for (const file of files) file.hunks.forEach((hunk, hi) => map.set(hunk.id, { file, hunk, hi }));
  return map;
}

/** Comments on code that still exists */
export function currentComments(state: ReviewState, files: ReviewFile[]): ReviewComment[] {
  const live = liveHunks(files);
  return state.comments.filter((c) => live.has(c.hunkId));
}

/** This round's comments whose code changed since (GitHub's "outdated") */
export function outdatedComments(state: ReviewState, files: ReviewFile[]): ReviewComment[] {
  const live = liveHunks(files);
  return state.comments.filter((c) => !live.has(c.hunkId));
}

/** Where a comment's line is now: its index if the text still matches,
 * else the first line of the same kind and text; -1 when gone. */
export function commentLineIndex(hunk: ReviewHunk, c: ReviewComment): number {
  const at = hunk.lines[c.li];
  if (at && at.kind === c.kind && at.text === c.lineText) return c.li;
  return hunk.lines.findIndex((l) => l.kind === c.kind && l.text === c.lineText);
}

export interface RoundItem {
  comment: ReviewComment;
  /** open: the commented change is still there, untouched; changed: it is gone */
  status: 'open' | 'changed' | 'resolved';
}

/** The last copied review's comments, each with what happened to its code. */
export function previousRoundItems(review: ProjectReview, state: ReviewState): RoundItem[] {
  const round = state.rounds[state.rounds.length - 1];
  if (!round) return [];
  const live = liveHunks(review.files);
  return round.comments.map((comment) => ({
    comment,
    status: comment.resolved ? 'resolved' : live.has(comment.hunkId) ? 'open' : 'changed',
  }));
}

// --- counts --------------------------------------------------------------------

export interface Counts {
  approved: number;
  rejected: number;
  changes: number;
  mixed: number;
  pending: number;
  hunks: number;
  hunksDecided: number;
}

export function countVerdicts(state: ReviewState, files: ReviewFile[]): Counts {
  const c: Counts = { approved: 0, rejected: 0, changes: 0, mixed: 0, pending: 0, hunks: 0, hunksDecided: 0 };
  for (const f of files) {
    const v = fileVerdict(state, f);
    if (v === 'approved') c.approved++;
    else if (v === 'rejected') c.rejected++;
    else if (v === 'changes') c.changes++;
    else if (v === 'mixed') c.mixed++;
    else c.pending++;
    const p = fileProgress(state, f);
    c.hunks += p.total;
    c.hunksDecided += p.decided;
  }
  return c;
}

// --- triage ------------------------------------------------------------------

const SEVERITY_RANK: Record<string, number> = { info: 0, low: 1, medium: 2, high: 3, critical: 4 };

export function findingsOf(review: ProjectReview, file: ReviewFile): ReviewerFinding[] {
  return review.findings.filter((f) => f.filePath === file.path);
}

/** Changes no recorded agent edit explains (only meaningful with sessions) */
function unrecordedHunks(review: ProjectReview, file: ReviewFile): number {
  if (review.sessions.length === 0) return 0;
  return file.hunks.filter((h) => h.origin.kind !== 'agent').length;
}

function fileWeight(file: ReviewFile, review: ProjectReview): number {
  const critical = review.areas.some((a) => a.id === file.areaId && a.critical);
  let worst = 0;
  for (const f of findingsOf(review, file)) worst = Math.max(worst, Math.max(0, (SEVERITY_RANK[f.severity] ?? 0) - 1));
  return 1 + file.riskFlags.length + (critical ? 1 : 0) + worst + (file.verification === 'unverified' ? 1 : 0);
}

/** Why a file deserves a closer look, in words; empty when it is routine. */
export function attentionReasons(file: ReviewFile, review: ProjectReview): string[] {
  if (file.generated) return [];
  const out: string[] = [];
  if (file.since === 'changed') out.push('changed since your last review');
  for (const r of file.riskFlags) out.push(`touches ${r}`);
  const hints = findingsOf(review, file).filter((f) => (SEVERITY_RANK[f.severity] ?? 0) >= 2).length;
  if (hints) out.push(`${hints} hint${hints === 1 ? '' : 's'} worth a look`);
  if (review.areas.some((a) => a.id === file.areaId && a.critical)) out.push('a critical area');
  if (file.verification === 'unverified') out.push('edited after the last passing test run');
  const unrecorded = unrecordedHunks(review, file);
  if (unrecorded) out.push(`${unrecorded} change${unrecorded === 1 ? '' : 's'} no recorded agent edit explains`);
  return out;
}

export function needsAttention(file: ReviewFile, review: ProjectReview): boolean {
  return attentionReasons(file, review).length > 0;
}

export interface FileGroup {
  key: 'attention' | 'routine' | 'generated';
  label: string;
  files: ReviewFile[];
}

/** How a file changed, in a word or two; '' for a plain edit */
export function changeKind(file: ReviewFile): string {
  return file.changeType === 'created' ? 'new file' : file.changeType === 'modified' ? '' : file.changeType;
}

export function dirOf(path: string): string {
  const idx = path.lastIndexOf('/');
  return idx === -1 ? '' : path.slice(0, idx);
}

/** One reading order: what deserves a closer look first (weightiest on
 * top), then the routine changes, then generated files. */
export function groupFiles(files: ReviewFile[], review: ProjectReview): FileGroup[] {
  const regular = files.filter((f) => !f.generated);
  const attention = regular.filter((f) => needsAttention(f, review));
  attention.sort((a, b) => fileWeight(b, review) - fileWeight(a, review));
  const groups: FileGroup[] = [
    { key: 'attention', label: 'Look closely', files: attention },
    { key: 'routine', label: 'Routine', files: regular.filter((f) => !needsAttention(f, review)) },
    { key: 'generated', label: 'Generated', files: files.filter((f) => f.generated) },
  ];
  return groups.filter((g) => g.files.length > 0);
}

/** The numbered items of a review text (an edited one, say) */
export function countItems(text: string): number {
  return text.split('\n').filter((l) => /^\d+\. /.test(l)).length;
}

// --- where a change sits -----------------------------------------------------

/** "12–18": the added lines, else the removed ones (numbered as in HEAD) */
export function hunkRange(h: ReviewHunk): string {
  const adds = h.lines.filter((l) => l.kind === 'add' && l.newLine != null).map((l) => l.newLine as number);
  const dels = h.lines.filter((l) => l.kind === 'del' && l.oldLine != null).map((l) => l.oldLine as number);
  const ns = adds.length ? adds : dels;
  if (!ns.length) return '';
  return ns.length === 1 ? `${ns[0]}` : `${Math.min(...ns)}–${Math.max(...ns)}`;
}

/** Git's function context: "fn ctx" from "@@ -40,3 +40,3 @@ fn ctx" */
export function hunkSection(header: string): string {
  const parts = header.split('@@');
  return parts.length >= 3 ? parts.slice(2).join('@@').trim() : '';
}

export function sortFindings(findings: ReviewerFinding[]): ReviewerFinding[] {
  return [...findings].sort((a, b) => {
    const bySeverity = (SEVERITY_RANK[b.severity] ?? 0) - (SEVERITY_RANK[a.severity] ?? 0);
    if (bySeverity !== 0) return bySeverity;
    return Number(b.kind === 'potential-bug') - Number(a.kind === 'potential-bug');
  });
}
