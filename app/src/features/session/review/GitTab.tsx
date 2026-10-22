import { useCallback, useEffect, useState } from 'react';
import type { AgentSession } from '../../../types/domain';
import { apiFetch, errorMessage } from '../../../lib/api';
import { useApproval } from '../../../state/approvalStore';
import { GitPanel, type RepoStatus } from './GitPanel';

// The Git tab — the complete-control home for the change: the control ladder
// first (branch → commit → push → PR), then where it sits in history (commit
// graph, with HEAD / origin / the pending uncommitted node). Status + log are
// fetched once here and shown together after a single loading state.

interface GitCommit {
  sha: string;
  shortSha: string;
  subject: string;
  relTime: string;
  refs: string[];
}

/** Ref decorations → head/origin/branch chips */
function CommitRefs({ refs }: { refs: string[] }) {
  return (
    <>
      {refs.map((raw) => {
        const isHead = raw.includes('HEAD');
        const isOrigin = raw.includes('origin/');
        const cls = isHead ? 'head' : isOrigin ? 'origin' : 'branch';
        return (
          <span key={raw} className={`gg-ref ${cls}`}>
            {raw.replace('HEAD ->', 'HEAD →')}
          </span>
        );
      })}
    </>
  );
}

export function GitTab({
  session,
  approvedPaths,
  onChanged,
}: {
  session: AgentSession;
  approvedPaths: string[];
  onChanged: () => void;
}) {
  const [status, setStatus] = useState<RepoStatus | null>(null);
  const [commits, setCommits] = useState<GitCommit[]>([]);
  const [logError, setLogError] = useState<string | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const { pending: saving, saveError } = useApproval(session.sessionId, session.reviewRevision, session.contentHash);

  const load = useCallback(() => {
    Promise.allSettled([
      apiFetch<RepoStatus>('/api/git/status', { method: 'POST', body: { sessionId: session.sessionId } }),
      apiFetch<{ commits?: GitCommit[] }>('/api/git/log', { method: 'POST', body: { sessionId: session.sessionId } }),
    ]).then(([s, l]) => {
      setStatus(s.status === 'fulfilled' ? s.value : null);
      setStatusError(s.status === 'rejected' ? errorMessage(s.reason) : null);
      if (l.status === 'fulfilled') {
        setCommits(l.value.commits ?? []);
        setLogError(null);
      } else {
        setCommits([]);
        setLogError(errorMessage(l.reason));
      }
      setLoaded(true);
    });
  }, [session.sessionId]);

  useEffect(() => {
    setLoaded(false);
    load();
  }, [load]);

  // After a git action, refresh both this panel and the session list
  const reload = useCallback(() => {
    onChanged();
    load();
  }, [onChanged, load]);

  if (!loaded) {
    return (
      <div className="git-loading">
        <span className="scan-spinner sm" aria-hidden="true" />
        <span>Reading git state…</span>
      </div>
    );
  }

  const treeFiles = session.files.filter((f) => f.diffSource === 'git');
  const pending = {
    count: treeFiles.length,
    added: treeFiles.reduce((s, f) => s + f.linesAdded, 0),
    removed: treeFiles.reduce((s, f) => s + f.linesRemoved, 0),
  };

  return (
    <div className="git-tab">
      {/* 1 · Control — the form first */}
      <div className="gg-sect">
        <div className="gg-label">Control</div>
        {statusError && (
          <div role="alert" style={{ fontSize: 'var(--fs-xs)', color: 'var(--err)', marginBottom: 'var(--sp-2)' }}>
            Could not read git status: {statusError}
          </div>
        )}
        <GitPanel session={session} approvedPaths={saving || saveError ? [] : approvedPaths} status={status} onChanged={reload} />
      </div>

      {/* 2 · Where it sits in history */}
      {status?.gitRepo && (
        <div className="gg-sect">
          <div className="gg-label">Commit graph</div>
          {logError ? (
            <div className="faint" style={{ fontSize: 'var(--fs-xs)' }}>{logError}</div>
          ) : (
            <div className="gg-graph">
              {pending.count > 0 && (
                <div className="gg-node pending">
                  <span className="gg-rail"><span className="gg-dot" /></span>
                  <div className="gg-body">
                    <div className="gg-msg pending">◇ Uncommitted — this session</div>
                    <div className="gg-meta">
                      <span>{pending.count} file{pending.count === 1 ? '' : 's'}</span>
                      <span className="add">+{pending.added}</span>
                      <span className="del">−{pending.removed}</span>
                    </div>
                  </div>
                </div>
              )}
              {commits.map((c) => {
                const isHead = c.refs.some((r) => r.includes('HEAD'));
                const isOrigin = c.refs.some((r) => r.includes('origin/'));
                return (
                  <div key={c.sha} className={`gg-node${isHead ? ' head' : ''}${isOrigin ? ' origin' : ''}`}>
                    <span className="gg-rail"><span className="gg-dot" /></span>
                    <div className="gg-body">
                      <div className="gg-msg">{c.subject}</div>
                      <div className="gg-meta">
                        <CommitRefs refs={c.refs} />
                        <span className="gg-sha mono">{c.shortSha}</span>
                        <span>{c.relTime}</span>
                      </div>
                    </div>
                  </div>
                );
              })}
              {commits.length === 0 && (
                <div className="faint" style={{ fontSize: 'var(--fs-xs)' }}>No commits yet on this branch.</div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
