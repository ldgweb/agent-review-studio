import { Link } from 'react-router-dom';
import { useSessions } from '../../state/sessionsStore';
import { useSampleTour } from './useSampleTour';

// The first-run welcome, on the home screen while no project is listed:
// where sessions come from, what happens next, and a sample to try the
// whole review before your own agents show up.

export function WelcomePanel() {
  const { sources, refresh } = useSessions();
  const tour = useSampleTour();

  const active = sources.filter((s) => s.enabled);

  return (
    <div className="card" style={{ maxWidth: 720, padding: 'var(--sp-5)' }}>
      <div style={{ fontSize: 'var(--fs-lg)', fontWeight: 700 }}>Welcome to Agent Review Studio</div>
      <p className="muted mt-2" style={{ maxWidth: '62ch' }}>
        Agent Review Studio shows what your coding agents changed in a project, before you commit: every change, why the agent
        made it, and a hint where a fixed check sees a risk. Your decisions become one review to paste back into the agent.
        Two things to know:
      </p>

      <div className="mt-4">
        <div className="section-label">1 · Sessions appear by themselves</div>
        <p className="muted" style={{ fontSize: 'var(--fs-sm)' }}>
          Run any task with your agents. ARS watches the sources below.
        </p>
        <div className="chip-row mt-2">
          {active.map((s) => (
            <span key={s.id} className={`badge ${s.sessions > 0 ? 'badge-ok' : 'badge-neutral'}`} title={s.hint ?? s.source}>
              {s.label}: {s.sessions}
            </span>
          ))}
        </div>
        <div className="faint mt-2" style={{ fontSize: 'var(--fs-xs)' }}>
          Custom agents or CI? Drop <span className="mono">ars/v1</span> JSON files into the watched folder. See{' '}
          <Link to="/settings">Settings</Link>.
        </div>
      </div>

      <div className="mt-4">
        <div className="section-label">2 · Review, then copy the review</div>
        <p className="muted" style={{ fontSize: 'var(--fs-sm)' }}>
          Run <span className="mono">ars .</span> in your project, or open it here. You see every uncommitted change with the
          agent's reason next to it. Approve what is fine, comment where it must change, reject the rest, then copy the review
          into the agent. Next time you see only what changed since.
        </p>
      </div>

      <div className="mt-4">
        <div className="section-label">Or take the tour right now</div>
        <p className="muted" style={{ fontSize: 'var(--fs-sm)' }}>
          One click sets up a sample repository with <strong>two sample sessions</strong>: a new feature waiting for your
          decisions (a planted secret, a leftover TODO, a shrunk test) and a fix you <strong>already reviewed once</strong>,
          where "since last review" shows only what the agent changed after your comments. ARS never edits the code: reject
          something and the review tells the agent exactly what to undo.
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
