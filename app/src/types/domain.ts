// ---------------------------------------------------------------------------
// Agent Review Studio — domain model
// The primary entity is the AgentSession: the full lifecycle of an AI coding
// agent working on a task (prompt → plan → actions → decisions → outcome).
// ---------------------------------------------------------------------------

export type AgentName = 'claude-code' | 'codex' | 'cursor' | 'copilot' | 'aider' | 'gemini-cli' | 'custom';

export type SessionStatus =
  | 'needs-review'
  | 'in-review'
  | 'approved'
  | 'partially-approved'
  | 'corrections-requested'
  | 'rejected';

export type RiskDimension = 'security' | 'database' | 'performance' | 'maintainability';

export type RiskLevel = 'low' | 'medium' | 'high' | 'critical';

export interface RiskScore {
  dimension: RiskDimension;
  /** 0–100, higher = riskier */
  score: number;
  level: RiskLevel;
  /** One-line reviewer justification */
  rationale: string;
  contributingFileIds: string[];
}

export interface DiffLine {
  kind: 'context' | 'add' | 'del';
  text: string;
  oldLine?: number;
  newLine?: number;
}

export interface DiffHunk {
  header: string;
  lines: DiffLine[];
}

export interface FileChange {
  id: string;
  path: string;
  changeType: 'created' | 'modified' | 'deleted';
  language: string;
  linesAdded: number;
  linesRemoved: number;
  /** FK → SystemArea */
  areaId: string;
  riskFlags: RiskDimension[];
  /** What changed, in one sentence */
  summary: string;
  /** Diff hunks for this file (may be truncated) */
  hunks: DiffHunk[];
  /** Where the hunks come from: the live repo or the reconstructed transcript.
   * Set at serve time by the git enrichment — never persisted, never hashed. */
  diffSource?: 'transcript' | 'git';
  /** Transcript changed the file but the working tree shows nothing:
   * 'committed' after the session, or 'missing' (reverted / overwritten) */
  drift?: 'committed' | 'missing';
}

export type TimelineEventKind =
  | 'plan'
  | 'analysis'
  | 'file-read'
  | 'file-edit'
  | 'command'
  | 'test-run'
  | 'error'
  | 'decision'
  | 'note';

export interface TimelineEvent {
  id: string;
  /** ISO 8601 */
  timestamp: string;
  kind: TimelineEventKind;
  title: string;
  /** Command output excerpt, error text, reasoning excerpt */
  detail?: string;
  filePath?: string;
  command?: string;
  exitCode?: number;
}

export type FindingKind = 'issue' | 'suggestion' | 'question-for-agent' | 'potential-bug';
export type FindingSeverity = 'info' | 'low' | 'medium' | 'high' | 'critical';

/** A line-anchored finding. No LLM involved: 'local' = the built-in checks
 * computed in normalize (hardcoded secrets, leftover debug, oversized files,
 * shrunk tests); 'custom' = the user's rules from Settings → Review
 * checks and the repo's .ars/rules.json. */
export interface ReviewerFinding {
  id: string;
  kind: FindingKind;
  severity: FindingSeverity;
  title: string;
  body: string;
  filePath?: string;
  line?: number;
  dimension?: RiskDimension;
  suggestedFix?: string;
  source?: 'local' | 'custom';
}

/** A custom review check (Settings → Review checks; repo scope in .ars/rules.json) */
export interface FindingRule {
  id: string;
  enabled: boolean;
  label: string;
  kind: FindingKind;
  severity: FindingSeverity;
  /** What the pattern runs against: the diff's added lines, or the file path */
  target: 'added-lines' | 'path';
  /** Optional glob limiting which files the rule sees (added-lines only) */
  fileGlob?: string;
  /** Rust regex (no lookaround) */
  pattern: string;
  /** Shown as the finding body; empty = show the matched text */
  message?: string;
}

/** Folder group used for review filtering, not a dependency analysis. */
export interface SystemArea {
  id: string;
  label: string;
  /** Critical zones get highlighted */
  critical: boolean;
  /** Reserved for format compatibility; no dependencies are inferred. */
  dependsOn: string[];
}

export interface SessionStats {
  filesAnalyzed: number;
  filesModified: number;
  linesAdded: number;
  linesRemoved: number;
  commandsRun: number;
  testsRun: number;
  testsFailed: number;
  durationMinutes: number;
}

export interface SessionSummary {
  text: string;
  impactedAreas: string[];
  riskWarnings: string[];
}

/** THE primary entity */
export interface AgentSession {
  sessionId: string;
  /** Fingerprint of the transcript, HEAD and current session files. */
  reviewRevision?: string;
  contentHash?: string;
  /** 'live' = ingested from a real agent transcript */
  source?: 'live';
  timestamp: string;
  agent: AgentName;
  agentLabel: string;
  model: string;
  projectName: string;
  /** Absolute path of the project the agent worked in, when the source reveals it */
  projectPath?: string;
  branch: string;
  originalPrompt: string;
  declaredGoal: string;
  /** Ordered plan steps declared by the agent before acting */
  initialPlan: string[];
  status: SessionStatus;
  /** Agent's final self-report */
  outcome: string;
  stats: SessionStats;
  summary: SessionSummary;
  systemAreas: SystemArea[];
  files: FileChange[];
  timeline: TimelineEvent[];
  risks: RiskScore[];
  findings: ReviewerFinding[];
  /** Hidden from the default inbox; set from the archive action (server-persisted) */
  archived?: boolean;
  /** projectPath is a git work tree — commit/revert actions are available.
   * Set at serve time by the git enrichment. */
  gitRepo?: boolean;
  /** Governance from the repo's .ars/policy.json, attached at serve time */
  policy?: ArsPolicy;
}

/** Repo policy (.ars/policy.json) — committed, shared repo governance */
export interface ArsPolicy {
  /** Block an approving finalize when a risk score meets its threshold */
  blockFinalizeAboveRisk?: Partial<Record<RiskDimension, number>>;
  /** Set by the loader when the file exists but does not parse */
  error?: string;
}

// ---------------------------------------------------------------------------
// Mutable client state (approval store: saved via the core's /approvals, cached in localStorage)
// ---------------------------------------------------------------------------

export type FileApproval = 'pending' | 'approved' | 'rejected' | 'correction-requested';

export type SessionDecision =
  | 'approved'
  | 'partially-approved'
  | 'corrections-requested'
  | 'rejected';

/** A reviewer comment anchored to a diff line */
export interface LineComment {
  /** stable anchor within the file's rendered diff: "<hunkIndex>:<lineIndex>" */
  anchor: string;
  /** display line number (new side when available) */
  line?: number;
  text: string;
}

/** Human stance on a finding; 'correction' feeds the correction draft */
export type FindingReviewState = 'accepted' | 'dismissed' | 'correction';

export interface ApprovalState {
  sessionId: string;
  reviewRevision?: string;
  contentHash?: string;
  /** fileId → state */
  fileStates: Record<string, FileApproval>;
  /** fileId or findingId → note for the correction request */
  correctionNotes: Record<string, string>;
  /** fileId → comments left on specific diff lines */
  lineComments: Record<string, LineComment[]>;
  /** findingId → human stance on the finding */
  findingStates: Record<string, FindingReviewState>;
  /** e.g. "reject all database changes" */
  rejectedCategories: RiskDimension[];
  sessionDecision?: SessionDecision;
}
