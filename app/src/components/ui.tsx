import type { AgentName, FindingSeverity } from '../types/domain';

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

const agentColors: Record<AgentName, string> = {
  'claude-code': '#d97757',
  codex: '#7ec8a5',
  cursor: '#9a8cf0',
  copilot: '#58a6ff',
  aider: '#5b8def',
  'gemini-cli': '#d9a63c',
  custom: '#9aa5b8',
};

export function AgentBadge({ agent, label }: { agent: AgentName; label: string }) {
  return (
    <span className="badge badge-outline">
      <span className="dot" style={{ background: agentColors[agent] ?? agentColors.custom }} />
      {label}
    </span>
  );
}

