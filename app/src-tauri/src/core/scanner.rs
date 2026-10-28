// The scanner keeps SCANNING (adapters → normalize → SQLite) apart from
// SERVING (`GET /sessions`). Serving reads an in-memory snapshot and never
// scans. Scans run in the background, debounced behind filesystem events,
// plus a slow tick for the sources that are not watched.
//
// The version counter only bumps when the *outcome* of a scan changes
// (the change signature), not on every filesystem event. Cursor rewrites
// its state.vscdb continuously while open; without this gate, every write
// would notify the window and make it fetch the sessions again.

use super::adapters::{
    aider::AiderAdapter, claude_code::ClaudeCodeAdapter, codex::CodexAdapter,
    copilot::CopilotAdapter, cursor::CursorAdapter, custom::CustomAdapter,
    gemini::GeminiCliAdapter, AdapterConfig, SessionAdapter,
};
use super::config::{load_config, AppConfig};
use super::domain::AgentSession;
use super::normalize::session_hash;
use super::storage;
use notify::Watcher;
use once_cell::sync::Lazy;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// Served history caps: the freshest N in each bucket (shown, hidden) stay
/// listed in the app; older rows stay in SQLite (and reachable by id).
const MAX_SERVED_ACTIVE: usize = 200;
const MAX_SERVED_ARCHIVED: usize = 200;

const DEBOUNCE: Duration = Duration::from_millis(1500);
const POLL_TICK: Duration = Duration::from_secs(45);

pub struct SessionRecord {
    /// Live records hold the full transcript-shaped session; stored-only
    /// records hold the summary shape (empty hunks/timeline).
    pub session: Arc<AgentSession>,
    /// session_hash of the full session (for stored-only records, the
    /// content_hash stored with it), never of the summary shape.
    pub hash: String,
    pub archived: bool,
    pub live: bool,
}

pub struct ScanState {
    pub generation: u64,
    pub sources: Vec<Value>,
    /// Session ids, newest first, capped.
    pub order: Vec<String>,
    pub by_id: HashMap<String, Arc<SessionRecord>>,
}

impl ScanState {
    fn empty() -> ScanState {
        ScanState {
            generation: 0,
            sources: Vec::new(),
            order: Vec::new(),
            by_id: HashMap::new(),
        }
    }
}

static ADAPTERS: Lazy<Vec<Box<dyn SessionAdapter + Send + Sync>>> = Lazy::new(|| {
    vec![
        Box::new(ClaudeCodeAdapter::new()),
        Box::new(CodexAdapter::new()),
        Box::new(CursorAdapter::new()),
        Box::new(CopilotAdapter::new()),
        Box::new(AiderAdapter::new()),
        Box::new(GeminiCliAdapter::new()),
        Box::new(CustomAdapter::new()),
    ]
});

static STATE: Lazy<RwLock<Arc<ScanState>>> =
    Lazy::new(|| RwLock::new(Arc::new(ScanState::empty())));
static SCAN_MUTEX: Mutex<()> = Mutex::new(());
static SOURCES_VERSION: AtomicU64 = AtomicU64::new(1);
static LAST_SIGNATURE: AtomicU64 = AtomicU64::new(0);

// Watcher plumbing. The callback only marks state dirty; the debounce loop
// decides when to actually rescan. No version bump from raw FS events.
static WATCHER: Lazy<Mutex<Option<(String, notify::RecommendedWatcher)>>> =
    Lazy::new(|| Mutex::new(None));
static DIRTY: AtomicBool = AtomicBool::new(false);
static LAST_EVENT: Lazy<Mutex<Option<Instant>>> = Lazy::new(|| Mutex::new(None));
/// True only in the desktop process (start_background ran): the one-shot
/// CLI scan must not install watchers or spawn threads.
static BACKGROUND: AtomicBool = AtomicBool::new(false);

/// True in the desktop process (background scanning runs there only).
pub fn is_desktop() -> bool {
    BACKGROUND.load(Ordering::Relaxed)
}

