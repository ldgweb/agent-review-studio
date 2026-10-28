import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import { apiFetch, errorMessage, isDesktop } from '../lib/api';

// The status of each session source and of the background scan, for the
// home screen and Settings. The core announces a changed session index with
// an event (ars://sessions-changed); the status reloads then, never on a
// timer.

export interface SourceStatus {
  id: string;
  label: string;
  enabled: boolean;
  source: string;
  sessions: number;
  hint?: string;
}

interface SessionsState {
  loading: boolean;
  liveError: string | null;
  /** Session-history persistence failures reported by the scanner */
  storageError: string | null;
  sources: SourceStatus[];
  /** Bumped whenever the index changed, so lists reload what they derive */
  version: number;
  /** Scan the sources again now */
  refresh: () => void;
}

const SessionsContext = createContext<SessionsState | null>(null);

export function SessionsProvider({ children }: { children: ReactNode }) {
  const [loading, setLoading] = useState(true);
  const [liveError, setLiveError] = useState<string | null>(null);
  const [storageError, setStorageError] = useState<string | null>(null);
  const [sources, setSources] = useState<SourceStatus[]>([]);
  const [version, setVersion] = useState(0);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const doFetch = useCallback((silent: boolean, forceRescan = false) => {
    if (!silent) setLoading(true);
    apiFetch<{ sources?: SourceStatus[]; storageError?: string }>(forceRescan ? '/api/sessions?refresh=1' : '/api/sessions')
      .then((data) => {
        if (!mounted.current) return;
        setSources(Array.isArray(data.sources) ? data.sources : []);
        setLiveError(null);
        setStorageError(data.storageError ?? null);
        setVersion((v) => v + 1);
      })
      .catch((err) => {
        if (mounted.current) setLiveError(errorMessage(err));
      })
      .finally(() => {
        if (!silent && mounted.current) setLoading(false);
      });
  }, []);

  const refresh = useCallback(() => doFetch(false, true), [doFetch]);

  useEffect(() => {
    doFetch(false);
    if (!isDesktop()) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const off = listen('ars://sessions-changed', () => {
      clearTimeout(timer);
      timer = setTimeout(() => doFetch(true), 300);
    });
    return () => {
      clearTimeout(timer);
      void off.then((f) => f());
    };
  }, [doFetch]);

  const value = useMemo<SessionsState>(
    () => ({ loading, liveError, storageError, sources, version, refresh }),
    [loading, liveError, storageError, sources, version, refresh],
  );

  return <SessionsContext.Provider value={value}>{children}</SessionsContext.Provider>;
}

export function useSessions(): SessionsState {
  const ctx = useContext(SessionsContext);
  if (!ctx) throw new Error('useSessions must be used within SessionsProvider');
  return ctx;
}
