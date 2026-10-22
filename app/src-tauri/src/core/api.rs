// The API the frontend consumes, exposed as one Tauri command instead of an
// HTTP server. The CLI drives the same router; see `handle` for the routes
// and their JSON shapes.
//
// Scanning is the scanner module's job — serving never scans, except the
// very first call, explicit ?refresh=1, and writes that change what is
// served (archive, settings, sample install), which rescan. List responses
// are summaries (no hunks, no timeline); the full session is served per id.
//
// Routes match exact (method, path) pairs. Anything that spawns git, hashes
// files or touches SQLite runs on the blocking pool, never inline on the
// async runtime.

use super::config::{config_error, config_path, load_config, save_config};
use super::domain::{summary_value, AgentSession};
use super::{git, rules, scanner, storage};
use percent_encoding::percent_decode_str;
use serde_json::{json, Value};
use std::sync::Arc;

fn indexed_session(session_id: &str) -> Option<Arc<AgentSession>> {
    scanner::session_by_id(session_id)
}

fn err(message: impl Into<String>) -> Value {
    json!({ "error": message.into() })
}

/// Run blocking work (git subprocesses, file hashing, SQLite, rescans) on
/// the blocking pool.
async fn blocking(f: impl FnOnce() -> Value + Send + 'static) -> Value {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| err(format!("The operation failed unexpectedly: {e}")))
}

/// `body.sessionId` → (id, indexed session, its JSON). Errors are ready-made
/// `{error}` responses.
fn session_from_body(body: &Value) -> Result<(String, Arc<AgentSession>, Value), Value> {
    let id = body
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err("Missing sessionId."))?;
    let session =
        indexed_session(id).ok_or_else(|| err(format!("No live session found for {id}.")))?;
    let value = serde_json::to_value(&*session)
        .map_err(|e| err(format!("Could not serialize session {id}: {e}")))?;
    Ok((id.to_string(), session, value))
}

