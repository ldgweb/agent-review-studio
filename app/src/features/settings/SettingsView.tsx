import { useEffect, useState } from 'react';
import { apiFetch, errorMessage } from '../../lib/api';
import { useSessions } from '../../state/sessionsStore';
import type { FindingKind, FindingRule, FindingSeverity } from '../../types/domain';
import { SeverityPill } from '../../components/ui';
import { useSampleTour } from '../home/useSampleTour';

// Sources and review checks have a Save action; data operations act immediately.
type SettingsSection = 'sources' | 'checks' | 'data';

const SECTIONS: Array<{ key: SettingsSection; label: string; desc: string }> = [
  { key: 'sources', label: 'Session sources', desc: 'Where sessions come from' },
  { key: 'checks', label: 'Review checks', desc: 'Built-in and custom hints' },
  { key: 'data', label: 'Data', desc: 'What lives on this machine' },
];

interface AdapterConfig {
  enabled: boolean;
  dir?: string;
  projectDirs?: string[];
  dirs?: string[];
}

interface SettingsPayload {
  configPath: string;
  maxSessions: number;
  adapters: Record<string, AdapterConfig>;
  findingRules?: FindingRule[];
  /** The config file is unreadable or malformed: defaults are shown and the
   * core refuses to save over it */
  configError?: string;
}

const MAX_SESSIONS_MIN = 5;
const MAX_SESSIONS_MAX = 200;

/** Clamp a typed max-sessions value; unparsable input keeps `fallback` */
function clampMaxSessions(raw: string, fallback: number): number {
  const n = Number(raw);
  if (raw.trim() === '' || !Number.isFinite(n)) return fallback;
  return Math.max(MAX_SESSIONS_MIN, Math.min(MAX_SESSIONS_MAX, Math.round(n)));
}

// The built-in checks, always on. Listed so you see every check your
// reviews rest on, not only your own.
const BUILT_IN_CHECKS: Array<{ label: string; desc: string; severity: FindingSeverity }> = [
  { label: 'Hardcoded secret', desc: 'API keys, tokens and passwords appearing in added lines', severity: 'high' },
  { label: 'Leftover debug / TODO', desc: 'console.log/debug, debugger, println!/dbg!, TODO/FIXME/XXX/HACK in added lines', severity: 'low' },
  { label: 'Oversized change', desc: 'A single file gaining more than 400 lines', severity: 'low' },
  { label: 'Shrunk test file', desc: 'A test file that lost more lines than it gained: tests may have been deleted', severity: 'medium' },
];

const SEVERITY_OPTIONS: FindingSeverity[] = ['info', 'low', 'medium', 'high', 'critical'];
const KIND_OPTIONS: Array<{ value: FindingKind; label: string }> = [
  { value: 'issue', label: 'Issue' },
  { value: 'potential-bug', label: 'Potential bug' },
  { value: 'suggestion', label: 'Suggestion' },
  { value: 'question-for-agent', label: 'Question for agent' },
];

type TesterVerdict = { kind: 'match' } | { kind: 'no-match' } | { kind: 'invalid'; error: string };

/** Live rule tester: type a sample and see whether the rule matches, before
 * saving it. The core runs it with the same regex engine as the checks
 * (Rust syntax). */
function RuleTester({ pattern, target }: { pattern: string; target: FindingRule['target'] }) {
  const [sample, setSample] = useState('');
  const [verdict, setVerdict] = useState<TesterVerdict | null>(null);
  const active = Boolean(sample && pattern.trim());

  useEffect(() => {
    setVerdict(null);
    if (!active) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      apiFetch<{ matches?: boolean }>('/api/rules/test', {
        method: 'POST',
        body: { pattern, target: target === 'path' ? 'path' : 'line', sample },
      })
        .then((r) => {
          if (!cancelled) setVerdict({ kind: r.matches ? 'match' : 'no-match' });
        })
        .catch((e: unknown) => {
          if (!cancelled) setVerdict({ kind: 'invalid', error: errorMessage(e) });
        });
    }, 300);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [active, pattern, target, sample]);

  return (
    <div>
      <div className="flex" style={{ gap: 'var(--sp-2)' }}>
        <input
          className="settings-input mono"
          style={{ flex: 1 }}
          aria-label="Sample to test the pattern against"
          placeholder={target === 'path' ? 'Try it: paste a file path…' : 'Try it: paste a line of code…'}
          value={sample}
          onChange={(e) => setSample(e.target.value)}
        />
        <span role="status" style={{ flexShrink: 0, display: 'inline-flex' }}>
          {verdict?.kind === 'match' && <span className="badge badge-ok">✓ would flag</span>}
          {verdict?.kind === 'no-match' && <span className="badge badge-neutral">no match</span>}
          {verdict?.kind === 'invalid' && <span className="badge badge-err">invalid pattern</span>}
        </span>
      </div>
      {verdict?.kind === 'invalid' && (
        <div className="mono mt-2" style={{ color: 'var(--err)', fontSize: 'var(--fs-xs)', whiteSpace: 'pre-wrap' }}>
          {verdict.error}
        </div>
      )}
    </div>
  );
}

