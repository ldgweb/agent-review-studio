// OpenAI Codex CLI adapter: parses rollout JSONL files from ~/.codex/sessions.
// Defensive by design: Codex's format evolves, so every field access is
// guarded and unrecognized records are skipped rather than failing the scan.

use super::super::domain::*;
use super::super::normalize::{duration_minutes, first_line, normalize, truncate, TEST_COMMAND};
use super::{mtime_iso, mtime_ms, AdapterConfig, CollectResult, ParseCache, SessionAdapter};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::sync::Mutex;

const MIN_FILE_BYTES: u64 = 2_000;

static PATCH_FILE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\*\*\* (Add|Update|Delete) File: (.+)$").unwrap());
static SHELL_TOOL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)shell|exec|command").unwrap());
static HUNK_HEADER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^@@.*@@\s?").unwrap());

/// Truthy: a non-empty string, a non-zero number, true, or any array/object.
fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// Length in UTF-16 code units (what the fixture contract counts).
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The value when it is a string, '' otherwise.
fn pstr<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// A command part as text: strings as-is, numbers and booleans formatted,
/// null as "null", arrays and objects as ''.
fn scalar_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(_) | Value::Object(_) => String::new(),
    }
}

/// Final component of a forward-slash path, trailing slashes ignored (the
/// path itself when nothing but slashes remain).
fn basename(p: &str) -> String {
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return p.to_string();
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

fn push_event(timeline: &mut Vec<TimelineEvent>, seq: &mut u64, d: TimelineEventDraft) {
    *seq += 1;
    timeline.push(TimelineEvent {
        id: format!("ev-{}", *seq),
        timestamp: d.timestamp,
        kind: d.kind,
        title: d.title,
        detail: d.detail,
        file_path: d.file_path,
        command: d.command,
        exit_code: d.exit_code,
    });
}

fn flush_hunk(file_map: &mut [RawFile], current: Option<usize>, hunk_lines: &mut Vec<DiffLine>) {
    if let Some(i) = current {
        if !hunk_lines.is_empty() {
            let lines: Vec<DiffLine> = hunk_lines.iter().take(120).cloned().collect();
            let header = format!("@@ {} @@", file_map[i].path);
            file_map[i].hunks.push(DiffHunk { header, lines });
            hunk_lines.clear();
        }
    }
}

/// One file section of an apply_patch envelope: (path, added, removed).
type PatchSection = (String, Vec<String>, Vec<String>);

/// Parse Codex apply_patch envelopes: "*** Update File: x" / "*** Add File: y"
/// / "*** Delete File: z". `sections` receives each file section's changed
/// lines (the attribution input).
fn parse_apply_patch(
    input: &str,
    file_map: &mut Vec<RawFile>,
    sections: &mut Vec<PatchSection>,
) -> Vec<String> {
    let mut touched: Vec<String> = Vec::new();
    let mut current: Option<usize> = None;
    let mut hunk_lines: Vec<DiffLine> = Vec::new();

    for line in input.split('\n') {
        if let Some(m) = PATCH_FILE_RE.captures(line) {
            flush_hunk(file_map, current, &mut hunk_lines);
            let action = m.get(1).map(|x| x.as_str()).unwrap_or("");
            let path = m
                .get(2)
                .map(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .replace('\\', "/");
            let idx = match file_map.iter().position(|f| f.path == path) {
                Some(i) => i,
                None => {
                    file_map.push(RawFile::new(path.clone(), action == "Add"));
                    file_map.len() - 1
                }
            };
            // The last action on a path wins: a deletion is recorded as such,
            // a later Add/Update of the same path brings it back.
            if action == "Add" {
                file_map[idx].created = true;
            }
            file_map[idx].deleted = action == "Delete";
            current = Some(idx);
            sections.push((path.clone(), Vec::new(), Vec::new()));
            touched.push(path);
            continue;
        }
        let idx = match current {
            Some(i) if !line.starts_with("***") => i,
            _ => continue,
        };
        let section = sections.last_mut();
        if let Some(rest) = line.strip_prefix('+') {
            if let Some((_, added, _)) = section {
                added.push(rest.to_string());
            }
            file_map[idx].added += 1;
            file_map[idx].added_lines.push(rest.to_string());
            hunk_lines.push(DiffLine {
                kind: DiffLineKind::Add,
                text: rest.to_string(),
                old_line: None,
                new_line: None,
            });
        } else if let Some(rest) = line.strip_prefix('-') {
            if let Some((_, _, removed)) = section {
                removed.push(rest.to_string());
            }
            file_map[idx].removed += 1;
            hunk_lines.push(DiffLine {
                kind: DiffLineKind::Del,
                text: rest.to_string(),
                old_line: None,
                new_line: None,
            });
        } else if line.starts_with(' ') || line.starts_with("@@") {
            let stripped = HUNK_HEADER_RE.replace(line, "").to_string();
            let text = stripped.strip_prefix(' ').unwrap_or(&stripped).to_string();
            hunk_lines.push(DiffLine {
                kind: DiffLineKind::Context,
                text,
                old_line: None,
                new_line: None,
            });
        }
    }
    flush_hunk(file_map, current, &mut hunk_lines);
    touched
}

/// A new human prompt starts a turn. Codex logs a prompt both as an event
/// and as a response item, so a repeat of the last prompt is the same turn.
fn push_turn(turns: &mut Vec<SessionTurn>, prompt: &str, ts: &str) -> bool {
    let prompt = truncate(prompt.trim(), 500);
    if prompt.is_empty() || turns.last().is_some_and(|t| t.prompt == prompt) {
        return false;
    }
    turns.push(SessionTurn {
        index: turns.len() as u32,
        prompt,
        timestamp: ts.to_string(),
    });
    true
}

/// The recorded edits of one applied patch, attributed to the current turn.
fn record_patch(
    sections: Vec<PatchSection>,
    edits: &mut Vec<RecordedEdit>,
    turns: &[SessionTurn],
    rationale: &str,
    ts: &str,
    timeline: &[TimelineEvent],
) {
    let event_id = timeline.last().map(|e| e.id.clone());
    for (path, added, removed) in sections {
        let mut edit = RecordedEdit {
            seq: 0,
            path,
            added: Vec::new(),
            removed: Vec::new(),
            turn: turns.last().map(|t| t.index).unwrap_or(0),
            rationale: rationale.to_string(),
            timestamp: ts.to_string(),
            event_id: event_id.clone(),
        };
        added.iter().for_each(|l| edit.push_added(l));
        removed.iter().for_each(|l| edit.push_removed(l));
        if edit.line_count() > 0 {
            edit.seq = edits.len() as u32 + 1;
            edits.push(edit);
        }
    }
}

pub fn parse_codex_rollout(raw: &str, fallback_id: &str, mtime_iso: &str) -> Option<AgentSession> {
    let mut records: Vec<Value> = Vec::new();
    for line in raw.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            records.push(v);
        }
    }
    if records.is_empty() {
        return None;
    }

    let mut session_id = fallback_id.to_string();
    let mut cwd = String::new();
    let mut branch = "(no git)".to_string();
    let mut model = "unknown model".to_string();
    let mut original_prompt = String::new();
    let mut last_agent_text = String::new();
    let mut first_ts = String::new();
    let mut last_ts = String::new();
    let mut commands_run: i64 = 0;
    let mut tests_run: i64 = 0;
    let mut tests_failed: i64 = 0;
    let mut errors: i64 = 0;
    let mut timeline: Vec<TimelineEvent> = Vec::new();
    // Insertion-ordered path → RawFile map.
    let mut file_map: Vec<RawFile> = Vec::new();
    // call_id → command
    let mut call_by_id: Vec<(String, String)> = Vec::new();
    let mut event_seq: u64 = 0;
    let mut turns: Vec<SessionTurn> = Vec::new();
    let mut edits: Vec<RecordedEdit> = Vec::new();
    // The agent's latest message in the current turn (edit rationale)
    let mut rationale = String::new();

    for rec in &records {
        let ts_raw = rec.get("timestamp");
        let ts: String = match ts_raw.and_then(Value::as_str) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => mtime_iso.to_string(),
        };
        if ts_raw.map(is_truthy).unwrap_or(false) {
            if first_ts.is_empty() {
                first_ts = ts.clone();
            }
            last_ts = ts.clone();
        }
        let p: &Value = rec.get("payload").unwrap_or(&Value::Null);
        let rtype = rec.get("type").and_then(Value::as_str).unwrap_or("");

        if rtype == "session_meta" {
            if !pstr(p, "id").is_empty() {
                session_id = pstr(p, "id").to_string();
            }
            if !pstr(p, "cwd").is_empty() {
                cwd = pstr(p, "cwd").to_string();
            }
            if let Some(git) = p.get("git") {
                let b = git.get("branch").and_then(Value::as_str).unwrap_or("");
                if !b.is_empty() {
                    branch = b.to_string();
                }
            }
            continue;
        }
        if rtype == "turn_context" {
            if !pstr(p, "model").is_empty() {
                model = pstr(p, "model").to_string();
            }
            continue;
        }
        if rtype == "event_msg" {
            let kind = pstr(p, "type");
            let message = pstr(p, "message");
            if kind == "user_message" && !message.is_empty() && original_prompt.is_empty() {
                original_prompt = truncate(message, 1500);
            }
            if kind == "user_message" && push_turn(&mut turns, message, &ts) {
                rationale.clear();
            }
            if kind == "agent_message" && !message.is_empty() {
                last_agent_text = message.to_string();
                rationale = truncate(message.trim(), 600);
            }
            continue;
        }
        if rtype != "response_item" {
            continue;
        }

        let item_type = pstr(p, "type");
        if item_type == "message" {
            let role = pstr(p, "role");
            let content = p
                .get("content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let text = content
                .iter()
                .map(|c| c.get("text").and_then(Value::as_str).unwrap_or(""))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if role == "user"
                && !text.is_empty()
                && original_prompt.is_empty()
                && !text.starts_with('<')
            {
                original_prompt = truncate(&text, 1500);
            }
            if role == "user" && !text.starts_with('<') && push_turn(&mut turns, &text, &ts) {
                rationale.clear();
            }
            if role == "assistant" && utf16_len(text.trim()) > 40 {
                last_agent_text = text.trim().to_string();
            }
            if role == "assistant" && !text.trim().is_empty() {
                rationale = truncate(text.trim(), 600);
            }
        } else if item_type == "reasoning" {
            // opaque; skip
        } else if item_type == "function_call" {
            let name = pstr(p, "name");
            let args: Value = {
                let s = pstr(p, "arguments");
                let s = if s.is_empty() { "{}" } else { s };
                serde_json::from_str(s).unwrap_or_else(|_| Value::Object(Default::default()))
            };
            if name == "apply_patch" || name == "applyPatch" {
                let input = {
                    let i = pstr(&args, "input");
                    if i.is_empty() {
                        pstr(&args, "patch").to_string()
                    } else {
                        i.to_string()
                    }
                };
                let mut sections = Vec::new();
                let touched = parse_apply_patch(&input, &mut file_map, &mut sections);
                let joined = touched.join(", ");
                let target = if joined.is_empty() {
                    "(files)".to_string()
                } else {
                    joined
                };
                push_event(
                    &mut timeline,
                    &mut event_seq,
                    TimelineEventDraft::new(
                        ts.clone(),
                        TimelineEventKind::FileEdit,
                        format!("Applied patch to {}", target),
                    ),
                );
                record_patch(sections, &mut edits, &turns, &rationale, &ts, &timeline);
            } else if SHELL_TOOL_RE.is_match(name) {
                let cmd_raw = args.get("command");
                let cmd_parts: Vec<String> = match cmd_raw {
                    Some(Value::Array(arr)) => arr.iter().map(scalar_string).collect(),
                    other => vec![other.and_then(|v| v.as_str()).unwrap_or("").to_string()],
                };
                let command = cmd_parts.join(" ");
                // Codex frequently edits files by running `apply_patch` through the
                // shell tool, with the patch body as another argument.
                let patch_idx = cmd_parts.iter().position(|c| c.contains("apply_patch"));
                let patch_body = cmd_parts
                    .iter()
                    .find(|c| c.contains("*** Begin Patch") || c.contains("*** Update File"));
                if let (Some(_), Some(body)) = (patch_idx, patch_body) {
                    let mut sections = Vec::new();
                    let touched = parse_apply_patch(body, &mut file_map, &mut sections);
                    let joined = touched.join(", ");
                    let target = if joined.is_empty() {
                        "(files)".to_string()
                    } else {
                        joined
                    };
                    push_event(
                        &mut timeline,
                        &mut event_seq,
                        TimelineEventDraft::new(
                            ts.clone(),
                            TimelineEventKind::FileEdit,
                            format!("Applied patch to {}", target),
                        ),
                    );
                    record_patch(sections, &mut edits, &turns, &rationale, &ts, &timeline);
                } else {
                    let is_test = TEST_COMMAND.is_match(&command);
                    commands_run += 1;
                    if is_test {
                        tests_run += 1;
                    }
                    let call_id = pstr(p, "call_id");
                    if !call_id.is_empty() {
                        match call_by_id.iter_mut().find(|(k, _)| k == call_id) {
                            Some(entry) => entry.1 = command.clone(),
                            None => call_by_id.push((call_id.to_string(), command.clone())),
                        }
                    }
                    let fl = first_line(&command);
                    let base = if fl.is_empty() { name } else { fl.as_str() };
                    let mut d = TimelineEventDraft::new(
                        ts.clone(),
                        if is_test {
                            TimelineEventKind::TestRun
                        } else {
                            TimelineEventKind::Command
                        },
                        truncate(base, 80),
                    );
                    d.command = Some(truncate(&command, 300));
                    push_event(&mut timeline, &mut event_seq, d);
                }
            } else {
                let shown = if name.is_empty() { "(unknown)" } else { name };
                push_event(
                    &mut timeline,
                    &mut event_seq,
                    TimelineEventDraft::new(
                        ts.clone(),
                        TimelineEventKind::Note,
                        format!("Used tool {}", shown),
                    ),
                );
            }
        } else if item_type == "function_call_output" {
            let out_raw = p.get("output");
            let mut out_text = String::new();
            let mut exit: Option<i64> = None;
            match out_raw {
                Some(Value::String(s)) => {
                    out_text = s.clone();
                    if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                        if let Some(o) = parsed.get("output").and_then(Value::as_str) {
                            out_text = o.to_string();
                        }
                        exit = parsed
                            .get("metadata")
                            .and_then(|m| m.get("exit_code"))
                            .and_then(Value::as_i64);
                    }
                }
                Some(v) if v.is_object() => {
                    out_text = v
                        .get("output")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    exit = v
                        .get("metadata")
                        .and_then(|m| m.get("exit_code"))
                        .and_then(Value::as_i64);
                }
                _ => {}
            }
            if let Some(exit_code) = exit {
                if exit_code != 0 {
                    errors += 1;
                    let call_id = pstr(p, "call_id");
                    let cmd = call_by_id
                        .iter()
                        .find(|(k, _)| k == call_id)
                        .map(|(_, c)| c.as_str())
                        .unwrap_or("");
                    if TEST_COMMAND.is_match(cmd) {
                        tests_failed += 1;
                    }
                    let mut d = TimelineEventDraft::new(
                        ts.clone(),
                        TimelineEventKind::Error,
                        format!("Command failed (exit {})", exit_code),
                    );
                    d.detail = Some(truncate(&out_text, 500));
                    d.exit_code = Some(exit_code);
                    push_event(&mut timeline, &mut event_seq, d);
                }
            }
        }
    }

    if original_prompt.is_empty() {
        return None;
    }
    let dur = if !first_ts.is_empty() && !last_ts.is_empty() {
        duration_minutes(&first_ts, &last_ts)
    } else {
        1
    };

    let mut session = normalize(RawSession {
        session_id,
        timestamp: if first_ts.is_empty() {
            mtime_iso.to_string()
        } else {
            first_ts.clone()
        },
        agent: "codex".to_string(),
        agent_label: "OpenAI Codex".to_string(),
        model,
        project_name: if cwd.is_empty() {
            "(unknown)".to_string()
        } else {
            basename(&cwd.replace('\\', "/"))
        },
        project_path: if cwd.is_empty() {
            None
        } else {
            Some(cwd.clone())
        },
        branch,
        original_prompt,
        declared_goal: None,
        initial_plan: Vec::new(),
        outcome: truncate(&last_agent_text, 700),
        timeline,
        files: file_map,
        files_analyzed: 0,
        commands_run,
        tests_run,
        tests_failed,
        errors,
        duration_minutes: dur,
    })?;
    session.turns = turns;
    cap_edits(&mut edits);
    session.edits = edits;
    Some(session)
}

