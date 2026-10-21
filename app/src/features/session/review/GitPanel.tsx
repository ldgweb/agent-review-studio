import { useState } from 'react';
import type { AgentSession } from '../../../types/domain';
import { apiFetch, errorMessage } from '../../../lib/api';
import { openExternal } from '../../../lib/desktop';
import { IconGitBranch, IconCheck, IconAlert } from '../../../components/icons';

// The control ladder for a reviewed change: commit the approved files,
// optionally on a dedicated branch, push (or keep local), open a PR. Branch
// state and the change-state summary are shown for context. Git status is
// owned by the parent (GitTab) and passed in, so it's fetched once. Backend:
// core/git.rs.

export interface RepoStatus {
  gitRepo: boolean;
  branch: string;
  upstream: string | null;
  ahead: number;
  behind: number;
  hasRemote: boolean;
  remoteUrl: string | null;
  dirtyPaths: string[];
}

type Notice = { tone: 'ok' | 'err'; text: string } | null;

export function GitPanel({
  session,
  approvedPaths,
  status,
  onChanged,
}: {
  session: AgentSession;
  approvedPaths: string[];
  status: RepoStatus | null;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice>(null);
  const [msg, setMsg] = useState(
    `Approve agent session: ${session.declaredGoal}\n\nReviewed-with: Agent Review Studio (${session.sessionId})`,
  );
  const [branchName, setBranchName] = useState('');

  const run = <T,>(path: string, body: object, onOk: (r: T) => string): void => {
    setBusy(true);
    setNotice(null);
    apiFetch<T>(path, { method: 'POST', body: { sessionId: session.sessionId, reviewRevision: session.reviewRevision, ...body } })
      .then((r) => {
        setNotice({ tone: 'ok', text: onOk(r) });
        onChanged();
      })
      .catch((e: unknown) => setNotice({ tone: 'err', text: errorMessage(e) }))
      .finally(() => setBusy(false));
  };

  // Commit ONLY the approved paths that still differ from HEAD: paths whose
  // change is already committed (or reverted) are left alone, so approving an
  // old session can never sweep unrelated later work into the commit beyond
  // what the diff on screen shows.
  const doCommit = () =>
    run<{ sha?: string; committed?: number; warning?: string }>(
      '/api/git/commit',
      { paths: approvedDirty, message: msg },
      (r) => `Committed ${r.committed} file${(r.committed ?? 0) > 1 ? 's' : ''} as ${r.sha}.${r.warning ? ` ${r.warning}` : ''}`,
    );

  const doBranch = () =>
    run<{ branch?: string }>('/api/git/branch', { name: branchName }, (r) => {
      setBranchName('');
      return `Switched to a new branch: ${r.branch}. Commit lands here, not on the previous branch.`;
    });

  const doPush = (setUpstream: boolean) =>
    run<{ pushed?: string }>('/api/git/push', { setUpstream }, (r) => `Pushed ${r.pushed} to origin.`);

  const doPr = () => {
    setBusy(true);
    setNotice(null);
    apiFetch<{ url?: string }>('/api/git/pr', {
      method: 'POST',
      body: { sessionId: session.sessionId, title: `Review: ${session.declaredGoal}`, body: '' },
    })
      .then(async (r) => {
        if (!r.url) {
          setNotice({ tone: 'err', text: 'No pull-request URL was returned.' });
          return;
        }
        await openExternal(r.url);
        setNotice({ tone: 'ok', text: 'Opened the pull-request page in your browser.' });
      })
      .catch((e: unknown) => setNotice({ tone: 'err', text: errorMessage(e) }))
      .finally(() => setBusy(false));
  };

  const suggestedBranch = `ars/review-${session.sessionId.slice(0, 8).replace(/[^A-Za-z0-9._/-]/g, '-')}`;

  if (status && !status.gitRepo) {
    return (
      <div className="faint" style={{ fontSize: 'var(--fs-xs)' }}>
        This session's project folder is not a git work tree — commit / push / PR are unavailable.
      </div>
    );
  }

  const dirty = new Set(status?.dirtyPaths ?? []);
  const approvedDirty = approvedPaths.filter((p) => dirty.has(p));
  const approvedClean = approvedPaths.length - approvedDirty.length;
  const onBranch = status?.branch || '…';

  // Change-state summary — what's in the working tree vs already committed vs
  // no longer present (drift). The "missing" count is the signal that matters.
  const inTree = session.files.filter((f) => f.diffSource === 'git').length;
  const committed = session.files.filter((f) => f.drift === 'committed').length;
  const missing = session.files.filter((f) => f.drift === 'missing').length;

  const rowStyle = { display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' as const };

  return (
    <div className="git-panel" style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
      {/* Where you are + what's there / missing */}
      <div style={rowStyle}>
        <span className="badge badge-neutral" title="Current branch">
          <IconGitBranch size={11} /> {onBranch}
        </span>
        {status?.upstream ? (
          <span className="faint" style={{ fontSize: 'var(--fs-xs)' }}>
            ↑{status.ahead} ↓{status.behind} vs {status.upstream}
          </span>
        ) : (
          <span className="faint" style={{ fontSize: 'var(--fs-xs)' }}>
            no upstream set
          </span>
        )}
        {status && !status.hasRemote && (
          <span className="faint" style={{ fontSize: 'var(--fs-xs)' }}>
            · no “origin” remote
          </span>
        )}
      </div>

      <div className="gg-summary">
        <span className="tnum">{session.files.length} files</span>
        <span className="tnum">· {inTree} in working tree</span>
        {committed > 0 && <span className="tnum">· {committed} committed</span>}
        {missing > 0 && <span className="tnum gg-warn">· {missing} missing</span>}
      </div>

      {/* Verify the tree still matches the review */}
      {approvedPaths.length > 0 && status && (
        <div className="faint" style={{ fontSize: 'var(--fs-xs)' }}>
          {approvedDirty.length} of {approvedPaths.length} approved file{approvedPaths.length > 1 ? 's' : ''} still differ from HEAD.
          {approvedClean > 0 && (
            <span style={{ color: 'var(--warn)' }}>
              {' '}
              <IconAlert size={11} /> {approvedClean} no longer show working-tree changes (already committed, reverted, or edited
              since the review).
            </span>
          )}
        </div>
      )}

      {/* Optional dedicated branch */}
      <div style={rowStyle}>
        <input
          className="settings-input mono"
          style={{ flex: 1, minWidth: 160 }}
          placeholder={suggestedBranch}
          value={branchName}
          onChange={(e) => setBranchName(e.target.value)}
        />
        <button className="btn btn-sm" disabled={busy} onClick={() => setBranchName(suggestedBranch)}>
          suggest
        </button>
        <button
          className="btn btn-sm"
          disabled={busy || !branchName.trim()}
          title={`git checkout -b — commit on a dedicated branch instead of ${onBranch}`}
          onClick={doBranch}
        >
          <IconGitBranch size={12} /> New branch
        </button>
      </div>

      {/* Commit the approved files */}
      <textarea className="textarea" rows={3} value={msg} onChange={(e) => setMsg(e.target.value)} />
      <div className="faint" style={{ fontSize: 'var(--fs-xs)' }}>
        git add + git commit of the {approvedPaths.length} approved path{approvedPaths.length > 1 ? 's' : ''} only, in{' '}
        {session.projectPath}. Stays local until you push.
      </div>
      <div style={{ ...rowStyle, justifyContent: 'flex-end' }}>
        <button
          className="btn btn-sm btn-primary"
          disabled={busy || !msg.trim() || approvedDirty.length === 0}
          title={
            approvedPaths.length === 0
              ? 'Approve at least one file first'
              : approvedDirty.length === 0
                ? 'Nothing left to commit — the approved files show no working-tree changes (already committed or reverted)'
                : `git add + git commit of ${approvedDirty.length} approved path${approvedDirty.length > 1 ? 's' : ''}`
          }
          onClick={doCommit}
        >
          {busy ? 'Working…' : <><IconCheck size={12} /> Commit {approvedDirty.length} file{approvedDirty.length === 1 ? '' : 's'}</>}
        </button>
      </div>

      {/* Push or keep local, then open a PR */}
      <div style={{ ...rowStyle, justifyContent: 'flex-end', borderTop: '1px solid var(--border-1)', paddingTop: 'var(--sp-3)' }}>
        <span className="faint" style={{ fontSize: 'var(--fs-xs)', marginRight: 'auto' }}>
          Then: push to origin, or keep it local.
        </span>
        <button
          className="btn btn-sm"
          disabled={busy || !status?.hasRemote}
          title={status?.hasRemote ? `git push origin ${onBranch}` : 'No “origin” remote configured'}
          onClick={() => doPush(!status?.upstream)}
        >
          <IconGitBranch size={12} /> {status && !status.upstream ? 'Push & set upstream' : `Push to origin/${onBranch}`}
        </button>
        <button
          className="btn btn-sm"
          disabled={busy || !status?.hasRemote}
          title="Open a pre-filled GitHub pull-request page for this branch (push first)"
          onClick={doPr}
        >
          Open PR ↗
        </button>
      </div>

      {notice && (
        <div
          className="mono"
          style={{ fontSize: 'var(--fs-xs)', color: notice.tone === 'err' ? 'var(--err)' : 'var(--ok)' }}
        >
          {notice.tone === 'err' ? '✗ ' : '✓ '}
          {notice.text}
        </div>
      )}
    </div>
  );
}
