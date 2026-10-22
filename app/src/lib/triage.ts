import type {
  AgentSession,
  FileApproval,
  FileChange,
  FindingKind,
  FindingSeverity,
  ReviewerFinding,
  RiskDimension,
} from '../types/domain';

// Risk-first triage: ordering, grouping and weighting that answer
// "what should the reviewer look at first?". Pure functions over the
// session — no state, no React.

export type GroupMode = 'dir' | 'risk' | 'area';

export interface FileGroup {
  key: string;
  label: string;
  /** Extra context rendered next to the label (e.g. "critical area") */
  meta?: string;
  files: FileChange[];
}

const SEVERITY_RANK: Record<FindingSeverity, number> = {
  info: 0,
  low: 1,
  medium: 2,
  high: 3,
  critical: 4,
};

export function severityRank(severity: FindingSeverity): number {
  return SEVERITY_RANK[severity];
}

const KIND_LABEL: Record<FindingKind, string> = {
  issue: 'issue',
  suggestion: 'suggestion',
  'question-for-agent': 'question for agent',
  'potential-bug': 'potential bug',
};

export function findingKindLabel(kind: FindingKind): string {
  return KIND_LABEL[kind];
}

export function dirOf(path: string): string {
  const idx = path.lastIndexOf('/');
  return idx === -1 ? '' : path.slice(0, idx);
}

export function findingsForFile(session: AgentSession, file: FileChange): ReviewerFinding[] {
  return session.findings.filter((f) => f.filePath === file.path);
}

export interface WeightBreakdown {
  total: number;
  riskFlags: number;
  criticalArea: boolean;
  /** Contribution of the worst finding on the file: critical 3 / high 2 / medium 1 */
  findingSeverity: number;
}

export function fileWeight(file: FileChange, session: AgentSession): WeightBreakdown {
  const criticalArea = session.systemAreas.some((a) => a.id === file.areaId && a.critical);
  let findingSeverity = 0;
  for (const f of findingsForFile(session, file)) {
    const rank = SEVERITY_RANK[f.severity];
    const contribution = rank >= 4 ? 3 : rank >= 3 ? 2 : rank >= 2 ? 1 : 0;
    if (contribution > findingSeverity) findingSeverity = contribution;
  }
  return {
    total: 1 + file.riskFlags.length + (criticalArea ? 1 : 0) + findingSeverity,
    riskFlags: file.riskFlags.length,
    criticalArea,
    findingSeverity,
  };
}

export function describeWeight(w: WeightBreakdown): string {
  const parts: string[] = [];
  if (w.riskFlags > 0) parts.push(`${w.riskFlags} risk flag${w.riskFlags > 1 ? 's' : ''}`);
  if (w.criticalArea) parts.push('critical area');
  if (w.findingSeverity > 0) parts.push('reviewer findings');
  return parts.length > 0 ? parts.join(' · ') : 'no risk signals';
}

export function needsAttention(
  file: FileChange,
  session: AgentSession,
  unverifiedIds: ReadonlySet<string>,
): boolean {
  if (file.riskFlags.length > 0) return true;
  if (findingsForFile(session, file).some((f) => SEVERITY_RANK[f.severity] >= SEVERITY_RANK.medium)) return true;
  if (session.systemAreas.some((a) => a.id === file.areaId && a.critical)) return true;
  return unverifiedIds.has(file.id);
}

export function groupFiles(
  files: FileChange[],
  session: AgentSession,
  mode: GroupMode,
  unverifiedIds: ReadonlySet<string>,
): FileGroup[] {
  if (mode === 'risk') {
    const attention = files.filter((f) => needsAttention(f, session, unverifiedIds));
    const routine = files.filter((f) => !needsAttention(f, session, unverifiedIds));
    attention.sort((a, b) => fileWeight(b, session).total - fileWeight(a, session).total);
    const groups: FileGroup[] = [];
    if (attention.length > 0) {
      groups.push({ key: 'attention', label: 'Needs attention', meta: 'highest weight first', files: attention });
    }
    if (routine.length > 0) {
      groups.push({ key: 'routine', label: 'Routine', meta: 'no risk signals', files: routine });
    }
    return groups;
  }

  if (mode === 'area') {
    const areas = [...session.systemAreas].sort((a, b) => Number(b.critical) - Number(a.critical));
    const groups: FileGroup[] = [];
    const claimed = new Set<string>();
    for (const area of areas) {
      const inArea = files.filter((f) => f.areaId === area.id);
      if (inArea.length === 0) continue;
      inArea.forEach((f) => claimed.add(f.id));
      groups.push({
        key: `area:${area.id}`,
        label: area.label,
        meta: area.critical ? 'critical area' : undefined,
        files: inArea,
      });
    }
    const rest = files.filter((f) => !claimed.has(f.id));
    if (rest.length > 0) groups.push({ key: 'area:none', label: 'Other', files: rest });
    return groups;
  }

  const byDir = new Map<string, FileChange[]>();
  for (const f of files) {
    const dir = dirOf(f.path);
    byDir.set(dir, [...(byDir.get(dir) ?? []), f]);
  }
  return [...byDir.entries()].map(([dir, dirFiles]) => ({
    key: `dir:${dir}`,
    label: dir ? `${dir}/` : '(root)',
    files: dirFiles,
  }));
}

export interface RiskCoverage {
  done: number;
  total: number;
  /** 0–100 */
  pct: number;
}

/** Review coverage weighted by risk: deciding a risky file counts more */
export function riskCoverage(
  files: FileChange[],
  session: AgentSession,
  fileStates: Record<string, FileApproval>,
): RiskCoverage {
  let done = 0;
  let total = 0;
  for (const f of files) {
    const w = fileWeight(f, session).total;
    total += w;
    if ((fileStates[f.id] ?? 'pending') !== 'pending') done += w;
  }
  return { done, total, pct: total === 0 ? 100 : Math.round((done / total) * 100) };
}

/** Severity descending; potential-bug outranks other kinds at equal severity */
export function sortFindings(findings: ReviewerFinding[]): ReviewerFinding[] {
  return [...findings].sort((a, b) => {
    const bySeverity = SEVERITY_RANK[b.severity] - SEVERITY_RANK[a.severity];
    if (bySeverity !== 0) return bySeverity;
    return Number(b.kind === 'potential-bug') - Number(a.kind === 'potential-bug');
  });
}

export interface FindingCounts {
  byKind: Partial<Record<FindingKind, number>>;
  bySeverity: Partial<Record<FindingSeverity, number>>;
}

export function findingCounts(findings: ReviewerFinding[]): FindingCounts {
  const byKind: Partial<Record<FindingKind, number>> = {};
  const bySeverity: Partial<Record<FindingSeverity, number>> = {};
  for (const f of findings) {
    byKind[f.kind] = (byKind[f.kind] ?? 0) + 1;
    bySeverity[f.severity] = (bySeverity[f.severity] ?? 0) + 1;
  }
  return { byKind, bySeverity };
}

/** Files a category verdict applies to: cited by the risk score, or flagged with the dimension */
export function categoryFileIds(session: AgentSession, dimension: RiskDimension): string[] {
  const ids = new Set<string>();
  const risk = session.risks.find((r) => r.dimension === dimension);
  for (const id of risk?.contributingFileIds ?? []) ids.add(id);
  for (const f of session.files) {
    if (f.riskFlags.includes(dimension)) ids.add(f.id);
  }
  return session.files.filter((f) => ids.has(f.id)).map((f) => f.id);
}
