import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { AgentSession } from '../types/domain';
import { apiFetch, errorMessage } from '../lib/api';

// Live sessions ingested from real agent transcripts, served in-process by
// the native Rust core (core/api.rs) via apiFetch.
//
// The list (`sessions`) holds SUMMARIES — no hunks, no timeline. The full
// payload of the open session is fetched per id (`fetchSessionDetail`) and
// cached in `details`; it refreshes when the server version moves. A refresh
// that brings the same reviewRevision + contentHash keeps the cached object,
// so the open review does not re-render (and re-highlight) every poll.

export interface SourceStatus {
  id: string;
  label: string;
  enabled: boolean;
  source: string;
  sessions: number;
  hint?: string;
}

interface SessionsState {
  sessions: AgentSession[];
  loading: boolean;
  liveError: string | null;
  /** Session-history persistence failures reported by the scanner */
  storageError: string | null;
  /** Blocking: the session could not be loaded and nothing is cached */
  detailErrors: Record<string, string>;
  /** Non-blocking: the last refresh failed; the cached detail is shown */
  detailStale: Record<string, string>;
  sources: SourceStatus[];
  configPath: string | null;
  /** Manual rescan (the "Rescan sources now" button) — forces a server scan */
  refresh: () => void;
  getSession: (id: string) => AgentSession | undefined;
  /** Full payload of one session (undefined until fetched) */
  getSessionDetail: (id: string) => AgentSession | undefined;
  /** Fetch/refresh the full payload of one session */
  fetchSessionDetail: (id: string) => void;
}

const SessionsContext = createContext<SessionsState | null>(null);

export function SessionsProvider({ children }: { children: ReactNode }) {
  const [sessions, setSessions] = useState<AgentSession[]>([]);
  const [loading, setLoading] = useState(true);
  const [liveError, setLiveError] = useState<string | null>(null);
  const [storageError, setStorageError] = useState<string | null>(null);
  const [sources, setSources] = useState<SourceStatus[]>([]);
  const [configPath, setConfigPath] = useState<string | null>(null);
  const [details, setDetails] = useState<Record<string, AgentSession>>({});
  const [detailErrors, setDetailErrors] = useState<Record<string, string>>({});
  const [detailStale, setDetailStale] = useState<Record<string, string>>({});
  const detailsRef = useRef(details);
  detailsRef.current = details;
  const requests = useRef<Record<string, number>>({});
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const doFetch = useCallback((silent: boolean, forceRescan = false) => {
    if (!silent) setLoading(true);
    apiFetch<{
      configPath?: string;
      sources?: SourceStatus[];
      sessions?: AgentSession[];
      storageError?: string;
    }>(forceRescan ? '/api/sessions?refresh=1' : '/api/sessions')
      .then((data) => {
        if (!mounted.current) return;
        setSessions(Array.isArray(data.sessions) ? data.sessions : []);
        setSources(Array.isArray(data.sources) ? data.sources : []);
        setConfigPath(data.configPath ?? null);
        setLiveError(null);
        setStorageError(data.storageError ?? null);
      })
      .catch((err) => {
        if (!mounted.current) return;
        // A failed background refresh keeps the list; say so all the same
        if (!silent) setSessions([]);
        setLiveError(errorMessage(err));
      })
      .finally(() => {
        if (!silent && mounted.current) setLoading(false);
      });
  }, []);

  const refresh = useCallback(() => doFetch(false, true), [doFetch]);

  const fetchSessionDetail = useCallback((id: string) => {
    const request = (requests.current[id] ?? 0) + 1;
    requests.current[id] = request;
    apiFetch<{ session?: AgentSession }>(`/api/sessions/${encodeURIComponent(id)}`)
      .then((data) => {
        if (!mounted.current || requests.current[id] !== request) return;
        if (!data.session) throw new Error('Session unavailable');
        const session = data.session;
        setDetails((prev) => {
          const cached = prev[id];
          if (cached && cached.reviewRevision === session.reviewRevision && cached.contentHash === session.contentHash) {
            return prev;
          }
          return { ...prev, [id]: session };
        });
        setDetailErrors((prev) => (prev[id] ? { ...prev, [id]: '' } : prev));
        setDetailStale((prev) => (prev[id] ? { ...prev, [id]: '' } : prev));
      })
      .catch((e: unknown) => {
        if (!mounted.current || requests.current[id] !== request) return;
        const message = errorMessage(e);
        // Keep reviewing the cached detail; only block when there is none
        if (detailsRef.current[id]) setDetailStale((prev) => ({ ...prev, [id]: message }));
        else setDetailErrors((prev) => ({ ...prev, [id]: message }));
      });
  }, []);

  useEffect(() => {
    doFetch(false);
    // Auto-rescan: the scanner watches the adapter sources in the background
    // and bumps the version counter ONLY when a scan actually changed the
    // outcome. Polling that tiny endpoint every 5s keeps new sessions
    // appearing within seconds while list fetches happen only on change.
    let lastVersion = 0;
    const versionTimer = setInterval(() => {
      apiFetch<{ version?: number }>('/api/sessions/version')
        .then((data) => {
          if (typeof data.version !== 'number') return;
          if (lastVersion !== 0 && data.version !== lastVersion) doFetch(true);
          lastVersion = data.version;
        })
        // Heartbeat only: a missed tick retries in 5s, and real list/detail
        // failures surface through liveError / detailStale
        .catch(() => undefined);
    }, 5_000);
    return () => {
      clearInterval(versionTimer);
    };
  }, [doFetch]);

  const value = useMemo<SessionsState>(
    () => ({
      sessions,
      loading,
      liveError,
      storageError,
      detailErrors,
      detailStale,
      sources,
      configPath,
      refresh,
      getSession: (id) => sessions.find((s) => s.sessionId === id),
      getSessionDetail: (id) => details[id],
      fetchSessionDetail,
    }),
    [sessions, loading, liveError, storageError, detailErrors, detailStale, sources, configPath, refresh, details, fetchSessionDetail],
  );

  return <SessionsContext.Provider value={value}>{children}</SessionsContext.Provider>;
}

export function useSessions(): SessionsState {
  const ctx = useContext(SessionsContext);
  if (!ctx) throw new Error('useSessions must be used within SessionsProvider');
  return ctx;
}
