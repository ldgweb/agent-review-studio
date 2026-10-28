import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import type { ProjectRow } from '../../types/review';
import { AgentBadge } from '../../components/ui';
import { IconRefresh } from '../../components/icons';
import { apiFetch, errorMessage } from '../../lib/api';
import { formatDateTime, formatRelative } from '../../lib/format';
import { useSessions } from '../../state/sessionsStore';
import { WelcomePanel } from './WelcomePanel';

// The home screen: first the projects with uncommitted changes, the ones
// worth opening; everything else (all committed, no Git repository, a
// folder that is gone) folds below, each saying why, each one click from
// being hidden. A project opens its review, the same one `ars .` opens
// from a terminal.

interface ProjectsPayload {
  projects?: ProjectRow[];
  hiddenCount?: number;
  /** Something that could not be read, said in words */
  warning?: string;
}

const HIDE_TITLE = 'Hide this project from the list. It comes back with its next agent session.';

const needsReview = (p: ProjectRow) => p.git && !p.missing && (p.changes ?? 0) > 0;

export function ProjectsHome() {
  const { version, loading: scanning, refresh, liveError, storageError } = useSessions();
  const navigate = useNavigate();
  const [data, setData] = useState<ProjectsPayload | null>(null);
  const [error, setError] = useState('');
  const [showQuiet, setShowQuiet] = useState(false);
  const [showHidden, setShowHidden] = useState(false);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    let cancelled = false;
    apiFetch<ProjectsPayload>(showHidden ? '/api/projects?hidden=1' : '/api/projects')
      .then((r) => {
        if (cancelled) return;
        setData(r);
        setError('');
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
    // The session index moved: the projects (and their counts) may have too
  }, [version, showHidden, reload]);

  // Commits happen in other tools: coming back to the window reloads the counts
  useEffect(() => {
    const again = () => setReload((n) => n + 1);
    window.addEventListener('focus', again);
    return () => window.removeEventListener('focus', again);
  }, []);

  const setHidden = useCallback(async (p: ProjectRow, hidden: boolean) => {
    try {
      await apiFetch('/api/sessions/archive', { method: 'POST', body: { sessionIds: p.sessionIds, archived: hidden } });
      setReload((n) => n + 1);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  const open = (root: string) => navigate(`/project?path=${encodeURIComponent(root)}`);
  const projects = data?.projects ?? null;
  const visible = projects?.filter((p) => !p.hidden) ?? [];
  const toReview = visible.filter(needsReview);
  const quiet = visible.filter((p) => !needsReview(p));
  const hidden = projects?.filter((p) => p.hidden) ?? [];
  const hiddenCount = data?.hiddenCount ?? 0;

  return (
    <main className="page">
      <div className="home">
        <div className="home-head">
          <div>
            <h1 className="home-title">Projects</h1>
            <p className="faint">
              Run <span className="mono">ars .</span> in a project to open its review straight away.
            </p>
          </div>
          <button className="btn-tool" disabled={scanning} onClick={refresh} title="Read every agent source again">
            <IconRefresh size={13} /> Rescan
          </button>
        </div>

        {(error || liveError) && <div className="banner banner-err" role="alert">{error || liveError}</div>}
        {storageError && <div className="banner banner-warn" role="status">{storageError}</div>}
        {data?.warning && <div className="banner banner-warn" role="status">{data.warning}</div>}
        {projects === null && !error && (
          <div className="banner banner-info" role="status">
            <span className="spinner" aria-hidden="true" /> Looking for your projects…
          </div>
        )}
        {projects !== null && projects.length === 0 && hiddenCount === 0 && <WelcomePanel />}

        {projects !== null && (projects.length > 0 || hiddenCount > 0) && (
          <>
            <section className="home-section">
              <div className="files-label">To review</div>
              {toReview.length === 0 ? (
                <div className="home-empty">
                  Nothing is waiting for a review. When an agent changes one of your projects, it shows up here.
                </div>
              ) : (
                toReview.map((p) => <ProjectCard key={p.root} project={p} onOpen={() => open(p.root)} onHide={() => void setHidden(p, true)} />)
              )}
            </section>

            {quiet.length > 0 && (
              <section className="home-section">
                <button className="home-toggle" aria-expanded={showQuiet} onClick={() => setShowQuiet(!showQuiet)}>
                  <span className={`chev${showQuiet ? ' open' : ''}`} aria-hidden="true">›</span>
                  {quiet.length} project{quiet.length === 1 ? '' : 's'} with nothing to review
                </button>
                {showQuiet && quiet.map((p) => <QuietRow key={p.root} project={p} onOpen={() => open(p.root)} onHide={() => void setHidden(p, true)} />)}
              </section>
            )}

            {(hiddenCount > 0 || showHidden) && (
              <section className="home-section">
                <button className="home-toggle" aria-expanded={showHidden} onClick={() => setShowHidden(!showHidden)}>
                  <span className={`chev${showHidden ? ' open' : ''}`} aria-hidden="true">›</span>
                  {hiddenCount} hidden project{hiddenCount === 1 ? '' : 's'}
                </button>
                {showHidden &&
                  hidden.map((p) => (
                    <QuietRow key={p.root} project={p} onOpen={() => open(p.root)} onUnhide={() => void setHidden(p, false)} />
                  ))}
              </section>
            )}
          </>
        )}
      </div>
    </main>
  );
}

function ProjectCard({ project: p, onOpen, onHide }: { project: ProjectRow; onOpen: () => void; onHide: () => void }) {
  return (
    <div className="project-card">
      <button className="project-open" onClick={onOpen} title={p.root}>
        <span className="project-line">
          <strong>{p.name}</strong>
          <span className="faint mono project-path">{p.root}</span>
          <span style={{ flex: 1 }} />
          <span className="badge badge-warn">{p.changes} to review</span>
          {p.lastActivity && (
            <span className="faint project-when" title={formatDateTime(p.lastActivity)}>
              {formatRelative(p.lastActivity)}
            </span>
          )}
        </span>
        {p.sessions.length > 0 && (
          <span className="project-sessions">
            {p.sessions.slice(0, 3).map((s) => (
              <span key={s.sessionId} className="project-session">
                <AgentBadge agent={s.agent} label={s.agentLabel} />
                <span className="muted">{s.declaredGoal}</span>
              </span>
            ))}
          </span>
        )}
      </button>
      {p.sessionIds.length > 0 && (
        <button className="btn-mini" title={HIDE_TITLE} onClick={onHide}>
          Hide
        </button>
      )}
    </div>
  );
}

/** Why a project has nothing to review, in words */
function quietReason(p: ProjectRow): { badge: string; tone: string; text?: string; openable: boolean } {
  if (p.missing) return { badge: 'folder not found', tone: 'badge-err', text: 'It was moved or deleted.', openable: false };
  if (!p.git) {
    return { badge: 'no Git repository', tone: 'badge-neutral', text: 'ARS reads the changes from Git, and this folder has no repository.', openable: false };
  }
  return { badge: 'all committed', tone: 'badge-ok', openable: true };
}

function QuietRow({
  project: p,
  onOpen,
  onHide,
  onUnhide,
}: {
  project: ProjectRow;
  onOpen: () => void;
  onHide?: () => void;
  onUnhide?: () => void;
}) {
  const why = quietReason(p);
  return (
    <div className="project-quiet">
      <button className="project-open" onClick={onOpen} disabled={!why.openable} title={why.openable ? p.root : `${p.root}: ${why.text}`}>
        <span className="project-line">
          <span className="project-name">{p.name}</span>
          <span className="faint mono project-path">{p.root}</span>
          <span style={{ flex: 1 }} />
          {needsReview(p) ? (
            <span className="badge badge-warn">{p.changes} to review</span>
          ) : (
            <span className={`badge ${why.tone}`}>{why.badge}</span>
          )}
          {p.lastActivity && (
            <span className="faint project-when" title={formatDateTime(p.lastActivity)}>
              {formatRelative(p.lastActivity)}
            </span>
          )}
        </span>
        {why.text && !needsReview(p) && <span className="project-why">{why.text}</span>}
      </button>
      {onHide && p.sessionIds.length > 0 && (
        <button className="btn-mini" title={HIDE_TITLE} onClick={onHide}>
          Hide
        </button>
      )}
      {onUnhide && (
        <button className="btn-mini" onClick={onUnhide}>
          Show again
        </button>
      )}
    </div>
  );
}