/// The strings of `body[key]` (an array); non-strings are dropped.
fn str_list(body: &Value, key: &str) -> Vec<String> {
    body.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// A git write needs a SAVED verdict of `wanted` for every path, recorded at
/// the same review revision the request carries.
fn require_saved_verdicts(
    session_id: &str,
    session: &AgentSession,
    revision: &Value,
    paths: &[String],
    wanted: &str,
) -> Result<(), String> {
    let approvals = storage::get_approvals()?;
    let approval = &approvals[session_id];
    let revision = revision.as_str().filter(|s| !s.is_empty());
    let saved = revision.is_some()
        && approval["reviewRevision"].as_str() == revision
        && paths.iter().all(|path| {
            session
                .files
                .iter()
                .any(|f| &f.path == path && approval["fileStates"][&f.id] == wanted)
        });
    if saved {
        Ok(())
    } else {
        Err(match wanted {
            "approved" => {
                "Save an approval for each file at the current revision before committing."
            }
            _ => "Save a rejection for each file at the current revision before reverting.",
        }
        .into())
    }
}

/// The inbox list: summaries in scan order, with archived flags. No git
/// subprocesses, no policy reads, no full payloads.
fn serve_summaries(state: &scanner::ScanState) -> Vec<Value> {
    state
        .order
        .iter()
        .filter_map(|id| {
            let record = state.by_id.get(id)?;
            let mut value = summary_value(&record.session);
            // The full-session hash (the detail route's), not one of the
            // summary shape stored-only records hold.
            value["contentHash"] = json!(record.hash);
            if record.archived {
                if let Some(map) = value.as_object_mut() {
                    map.insert("archived".into(), json!(true));
                }
            }
            Some(value)
        })
        .collect()
}

/// One full session: live git enrichment, repo policy.
fn serve_detail(session_id: &str) -> Value {
    let Some(session) = indexed_session(session_id) else {
        return err(format!("No session found for {}.", session_id));
    };
    let mut value = match serde_json::to_value(&*session) {
        Ok(v) => v,
        Err(e) => return err(format!("Could not serialize session {session_id}: {e}")),
    };
    let raw = value.clone();
    let revision = match git::review_revision(&raw) {
        Ok(r) => r,
        Err(e) => return err(e),
    };
    git::enrich_session(&mut value);
    // The diff shown must describe the bytes the revision token was taken
    // from: if anything changed while enriching, the client reloads.
    if let Err(e) = git::require_revision(&raw, Some(&revision)) {
        return err(e);
    }
    value["reviewRevision"] = json!(revision);
    value["contentHash"] = json!(super::normalize::session_hash(&session));
    let project_path = value
        .get("projectPath")
        .and_then(Value::as_str)
        .map(String::from);
    if let Some(policy) = super::policy::load_policy_cached(project_path.as_deref()) {
        if let Some(map) = value.as_object_mut() {
            map.insert("policy".into(), policy);
        }
    }
    if scanner::is_archived(session_id) {
        if let Some(map) = value.as_object_mut() {
            map.insert("archived".into(), json!(true));
        }
    }
    json!({ "session": value })
}

fn config_path_slashes() -> String {
    config_path().to_string_lossy().replace('\\', "/")
}

/// Open a URL with the OS default handler. The WebView cannot open custom
/// schemes (vscode://, cursor://) itself, so the frontend routes through
/// here. Scheme-whitelisted; the URL is passed as a bare argv entry — never
/// through a shell — so no metacharacter can smuggle a command.
fn open_external(url: &str) -> Value {
    const ALLOWED: [&str; 4] = ["https://", "http://", "vscode://", "cursor://"];
    if !ALLOWED.iter().any(|p| url.starts_with(p)) {
        return err("URL scheme not allowed.");
    }
    #[cfg(target_os = "windows")]
    let result = {
        let mut cmd = std::process::Command::new("rundll32");
        cmd.args(["url.dll,FileProtocolHandler", url]);
        git::hide_window(&mut cmd);
        cmd.spawn()
    };
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    match result {
        Ok(_) => json!({ "ok": true }),
        Err(e) => err(format!("Could not open the URL: {e}")),
    }
}

fn settings_payload() -> Value {
    let config = load_config();
    let mut payload = json!({
        "configPath": config_path_slashes(),
        "maxSessions": config.max_sessions,
        "adapters": config.adapters,
        "findingRules": config.finding_rules,
    });
    // A malformed config.json is ignored (defaults shown) and never saved
    // over — say so instead of pretending the defaults are the user's.
    if let Some(e) = config_error() {
        payload["configError"] = json!(e);
    }
    payload
}

fn approvals_payload() -> Value {
    match storage::get_approvals() {
        Ok(v) => json!({ "approvals": v }),
        Err(e) => err(e),
    }
}

fn save_approvals(body: &Value) -> Value {
    if body
        .get("clearAll")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        if let Err(e) = storage::clear_approvals() {
            return err(e);
        }
        return approvals_payload();
    }
    let Some(state) = body.get("state").filter(|s| s.is_object()) else {
        return err("Missing sessionId or state.");
    };
    let (session_id, _, value) = match session_from_body(body) {
        Ok(found) => found,
        Err(e) => return e,
    };
    if let Err(e) = git::require_revision(&value, state["reviewRevision"].as_str()) {
        return err(e);
    }
    if let Err(e) = storage::save_approval(&session_id, state) {
        return err(e);
    }
    approvals_payload()
}

/// Settings → Review checks tester: the real Rust regex engine, compiled the
/// way scan-time rules are. `line` (alias `added-lines`) tests each line of
/// the sample as one added line; `path` tests the sample as a file path.
fn test_rule(body: &Value) -> Value {
    let Some(pattern) = body.get("pattern").and_then(Value::as_str) else {
        return err("Missing pattern.");
    };
    let sample = body.get("sample").and_then(Value::as_str).unwrap_or("");
    let result = match body.get("target").and_then(Value::as_str).unwrap_or("line") {
        "path" => rules::test_pattern(pattern, sample),
        "line" | "added-lines" => {
            let text = sample
                .strip_suffix("\r\n")
                .or_else(|| sample.strip_suffix('\n'))
                .unwrap_or(sample);
            let mut result = Ok(false);
            for line in text.split('\n') {
                let line = line.strip_suffix('\r').unwrap_or(line);
                match rules::test_pattern(pattern, line) {
                    Ok(false) => {}
                    other => {
                        result = other;
                        break;
                    }
                }
            }
            result
        }
        other => {
            return err(format!(
                "Unknown target \"{other}\" (expected \"line\" or \"path\")."
            ))
        }
    };
    match result {
        Ok(matches) => json!({ "matches": matches }),
        Err(e) => err(e),
    }
}

