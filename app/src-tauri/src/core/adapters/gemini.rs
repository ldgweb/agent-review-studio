// Gemini CLI adapter — parses saved conversations from
// ~/.gemini/tmp/<hash>/: `checkpoint*.json` (raw Gemini API Content[] written
// by /chat save and --checkpointing) and `chats/*.json` (session files with a
// messages array). Defensive by design: Gemini CLI's on-disk format is not a
// stable API, so both shapes are detected structurally and unknown records
// are skipped.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use super::super::domain::{
    AgentSession, DiffHunk, DiffLine, DiffLineKind, RawFile, RawSession, TimelineEvent,
    TimelineEventDraft, TimelineEventKind,
};
use super::super::normalize::{duration_minutes, first_line, normalize, truncate, TEST_COMMAND};
use super::project_root::{ascend_to_project_root, common_root, git_branch_of, relative_to};
use super::{mtime_iso, mtime_ms, AdapterConfig, CollectResult, ParseCache, SessionAdapter};

/// The string value or ''.
fn s(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or("")
}

/// Length in UTF-16 code units (what the fixture contract counts).
fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Truthy: a non-empty string, a non-zero number, true, or any array/object.
fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

/// First non-empty candidate or ''.
fn pick<'a>(candidates: &[&'a str]) -> &'a str {
    candidates
        .iter()
        .copied()
        .find(|c| !c.is_empty())
        .unwrap_or("")
}

struct ParseState {
    /// Insertion order preserved with a Vec keyed by path
    files: Vec<RawFile>,
    timeline: Vec<TimelineEvent>,
    event_seq: usize,
    original_prompt: String,
    last_agent_text: String,
    commands_run: i64,
    tests_run: i64,
    tests_failed: i64,
    errors: i64,
    files_analyzed: i64,
    /// Whether the last function call ran a test command, to attribute failures to test runs.
    last_command_was_test: bool,
}

impl ParseState {
    fn new() -> Self {
        Self {
            files: Vec::new(),
            timeline: Vec::new(),
            event_seq: 0,
            original_prompt: String::new(),
            last_agent_text: String::new(),
            commands_run: 0,
            tests_run: 0,
            tests_failed: 0,
            errors: 0,
            files_analyzed: 0,
            last_command_was_test: false,
        }
    }

    fn push(&mut self, draft: TimelineEventDraft) {
        self.event_seq += 1;
        self.timeline.push(TimelineEvent {
            id: format!("ev-{}", self.event_seq),
            timestamp: draft.timestamp,
            kind: draft.kind,
            title: draft.title,
            detail: draft.detail,
            file_path: draft.file_path,
            command: draft.command,
            exit_code: draft.exit_code,
        });
    }

    fn file_for(&mut self, path: &str, created: bool) -> usize {
        let clean = path.replace('\\', "/");
        let idx = match self.files.iter().position(|f| f.path == clean) {
            Some(i) => i,
            None => {
                self.files.push(RawFile::new(clean, created));
                self.files.len() - 1
            }
        };
        if created {
            self.files[idx].created = true;
        }
        idx
    }
}

fn add_lines(file: &mut RawFile, added: &str, removed: &str) {
    let mut lines: Vec<DiffLine> = Vec::new();
    if !removed.is_empty() {
        for line in removed.split('\n') {
            file.removed += 1;
            lines.push(DiffLine {
                kind: DiffLineKind::Del,
                text: line.to_string(),
                old_line: None,
                new_line: None,
            });
        }
    }
    if !added.is_empty() {
        for line in added.split('\n') {
            file.added += 1;
            file.added_lines.push(line.to_string());
            lines.push(DiffLine {
                kind: DiffLineKind::Add,
                text: line.to_string(),
                old_line: None,
                new_line: None,
            });
        }
    }
    if !lines.is_empty() {
        lines.truncate(120);
        file.hunks.push(DiffHunk {
            header: format!("@@ {} @@", file.path),
            lines,
        });
    }
}