/** One custom rule: a header (on/off, name, what it reports, delete), what
 * to match, then the live tester. */
function RuleCard({
  rule,
  onChange,
  onRemove,
}: {
  rule: FindingRule;
  onChange: (patch: Partial<FindingRule>) => void;
  onRemove: () => void;
}) {
  return (
    <div className="card rule-card" style={{ opacity: rule.enabled ? 1 : 0.55 }}>
      <div className="flex" style={{ gap: 'var(--sp-2)', flexWrap: 'wrap' }}>
        <Switch
          label={`Enable check ${rule.label || '(unnamed)'}`}
          checked={rule.enabled}
          onChange={(v) => onChange({ enabled: v })}
        />
        <input
          className="settings-input"
          style={{ flex: '2 1 180px', fontWeight: 600 }}
          aria-label="Check name"
          placeholder="Name this check (e.g. No eval())"
          value={rule.label}
          onChange={(e) => onChange({ label: e.target.value })}
        />
        <select
          className="settings-input"
          style={{ width: 104, flexShrink: 0 }}
          title="Severity of the hint"
          aria-label="Severity of the hint"
          value={rule.severity}
          onChange={(e) => onChange({ severity: e.target.value as FindingSeverity })}
        >
          {SEVERITY_OPTIONS.map((s) => (
            <option key={s} value={s}>{s}</option>
          ))}
        </select>
        <select
          className="settings-input"
          style={{ width: 150, flexShrink: 0 }}
          title="What kind of hint it is"
          aria-label="Kind of hint"
          value={rule.kind}
          onChange={(e) => onChange({ kind: e.target.value as FindingKind })}
        >
          {KIND_OPTIONS.map((k) => (
            <option key={k.value} value={k.value}>{k.label}</option>
          ))}
        </select>
        <button className="btn btn-sm" title="Delete this check" aria-label="Delete this check" onClick={onRemove}>✕</button>
      </div>

      <div className="flex mt-2" style={{ gap: 'var(--sp-2)', flexWrap: 'wrap' }}>
        <select
          className="settings-input"
          style={{ width: 150, flexShrink: 0 }}
          title="What the pattern runs against"
          aria-label="What the pattern runs against"
          value={rule.target}
          onChange={(e) => onChange({ target: e.target.value as FindingRule['target'] })}
        >
          <option value="added-lines">Added lines</option>
          <option value="path">File path</option>
        </select>
        <input
          className="settings-input mono"
          style={{ flex: '2 1 200px' }}
          aria-label="Pattern"
          placeholder={rule.target === 'path' ? String.raw`Pattern, e.g. \.env$` : String.raw`Pattern, e.g. \beval\(`}
          value={rule.pattern}
          onChange={(e) => onChange({ pattern: e.target.value })}
        />
        {rule.target === 'added-lines' && (
          <input
            className="settings-input mono"
            style={{ flex: '1 1 140px' }}
            title="Optional: only files matching this glob"
            aria-label="Only files matching this glob (optional)"
            placeholder="Files: src/**/*.ts (all)"
            value={rule.fileGlob ?? ''}
            onChange={(e) => onChange({ fileGlob: e.target.value })}
          />
        )}
      </div>

      <input
        className="settings-input mt-2"
        style={{ width: '100%' }}
        aria-label="Message shown on the hint"
        placeholder="Message shown on the hint (optional; the matched text otherwise)"
        value={rule.message ?? ''}
        onChange={(e) => onChange({ message: e.target.value })}
      />
      <div className="mt-2">
        <RuleTester pattern={rule.pattern} target={rule.target} />
      </div>
    </div>
  );
}