fn git_commit(body: &Value) -> Value {
    let paths = str_list(body, "paths");
    let Some(message) = body.get("message").and_then(Value::as_str) else {
        return err("Missing sessionId, paths or message.");
    };
    if paths.is_empty() {
        return err("Missing sessionId, paths or message.");
    }
    let (session_id, session, mut value) = match session_from_body(body) {
        Ok(found) => found,
        Err(e) => return e,
    };
    value["reviewRevision"] = body["reviewRevision"].clone();
    if let Err(e) = require_saved_verdicts(
        &session_id,
        &session,
        &value["reviewRevision"],
        &paths,
        "approved",
    ) {
        return err(e);
    }
    git::commit_files(&value, &paths, message)
}

fn git_revert(body: &Value) -> Value {
    let paths = str_list(body, "paths");
    if paths.is_empty() {
        return err("Missing sessionId or paths.");
    }
    let (session_id, session, mut value) = match session_from_body(body) {
        Ok(found) => found,
        Err(e) => return e,
    };
    value["reviewRevision"] = body["reviewRevision"].clone();
    if let Err(e) = require_saved_verdicts(
        &session_id,
        &session,
        &value["reviewRevision"],
        &paths,
        "rejected",
    ) {
        return err(e);
    }
    git::revert_files(&value, &paths)
}

fn git_file(body: &Value) -> Value {
    let Some(file_path) = body
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
    else {
        return err("Missing sessionId or path.");
    };
    match session_from_body(body) {
        Ok((_, _, value)) => git::read_file_lines(&value, file_path),
        Err(e) => e,
    }
}

fn git_push(body: &Value) -> Value {
    let set_upstream = body
        .get("setUpstream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match session_from_body(body) {
        Ok((_, _, value)) => git::push_branch(&value, set_upstream),
        Err(e) => e,
    }
}

fn git_branch(body: &Value) -> Value {
    let Some(name) = body.get("name").and_then(Value::as_str) else {
        return err("Missing sessionId or name.");
    };
    match session_from_body(body) {
        Ok((_, _, value)) => git::create_branch(&value, name),
        Err(e) => e,
    }
}

fn git_pr(body: &Value) -> Value {
    let title = body.get("title").and_then(Value::as_str);
    let body_text = body.get("body").and_then(Value::as_str);
    match session_from_body(body) {
        Ok((_, _, value)) => git::pr_url(&value, title, body_text),
        Err(e) => e,
    }
}

