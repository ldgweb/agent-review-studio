import type { ReactNode } from 'react';
import type {
  AgentName,
  FindingSeverity,
  RiskLevel,
  SessionStatus,
} from '../types/domain';
import { riskColor, statusMeta } from '../lib/risk';

export function StatTile({ value, label }: { value: ReactNode; label: string }) {
  return (
    <div className="stat-tile">
      <div className="stat-value">{value}</div>
      <div className="stat-label">{label}</div>
    </div>
  );
}

export function RiskPill({ level, label }: { level: RiskLevel; label?: string }) {
  const cls =
    level === 'low' ? 'badge-ok' : level === 'medium' ? 'badge-warn' : level === 'high' ? 'badge-high' : 'badge-err';
  return (
    <span className={`badge ${cls}`}>
      <span className="dot" style={{ background: riskColor(level) }} />
      {label ?? level}
    </span>
  );
}

export function SeverityPill({ severity }: { severity: FindingSeverity }) {
  const cls =
    severity === 'info'
      ? 'badge-accent'
      : severity === 'low'
        ? 'badge-ok'
        : severity === 'medium'
          ? 'badge-warn'
          : severity === 'high'
            ? 'badge-high'
            : 'badge-err';
  return <span className={`badge ${cls}`}>{severity}</span>;
}

export function StatusPill({ status }: { status: SessionStatus }) {
  const meta = statusMeta[status];
  return <span className={`badge ${meta.className}`}>{meta.label}</span>;
}

const agentColors: Record<AgentName, string> = {
  'claude-code': '#d97757',
  codex: '#7ec8a5',
  cursor: '#9a8cf0',
  copilot: '#58a6ff',
  aider: '#5b8def',
  'gemini-cli': '#d9a63c',
  custom: '#9aa5b8',
};

export function agentColor(agent: AgentName): string {
  return agentColors[agent] ?? agentColors.custom;
}

export function AgentBadge({ agent, label }: { agent: AgentName; label: string }) {
  return (
    <span className="badge badge-outline">
      <span className="dot" style={{ background: agentColors[agent] ?? agentColors.custom }} />
      {label}
    </span>
  );
}