const ADAPTER_META: Record<string, { label: string; desc: string; dirLabel?: string; listKey?: 'projectDirs' | 'dirs'; listLabel?: string }> = {
  'claude-code': { label: 'Claude Code', desc: 'JSONL transcripts, zero-config', dirLabel: 'Transcripts folder' },
  codex: { label: 'OpenAI Codex', desc: 'CLI rollouts, zero-config', dirLabel: 'Sessions folder' },
  cursor: { label: 'Cursor', desc: 'Composer/Agent chats from Cursor’s local database, zero-config', dirLabel: 'globalStorage folder' },
  copilot: { label: 'VS Code Copilot', desc: 'Copilot Chat sessions from workspaceStorage, zero-config', dirLabel: 'workspaceStorage folder' },
  aider: { label: 'Aider', desc: 'Chat history inside each project', listKey: 'projectDirs', listLabel: 'Project folders, one per line' },
  'gemini-cli': { label: 'Gemini CLI', desc: 'Saved chats and checkpoints (/chat save, --checkpointing)', dirLabel: 'Gemini tmp folder' },
  custom: { label: 'Custom agents', desc: 'Open ars/v1 JSON format, for any agent or CI', listKey: 'dirs', listLabel: 'Watched folders, one per line' },
};

function Switch({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <label className="switch">
      <input type="checkbox" role="switch" aria-label={label} checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span className="track" />
      <span className="thumb" />
    </label>
  );
}

