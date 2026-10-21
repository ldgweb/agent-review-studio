// The ~/.agent-review-studio/ars.db SQLite store: session history (scans
// upsert, so sessions survive transcript rotation) and approvals.
//
// Failures are never swallowed: review writes (approvals, archive) return
// Result for the API to surface, and the scanner-facing calls — which keep
// their infallible signatures so a broken store never breaks a scan —
// record what failed in `last_error`, which the inbox route reports.

use super::config::config_dir;
use super::domain::AgentSession;
use once_cell::sync::Lazy;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  agent TEXT NOT NULL,
  model TEXT NOT NULL,
  project_name TEXT NOT NULL,
  project_path TEXT,
  branch TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  declared_goal TEXT NOT NULL,
  status TEXT NOT NULL,
  files_modified INTEGER NOT NULL,
  lines_added INTEGER NOT NULL,
  lines_removed INTEGER NOT NULL,
  duration_minutes INTEGER NOT NULL,
  content_hash TEXT NOT NULL,
  archived INTEGER NOT NULL DEFAULT 0,
  archived_at TEXT,
  last_seen_at TEXT NOT NULL,
  payload TEXT NOT NULL,
  summary TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_inbox ON sessions (archived, timestamp DESC);
CREATE TABLE IF NOT EXISTS approvals (
  session_id TEXT PRIMARY KEY,
  updated_at TEXT NOT NULL,
  payload TEXT NOT NULL
);
"#;

static DB: Lazy<Mutex<Option<Connection>>> = Lazy::new(|| Mutex::new(None));

/// operation → its latest failure, cleared when that operation next succeeds.
static STATUS: Lazy<Mutex<BTreeMap<&'static str, String>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));

fn now_iso() -> String {
    super::normalize::to_iso(chrono::Utc::now())
}

/// Open the database and bring its schema up to date. A connection whose
/// schema failed is never cached: the next call tries again.
fn open() -> Result<Connection, String> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let path = dir.join("ars.db");
    let conn =
        Connection::open(&path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    conn.execute_batch(SCHEMA)
        .map_err(|e| format!("Could not prepare {}: {e}", path.display()))?;
    // Migration: older databases lack the summary column.
    let has_summary = conn
        .prepare("SELECT 1 FROM pragma_table_info('sessions') WHERE name = 'summary'")
        .and_then(|mut stmt| stmt.exists([]))
        .map_err(|e| format!("Could not inspect {}: {e}", path.display()))?;
    if !has_summary {
        conn.execute("ALTER TABLE sessions ADD COLUMN summary TEXT", [])
            .map_err(|e| format!("Could not migrate {}: {e}", path.display()))?;
    }
    Ok(conn)
}

/// Runs `f` against the shared connection (opened on first use). A poisoned
/// lock is recovered — a panic elsewhere must not disable persistence.
fn with_db<T>(f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, String> {
    let mut guard = DB.lock().unwrap_or_else(|e| e.into_inner());
    let conn = match guard.take() {
        Some(conn) => conn,
        None => open()?,
    };
    let result = f(&conn);
    *guard = Some(conn);
    result.map_err(|e| e.to_string())
}

/// Record the outcome of a scanner-facing operation for `last_error`.
fn note<T>(op: &'static str, result: Result<T, String>) -> Option<T> {
    let mut status = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok(v) => {
            status.remove(op);
            Some(v)
        }
        Err(e) => {
            eprintln!("ars: session history {op} failed: {e}");
            status.insert(op, e);
            None
        }
    }
}

/// The current session-history failures (scan-time reads and writes), if
/// any — surfaced by the inbox route as `storageError`.
pub fn last_error() -> Option<String> {
    let status = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    if status.is_empty() {
        None
    } else {
        Some(
            status
                .iter()
                .map(|(op, e)| format!("Session history {op} failed: {e}"))
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

/// Upsert scanned sessions. Never fails the scan; failures (including
/// individual rows) are recorded for `last_error`.
pub fn upsert_sessions_ref(sessions: &[&(AgentSession, String)]) {
    note("save", upsert_sessions(sessions));
}

fn upsert_sessions(sessions: &[&(AgentSession, String)]) -> Result<(), String> {
    let failures = with_db(|conn| {
        let mut stmt = conn.prepare(
            "INSERT INTO sessions (id, agent, model, project_name, project_path, branch, timestamp, declared_goal, status,
                                   files_modified, lines_added, lines_removed, duration_minutes, content_hash, last_seen_at, payload, summary)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(id) DO UPDATE SET
               agent = excluded.agent, model = excluded.model, project_name = excluded.project_name,
               project_path = excluded.project_path, branch = excluded.branch, timestamp = excluded.timestamp,
               declared_goal = excluded.declared_goal, status = excluded.status, files_modified = excluded.files_modified,
               lines_added = excluded.lines_added, lines_removed = excluded.lines_removed,
               duration_minutes = excluded.duration_minutes, content_hash = excluded.content_hash,
               last_seen_at = excluded.last_seen_at, payload = excluded.payload, summary = excluded.summary",
        )?;
        let now = now_iso();
        let mut failures: Vec<String> = Vec::new();
        for (s, hash) in sessions {
            let payload = match serde_json::to_string(s) {
                Ok(p) => p,
                Err(e) => {
                    failures.push(format!("{}: {e}", s.session_id));
                    continue;
                }
            };
            let summary = super::domain::summary_value(s).to_string();
            if let Err(e) = stmt.execute(rusqlite::params![
                s.session_id,
                s.agent,
                s.model,
                s.project_name,
                s.project_path,
                s.branch,
                s.timestamp,
                s.declared_goal,
                s.status,
                s.stats.files_modified,
                s.stats.lines_added,
                s.stats.lines_removed,
                s.stats.duration_minutes,
                hash,
                now,
                payload,
                summary,
            ]) {
                failures.push(format!("{}: {e}", s.session_id));
            }
        }
        Ok(failures)
    })?;
    match failures.first() {
        None => Ok(()),
        Some(first) => Err(format!(
            "{} of {} sessions could not be saved (first: {first}).",
            failures.len(),
            sessions.len()
        )),
    }
}

/// id → content_hash for every stored row (skip-unchanged upserts).
pub fn all_content_hashes() -> HashMap<String, String> {
    note(
        "read",
        with_db(|conn| {
            let mut stmt = conn.prepare("SELECT id, content_hash FROM sessions")?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<rusqlite::Result<HashMap<String, String>>>()
        }),
    )
    .unwrap_or_default()
}

/// id → archived for every stored row.
pub fn archived_map() -> HashMap<String, bool> {
    note(
        "read",
        with_db(|conn| {
            let mut stmt = conn.prepare("SELECT id, archived FROM sessions")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
            })?;
            rows.collect::<rusqlite::Result<HashMap<String, bool>>>()
        }),
    )
    .unwrap_or_default()
}