pub fn snapshot() -> Arc<ScanState> {
    // The state is a whole-Arc swap, so a poisoned lock still holds a
    // consistent snapshot.
    STATE.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Serve-ready state: scan once if no scan ever completed, else the snapshot.
pub fn ensure_scanned() -> Arc<ScanState> {
    {
        let s = snapshot();
        if s.generation > 0 {
            return s;
        }
    }
    rescan()
}

/// Run a full scan now (serialized; concurrent callers coalesce on the lock).
pub fn rescan() -> Arc<ScanState> {
    // A scan that panicked poisons the mutex; the guarded data is `()`, so
    // recover instead of serving a stale snapshot forever.
    let _guard = SCAN_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    do_scan()
}

/// The newest sessions of the project at `root`, newest activity first.
/// Sources that can find a project's sessions directly (Claude Code keys its
/// folders by project path) are asked directly; the others come from the
/// index of the last full scan. Never waits for a full scan.
pub fn project_sessions(root: &str, limit: usize) -> Vec<AgentSession> {
    let config = load_config();
    let state = snapshot();
    let mut out: Vec<AgentSession> = Vec::new();
    for adapter in ADAPTERS.iter() {
        let cfg = adapter_config_of(&config, adapter.id());
        if !cfg.enabled {
            continue;
        }
        let direct = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            adapter.project_sessions(&cfg, root, limit)
        }))
        .ok()
        .flatten();
        match direct {
            Some(found) => out.extend(found),
            None => out.extend(
                state
                    .order
                    .iter()
                    .filter_map(|id| state.by_id.get(id))
                    .filter(|r| r.live && r.session.agent == adapter.id())
                    .filter(|r| {
                        r.session
                            .project_path
                            .as_deref()
                            .is_some_and(|p| super::paths::is_within(p, root))
                    })
                    .map(|r| (*r.session).clone()),
            ),
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.session_id.clone()));
    let activity = |s: &AgentSession| {
        s.timeline
            .iter()
            .rev()
            .find_map(|e| super::normalize::parse_date(&e.timestamp))
            .or_else(|| super::normalize::parse_date(&s.timestamp))
            .map(|d| d.timestamp_millis())
    };
    out.sort_by_cached_key(|s| std::cmp::Reverse(activity(s)));
    out.truncate(limit);
    out
}

/// Desktop only: watcher + debounce loop + slow poll tick. Never the CLI.
pub fn start_background() {
    if BACKGROUND.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("ars-scanner".into())
        .spawn(|| {
            // Warm the state while the webview is still booting: by the time
            // the frontend asks for /sessions the scan is done or in flight.
            // A panic outside the per-adapter guard (storage, rules) fails
            // that one scan, not the thread: the next tick retries.
            let guarded_rescan = || {
                if std::panic::catch_unwind(rescan).is_err() {
                    eprintln!("ars: background scan panicked; retrying on the next tick");
                }
            };
            guarded_rescan();
            let mut last_scan = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let debounced_due = DIRTY.load(Ordering::Relaxed)
                    && LAST_EVENT
                        .lock()
                        .map(|g| *g)
                        .unwrap_or_else(|e| *e.into_inner())
                        .map(|t| t.elapsed() >= DEBOUNCE)
                        .unwrap_or(true);
                let tick_due = last_scan.elapsed() >= POLL_TICK;
                if debounced_due || tick_due {
                    DIRTY.store(false, Ordering::Relaxed);
                    guarded_rescan();
                    last_scan = Instant::now();
                }
            }
        })
        .ok();
}

fn adapter_config_of(config: &AppConfig, id: &str) -> AdapterConfig {
    let file = config.adapters.get(id).cloned().unwrap_or_default();
    AdapterConfig {
        enabled: file.enabled,
        dir: file.dir,
        project_dirs: file.project_dirs.unwrap_or_default(),
        dirs: file.dirs.unwrap_or_default(),
    }
}

/// Labels of the sources switched off in Settings (the review says when
/// that is why a change has no explanation).
pub fn disabled_source_labels() -> Vec<String> {
    let config = load_config();
    let off = disabled_sources(&config);
    ADAPTERS
        .iter()
        .filter(|a| off.contains(a.id()))
        .map(|a| a.label().to_string())
        .collect()
}