/// Handle one API call; `path` has the /api prefix already stripped.
///
/// Routes (exact method + path; `?query` only on GET /sessions):
///   GET  /sessions[?refresh=1]  GET /sessions/version  GET /sessions/<id>
///   POST /sessions/archive      POST /open             POST /demo/install
///   GET|POST /approvals         GET|POST /settings     POST /rules/test
///   POST /git/{commit,revert,file,status,push,branch,log,pr}
pub async fn handle(path: &str, method: &str, body: Option<&str>) -> Value {
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    let body_json: Value = match body.map(str::trim).filter(|b| !b.is_empty()) {
        None => Value::Null,
        Some(raw) => match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(_) => return err("Invalid JSON body"),
        },
    };
    let method = method.to_ascii_uppercase();

    match (method.as_str(), route) {
        ("GET", "/sessions") => {
            let refresh = query.split('&').any(|p| p == "refresh=1");
            let state = if refresh {
                tokio::task::spawn_blocking(scanner::rescan)
                    .await
                    .unwrap_or_else(|_| scanner::snapshot())
            } else if scanner::snapshot().generation == 0 {
                tokio::task::spawn_blocking(scanner::ensure_scanned)
                    .await
                    .unwrap_or_else(|_| scanner::snapshot())
            } else {
                scanner::snapshot()
            };
            let mut response = json!({
                "configPath": config_path_slashes(),
                "sources": state.sources.clone(),
                "sessions": serve_summaries(&state),
            });
            if let Some(e) = storage::last_error() {
                response["storageError"] = json!(e);
            }
            response
        }

        ("GET", "/sessions/version") => json!({ "version": scanner::version() }),

        ("POST", "/sessions/archive") => {
            let ids = str_list(&body_json, "sessionIds");
            if ids.is_empty() {
                return err("No sessions selected.");
            }
            let archived = body_json
                .get("archived")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            blocking(move || match storage::set_archived(&ids, archived) {
                Ok(changed) => {
                    // Rescan so the served state (and the version counter)
                    // reflect the change immediately, not on the next tick.
                    scanner::rescan();
                    json!({ "changed": changed })
                }
                Err(e) => err(e),
            })
            .await
        }

        ("GET", detail)
            if detail.len() > "/sessions/".len() && detail.starts_with("/sessions/") =>
        {
            let id = percent_decode_str(&detail["/sessions/".len()..])
                .decode_utf8_lossy()
                .to_string();
            blocking(move || serve_detail(&id)).await
        }

        ("POST", "/open") => match body_json.get("url").and_then(Value::as_str) {
            Some(url) => open_external(url),
            None => err("Missing url."),
        },

        // First-run tour: a sandbox git repo + two ars/v1 sample sessions
        // (one fresh, one already reviewed) — all through the REAL pipeline.
        // Archiving them is the normal archive action; re-installing resets
        // and un-archives. See core/demo.rs.
        ("POST", "/demo/install") => tokio::task::spawn_blocking(super::demo::install)
            .await
            .unwrap_or_else(|_| err("The sample installer crashed.")),

        ("GET", "/approvals") => blocking(approvals_payload).await,
        ("POST", "/approvals") => blocking(move || save_approvals(&body_json)).await,

        ("GET", "/settings") => settings_payload(),
        ("POST", "/settings") => {
            if body_json.get("adapters").is_some()
                || body_json.get("maxSessions").is_some()
                || body_json.get("findingRules").is_some()
            {
                if let Err(e) = save_config(&body_json) {
                    return err(e);
                }
                // Rules changed → re-evaluate the live sessions right away.
                let _ = tokio::task::spawn_blocking(scanner::rescan).await;
            }
            settings_payload()
        }

        ("POST", "/rules/test") => test_rule(&body_json),

        ("POST", "/git/commit") => blocking(move || git_commit(&body_json)).await,
        ("POST", "/git/revert") => blocking(move || git_revert(&body_json)).await,
        ("POST", "/git/file") => blocking(move || git_file(&body_json)).await,
        ("POST", "/git/status") => {
            blocking(move || match session_from_body(&body_json) {
                Ok((_, _, value)) => git::repo_status(&value),
                Err(e) => e,
            })
            .await
        }
        ("POST", "/git/push") => blocking(move || git_push(&body_json)).await,
        ("POST", "/git/branch") => blocking(move || git_branch(&body_json)).await,
        ("POST", "/git/log") => {
            blocking(move || match session_from_body(&body_json) {
                Ok((_, _, value)) => git::git_log(&value, 18),
                Err(e) => e,
            })
            .await
        }
        ("POST", "/git/pr") => blocking(move || git_pr(&body_json)).await,

        _ => err("Unknown API endpoint"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn routes_match_exactly_and_bodies_must_parse() {
        for (method, path) in [
            ("POST", "/git/commitX"),
            ("POST", "/approvals/foo"),
            ("POST", "/settingsX"),
            ("GET", "/git/commit"),
            ("POST", "/sessions"),
        ] {
            assert_eq!(
                handle(path, method, Some("{}")).await["error"],
                "Unknown API endpoint",
                "{method} {path}"
            );
        }
        assert_eq!(
            handle("/git/commit", "POST", Some("{not json")).await["error"],
            "Invalid JSON body"
        );
        assert_eq!(
            handle("/git/status", "POST", Some("{}")).await["error"],
            "Missing sessionId."
        );
    }

    #[tokio::test]
    async fn rule_tester_uses_the_rust_engine() {
        let call = |body: Value| async move {
            handle("/rules/test", "POST", Some(&body.to_string())).await
        };
        let hit =
            call(json!({ "pattern": "\\beval\\(", "target": "line", "sample": "x = eval(y)" }));
        assert_eq!(hit.await["matches"], true);
        let multi = call(json!({ "pattern": "^TODO", "target": "line", "sample": "a\nTODO: b\n" }));
        assert_eq!(multi.await["matches"], true);
        let miss = call(json!({ "pattern": "^$", "target": "line", "sample": "a\n" }));
        assert_eq!(miss.await["matches"], false);
        let path = call(json!({ "pattern": "\\.sql$", "target": "path", "sample": "db/001.sql" }));
        assert_eq!(path.await["matches"], true);
        // Lookaround is unsupported by the Rust regex engine: the tester reports it.
        let bad = call(json!({ "pattern": "(?=x)", "target": "line", "sample": "x" })).await;
        assert!(bad["error"].as_str().unwrap().contains("look"), "{bad}");
        let unknown = call(json!({ "pattern": "x", "target": "diff", "sample": "x" })).await;
        assert!(unknown["error"].is_string());
    }

    #[test]
    fn list_hash_is_the_full_session_hash_for_stored_records() {
        let expected: Value = serde_json::from_str(&super::super::fixture::read(
            "claude-code/expected/basic.json",
        ))
        .unwrap();
        let full: AgentSession = serde_json::from_value(expected["session"].clone()).unwrap();
        let full_hash = super::super::normalize::session_hash(&full);
        // Stored-only records hold the summary shape, whose own hash differs.
        let summary: AgentSession = serde_json::from_value(summary_value(&full)).unwrap();
        assert_ne!(super::super::normalize::session_hash(&summary), full_hash);
        let id = full.session_id.clone();
        let state = scanner::ScanState {
            generation: 1,
            sources: Vec::new(),
            order: vec![id.clone()],
            by_id: [(
                id,
                Arc::new(scanner::SessionRecord {
                    session: Arc::new(summary),
                    hash: full_hash.clone(),
                    archived: false,
                    live: false,
                }),
            )]
            .into_iter()
            .collect(),
        };
        assert_eq!(serve_summaries(&state)[0]["contentHash"], json!(full_hash));
    }

    /// Machine-dependent smoke test — run explicitly with `cargo test -- --ignored`.
    /// Scans the real adapter sources on this machine through the router and
    /// prints what the inbox would show.
    #[tokio::test]
    #[ignore]
    async fn smoke_sessions() {
        let response = handle("/sessions", "GET", None).await;
        let sources = response
            .get("sources")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let sessions = response
            .get("sessions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for source in &sources {
            println!(
                "source {}: {} sessions {}",
                source.get("id").and_then(Value::as_str).unwrap_or("?"),
                source.get("sessions").and_then(Value::as_u64).unwrap_or(0),
                source.get("hint").and_then(Value::as_str).unwrap_or("")
            );
        }
        println!("total sessions served: {}", sessions.len());
        for s in sessions.iter().take(8) {
            println!(
                "- {} | {} | {} | archived={}",
                s.get("agent").and_then(Value::as_str).unwrap_or("?"),
                s.get("projectName").and_then(Value::as_str).unwrap_or("?"),
                s.get("declaredGoal").and_then(Value::as_str).unwrap_or("?"),
                s.get("archived").and_then(Value::as_bool).unwrap_or(false)
            );
        }
        assert!(!sources.is_empty());
        let version = handle("/sessions/version", "GET", None).await;
        assert!(version.get("version").and_then(Value::as_u64).unwrap_or(0) >= 1);
        let approvals = handle("/approvals", "GET", None).await;
        assert!(approvals.get("approvals").is_some());
        let settings = handle("/settings", "GET", None).await;
        assert!(settings.get("adapters").is_some());
    }
}