/// The full payload of one stored session.
pub fn get_session_payload(id: &str) -> Option<Value> {
    note(
        "read",
        with_db(|conn| {
            conn.query_row("SELECT payload FROM sessions WHERE id = ?1", [id], |row| {
                row.get::<_, String>(0)
            })
            .optional()
        }),
    )
    .flatten()
    .and_then(|raw| serde_json::from_str(&raw).ok())
}

/// (summary, archived, content_hash) for the freshest rows per bucket,
/// newest first, read from the summary column. Rows without a stored
/// summary fall back to their full payload once: the derived summary is
/// written back.
pub fn list_stored_summaries(
    active_limit: usize,
    archived_limit: usize,
) -> Vec<(Value, bool, String)> {
    let (mut out, heal) = note(
        "read",
        with_db(|conn| {
            let mut collected: Vec<(Value, bool, String)> = Vec::new();
            let mut heal: Vec<(String, String)> = Vec::new();
            let mut payload_stmt = conn.prepare("SELECT payload FROM sessions WHERE id = ?1")?;
            for (archived, limit) in [(0i64, active_limit), (1i64, archived_limit)] {
                let mut stmt = conn.prepare(
                    "SELECT id, summary, archived, content_hash FROM sessions
                     WHERE archived = ?1 ORDER BY timestamp DESC LIMIT ?2",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![archived, limit as i64], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, i64>(2)? != 0,
                            row.get::<_, String>(3)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for (id, summary, is_archived, hash) in rows {
                    let stored = summary.and_then(|s| serde_json::from_str::<Value>(&s).ok());
                    let value = match stored {
                        Some(v) => v,
                        None => {
                            let raw: String = payload_stmt.query_row([&id], |row| row.get(0))?;
                            let Ok(mut v) = serde_json::from_str::<Value>(&raw) else {
                                continue;
                            };
                            super::domain::strip_to_summary(&mut v);
                            heal.push((id, v.to_string()));
                            v
                        }
                    };
                    collected.push((value, is_archived, hash));
                }
            }
            Ok((collected, heal))
        }),
    )
    .unwrap_or_default();
    if !heal.is_empty() {
        note(
            "summary write-back",
            with_db(|conn| {
                let mut write = conn.prepare("UPDATE sessions SET summary = ?2 WHERE id = ?1")?;
                for (id, summary) in &heal {
                    write.execute(rusqlite::params![id, summary])?;
                }
                Ok(())
            }),
        );
    }
    out.sort_by(|a, b| {
        let ta = a.0.get("timestamp").and_then(Value::as_str).unwrap_or("");
        let tb = b.0.get("timestamp").and_then(Value::as_str).unwrap_or("");
        tb.cmp(ta)
    });
    out
}

/// Archive / un-archive stored sessions, atomically. Returns how many rows
/// actually exist and were updated.
pub fn set_archived(session_ids: &[String], archived: bool) -> Result<usize, String> {
    with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        let now = if archived { Some(now_iso()) } else { None };
        let mut changed = 0usize;
        {
            let mut stmt =
                tx.prepare("UPDATE sessions SET archived = ?1, archived_at = ?2 WHERE id = ?3")?;
            for id in session_ids {
                changed += stmt.execute(rusqlite::params![archived as i64, now, id])?;
            }
        }
        tx.commit()?;
        Ok(changed)
    })
    .map_err(|e| format!("Could not update the archive in SQLite: {e}"))
}

pub fn get_approvals() -> Result<Value, String> {
    let mut out = serde_json::Map::new();
    let rows = with_db(|conn| {
        let mut stmt = conn.prepare("SELECT session_id, payload FROM approvals")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
    .map_err(|e| format!("Could not read reviews from SQLite: {e}"))?;
    for (session_id, payload) in rows {
        let value = serde_json::from_str::<Value>(&payload)
            .map_err(|_| format!("The stored review for {session_id} is invalid JSON."))?;
        out.insert(session_id, value);
    }
    Ok(Value::Object(out))
}

pub fn save_approval(session_id: &str, state: &Value) -> Result<(), String> {
    with_db(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO approvals (session_id, updated_at, payload) VALUES (?1, ?2, ?3)",
            rusqlite::params![session_id, now_iso(), state.to_string()],
        )
    })
    .map(|_| ())
    .map_err(|e| format!("Could not save the review to SQLite: {e}"))
}

pub fn clear_approvals() -> Result<(), String> {
    with_db(|conn| conn.execute("DELETE FROM approvals", []))
        .map(|_| ())
        .map_err(|e| format!("Could not clear reviews from SQLite: {e}"))
}