/// Ids of the adapters switched off in Settings. A session's `agent` is
/// the id of the adapter that produced it.
fn disabled_sources(config: &AppConfig) -> std::collections::HashSet<&'static str> {
    ADAPTERS
        .iter()
        .map(|adapter| adapter.id())
        .filter(|id| !adapter_config_of(config, id).enabled)
        .collect()
}

fn ensure_watchers(config: &AppConfig) {
    let mut targets: Vec<super::adapters::WatchTarget> = Vec::new();
    for adapter in ADAPTERS.iter() {
        let cfg = adapter_config_of(config, adapter.id());
        if !cfg.enabled {
            continue;
        }
        targets.extend(adapter.watch_targets(&cfg));
    }
    let key: String = targets
        .iter()
        .map(|t| format!("{}|{}", t.path, t.recursive))
        .collect::<Vec<_>>()
        .join(";");
    let mut guard = WATCHER.lock().unwrap_or_else(|e| e.into_inner());
    if guard.as_ref().map(|(k, _)| k == &key).unwrap_or(false) {
        return;
    }
    let mut watcher =
        match notify::recommended_watcher(|event: Result<notify::Event, notify::Error>| {
            if event.is_ok() {
                DIRTY.store(true, Ordering::Relaxed);
                *LAST_EVENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
            }
        }) {
            Ok(w) => w,
            Err(_) => return,
        };
    for t in &targets {
        let mode = if t.recursive {
            notify::RecursiveMode::Recursive
        } else {
            notify::RecursiveMode::NonRecursive
        };
        let _ = watcher.watch(std::path::Path::new(&t.path), mode);
    }
    *guard = Some((key, watcher));
}

/// Newest first by parsed instant: adapters emit mixed formats and UTC
/// offsets, so raw string order is wrong. Unparseable timestamps sort last;
/// the sort is stable and serialized timestamps are left untouched.
fn sort_newest_first<T>(items: &mut [T], timestamp: impl Fn(&T) -> &String) {
    items.sort_by_cached_key(|item| {
        std::cmp::Reverse(
            super::normalize::parse_date(timestamp(item)).map(|d| d.timestamp_millis()),
        )
    });
}