fn handle_function_call(state: &mut ParseState, name: &str, args: &Value, ts: &str) {
    let target = pick(&[
        s(args.get("file_path")),
        s(args.get("absolute_path")),
        s(args.get("path")),
    ])
    .to_string();
    state.last_command_was_test = false;

    if name == "write_file" && !target.is_empty() {
        let idx = state.file_for(&target, true);
        add_lines(&mut state.files[idx], s(args.get("content")), "");
        let path = state.files[idx].path.clone();
        let mut ev = TimelineEventDraft::new(
            ts.to_string(),
            TimelineEventKind::FileEdit,
            format!("Wrote {}", path),
        );
        ev.file_path = Some(path);
        state.push(ev);
    } else if (name == "replace" || name == "edit") && !target.is_empty() {
        let idx = state.file_for(&target, false);
        add_lines(
            &mut state.files[idx],
            s(args.get("new_string")),
            s(args.get("old_string")),
        );
        let path = state.files[idx].path.clone();
        let mut ev = TimelineEventDraft::new(
            ts.to_string(),
            TimelineEventKind::FileEdit,
            format!("Edited {}", path),
        );
        ev.file_path = Some(path);
        state.push(ev);
    } else if name == "run_shell_command" {
        let command = s(args.get("command")).to_string();
        let is_test = TEST_COMMAND.is_match(&command);
        state.commands_run += 1;
        state.last_command_was_test = is_test;
        if is_test {
            state.tests_run += 1;
        }
        let fl = first_line(&command);
        let title = pick(&[s(args.get("description")), fl.as_str(), name]);
        let mut ev = TimelineEventDraft::new(
            ts.to_string(),
            if is_test {
                TimelineEventKind::TestRun
            } else {
                TimelineEventKind::Command
            },
            truncate(title, 80),
        );
        ev.command = Some(truncate(&command, 300));
        state.push(ev);
    } else if name == "read_file" || name == "read_many_files" {
        state.files_analyzed += 1;
        if !target.is_empty() {
            state.push(TimelineEventDraft::new(
                ts.to_string(),
                TimelineEventKind::FileRead,
                format!("Read {}", target.replace('\\', "/")),
            ));
        }
    } else if name == "search_file_content" || name == "glob" || name == "list_directory" {
        let query = pick(&[s(args.get("pattern")), s(args.get("path")), name]);
        state.push(TimelineEventDraft::new(
            ts.to_string(),
            TimelineEventKind::Analysis,
            format!("Searched: {}", truncate(query, 70)),
        ));
    } else if name == "google_web_search" || name == "web_fetch" {
        let query = pick(&[
            s(args.get("query")),
            s(args.get("prompt")),
            s(args.get("url")),
        ]);
        state.push(TimelineEventDraft::new(
            ts.to_string(),
            TimelineEventKind::Note,
            format!("Web: {}", truncate(query, 70)),
        ));
    } else if !name.is_empty() {
        state.push(TimelineEventDraft::new(
            ts.to_string(),
            TimelineEventKind::Note,
            format!("Used tool {}", name),
        ));
    }
}