fn walk_jsonl(dir: &std::path::Path, depth: i32, out: &mut Vec<(String, f64)>) {
    if depth < 0 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let full = entry.path();
        let st = match std::fs::metadata(&full) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if st.is_dir() {
            walk_jsonl(&full, depth - 1, out);
        } else {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".jsonl") && st.len() >= MIN_FILE_BYTES {
                out.push((full.to_string_lossy().to_string(), mtime_ms(&st)));
            }
        }
    }
}

pub struct CodexAdapter {
    cache: Mutex<ParseCache>,
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(ParseCache::default()),
        }
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionAdapter for CodexAdapter {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "OpenAI Codex"
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

        let mut candidates: Vec<(String, f64)> = Vec::new();
        walk_jsonl(std::path::Path::new(&dir), 4, &mut candidates);
        if candidates.is_empty() {
            status.hint = Some(
                "No Codex CLI sessions found. Is Codex installed on this machine?".to_string(),
            );
            return CollectResult {
                sessions: Vec::new(),
                status,
            };
        }
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let seen: std::collections::HashSet<String> =
            candidates.iter().map(|(p, _)| p.clone()).collect();
        let mut sessions: Vec<AgentSession> = Vec::new();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain_paths(&seen);
        for (path, ms) in candidates {
            if sessions.len() >= max_sessions {
                break;
            }
            if let Some(cached) = cache.get(&path, ms) {
                if let Some(s) = cached {
                    sessions.push(s.clone());
                }
                continue;
            }
            let file_name = std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let fallback_id = file_name
                .strip_suffix(".jsonl")
                .unwrap_or(&file_name)
                .to_string();
            let session = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| parse_codex_rollout(&raw, &fallback_id, &mtime_iso(ms)));
            cache.put(path, ms, session.clone());
            if let Some(s) = session {
                sessions.push(s);
            }
        }

        status.sessions = sessions.len();
        status.hint = if sessions.is_empty() {
            Some("Codex rollouts found but none contained reviewable file changes.".to_string())
        } else {
            None
        };
        CollectResult { sessions, status }
    }

    fn project_sessions(
        &self,
        config: &AdapterConfig,
        root: &str,
        limit: usize,
    ) -> Option<Vec<AgentSession>> {
        Some(super::collect_for_project(self, config, root, limit))
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::domain::ChangeType;
    use super::super::super::fixture::{assert_session, read as read_fixture};
    use super::parse_codex_rollout;

    #[test]
    fn delete_file_is_a_deleted_change() {
        let patch = "*** Begin Patch\n*** Delete File: src/old.ts\n*** Update File: src/a.ts\n@@\n-x\n+y // TODO\n*** End Patch";
        let call = serde_json::json!({
            "timestamp": "2026-03-10T10:00:01.000Z",
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "apply_patch",
                "arguments": serde_json::json!({ "input": patch }).to_string(),
            }
        });
        let raw = format!(
            "{}\n{}\n",
            serde_json::json!({
                "timestamp": "2026-03-10T10:00:00.000Z",
                "type": "event_msg",
                "payload": { "type": "user_message", "message": "Drop the old module" }
            }),
            call
        );
        let session = parse_codex_rollout(&raw, "t", "2026-03-10T12:00:00.000Z").expect("session");
        assert_eq!(session.files[0].path, "src/old.ts");
        assert_eq!(session.files[0].change_type, ChangeType::Deleted);
        assert_eq!(session.files[1].change_type, ChangeType::Modified);
        // Unanchored (no line numbers) local findings still fire.
        let todo = session
            .findings
            .iter()
            .find(|f| f["filePath"] == "src/a.ts")
            .expect("leftover TODO finding");
        assert!(todo.get("line").is_none());
    }

    #[test]
    fn parses_basic_fixture() {
        let raw = read_fixture("codex/basic.jsonl");
        let session =
            parse_codex_rollout(&raw, "basic", "2026-03-10T12:00:00.000Z").expect("session");
        assert_session(&session, "codex/expected/basic.json");
    }
}
