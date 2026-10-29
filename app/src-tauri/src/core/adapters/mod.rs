// Adapter layer: each adapter knows one agent's native on-disk format and
// produces normalized AgentSessions. Product semantics (risk, findings,
// warnings) live in the normalizer, not here.

pub mod aider;
pub mod claude_code;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod custom;
pub mod gemini;
pub mod project_root;

use super::domain::AgentSession;
use serde::Serialize;
use std::collections::HashMap;

/// Health/status of one ingestion source, shown in Settings → Session sources
#[derive(Debug, Clone, Serialize)]
pub struct AdapterStatus {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub source: String,
    pub sessions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

pub struct CollectResult {
    pub sessions: Vec<AgentSession>,
    pub status: AdapterStatus,
}

/// Per-adapter slice of ~/.agent-review-studio/config.json
#[derive(Debug, Clone, Default)]
pub struct AdapterConfig {
    pub enabled: bool,
    pub dir: Option<String>,
    pub project_dirs: Vec<String>,
    pub dirs: Vec<String>,
}

/// One directory (or file) the scanner should watch for this adapter.
pub struct WatchTarget {
    pub path: String,
    pub recursive: bool,
}

pub trait SessionAdapter {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn collect(&self, config: &AdapterConfig, max_sessions: usize) -> CollectResult;

    /// The newest sessions of one project (`root`, a Git work tree root),
    /// found without a full scan when the source allows it. None: the source
    /// cannot, and the caller uses the scanner's index instead.
    fn project_sessions(
        &self,
        _config: &AdapterConfig,
        _root: &str,
        _limit: usize,
    ) -> Option<Vec<AgentSession>> {
        None
    }

    /// What to watch for near-real-time pickup. Default: everything the
    /// config names, recursively. Adapters whose sources are too noisy to
    /// watch (Cursor's state.vscdb, VS Code workspaceStorage) return nothing
    /// and rely on the scanner's slow poll tick instead.
    fn watch_targets(&self, config: &AdapterConfig) -> Vec<WatchTarget> {
        let mut out: Vec<WatchTarget> = Vec::new();
        if let Some(dir) = &config.dir {
            out.push(WatchTarget {
                path: dir.clone(),
                recursive: true,
            });
        }
        for dir in &config.project_dirs {
            out.push(WatchTarget {
                path: dir.clone(),
                recursive: true,
            });
        }
        for dir in &config.dirs {
            out.push(WatchTarget {
                path: dir.clone(),
                recursive: true,
            });
        }
        out
    }

    fn base_status(&self, config: &AdapterConfig, source: String) -> AdapterStatus {
        AdapterStatus {
            id: self.id().to_string(),
            label: self.label().to_string(),
            enabled: config.enabled,
            source,
            sessions: 0,
            hint: None,
        }
    }
}

/// `project_sessions` for small sources: their normal (cached) collect, kept
/// to the project.
pub fn collect_for_project(
    adapter: &dyn SessionAdapter,
    config: &AdapterConfig,
    root: &str,
    limit: usize,
) -> Vec<AgentSession> {
    let mut out: Vec<AgentSession> = adapter
        .collect(config, 60)
        .sessions
        .into_iter()
        .filter(|s| {
            s.project_path
                .as_deref()
                .is_some_and(|p| super::paths::is_within(p, root))
        })
        .collect();
    out.truncate(limit);
    out
}

/// Per-file mtime cache shared by the file-scanning adapters (parse only what
/// changed between scans).
#[derive(Default)]
pub struct ParseCache {
    entries: HashMap<String, (f64, Option<AgentSession>)>,
}

impl ParseCache {
    pub fn get(&self, path: &str, mtime_ms: f64) -> Option<&Option<AgentSession>> {
        match self.entries.get(path) {
            Some((cached_mtime, session)) if *cached_mtime == mtime_ms => Some(session),
            _ => None,
        }
    }

    pub fn put(&mut self, path: String, mtime_ms: f64, session: Option<AgentSession>) {
        self.entries.insert(path, (mtime_ms, session));
    }

    /// Drop entries for files no longer on disk: without this the cache
    /// grows monotonically across the process lifetime.
    pub fn retain_paths(&mut self, seen: &std::collections::HashSet<String>) {
        self.entries.retain(|path, _| seen.contains(path));
    }
}

/// mtime of a metadata entry in ms since the epoch (0 when unknown).
pub fn mtime_ms(meta: &std::fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

/// `ms` as an ISO-8601 UTC string (epoch on overflow).
pub fn mtime_iso(ms: f64) -> String {
    let secs = (ms / 1000.0).floor() as i64;
    let millis = (ms as i64).rem_euclid(1000) as u32;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, millis * 1_000_000)
        .map(super::normalize::to_iso)
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string())
}