/// The error text of a function response: `error` when it is a string, else `error.message`.
fn error_text(error: Option<&Value>) -> String {
    let direct = error.and_then(Value::as_str).unwrap_or("");
    if !direct.is_empty() {
        return direct.to_string();
    }
    error
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn record_error(state: &mut ParseState, error: Option<&Value>, ts: &str) {
    let err_text = error_text(error);
    if err_text.is_empty() {
        return;
    }
    state.errors += 1;
    if state.last_command_was_test {
        state.tests_failed += 1;
    }
    let mut ev = TimelineEventDraft::new(
        ts.to_string(),
        TimelineEventKind::Error,
        "Tool call failed".to_string(),
    );
    ev.detail = Some(truncate(&err_text, 500));
    state.push(ev);
}

fn handle_function_response(state: &mut ParseState, response: Option<&Value>, ts: &str) {
    match response {
        Some(v) if v.is_object() => record_error(state, v.get("error"), ts),
        _ => {}
    }
}

fn consume_content(state: &mut ParseState, role: &str, parts: Option<&Value>, ts: &str) {
    let empty_parts: Vec<Value> = Vec::new();
    let parts = parts.and_then(Value::as_array).unwrap_or(&empty_parts);
    for part in parts {
        if !part.is_object() {
            continue;
        }
        let text = s(part.get("text")).trim();
        if !text.is_empty() {
            if role == "user" && state.original_prompt.is_empty() && !text.starts_with('<') {
                state.original_prompt = truncate(text, 1500);
            } else if role == "model" && utf16_len(text) > 40 {
                state.last_agent_text = text.to_string();
            }
            continue;
        }
        let empty_args = Value::Object(Default::default());
        if let Some(fc) = part.get("functionCall").filter(|v| v.is_object()) {
            let args = fc
                .get("args")
                .filter(|v| v.is_object())
                .unwrap_or(&empty_args);
            handle_function_call(state, s(fc.get("name")), args, ts);
        }
        if let Some(fr) = part.get("functionResponse").filter(|v| v.is_object()) {
            handle_function_response(state, fr.get("response"), ts);
        }
    }
}

pub fn parse_gemini_file(raw: &str, fallback_id: &str, mtime_iso_ts: &str) -> Option<AgentSession> {
    let parsed: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return None,
    };

    let mut state = ParseState::new();
    let mut session_id = fallback_id.to_string();
    let mut first_ts = String::new();
    let mut last_ts = String::new();

    let contents: Option<&Vec<Value>> = parsed
        .as_array()
        .or_else(|| parsed.get("history").and_then(Value::as_array))
        .or_else(|| parsed.get("checkpoint").and_then(Value::as_array));

    if let Some(contents) = contents {
        // checkpoint*.json — raw Gemini API Content[] (no timestamps)
        for c in contents {
            if c.is_object() {
                consume_content(&mut state, s(c.get("role")), c.get("parts"), mtime_iso_ts);
            }
        }
    } else {
        // chats/*.json — session file with a messages array
        let messages = parsed.get("messages").and_then(Value::as_array)?;
        let sid = s(parsed.get("sessionId"));
        if !sid.is_empty() {
            session_id = sid.to_string();
        }
        for m in messages {
            if !m.is_object() {
                continue;
            }
            let m_ts = s(m.get("timestamp"));
            let ts = if m_ts.is_empty() {
                mtime_iso_ts.to_string()
            } else {
                m_ts.to_string()
            };
            if !m_ts.is_empty() {
                if first_ts.is_empty() {
                    first_ts = ts.clone();
                }
                last_ts = ts.clone();
            }
            if m.get("parts").map(Value::is_array).unwrap_or(false) {
                let role = {
                    let r = s(m.get("role"));
                    if !r.is_empty() {
                        r.to_string()
                    } else if s(m.get("type")) == "user" {
                        "user".to_string()
                    } else {
                        "model".to_string()
                    }
                };
                consume_content(&mut state, &role, m.get("parts"), &ts);
                continue;
            }
            let kind = pick(&[s(m.get("type")), s(m.get("role"))]).to_string();
            let text = pick(&[s(m.get("content")), s(m.get("text"))])
                .trim()
                .to_string();
            if (kind == "user" || kind == "human")
                && !text.is_empty()
                && state.original_prompt.is_empty()
            {
                state.original_prompt = truncate(&text, 1500);
            } else if utf16_len(&text) > 40 && kind != "user" && kind != "human" {
                state.last_agent_text = text.clone();
            }
            let empty_tool_calls: Vec<Value> = Vec::new();
            let empty_args = Value::Object(Default::default());
            for tc in m
                .get("toolCalls")
                .and_then(Value::as_array)
                .unwrap_or(&empty_tool_calls)
            {
                let args = tc
                    .get("args")
                    .filter(|v| v.is_object())
                    .unwrap_or(&empty_args);
                handle_function_call(&mut state, s(tc.get("name")), args, &ts);
                let truthy = |key: &str| tc.get(key).map(is_truthy).unwrap_or(false);
                if truthy("result") || truthy("error") {
                    // Inline result: treat it as a function response
                    record_error(&mut state, tc.get("error"), &ts);
                }
            }
        }
    }

    if state.original_prompt.is_empty() {
        return None;
    }

    let paths: Vec<String> = state.files.iter().map(|f| f.path.clone()).collect();
    let root_dir = if paths.is_empty() {
        String::new()
    } else {
        ascend_to_project_root(&common_root(&paths))
    };
    // paths (and hunk headers built from them) become project-relative once a common root is known
    let files: Vec<RawFile> = state
        .files
        .iter()
        .map(|f| {
            let path = relative_to(&root_dir, &f.path);
            let hunks = f
                .hunks
                .iter()
                .map(|h| DiffHunk {
                    header: format!("@@ {} @@", path),
                    lines: h.lines.clone(),
                })
                .collect();
            RawFile {
                path: path.clone(),
                created: f.created,
                deleted: f.deleted,
                added: f.added,
                removed: f.removed,
                hunks,
                added_lines: f.added_lines.clone(),
            }
        })
        .collect();

    let duration = if !first_ts.is_empty() && !last_ts.is_empty() {
        duration_minutes(&first_ts, &last_ts)
    } else {
        1
    };

    let model = {
        let m = s(parsed.get("model"));
        if m.is_empty() {
            "unknown model"
        } else {
            m
        }
    };
    let branch = if root_dir.is_empty() {
        None
    } else {
        git_branch_of(&root_dir)
    }
    .filter(|b| !b.is_empty())
    .unwrap_or_else(|| "(no git)".to_string());

    normalize(RawSession {
        session_id,
        timestamp: if first_ts.is_empty() {
            mtime_iso_ts.to_string()
        } else {
            first_ts.clone()
        },
        agent: "gemini-cli".to_string(),
        agent_label: "Gemini CLI".to_string(),
        model: model.to_string(),
        project_name: if root_dir.is_empty() {
            "(unknown)".to_string()
        } else {
            basename(&root_dir)
        },
        project_path: if root_dir.is_empty() {
            None
        } else {
            Some(root_dir.clone())
        },
        branch,
        original_prompt: state.original_prompt,
        declared_goal: None,
        initial_plan: Vec::new(),
        outcome: truncate(&state.last_agent_text, 700),
        timeline: state.timeline,
        files,
        files_analyzed: state.files_analyzed,
        commands_run: state.commands_run,
        tests_run: state.tests_run,
        tests_failed: state.tests_failed,
        errors: state.errors,
        duration_minutes: duration,
    })
}

