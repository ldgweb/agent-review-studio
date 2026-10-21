import { useMemo, useState } from 'react';
import { IconArchive, IconRefresh, IconRestore, IconSearch } from '../../components/icons';
import { agentColor } from '../../components/ui';
import { maxRiskLevel } from '../../lib/risk';
import { effectiveStatus, useAllApprovals, useApprovalStoreError } from '../../state/approvalStore';
import { useSessions } from '../../state/sessionsStore';
import { apiFetch, errorMessage } from '../../lib/api';
import { SessionList } from './SessionList';
import { WelcomePanel } from './WelcomePanel';
import type { AgentName, AgentSession, RiskLevel, SessionStatus } from '../../types/domain';

// The inbox is one review queue, sorted by the side panel (newest, riskiest, largest). Search and facets narrow it; row selection feeds archive/restore.

const RISK_ORDER: RiskLevel[] = ['low', 'medium', 'high', 'critical'];

type SortKey = 'newest' | 'risk' | 'size';

export function SessionsInbox() {
  const { sessions, loading, liveError, storageError, refresh } = useSessions();
  const approvals = useAllApprovals();
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState('');
  const [agentFilter, setAgentFilter] = useState<string | null>(null);
  const [projectFilter, setProjectFilter] = useState<string | null>(null);
  const [minRisk, setMinRisk] = useState<RiskLevel | null>(null);
  const [showArchived, setShowArchived] = useState(false);
  const [sort, setSort] = useState<SortKey>('newest');
  const [archiving, setArchiving] = useState(false);

  const approvalStoreError = useApprovalStoreError();
  const [archiveError, setArchiveError] = useState<string | null>(null);

  // "In review" the moment you start deciding, even before a finalize
  const displayStatusOf = (s: AgentSession): SessionStatus => effectiveStatus(s, approvals[s.sessionId]);

  const archivedCount = sessions.filter((s) => s.archived).length;

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = sessions.filter((s) => {
      if (Boolean(s.archived) !== showArchived) return false;
      if (agentFilter && s.agent !== agentFilter) return false;
      if (projectFilter && s.projectName !== projectFilter) return false;
      if (minRisk && RISK_ORDER.indexOf(maxRiskLevel(s)) < RISK_ORDER.indexOf(minRisk)) return false;
      if (q) {
        const haystack = `${s.declaredGoal}\n${s.projectName}\n${s.originalPrompt}\n${s.branch}`.toLowerCase();
        if (!haystack.includes(q)) return false;
      }
      return true;
    });
    if (sort === 'risk') {
      filtered.sort(
        (a, b) =>
          RISK_ORDER.indexOf(maxRiskLevel(b)) - RISK_ORDER.indexOf(maxRiskLevel(a)) ||
          new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime(),
      );
    } else if (sort === 'size') {
      filtered.sort(
        (a, b) =>
          b.stats.linesAdded + b.stats.linesRemoved - (a.stats.linesAdded + a.stats.linesRemoved) ||
          new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime(),
      );
    }
    return filtered;
  }, [sessions, query, agentFilter, projectFilter, minRisk, showArchived, sort]);

  const agents = useMemo(() => {
    const seen = new Map<string, string>();
    for (const s of sessions) if (!seen.has(s.agent)) seen.set(s.agent, s.agentLabel);
    return [...seen.entries()];
  }, [sessions]);

  const projects = useMemo(() => {
    const seen = new Set<string>();
    for (const s of sessions) if (Boolean(s.archived) === showArchived) seen.add(s.projectName);
    return [...seen];
  }, [sessions, showArchived]);

  const toggle = (id: string) =>
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const selected = visible.filter((s) => selectedIds.has(s.sessionId));

  const archive = (ids: string[], archived: boolean) => {
    if (ids.length === 0) return;
    setArchiving(true);
    setArchiveError(null);
    apiFetch('/api/sessions/archive', { method: 'POST', body: { sessionIds: ids, archived } })
      .then(() => {
        setSelectedIds(new Set());
        refresh();
      })
      .catch((e: unknown) => setArchiveError(`${archived ? 'Archive' : 'Restore'} failed: ${errorMessage(e)}`))
      .finally(() => setArchiving(false));
  };

  const agentCounts = useMemo(() => {
    const m = new Map<string, number>();
    for (const s of sessions) if (Boolean(s.archived) === showArchived) m.set(s.agent, (m.get(s.agent) ?? 0) + 1);
    return m;
  }, [sessions, showArchived]);

  const projectCounts = useMemo(() => {
    const m = new Map<string, number>();
    for (const s of sessions) if (Boolean(s.archived) === showArchived) m.set(s.projectName, (m.get(s.projectName) ?? 0) + 1);
    return m;
  }, [sessions, showArchived]);

  const RISK_LABEL: Record<RiskLevel, string> = { low: 'Low', medium: 'Medium', high: 'High', critical: 'Critical' };
  const SORT_LABEL: Record<SortKey, string> = { newest: 'Newest', risk: 'Riskiest', size: 'Largest' };

  return (
    <main className="page inbox-page">
      <header className="inbox-head">
        <h1 className="inbox-title">{showArchived ? 'Archive' : 'Review queue'}</h1>
        <span className="inbox-count tnum">{visible.length}</span>
        <div className="search-field">
          <IconSearch size={13} />
          <input
            className="search-input"
            placeholder="Search goal, project, prompt…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <span style={{ flex: 1 }} />
        {selected.length > 0 && (
          <button className="btn btn-sm" disabled={archiving} onClick={() => archive(selected.map((s) => s.sessionId), !showArchived)}>
            {showArchived ? <IconRestore size={12} /> : <IconArchive size={12} />}
            {showArchived ? ` Restore (${selected.length})` : ` Archive (${selected.length})`}
          </button>
        )}
        <button className="btn btn-sm" onClick={refresh} disabled={loading} title="Rescan sources now">
          <IconRefresh size={13} />
        </button>
      </header>

      {liveError && (
        <div className="banner banner-err" style={{ margin: '0 0 var(--sp-4)' }}>
          <span>✗</span>
          <span>Could not reach the local API: {liveError}</span>
        </div>
      )}

      {storageError && (
        <div className="banner banner-err" role="alert" style={{ margin: '0 0 var(--sp-4)' }}>
          <span>✗</span>
          <span>Session history could not be saved: {storageError}</span>
        </div>
      )}

      {archiveError && (
        <div className="banner banner-err" role="alert" style={{ margin: '0 0 var(--sp-4)' }}>
          <span>✗</span>
          <span style={{ flex: 1 }}>{archiveError}</span>
          <button className="linklike" title="Dismiss" onClick={() => setArchiveError(null)}>✕</button>
        </div>
      )}

      {approvalStoreError && (
        <div className="banner banner-err" role="alert" style={{ margin: '0 0 var(--sp-4)' }}>
          <span>✗</span>
          <span>{approvalStoreError}</span>
        </div>
      )}

      {loading && sessions.length === 0 && (
        <div className="scan-state">
          <span className="scan-spinner" aria-hidden="true" />
          <div className="scan-title">Scanning your agent sessions</div>
          <div className="scan-sub">Reading transcripts from Claude Code, Codex, Cursor, VS Code Copilot, Aider and Gemini CLI…</div>
        </div>
      )}

      {!loading && sessions.length === 0 && !liveError && <WelcomePanel />}

      {sessions.length > 0 && (
        <div className="inbox-body">
          <div className="inbox-main">
            {visible.length === 0 ? (
              <div className="faint" style={{ padding: 'var(--sp-4) 0' }}>No sessions match the current filters.</div>
            ) : (
              <SessionList
                sessions={visible}
                statusOf={displayStatusOf}
                selectedIds={selectedIds}
                onToggleSelect={toggle}
                onArchive={(id) => archive([id], !showArchived)}
                archivedView={showArchived}
              />
            )}
          </div>

          <aside className="inbox-side">
            <div className="filter-group">
              <div className="filter-label">Sort by</div>
              {(['newest', 'risk', 'size'] as SortKey[]).map((k) => (
                <button key={k} className={`filter-item${sort === k ? ' on' : ''}`} onClick={() => setSort(k)}>
                  {SORT_LABEL[k]}
                </button>
              ))}
            </div>

            <div className="filter-group">
              <div className="filter-label">Minimum risk</div>
              {(['medium', 'high', 'critical'] as RiskLevel[]).map((level) => (
                <button
                  key={level}
                  className={`filter-item${minRisk === level ? ' on' : ''}`}
                  onClick={() => setMinRisk(minRisk === level ? null : level)}
                >
                  <span className="filter-dot" style={{ background: `var(--risk-${level})` }} />
                  {RISK_LABEL[level]} &amp; up
                </button>
              ))}
            </div>

            {agents.length > 1 && (
              <div className="filter-group">
                <div className="filter-label">Agent</div>
                {agents.map(([agent, label]) => (
                  <button
                    key={agent}
                    className={`filter-item${agentFilter === agent ? ' on' : ''}`}
                    onClick={() => setAgentFilter(agentFilter === agent ? null : agent)}
                  >
                    <span className="filter-dot" style={{ background: agentColor(agent as AgentName) }} />
                    <span className="filter-name">{label}</span>
                    <span className="filter-count tnum">{agentCounts.get(agent) ?? 0}</span>
                  </button>
                ))}
              </div>
            )}

            {projects.length > 1 && (
              <div className="filter-group">
                <div className="filter-label">Project</div>
                {projects.map((p) => (
                  <button
                    key={p}
                    className={`filter-item${projectFilter === p ? ' on' : ''}`}
                    onClick={() => setProjectFilter(projectFilter === p ? null : p)}
                  >
                    <span className="filter-name mono">{p}</span>
                    <span className="filter-count tnum">{projectCounts.get(p) ?? 0}</span>
                  </button>
                ))}
              </div>
            )}

            {(archivedCount > 0 || showArchived) && (
              <div className="filter-group">
                <button
                  className="filter-item subtle"
                  onClick={() => {
                    setShowArchived(!showArchived);
                    setSelectedIds(new Set());
                  }}
                >
                  {showArchived ? '← Back to the queue' : `Archived (${archivedCount})`}
                </button>
              </div>
            )}
          </aside>
        </div>
      )}
    </main>
  );
}
