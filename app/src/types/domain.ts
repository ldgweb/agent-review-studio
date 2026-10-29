// ---------------------------------------------------------------------------
// Types shared across the app: agents, diff lines, what an agent did (its
// timeline), the hints of the fixed checks and the review checks behind
// them. The project review itself is in types/review.ts.
// ---------------------------------------------------------------------------

export type AgentName = 'claude-code' | 'codex' | 'cursor' | 'copilot' | 'aider' | 'gemini-cli' | 'custom';

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

/** A hint at a line, from fixed checks (no AI involved). 'local': the
 * built-in checks (a hardcoded secret, leftover debug code, a very large
 * change, a test that shrank). 'custom': your rules from Settings → Review
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

// ---------------------------------------------------------------------------
// What the diff viewer knows of a comment (the stored one is in
// types/review.ts)
// ---------------------------------------------------------------------------

/** A reviewer comment anchored to a diff line */
export interface LineComment {
  /** Where it sits in the file's diff: "<hunkIndex>:<lineIndex>" */
  anchor: string;
  /** The line's number now (the new side when it has one) */
  line?: number;
  text: string;
  /** Replacement code for the line ('' removes it) */
  suggestion?: string;
}

/** What you decided on a hint; 'correction' puts it in the review for the agent */
export type FindingReviewState = 'accepted' | 'dismissed' | 'correction';
