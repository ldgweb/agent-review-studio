// The API the frontend consumes, exposed as one Tauri command instead of an
// HTTP server. The CLI drives the same router; see `handle` for the routes
// and their JSON shapes.
//
// The project review (GET /review) is built on demand from Git and the
// project's own transcripts: it never waits for a scan. The session index
// is the scanner's job. Serving it never scans, except the very first call,
// explicit ?refresh=1, and writes that change what is served (hiding
// sessions, settings, sample install). The window learns about changes from events
// (core/events.rs), never by polling.
//
// Routes match exact (method, path) pairs. Anything that spawns git, hashes
// files or touches SQLite runs on the blocking pool, never inline on the
// async runtime.

use super::config::{config_error, config_path, load_config, save_config};
use super::domain::summary_value;
use super::{review, rules, scanner, storage};
use percent_encoding::percent_decode_str;
use serde_json::{json, Value};

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

/// The session list: summaries in scan order, with hidden (archived)
/// flags. No git subprocesses and no full payloads.
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

fn config_path_slashes() -> String {
    config_path().to_string_lossy().replace('\\', "/")
}

/// Open a URL with the OS default handler. The WebView cannot open the
/// editors' schemes (vscode://, cursor://, windsurf://) itself, so the
/// frontend routes through here. Only these schemes are allowed, and the
/// URL is passed as one argument, never through a shell, so no
/// metacharacter can smuggle a command.
fn open_external(url: &str) -> Value {
    const ALLOWED: [&str; 5] = [
        "https://",
        "http://",
        "vscode://",
        "cursor://",
        "windsurf://",
    ];
    if !ALLOWED.iter().any(|p| url.starts_with(p)) {
        return err("URL scheme not allowed.");
    }
    #[cfg(target_os = "windows")]
    let result = {
        let mut cmd = std::process::Command::new("rundll32");
        cmd.args(["url.dll,FileProtocolHandler", url]);
        super::git::hide_window(&mut cmd);
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

/// Show a file, selected, in the system's file manager (a folder opens as
/// itself). Only files of the open review get here; nothing is changed.
fn reveal(target: &std::path::Path) -> Value {
    let folder = target.is_dir();
    #[cfg(target_os = "windows")]
    let result = {
        use std::os::windows::process::CommandExt;
        // Explorer parses `/select,"path"` itself, so the argument goes as
        // written. A Windows path cannot contain a quote.
        let path = super::paths::plain(&target.to_string_lossy());
        let arg = if folder {
            format!("\"{path}\"")
        } else {
            format!("/select,\"{path}\"")
        };
        std::process::Command::new("explorer").raw_arg(arg).spawn()
    };
    #[cfg(target_os = "macos")]
    let result = if folder {
        std::process::Command::new("open").arg(target).spawn()
    } else {
        std::process::Command::new("open")
            .arg("-R")
            .arg(target)
            .spawn()
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open")
        .arg(if folder {
            target
        } else {
            target.parent().unwrap_or(target)
        })
        .spawn();
    match result {
        Ok(_) => json!({ "ok": true }),
        Err(e) => err(format!("Could not open the file manager: {e}")),
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
    // over. Say so instead of pretending the defaults are the user's.
    if let Some(e) = config_error() {
        payload["configError"] = json!(e);
    }
    payload
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

/// Handle one API call; `path` has the /api prefix already stripped.
///
/// Routes (exact method + path; `?query` only on GET /sessions):
///   GET  /sessions[?refresh=1]  POST /sessions/archive
///   GET  /projects              GET /review?project=<path>
///   POST /review/{state,handback,file,reveal,clear}
///   POST /open                  POST /demo/install
///   GET|POST /settings          POST /rules/test
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

        ("POST", "/open") => match body_json.get("url").and_then(Value::as_str) {
            Some(url) => open_external(url),
            None => err("Missing url."),
        },

        // First-run tour: a sandbox git repo and two ars/v1 sample sessions
        // (one fresh, one already reviewed), all through the REAL pipeline.
        // Hiding them works like hiding any session; re-installing resets
        // them and shows them again. See core/demo.rs.
        ("POST", "/demo/install") => tokio::task::spawn_blocking(super::demo::install)
            .await
            .unwrap_or_else(|_| err("The sample installer crashed.")),

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

        ("GET", "/projects") => {
            let hidden = query_param(query, "hidden").as_deref() == Some("1");
            blocking(move || review::projects(hidden)).await
        }

        // The review of the project containing `project` (any folder in it).
        // The desktop also starts watching that project for changes.
        ("GET", "/review") => {
            let Some(project) = query_param(query, "project") else {
                return err("Missing project.");
            };
            blocking(move || match review::build(&project) {
                Ok(r) => {
                    if scanner::is_desktop() {
                        super::watch::watch_project(&r.project.root);
                    }
                    serde_json::to_value(r).unwrap_or_else(|e| err(e.to_string()))
                }
                Err(e) => err(e),
            })
            .await
        }
        ("POST", "/review/state") => with_project(body_json, review::save_state).await,
        ("POST", "/review/handback") => with_project(body_json, review::handback).await,
        ("POST", "/review/file") => {
            blocking(move || {
                let project = body_json
                    .get("project")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let base = body_json.get("base").and_then(Value::as_bool) == Some(true);
                match body_json.get("path").and_then(Value::as_str) {
                    Some(path) if !project.is_empty() => review::file_lines(project, path, base),
                    _ => err("Missing project or path."),
                }
            })
            .await
        }
        ("POST", "/review/reveal") => {
            blocking(move || {
                let project = body_json
                    .get("project")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                match body_json.get("path").and_then(Value::as_str) {
                    Some(path) if !project.is_empty() => match review::located(project, path) {
                        Ok(target) => reveal(&target),
                        Err(e) => err(e),
                    },
                    _ => err("Missing project or path."),
                }
            })
            .await
        }
        ("POST", "/review/clear") => {
            blocking(|| match storage::clear_project_reviews() {
                Ok(()) => json!({ "ok": true }),
                Err(e) => err(e),
            })
            .await
        }

        _ => err("Unknown API endpoint"),
    }
}

/// A percent-decoded query parameter, when present and non-empty.
fn query_param(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|p| p.strip_prefix(name)?.strip_prefix('='))
        .map(|p| percent_decode_str(p).decode_utf8_lossy().to_string())
        .filter(|p| !p.is_empty())
}

/// Run `f(project, body)` on the blocking pool for a body naming a project.
async fn with_project(body: Value, f: fn(&str, &Value) -> Value) -> Value {
    blocking(move || match body.get("project").and_then(Value::as_str) {
        Some(project) if !project.is_empty() => f(project, &body),
        _ => err("Missing project."),
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::super::domain::AgentSession;
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn routes_match_exactly_and_bodies_must_parse() {
        for (method, path) in [
            ("POST", "/review/revert"),
            ("POST", "/git/commit"),
            ("POST", "/git/revert"),
            ("GET", "/approvals"),
            ("POST", "/settingsX"),
            ("GET", "/review/revert"),
            ("POST", "/sessions"),
        ] {
            assert_eq!(
                handle(path, method, Some("{}")).await["error"],
                "Unknown API endpoint",
                "{method} {path}"
            );
        }
        assert_eq!(
            handle("/review/state", "POST", Some("{not json")).await["error"],
            "Invalid JSON body"
        );
        assert_eq!(
            handle("/review/state", "POST", Some("{}")).await["error"],
            "Missing project."
        );
        assert_eq!(
            handle("/review", "GET", None).await["error"],
            "Missing project."
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

    /// Machine-dependent smoke test: run explicitly with `cargo test -- --ignored`.
    /// Scans the real adapter sources on this machine through the router and
    /// prints what the session list would show.
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
        let projects = handle("/projects", "GET", None).await;
        assert!(projects.get("projects").is_some());
        let settings = handle("/settings", "GET", None).await;
        assert!(settings.get("adapters").is_some());
    }
}