fn do_scan() -> Arc<ScanState> {
    let config = load_config();
    if BACKGROUND.load(Ordering::Relaxed) {
        ensure_watchers(&config);
    }

    // 1. Collect live sessions (adapter parse caches keep warm scans cheap).
    let mut sources: Vec<Value> = Vec::new();
    let mut live: Vec<AgentSession> = Vec::new();
    for adapter in ADAPTERS.iter() {
        let cfg = adapter_config_of(&config, adapter.id());
        // One malformed transcript source must not take down the scan (and
        // with it the scanner thread): a panicking adapter is reported as a
        // source with no sessions and the other sources still load.
        let collected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            adapter.collect(&cfg, config.max_sessions)
        }));
        match collected {
            Ok(result) => {
                sources.push(serde_json::to_value(&result.status).unwrap_or(Value::Null));
                live.extend(result.sessions);
            }
            Err(_) => {
                eprintln!("ars: the {} adapter panicked; source skipped", adapter.id());
                let mut status = adapter.base_status(&cfg, String::new());
                status.hint = Some(
                    "This source failed to load (internal error) and was skipped.".to_string(),
                );
                sources.push(serde_json::to_value(&status).unwrap_or(Value::Null));
            }
        }
    }

    // Two transcripts may claim the same session id (copied rollouts, a
    // re-exported custom file): keep one record per id, the newest.
    sort_newest_first(&mut live, |s| &s.timestamp);
    let mut live_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    live.retain(|s| live_ids.insert(s.session_id.clone()));

    // 1b. Custom review checks: global rules (Settings) + the repo's own
    // .ars/rules.json. Applied before hashing, so findings are part of
    // session_hash.
    let global_rules = super::rules::parse_rules(&config.finding_rules);
    for session in live.iter_mut() {
        let mut combined = global_rules.clone();
        combined.extend(super::rules::repo_rules_for(
            session.project_path.as_deref(),
        ));
        if !combined.is_empty() {
            super::rules::apply_rules(session, &combined);
        }
    }

    // 2. Hash once per live session; upsert ONLY new/changed rows.
    let stored_hashes = storage::all_content_hashes();
    let hashed: Vec<(AgentSession, String)> = live
        .into_iter()
        .map(|s| {
            let h = session_hash(&s);
            (s, h)
        })
        .collect();
    let changed: Vec<&(AgentSession, String)> = hashed
        .iter()
        .filter(|(s, h)| stored_hashes.get(&s.session_id) != Some(h))
        .collect();
    if !changed.is_empty() {
        storage::upsert_sessions_ref(&changed);
    }

    // 3. Merge in stored history (summary shape) and hidden (archived) flags.
    let archived_map = storage::archived_map();
    let mut records: Vec<Arc<SessionRecord>> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (session, hash) in hashed {
        let archived = archived_map
            .get(&session.session_id)
            .copied()
            .unwrap_or(false);
        seen.insert(session.session_id.clone());
        records.push(Arc::new(SessionRecord {
            session: Arc::new(session),
            hash,
            archived,
            live: true,
        }));
    }
    // History of a disabled source stays in the database but is not served:
    // turning a source off hides its sessions, turning it on restores them.
    let disabled = disabled_sources(&config);
    for (summary, archived, hash) in
        storage::list_stored_summaries(MAX_SERVED_ACTIVE, MAX_SERVED_ARCHIVED)
    {
        let Ok(session) = serde_json::from_value::<AgentSession>(summary) else {
            continue;
        };
        if disabled.contains(session.agent.as_str()) {
            continue;
        }
        if !seen.insert(session.session_id.clone()) {
            continue;
        }
        records.push(Arc::new(SessionRecord {
            session: Arc::new(session),
            hash,
            archived,
            live: false,
        }));
    }

    // 4. Newest first, then cap per bucket (live records always survive).
    sort_newest_first(&mut records, |r| &r.session.timestamp);
    let mut active = 0usize;
    let mut archived_n = 0usize;
    let mut order: Vec<String> = Vec::new();
    let mut by_id: HashMap<String, Arc<SessionRecord>> = HashMap::new();
    for record in records {
        let keep = if record.archived {
            archived_n += 1;
            record.live || archived_n <= MAX_SERVED_ARCHIVED
        } else {
            active += 1;
            record.live || active <= MAX_SERVED_ACTIVE
        };
        if keep {
            order.push(record.session.session_id.clone());
            by_id.insert(record.session.session_id.clone(), record);
        }
    }

    // 5. Change signature: bump the version only when the outcome changed.
    let mut hasher = DefaultHasher::new();
    for id in &order {
        let r = &by_id[id];
        id.hash(&mut hasher);
        r.hash.hash(&mut hasher);
        r.archived.hash(&mut hasher);
    }
    serde_json::to_string(&sources)
        .unwrap_or_default()
        .hash(&mut hasher);
    // Fold the active rule set into the signature so a rule edit always
    // bumps the version.
    serde_json::to_string(&config.finding_rules)
        .unwrap_or_default()
        .hash(&mut hasher);
    let signature = hasher.finish();
    let previous = LAST_SIGNATURE.swap(signature, Ordering::Relaxed);
    let first_scan = snapshot().generation == 0;
    if signature != previous || first_scan {
        let version = SOURCES_VERSION.fetch_add(1, Ordering::Relaxed) + 1;
        super::events::emit(
            "ars://sessions-changed",
            serde_json::json!({ "version": version }),
        );
    }

    let next = Arc::new(ScanState {
        generation: snapshot().generation + 1,
        sources,
        order,
        by_id,
    });
    if let Ok(mut guard) = STATE.write() {
        *guard = next.clone();
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_sources_follow_settings() {
        let mut config = super::super::config::defaults();
        for adapter in ADAPTERS.iter() {
            config
                .adapters
                .entry(adapter.id().to_string())
                .or_default()
                .enabled = true;
        }
        assert!(disabled_sources(&config).is_empty());
        config
            .adapters
            .get_mut("cursor")
            .expect("cursor config")
            .enabled = false;
        let disabled = disabled_sources(&config);
        assert_eq!(disabled.len(), 1);
        assert!(disabled.contains("cursor"));
    }
}