export function SettingsView() {
  const { refresh, sources } = useSessions();
  const tour = useSampleTour();
  const [section, setSection] = useState<SettingsSection>('sources');
  const [settings, setSettings] = useState<SettingsPayload | null>(null);
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState<number | null>(null);
  const [clearState, setClearState] = useState<
    { status: 'idle' | 'clearing' | 'cleared' } | { status: 'failed'; error: string }
  >({ status: 'idle' });
  const [error, setError] = useState<string | null>(null);
  /** Raw max-sessions text while editing; clamped on blur / save */
  const [maxSessionsRaw, setMaxSessionsRaw] = useState<string | null>(null);

  useEffect(() => {
    apiFetch<SettingsPayload>('/api/settings')
      .then(setSettings)
      .catch((err: unknown) => setError(errorMessage(err)));
  }, []);

  const clearDecisions = () => {
    const ok = window.confirm(
      'Clear every review decision on this machine?\n' +
        'Verdicts, comments, notes and the copied reviews of every project are removed. This cannot be undone.',
    );
    if (!ok) return;
    setClearState({ status: 'clearing' });
    apiFetch('/api/review/clear', { method: 'POST', body: {} })
      .then(() => {
        setClearState({ status: 'cleared' });
        setTimeout(() => setClearState((c) => (c.status === 'cleared' ? { status: 'idle' } : c)), 2500);
      })
      .catch((e: unknown) => setClearState({ status: 'failed', error: errorMessage(e) }));
  };

  const commitMaxSessions = () => {
    if (maxSessionsRaw === null) return;
    setSettings((s) => (s ? { ...s, maxSessions: clampMaxSessions(maxSessionsRaw, s.maxSessions) } : s));
    setMaxSessionsRaw(null);
  };

  const dirty = () => setSavedAt(null);

  const updateAdapter = (id: string, patch: Partial<AdapterConfig>) => {
    setSettings((s) => (s ? { ...s, adapters: { ...s.adapters, [id]: { ...s.adapters[id], ...patch } } } : s));
    dirty();
  };

  const rules = settings?.findingRules ?? [];
  const setRules = (next: FindingRule[]) => {
    setSettings((s) => (s ? { ...s, findingRules: next } : s));
    dirty();
  };
  const updateRule = (id: string, patch: Partial<FindingRule>) =>
    setRules(rules.map((r) => (r.id === id ? { ...r, ...patch } : r)));
  const removeRule = (id: string) => setRules(rules.filter((r) => r.id !== id));
  const addRule = () =>
    setRules([
      ...rules,
      {
        id: `custom-${Math.random().toString(36).slice(2, 8)}`,
        enabled: true,
        label: '',
        kind: 'issue',
        severity: 'medium',
        target: 'added-lines',
        fileGlob: '',
        pattern: '',
        message: '',
      },
    ]);

  const save = () => {
    if (!settings) return;
    const maxSessions =
      maxSessionsRaw === null ? settings.maxSessions : clampMaxSessions(maxSessionsRaw, settings.maxSessions);
    setMaxSessionsRaw(null);
    setSettings({ ...settings, maxSessions });
    setSaving(true);
    setError(null);
    apiFetch<SettingsPayload>('/api/settings', {
      method: 'POST',
      body: {
        adapters: settings.adapters,
        maxSessions,
        findingRules: settings.findingRules ?? [],
      },
    })
      .then((data) => {
        setSettings(data);
        setSavedAt(Date.now());
        refresh();
      })
      // The unsaved edits stay on screen next to the reason
      .catch((err: unknown) => setError(`Settings not saved: ${errorMessage(err)}`))
      .finally(() => setSaving(false));
  };

  if (!settings) {
    return (
      <main className="page">
        {error ? <div className="banner banner-err"><span>✗</span><span>{error}</span></div> : <span className="faint">Loading…</span>}
      </main>
    );
  }

  const statusOf = (id: string) => sources.find((s) => s.id === id);
  const savable = section === 'sources' || section === 'checks';

  return (
    <main className="page">
      <div className="settings-layout">
        <nav className="settings-nav" aria-label="Settings sections">
          {SECTIONS.map((s) => (
            <button
              key={s.key}
              className={`settings-nav-item${section === s.key ? ' active' : ''}`}
              onClick={() => setSection(s.key)}
            >
              <span className="settings-nav-label">{s.label}</span>
              <span className="settings-nav-desc">{s.desc}</span>
            </button>
          ))}
        </nav>

        <div className="settings-content">
          {settings.configError && (
            <div className="banner banner-err" role="alert">
              <span>✗</span>
              <span>
                <span className="mono">{settings.configPath}</span> could not be read, so defaults are shown and saving is
                refused until it is fixed or removed: {settings.configError}
              </span>
            </div>
          )}
          {section === 'sources' && (
            <div className="settings-section">
              <div className="settings-title">Session sources</div>
              <div className="settings-sub">Where agent sessions are discovered on this machine.</div>
              {Object.entries(ADAPTER_META).map(([id, meta]) => {
                const cfg = settings.adapters[id] ?? { enabled: false };
                const status = statusOf(id);
                const listKey = meta.listKey;
                return (
                  <div className="settings-row" key={id} style={{ alignItems: 'flex-start' }}>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div className="row-label">
                        {meta.label}
                        {status && cfg.enabled && (
                          <span className={`badge ${status.sessions > 0 ? 'badge-ok' : 'badge-neutral'}`} style={{ marginLeft: 10 }}>
                            {status.sessions} session{status.sessions === 1 ? '' : 's'}
                          </span>
                        )}
                      </div>
                      <div className="row-desc">{status?.hint ?? meta.desc}</div>
                      {cfg.enabled && meta.dirLabel && (
                        <input
                          className="settings-input mono mt-2"
                          title={meta.dirLabel}
                          aria-label={`${meta.label}: ${meta.dirLabel}`}
                          value={cfg.dir ?? ''}
                          onChange={(e) => updateAdapter(id, { dir: e.target.value })}
                        />
                      )}
                      {cfg.enabled && listKey && (
                        <textarea
                          className="settings-input mono mt-2"
                          style={{ resize: 'vertical' }}
                          rows={2}
                          aria-label={`${meta.label}: ${meta.listLabel ?? 'folders'}`}
                          placeholder={meta.listLabel}
                          value={(cfg[listKey] ?? []).join('\n')}
                          onChange={(e) =>
                            updateAdapter(id, { [listKey]: e.target.value.split('\n').map((s) => s.trim()).filter(Boolean) })
                          }
                        />
                      )}
                    </div>
                    <Switch label={`Enable ${meta.label}`} checked={cfg.enabled} onChange={(v) => updateAdapter(id, { enabled: v })} />
                  </div>
                );
              })}
              <div className="settings-row">
                <div>
                  <div className="row-label">Max sessions per source</div>
                  <div className="row-desc">Older sessions beyond this limit are not shown.</div>
                </div>
                <input
                  className="settings-input mono"
                  style={{ width: 90, textAlign: 'center' }}
                  type="number"
                  aria-label="Max sessions per source"
                  min={MAX_SESSIONS_MIN}
                  max={MAX_SESSIONS_MAX}
                  value={maxSessionsRaw ?? String(settings.maxSessions)}
                  onChange={(e) => {
                    setMaxSessionsRaw(e.target.value);
                    dirty();
                  }}
                  onBlur={commitMaxSessions}
                />
              </div>
            </div>
          )}

          {section === 'checks' && (
            <div className="settings-section">
              <div className="settings-title">Review checks</div>
              <div className="settings-sub">
                The fixed checks ARS runs on every review: no AI, all on your machine. Add your own to flag any
                pattern you care about, line by line, with the severity you choose.
              </div>

              <div className="section-label mt-3">Built-in · always on</div>
              {BUILT_IN_CHECKS.map((c) => (
                <div className="settings-row" key={c.label}>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div className="row-label">{c.label}</div>
                    <div className="row-desc">{c.desc}</div>
                  </div>
                  <SeverityPill severity={c.severity} />
                </div>
              ))}

              <div className="spread mt-4" style={{ marginBottom: 'var(--sp-2)' }}>
                <span className="section-label" style={{ marginBottom: 0 }}>
                  Your checks{rules.length > 0 ? ` · ${rules.length}` : ''}
                </span>
                <button className="btn btn-sm btn-primary" onClick={addRule}>+ Add check</button>
              </div>
              {rules.length === 0 && (
                <div className="faint" style={{ fontSize: 'var(--fs-sm)' }}>
                  No custom checks yet. Example: flag every added line matching <span className="mono">{'\\beval\\('}</span>{' '}
                  as a critical issue.
                </div>
              )}
              <div className="stack" style={{ gap: 'var(--sp-3)' }}>
                {rules.map((r) => (
                  <RuleCard key={r.id} rule={r} onChange={(patch) => updateRule(r.id, patch)} onRemove={() => removeRule(r.id)} />
                ))}
              </div>

              <div className="faint mt-3" style={{ fontSize: 'var(--fs-xs)' }}>
                Rust regex syntax (no lookaround). Repository checks live in a committed{' '}
                <span className="mono">.ars/rules.json</span>: everyone who reviews that repository sees them.
              </div>
            </div>
          )}

          {section === 'data' && (
            <div className="settings-section">
              <div className="settings-title">Data</div>
              <div className="settings-sub">
                Local data lives in <span className="mono">~/.agent-review-studio/</span>: sessions, decisions, settings.
              </div>
              <div className="settings-row">
                <div>
                  <div className="row-label">Clear review decisions</div>
                  <div className="row-desc">Removes every decision, comment, note and copied review stored on this machine.</div>
                </div>
                <button className="btn btn-danger btn-sm" disabled={clearState.status === 'clearing'} onClick={clearDecisions}>
                  {clearState.status === 'clearing' ? 'Clearing…' : clearState.status === 'cleared' ? '✓ Cleared' : 'Clear…'}
                </button>
              </div>
              {clearState.status === 'failed' && (
                <div role="alert" style={{ color: 'var(--err)', fontSize: 'var(--fs-sm)' }}>
                  Review decisions were not cleared: {clearState.error}
                </div>
              )}
              <div className="settings-row">
                <div>
                  <div className="row-label">Sample tour</div>
                  <div className="row-desc">
                    Installs a sandbox repository with two sample sessions, or resets it to the start. Your projects are not touched.
                  </div>
                </div>
                <button className="btn btn-sm" disabled={tour.busy} onClick={tour.install}>
                  {tour.busy ? 'Preparing…' : 'Install / reset'}
                </button>
              </div>
              {tour.error && (
                <div role="alert" style={{ color: 'var(--err)', fontSize: 'var(--fs-sm)' }}>
                  The sample tour could not be installed: {tour.error}
                </div>
              )}
            </div>
          )}

          {savable && (
            <div className="flex" style={{ position: 'sticky', bottom: 0, background: 'var(--bg-0)', padding: 'var(--sp-3) 0' }}>
              <button className="btn btn-primary" disabled={saving} onClick={save}>
                {saving ? 'Saving…' : 'Save changes'}
              </button>
              {savedAt && <span style={{ color: 'var(--ok)' }}>✓ Saved</span>}
              {error && <span role="alert" style={{ color: 'var(--err)' }}>{error}</span>}
              <span className="topbar-spacer" />
              <span className="faint" style={{ fontSize: 'var(--fs-xs)' }}>
                <code className="mono">{settings.configPath}</code>
              </span>
            </div>
          )}
        </div>
      </div>
    </main>
  );
}
