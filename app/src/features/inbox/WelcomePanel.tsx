import { Link } from 'react-router-dom';
import { useSessions } from '../../state/sessionsStore';
import { useSampleTour } from './useSampleTour';

// First-run onboarding, shown in the inbox while there is nothing to review:
// where sessions come from, what happens next — and a sample session to try
// the whole review before your own agents show up.

export function WelcomePanel() {
  const { sources, refresh } = useSessions();
  const tour = useSampleTour();

  const active = sources.filter((s) => s.enabled);

  return (
    <div className="card" style={{ maxWidth: 720, padding: 'var(--sp-5)' }}>
      <div style={{ fontSize: 'var(--fs-lg)', fontWeight: 700 }}>Welcome to Agent Review Studio</div>
      <p className="muted mt-2" style={{ maxWidth: '62ch' }}>
        Agent Review Studio reviews the work of your AI coding agents: what they did, why, and where heuristic checks flag risk —
        and turns your verdict into real git commits. Two things to know:
      </p>

      <div className="mt-4">
        <div className="section-label">1 · Sessions appear by themselves</div>
        <p className="muted" style={{ fontSize: 'var(--fs-sm)' }}>
          Run any task with your agents — the sources below are watched continuously.
        </p>
        <div className="chip-row mt-2">
          {active.map((s) => (
            <span key={s.id} className={`badge ${s.sessions > 0 ? 'badge-ok' : 'badge-neutral'}`} title={s.hint ?? s.source}>
              {s.label}: {s.sessions}
            </span>
          ))}
        </div>
        <div className="faint mt-2" style={{ fontSize: 'var(--fs-xs)' }}>
          Custom agents or CI? Drop <span className="mono">ars/v1</span> JSON files into the watched folder — see{' '}
          <Link to="/settings">Settings</Link>.
        </div>
      </div>

      <div className="mt-4">
        <div className="section-label">2 · Review, then let it count</div>
        <p className="muted" style={{ fontSize: 'var(--fs-sm)' }}>
          Open a session → judge the code risk-first with the evidence beside it → approve becomes a{' '}
          <span className="mono">git commit</span>, reject becomes a revert with backup, and corrections can be copied
          or saved for the agent.
        </p>
      </div>

      <div className="mt-4">
        <div className="section-label">Or take the tour right now</div>
        <p className="muted" style={{ fontSize: 'var(--fs-sm)' }}>
          One click installs a sandbox repo with <strong>two sample sessions</strong>: one waiting for your
          verdicts (a planted secret, a leftover TODO, a shrunk test, a file never re-tested) and one{' '}
          <strong>already reviewed</strong> — verdicts, line comments and a corrections draft ready to copy.
          Git actions work for real: commit and the graph in the Git tab, revert from the decision bar. Archive both when you're done.
        </p>
        {tour.error && <div style={{ color: 'var(--err)', fontSize: 'var(--fs-sm)' }} className="mt-2">{tour.error}</div>}
      </div>

      <div className="flex mt-4">
        <button className="btn btn-sm btn-primary" disabled={tour.busy} onClick={tour.install}>
          {tour.busy ? 'Preparing the tour…' : 'Try the sample tour'}
        </button>
        <button className="btn btn-sm" onClick={refresh}>Rescan sources now</button>
      </div>
    </div>
  );
}
