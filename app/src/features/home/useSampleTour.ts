import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { apiFetch, errorMessage } from '../../lib/api';
import { useSessions } from '../../state/sessionsStore';

// Installs (or resets) the sample tour and opens the review of its sandbox
// project. Shared by the first-run welcome panel and Settings → Data.
export function useSampleTour() {
  const { refresh } = useSessions();
  const navigate = useNavigate();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const install = () => {
    setBusy(true);
    setError(null);
    apiFetch<{ ok?: boolean; project?: string }>('/api/demo/install', { method: 'POST', body: {} })
      .then((r) => {
        if (!r.project) {
          setError('Could not load the sample.');
          return;
        }
        refresh();
        navigate(`/project?path=${encodeURIComponent(r.project)}`);
      })
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setBusy(false));
  };

  return { install, busy, error };
}
