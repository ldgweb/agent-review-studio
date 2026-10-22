import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { apiFetch, errorMessage } from '../../lib/api';
import { useSessions } from '../../state/sessionsStore';
import { useRehydrateApprovals } from '../../state/approvalStore';

// Installs (or resets) the sample tour and opens its fresh session. Shared by
// the first-run welcome panel and Settings → Data.
export function useSampleTour() {
  const { refresh } = useSessions();
  const navigate = useNavigate();
  const rehydrateApprovals = useRehydrateApprovals();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const install = () => {
    setBusy(true);
    setError(null);
    apiFetch<{ ok?: boolean; sessionId?: string }>('/api/demo/install', { method: 'POST', body: {} })
      .then(async (r) => {
        if (!r.sessionId) {
          setError('Could not load the sample.');
          return;
        }
        // The tour seeds a finished review server-side — pull it into the
        // client store so the reviewed sample opens in its Apply stage. A
        // failure is recorded on the store and shown in the workbench.
        await rehydrateApprovals();
        refresh();
        navigate(`/sessions/${encodeURIComponent(r.sessionId)}/review`);
      })
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setBusy(false));
  };

  return { install, busy, error };
}
