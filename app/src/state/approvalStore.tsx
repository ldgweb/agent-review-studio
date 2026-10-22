import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type {
  ApprovalState,
  FileApproval,
  FindingReviewState,
  LineComment,
  RiskDimension,
  SessionDecision,
} from '../types/domain';
import { apiFetch, errorMessage } from '../lib/api';

import { emptyState, withSession, reducer, type Store, type Action } from '../lib/approvals';
export { effectiveStatus, computeDecision, DRAFT_NOTE_KEY } from '../lib/approvals';
const STORAGE_KEY = 'agent-review-studio:approvals';

function loadInitial(): Store {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? (JSON.parse(raw) as Store) : {};
  } catch {
    return {};
  }
}

/** Result of one persisted write. Never rejects — failures are recorded on
 * the session (see `saveError`) and returned here for callers that wait. */
export type SaveOutcome = { ok: true } | { ok: false; error: string };

interface SyncState {
  /** Writes for this session still in flight */
  pending: number;
  /** Last failed write for this session; cleared only by its own successful save */
  error: string;
}

const NO_SYNC: SyncState = { pending: 0, error: '' };

interface StoreValue {
  store: Store;
  /** Server-confirmed approvals (what commit / revert are checked against) */
  saved: Store;
  sync: Record<string, SyncState>;
  /** Store-wide problems: local cache, initial load, rehydrate */
  error: string;
  dispatch: (a: UserAction) => Promise<SaveOutcome>;
  resync: (sessionId: string) => Promise<SaveOutcome>;
  rehydrate: () => Promise<SaveOutcome>;
}

/** Actions callers dispatch; 'hydrate' is applied internally on load only */
type UserAction = Exclude<Action, { type: 'hydrate' }>;

const StoreContext = createContext<StoreValue | null>(null);

export function ApprovalProvider({ children }: { children: ReactNode }) {
  const [store, setStore] = useState<Store>(loadInitial);
  const storeRef = useRef(store);
  const [saved, setSaved] = useState<Store>({});
  const [sync, setSync] = useState<Record<string, SyncState>>({});
  const syncRef = useRef(sync);
  syncRef.current = sync;
  const [cacheError, setCacheError] = useState('');
  const [loadError, setLoadError] = useState('');
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  /** Sessions edited locally since mount — the initial load must not overwrite them */
  const editedBeforeLoad = useRef(new Set<string>());
  const loaded = useRef(false);
  const resetBeforeLoad = useRef(false);
  /** A write response already carried newer server state than the initial GET */
  const savedFromWrite = useRef(false);

  const commit = (next: Store) => {
    storeRef.current = next;
    setStore(next);
  };

  useEffect(() => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(store));
      setCacheError('');
    } catch {
      setCacheError('Could not save the local review cache.');
    }
  }, [store]);

  const patchSync = (sessionId: string, patch: (s: SyncState) => SyncState) =>
    setSync((prev) => ({ ...prev, [sessionId]: patch(prev[sessionId] ?? NO_SYNC) }));

  // Initial load. Server state wins over the localStorage cache, except for
  // sessions the reviewer already edited while the request was in flight.
  useEffect(() => {
    apiFetch<{ approvals?: Store }>('/api/approvals')
      .then((data) => {
        const server = data.approvals ?? {};
        loaded.current = true;
        if (!savedFromWrite.current) setSaved(server);
        if (resetBeforeLoad.current) return;
        const merge: Store = {};
        for (const [id, state] of Object.entries(server)) {
          if (!editedBeforeLoad.current.has(id)) merge[id] = state;
        }
        commit(reducer(storeRef.current, { type: 'hydrate', approvals: merge }));
        setLoadError('');
      })
      .catch((e: unknown) => {
        loaded.current = true;
        setLoadError(`Could not load saved reviews: ${errorMessage(e)}`);
      });
  }, []);

  // Serialize writes so a slow earlier request cannot replace a newer decision.
  const post = useCallback((sessionId: string | null, body: unknown): Promise<SaveOutcome> => {
    if (sessionId) patchSync(sessionId, (s) => ({ ...s, pending: s.pending + 1 }));
    const run = queue.current.then(async (): Promise<SaveOutcome> => {
      try {
        const response = await apiFetch<{ approvals?: Store }>('/api/approvals', { method: 'POST', body });
        savedFromWrite.current = true;
        if (response.approvals) setSaved(response.approvals);
        if (sessionId) patchSync(sessionId, (s) => ({ ...s, pending: s.pending - 1, error: '' }));
        else setSync({});
        return { ok: true };
      } catch (e) {
        const error = errorMessage(e);
        if (sessionId) patchSync(sessionId, (s) => ({ ...s, pending: s.pending - 1, error }));
        return { ok: false, error };
      }
    });
    queue.current = run;
    return run;
  }, []);

  /** Pull the server's approval state into the client store NOW — needed when
   * the server seeds approvals after mount (the sample tour). Server wins,
   * except over sessions whose local edits are still unsaved. */
  const rehydrate = useCallback(
    () =>
      apiFetch<{ approvals?: Store }>('/api/approvals')
        .then((data): SaveOutcome => {
          const server = data.approvals ?? {};
          setSaved(server);
          const merge: Store = {};
          for (const [id, state] of Object.entries(server)) {
            const s = syncRef.current[id];
            if (!s || (s.pending === 0 && !s.error)) merge[id] = state;
          }
          commit(reducer(storeRef.current, { type: 'hydrate', approvals: merge }));
          setLoadError('');
          return { ok: true };
        })
        .catch((e: unknown): SaveOutcome => {
          const error = `Could not load saved reviews: ${errorMessage(e)}`;
          setLoadError(error);
          return { ok: false, error };
        }),
    [],
  );

  const dispatch = useCallback(
    (action: UserAction): Promise<SaveOutcome> => {
      if (action.type === 'reset-all') {
        if (!loaded.current) resetBeforeLoad.current = true;
        commit(reducer(storeRef.current, action));
        // A failed clear must not leave the screen pretending it worked
        return post(null, { clearAll: true }).then((r) => {
          if (!r.ok) void rehydrate();
          return r;
        });
      }
      if (!action.reviewRevision) {
        const error = 'Reload this session before making review decisions.';
        patchSync(action.sessionId, (s) => ({ ...s, error }));
        return Promise.resolve({ ok: false, error });
      }
      if (!loaded.current) editedBeforeLoad.current.add(action.sessionId);
      const next = reducer(storeRef.current, action);
      commit(next);
      return post(action.sessionId, { sessionId: action.sessionId, state: next[action.sessionId] });
    },
    [post, rehydrate],
  );

  /** Re-send the session's current local state (retry after a failed save) */
  const resync = useCallback(
    (sessionId: string): Promise<SaveOutcome> => {
      const state = storeRef.current[sessionId];
      if (!state?.reviewRevision) {
        return Promise.resolve({ ok: false, error: 'Nothing to save for this session yet.' });
      }
      return post(sessionId, { sessionId, state });
    },
    [post],
  );

  const error = loadError || cacheError;
  const value = useMemo<StoreValue>(
    () => ({ store, saved, sync, error, dispatch, resync, rehydrate }),
    [store, saved, sync, error, dispatch, resync, rehydrate],
  );

  return <StoreContext.Provider value={value}>{children}</StoreContext.Provider>;
}