/// Final path component of the forward-slash roots this adapter produces.
fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default()
}

static CHECKPOINT_NAME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^checkpoint.*\.json$").unwrap());
static CHATS_DIR: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(^|[/\\])chats$").unwrap());

fn is_candidate(dir: &str, name: &str) -> bool {
    if !name.ends_with(".json") {
        return false;
    }
    CHECKPOINT_NAME.is_match(name) || CHATS_DIR.is_match(dir)
}

struct Candidate {
    path: String,
    mtime_ms: f64,
}

fn walk_json(dir: &Path, depth: i32, out: &mut Vec<Candidate>) {
    if depth < 0 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let full = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = match std::fs::metadata(&full) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            walk_json(&full, depth - 1, out);
        } else if is_candidate(&dir.to_string_lossy(), &name) && meta.len() > 100 {
            out.push(Candidate {
                path: full.to_string_lossy().to_string(),
                mtime_ms: mtime_ms(&meta),
            });
        }
    }
}

pub struct GeminiCliAdapter {
    cache: Mutex<ParseCache>,
}

impl GeminiCliAdapter {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(ParseCache::default()),
        }
    }
}

impl Default for GeminiCliAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl SessionAdapter for GeminiCliAdapter {
    fn id(&self) -> &'static str {
        "gemini-cli"
    }

    fn label(&self) -> &'static str {
        "Gemini CLI"
    }

    fn collect(&self, config: &AdapterConfig, max_sessions: usize) -> CollectResult {
        let dir = config.dir.clone().unwrap_or_default();
        let mut status = self.base_status(config, dir.clone());
        if !config.enabled {
            return CollectResult {
                sessions: Vec::new(),
                status,
            };
        }

        let mut candidates: Vec<Candidate> = Vec::new();
        walk_json(Path::new(&dir), 3, &mut candidates);
        if candidates.is_empty() {
            status.hint =
                Some("No Gemini CLI conversations found — save one with /chat save or enable --checkpointing.".to_string());
            return CollectResult {
                sessions: Vec::new(),
                status,
            };
        }
        candidates.sort_by(|a, b| {
            b.mtime_ms
                .partial_cmp(&a.mtime_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let seen: std::collections::HashSet<String> =
            candidates.iter().map(|c| c.path.clone()).collect();
        let mut cache = lock(&self.cache);
        cache.retain_paths(&seen);
        let mut sessions: Vec<AgentSession> = Vec::new();
        for c in &candidates {
            if sessions.len() >= max_sessions {
                break;
            }
            if let Some(cached) = cache.get(&c.path, c.mtime_ms) {
                if let Some(session) = cached {
                    sessions.push(session.clone());
                }
                continue;
            }
            let fallback = {
                let name = Path::new(&c.path)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default();
                name.strip_suffix(".json").unwrap_or(&name).to_string()
            };
            let session = std::fs::read_to_string(&c.path)
                .ok()
                .and_then(|raw| parse_gemini_file(&raw, &fallback, &mtime_iso(c.mtime_ms)));
            cache.put(c.path.clone(), c.mtime_ms, session.clone());
            if let Some(session) = session {
                sessions.push(session);
            }
        }

        status.sessions = sessions.len();
        status.hint = if sessions.is_empty() {
            Some(
                "Gemini CLI conversations found but none contained reviewable file changes."
                    .to_string(),
            )
        } else {
            None
        };
        CollectResult { sessions, status }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixture::{assert_session, read as read_fixture};
    use super::*;

    const MTIME: &str = "2026-03-10T12:00:00.000Z";

    fn check(input_rel: &str, expected_rel: &str, fallback_id: &str) {
        let session = parse_gemini_file(&read_fixture(input_rel), fallback_id, MTIME)
            .expect("parser returned a session");
        assert_session(&session, expected_rel);
    }

    #[test]
    fn parses_checkpoint_fixture() {
        check(
            "gemini-cli/checkpoint-basic.json",
            "gemini-cli/expected/checkpoint-basic.json",
            "checkpoint-basic",
        );
    }

    #[test]
    fn parses_session_fixture() {
        check(
            "gemini-cli/session-basic.json",
            "gemini-cli/expected/session-basic.json",
            "session-basic",
        );
    }
}
