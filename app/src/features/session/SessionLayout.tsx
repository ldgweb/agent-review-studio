import { Suspense, useCallback, useEffect, useMemo } from 'react';
import { Outlet, useParams } from 'react-router-dom';
import { useSessions } from '../../state/sessionsStore';
import type { AgentSession } from '../../types/domain';

export interface SessionContext {
  session: AgentSession;
  /** Re-fetch the full payload (after git actions change the working tree) */
  refreshDetail: () => void;
}

// Session shell: the guard, the detail fetch and the outlet. The list holds
// summaries only — the full session (hunks, timeline, git enrichment,
// policy) is fetched per id here. All session chrome lives in the
// route-aware TopBar (App.tsx) — the page below belongs to the code.

export function SessionLayout() {
  const { id } = useParams();
  const { getSession, getSessionDetail, fetchSessionDetail, sessions, loading, detailErrors, detailStale } = useSessions();
  const summary = id ? getSession(id) : undefined;
  const detail = id ? getSessionDetail(id) : undefined;

  // (Re)fetch on open and whenever the list refreshed — a version bump means
  // the underlying transcript or working tree may have moved. The stale
  // detail keeps rendering until the fresh one lands.
  useEffect(() => {
    const refresh = () => { if (id && !document.hidden) void fetchSessionDetail(id); };
    refresh();
    const timer = setInterval(refresh, 5_000);
    window.addEventListener('focus', refresh);
    return () => { clearInterval(timer); window.removeEventListener('focus', refresh); };
  }, [id, sessions, fetchSessionDetail]);

  const session = detail;
  const refreshDetail = useCallback(() => {
    if (id) fetchSessionDetail(id);
  }, [id, fetchSessionDetail]);
  const outletContext = useMemo(
    () => (session ? ({ session, refreshDetail } satisfies SessionContext) : null),
    [session, refreshDetail],
  );
  if (!session && id && detailErrors[id]) return <main className="page"><div className="banner banner-err" role="alert">
    {detailErrors[id]} <button className="btn" onClick={() => fetchSessionDetail(id)}>Reload session</button>
  </div></main>;
  if (!session || !outletContext) {
    const stillLoading = loading || Boolean(summary);
    return (
      <main className="page">
        <div className={`banner ${stillLoading ? 'banner-info' : 'banner-err'}`}>
          {stillLoading ? 'Loading session…' : 'Session not found.'}
        </div>
      </main>
    );
  }

  return (
    <div className="session-shell">
      {id && detailStale[id] && (
        <div className="faint" role="status" title={detailStale[id]} style={{ fontSize: 'var(--fs-xs)', padding: '2px var(--sp-3)' }}>
          Showing the last loaded version — refresh failed: {detailStale[id]}
        </div>
      )}
      <div className="session-content">
        {/* Local boundary: the workbench chunk loads lazily on first open —
            without this, its suspension bubbles to the app-level boundary and
            swaps the whole page for a blank fallback for one frame. */}
        <Suspense
          fallback={
            <main className="page">
              <div className="banner banner-info">Loading session…</div>
            </main>
          }
        >
          <Outlet context={outletContext} />
        </Suspense>
      </div>
    </div>
  );
}
