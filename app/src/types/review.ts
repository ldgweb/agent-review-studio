// ---------------------------------------------------------------------------
// The project review (core/review): everything uncommitted in a Git work
// tree, hunk by hunk, with where each hunk came from and what the reviewer
// decided. Mirrors the Rust serialization (camelCase, optional fields
// omitted when absent).
// ---------------------------------------------------------------------------

import type {
  AgentName,
  DiffLine,
  FindingReviewState,
  ReviewerFinding,
  RiskDimension,
  RiskScore,
  SessionStats,
  SystemArea,
  TimelineEvent,
} from './domain';

export type Verdict = 'approved' | 'rejected' | 'changes';
/** A file's verdicts taken together */
export type FileVerdictView = Verdict | 'mixed' | 'pending';

export interface ReviewLine extends DiffLine {
  /** The same text was removed (or added) elsewhere in the changes */
  moved?: boolean;
  /** On a moved block's first line: where the other half is */
  movedPeer?: { path: string; line: number };
}

export interface HunkOrigin {
  /** agent: every changed line comes from recorded edits; partial: some do; none: none does */
  kind: 'agent' | 'partial' | 'none';
  sessionId?: string;
  agentLabel?: string;
  turn?: number;
  prompt?: string;
  /** The agent's own words right before the edit */
  rationale?: string;
  timestamp?: string;
  eventId?: string;
  matched: number;
  total: number;
  /** For unexplained lines: the newest command that names the file */
  commandHint?: { sessionId: string; eventId: string; command: string };
}

export interface ReviewHunk {
  /** Content id: the change itself, wherever it sits in the file */
  id: string;
  header: string;
  lines: ReviewLine[];
  origin: HunkOrigin;
  noise?: 'whitespace' | 'moved';
  /** The same change made in several places */
  group?: string;
  truncated?: boolean;
}

export interface SinceHunk {
  header: string;
  lines: ReviewLine[];
}

export interface ReviewFile {
  id: string;
  path: string;
  oldPath?: string;
  changeType: 'created' | 'modified' | 'deleted' | 'renamed';
  untracked: boolean;
  binary: boolean;
  language: string;
  linesAdded: number;
  linesRemoved: number;
  areaId: string;
  riskFlags: RiskDimension[];
  /** Why the file counts as generated (lockfile, minified, linguist-generated…) */
  generated?: string;
  /** HEAD's blob + the current bytes: what a file verdict binds to */
  fingerprint: string;
  hunks: ReviewHunk[];
  verification: 'verified' | 'unverified' | 'no-signal';
  sessionIds: string[];
  /** Against the last copied review */
  since?: 'changed' | 'unchanged' | 'new';
  sinceHunks?: SinceHunk[];
  truncated?: boolean;
  modeChange?: string;
}

export interface SessionTurn {
  index: number;
  prompt: string;
  timestamp: string;
}

export interface SessionContext {
  sessionId: string;
  agent: AgentName;
  agentLabel: string;
  model: string;
  timestamp: string;
  lastActivity: string;
  projectPath?: string;
  branch: string;
  originalPrompt: string;
  declaredGoal: string;
  initialPlan: string[];
  outcome: string;
  stats: SessionStats;
  timeline: TimelineEvent[];
  turns: SessionTurn[];
  attributedHunks: number;
}

export interface EditGroup {
  id: string;
  hunkIds: string[];
  paths: string[];
  preview: string;
}

export interface ReviewComment {
  id: string;
  path: string;
  hunkId: string;
  /** Line index within the hunk, and that line's kind and text */
  li: number;
  kind: DiffLine['kind'];
  lineText: string;
  line?: number;
  text: string;
  /** Code the reviewer proposes instead of the commented line ('' removes it) */
  suggestion?: string;
  at: string;
  /** A previous round's comment the reviewer marked as dealt with */
  resolved?: boolean;
}

export interface ReviewRound {
  index: number;
  at: string;
  head?: string | null;
  text: string;
  /** How many items the copied review had */
  items?: number;
  files: Record<string, string | null>;
  comments: ReviewComment[];
  notes: Record<string, string>;
}

export interface ReviewState {
  version: number;
  hunks: Record<string, { verdict: Verdict; path: string; at: string }>;
  /** Files without hunks (binary, mode-only): bound to the fingerprint */
  files: Record<string, { verdict: Verdict; fingerprint: string; at: string }>;
  /** path → change request for the agent */
  notes: Record<string, string>;
  comments: ReviewComment[];
  findings: Record<string, FindingReviewState>;
  draft?: string;
  rounds: ReviewRound[];
}

export interface SinceReview {
  round: number;
  at: string;
  changed: string[];
  unchanged: string[];
  added: string[];
  gone: string[];
}

/** The review for the agent, written by the core from the saved state */
export interface AgentPackage {
  /** Things it asks the agent to do */
  items: number;
  text: string;
}

export interface ProjectReview {
  project: { root: string; name: string; branch: string; head: string | null };
  fingerprint: string;
  files: ReviewFile[];
  sessions: SessionContext[];
  findings: ReviewerFinding[];
  groups: EditGroup[];
  risks: RiskScore[];
  areas: SystemArea[];
  state: ReviewState;
  stateRev: number;
  sinceReview?: SinceReview;
  stats: {
    files: number;
    added: number;
    removed: number;
    hunks: number;
    agentHunks: number;
    partialHunks: number;
    unattributedHunks: number;
  };
  warnings: string[];
  disabledSources: string[];
  package: AgentPackage;
}

/** A project on the home screen */
export interface ProjectRow {
  root: string;
  name: string;
  git: boolean;
  /** The folder is gone (moved or deleted) */
  missing: boolean;
  /** Hidden by the user: listed only when hidden projects are asked for */
  hidden: boolean;
  /** All its sessions: what hiding the project hides */
  sessionIds: string[];
  sessions: Array<{
    sessionId: string;
    agent: AgentName;
    agentLabel: string;
    declaredGoal: string;
    timestamp: string;
    filesModified: number;
  }>;
  sessionCount: number;
  agents: string[];
  lastActivity: string;
  reviewedAt?: string | null;
  /** Uncommitted changes (null: not a Git repo or unreadable) */
  changes?: number | null;
}