function useStore(hook: string): StoreValue {
  const ctx = useContext(StoreContext);
  if (!ctx) throw new Error(`${hook} must be used within ApprovalProvider`);
  return ctx;
}

/** The review state of one session. With `reviewRevision`, decisions saved
 * for another revision do not apply; without it (inbox summaries), a stored
 * review applies only to the transcript it was saved for (`contentHash`). */
export function useApproval(sessionId: string, reviewRevision?: string, contentHash?: string) {
  const ctx = useStore('useApproval');
  const previous = withSession(ctx.store, sessionId);
  const applies = (s: ApprovalState): boolean =>
    reviewRevision
      ? s.reviewRevision === reviewRevision
      : contentHash
        ? Boolean(s.reviewRevision) && s.contentHash === contentHash
        : true;
  const stale = Boolean(reviewRevision && previous.reviewRevision !== reviewRevision && Object.keys(previous.fileStates).length);
  const state = applies(previous) ? previous : emptyState(sessionId);
  const savedPrevious = withSession(ctx.saved, sessionId);
  const saved = reviewRevision && applies(savedPrevious) ? savedPrevious : emptyState(sessionId);
  const sync = ctx.sync[sessionId] ?? NO_SYNC;
  const dispatch = (a: UserAction) => ctx.dispatch({ ...a, reviewRevision, contentHash });
  return {
    state,
    /** Server-confirmed state at this revision (empty when none) */
    saved,
    stale,
    /** This session's failed save, if any — the decisions shown are not saved */
    saveError: sync.error,
    /** Session save error, else a store-wide problem */
    error: sync.error || ctx.error,
    pending: sync.pending,
    retrySave: () => ctx.resync(sessionId),
    setFiles: (fileIds: string[], fileState: FileApproval) =>
      dispatch({ type: 'set-files', sessionId, fileIds, state: fileState }),
    setNote: (key: string, note: string) => dispatch({ type: 'set-note', sessionId, key, note }),
    setLineComment: (fileId: string, comment: LineComment) =>
      dispatch({ type: 'set-line-comment', sessionId, fileId, comment }),
    removeLineComment: (fileId: string, anchor: string) =>
      dispatch({ type: 'remove-line-comment', sessionId, fileId, anchor }),
    setFindingState: (findingId: string, state?: FindingReviewState) =>
      dispatch({ type: 'set-finding-state', sessionId, findingId, state }),
    rejectCategory: (dimension: RiskDimension, fileIds: string[]) =>
      dispatch({ type: 'reject-category', sessionId, dimension, fileIds }),
    clearCategory: (dimension: RiskDimension, fileIds: string[]) =>
      dispatch({ type: 'clear-category', sessionId, dimension, fileIds }),
    finalize: (decision: SessionDecision) => dispatch({ type: 'finalize', sessionId, decision }),
  };
}

/** Whole approval store, for list-level filtering (inbox status facets) */
export function useAllApprovals(): Store {
  return useStore('useAllApprovals').store;
}

/** Store-wide approval problems (cache, load, rehydrate) */
export function useApprovalStoreError(): string {
  return useStore('useApprovalStoreError').error;
}

export function useClearDecisions(): () => Promise<SaveOutcome> {
  const { dispatch } = useStore('useClearDecisions');
  return useCallback(() => dispatch({ type: 'reset-all' }), [dispatch]);
}

export function useRehydrateApprovals(): () => Promise<SaveOutcome> {
  return useStore('useRehydrateApprovals').rehydrate;
}
