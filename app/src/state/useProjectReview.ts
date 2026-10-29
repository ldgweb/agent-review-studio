import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { apiFetch, errorMessage, isDesktop } from '../lib/api';
import type { AgentPackage, ProjectReview, ReviewState } from '../types/review';

// One project's review, live. The core sends an event when the project's
// files or any transcript change; the review reloads then, and when the
// window regains focus, never on a timer. Decisions show on screen at once
// and are saved in order behind them. The stored state wins only when no
// local edit is still on its way, or when another writer saved first (a
// conflict says so).

export interface ActionResult {
  ok: boolean;
  error?: string;
}

interface ServerState {
  state?: ReviewState;
  stateRev?: number;
  error?: string;
  conflict?: boolean;
  ok?: boolean;
  /** The review for the agent, rewritten from the state just saved */
  package?: AgentPackage | null;
}

export function useProjectReview(path: string) {
  const [review, setReview] = useState<ProjectReview | null>(null);
  const [state, setState] = useState<ReviewState | null>(null);
  // The core writes the review for the agent (the same text `ars feedback`
  // prints); it arrives with the review and with every save.
  const [pkg, setPkg] = useState<AgentPackage | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [saveError, setSaveError] = useState('');
  const [saving, setSaving] = useState(false);
  const stateRef = useRef<ReviewState | null>(null);
  const rev = useRef(0);
  const root = useRef(path);
  const pending = useRef(0);
  const queue = useRef<Promise<void>>(Promise.resolve());
  const inflight = useRef(false);
  const again = useRef(false);

  const commit = (next: ReviewState) => {
    stateRef.current = next;
    setState(next);
  };

  const load = useCallback(async () => {
    if (inflight.current) {
      again.current = true;
      return;
    }
    inflight.current = true;
    try {
      const r = await apiFetch<ProjectReview>(`/api/review?project=${encodeURIComponent(path)}`);
      root.current = r.project.root;
      // Same content: keep the object, so nothing re-renders (or re-highlights).
      setReview((prev) => (prev && prev.fingerprint === r.fingerprint && prev.stateRev === r.stateRev ? prev : r));
      if (pending.current === 0 && (r.stateRev !== rev.current || !stateRef.current)) {
        rev.current = r.stateRev;
        commit(r.state);
      }
      if (pending.current === 0) setPkg(r.package);
      setError('');
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      inflight.current = false;
      setLoading(false);
      if (again.current) {
        again.current = false;
        void load();
      }
    }
  }, [path]);

  useEffect(() => {
    setReview(null);
    stateRef.current = null;
    setState(null);
    rev.current = 0;
    setLoading(true);
    void load();
    if (!isDesktop()) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const schedule = () => {
      clearTimeout(timer);
      timer = setTimeout(() => void load(), 200);
    };
    const unlisten = [listen('ars://review-changed', schedule), listen('ars://sessions-changed', schedule)];
    window.addEventListener('focus', schedule);
    return () => {
      clearTimeout(timer);
      unlisten.forEach((p) => void p.then((off) => off()));
      window.removeEventListener('focus', schedule);
    };
  }, [load]);

  /** Apply a change to the review state now, save it behind the scenes. */
  const update = useCallback((fn: (s: ReviewState) => ReviewState) => {
    const current = stateRef.current;
    if (!current) return;
    commit(fn(current));
    pending.current++;
    setSaving(true);
    queue.current = queue.current.then(async () => {
      try {
        const res = await apiFetch<ServerState>('/api/review/state', {
          method: 'POST',
          body: { project: root.current, rev: rev.current, state: stateRef.current },
          allowError: true,
        });
        if (res.conflict && res.state && res.stateRev !== undefined) {
          rev.current = res.stateRev;
          commit(res.state);
          setSaveError(res.error ?? 'The review changed elsewhere; the latest version is loaded.');
        } else if (res.error || res.stateRev === undefined) {
          setSaveError(res.error ?? 'The review could not be saved.');
        } else {
          rev.current = res.stateRev;
          if (res.package) setPkg(res.package);
          setSaveError('');
        }
      } catch (e) {
        setSaveError(errorMessage(e));
      } finally {
        pending.current--;
        if (pending.current === 0) setSaving(false);
      }
    });
  }, []);

  const act = useCallback(
    async (route: string, body: Record<string, unknown>): Promise<ActionResult> => {
      await queue.current; // saved decisions first: the core re-checks them
      try {
        const res = await apiFetch<ServerState>(route, {
          method: 'POST',
          body: { project: root.current, rev: rev.current, ...body },
          allowError: true,
        });
        if (res.state && res.stateRev !== undefined) {
          rev.current = res.stateRev;
          commit(res.state);
        }
        if (res.package) setPkg(res.package);
        void load();
        return res.error ? { ok: false, error: res.error } : { ok: true };
      } catch (e) {
        return { ok: false, error: errorMessage(e) };
      }
    },
    [load],
  );

  /** The review was copied: record it as a round, with a snapshot of every
   * changed file, so the next review shows what changes from here. Nothing
   * in the project is touched. */
  const handback = useCallback((text: string, items: number) => act('/api/review/handback', { text, items }), [act]);

  /** A file's lines as they are now, or as HEAD has them (base). Throws
   * with the reason when the file cannot be read. */
  const readFile = useCallback(async (filePath: string, base = false): Promise<string[]> => {
    const res = await apiFetch<{ lines?: string[] }>('/api/review/file', {
      method: 'POST',
      body: { project: root.current, path: filePath, base },
    });
    return res.lines ?? [];
  }, []);
  /** The same, null when it cannot be read: for colors and context, which
   * simply do without */
  const fileLines = useCallback(
    (filePath: string, base = false): Promise<string[] | null> => readFile(filePath, base).catch(() => null),
    [readFile],
  );

  /** Show one of the review's files in the system's file manager */
  const reveal = useCallback(async (filePath: string): Promise<ActionResult> => {
    try {
      await apiFetch('/api/review/reveal', { method: 'POST', body: { project: root.current, path: filePath } });
      return { ok: true };
    } catch (e) {
      return { ok: false, error: errorMessage(e) };
    }
  }, []);

  return useMemo(
    () => ({ review, state, pkg, loading, error, saveError, saving, refresh: load, update, handback, readFile, fileLines, reveal }),
    [review, state, pkg, loading, error, saveError, saving, load, update, handback, readFile, fileLines, reveal],
  );
}
