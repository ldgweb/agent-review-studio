import type {
  AgentSession,
  RiskLevel,
  SessionStatus,
  FileApproval,
} from '../types/domain';

export function riskColor(level: RiskLevel): string {
  switch (level) {
    case 'low':
      return 'var(--risk-low)';
    case 'medium':
      return 'var(--risk-medium)';
    case 'high':
      return 'var(--risk-high)';
    case 'critical':
      return 'var(--risk-critical)';
  }
}

/** Highest risk level across the session's risk scores */
export function maxRiskLevel(session: AgentSession): RiskLevel {
  const order: RiskLevel[] = ['low', 'medium', 'high', 'critical'];
  return session.risks.reduce<RiskLevel>(
    (max, r) => (order.indexOf(r.level) > order.indexOf(max) ? r.level : max),
    'low',
  );
}

export const statusMeta: Record<SessionStatus, { label: string; className: string }> = {
  'needs-review': { label: 'Needs review', className: 'badge-warn' },
  'in-review': { label: 'In review', className: 'badge-accent' },
  approved: { label: 'Approved', className: 'badge-ok' },
  'partially-approved': { label: 'Partially approved', className: 'badge-accent' },
  'corrections-requested': { label: 'Corrections requested', className: 'badge-high' },
  rejected: { label: 'Rejected', className: 'badge-err' },
};

export const fileApprovalMeta: Record<FileApproval, { label: string; color: string }> = {
  pending: { label: 'Pending', color: 'var(--text-2)' },
  approved: { label: 'Approved', color: 'var(--ok)' },
  rejected: { label: 'Rejected', color: 'var(--err)' },
  'correction-requested': { label: 'Correction requested', color: 'var(--risk-high)' },
};
