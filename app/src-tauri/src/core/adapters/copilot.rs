// VS Code Copilot adapter: reads VS Code Copilot Chat sessions
// (ask/edit/agent mode) from workspaceStorage/<hash>/chatSessions/.
// File edits arrive as `textEditGroup` response parts (uri + inserted text
// with real line numbers); tool calls as `toolInvocationSerialized` parts.
// Line-level edits come from the tool calls the model made
// (result.metadata.toolCallRounds): apply_patch envelopes, replace_string /
// multi_replace_string old and new text, create_file content. Defensive by design: undocumented internal format,
// guarded field access, malformed files skipped.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};

use super::super::domain::{
    cap_edits, AgentSession, RawFile, RawSession, RecordedEdit, SessionTurn, TimelineEvent,
    TimelineEventDraft, TimelineEventKind,
};
use super::super::normalize::{first_line, normalize, truncate, TEST_COMMAND};
use super::project_root::{ascend_to_project_root, common_root, git_branch_of, relative_to};
use super::{mtime_iso, mtime_ms, AdapterConfig, CollectResult, ParseCache, SessionAdapter};

/// The string value or ''.
fn s(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or("")
}

/// The finite number value or 0.
fn n(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(0.0)
}

/// Length in UTF-16 code units (what the fixture contract counts).
fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn push_event(timeline: &mut Vec<TimelineEvent>, seq: &mut usize, draft: TimelineEventDraft) {
    *seq += 1;
    timeline.push(TimelineEvent {
        id: format!("ev-{}", seq),
        timestamp: draft.timestamp,
        kind: draft.kind,
        title: draft.title,
        detail: draft.detail,
        file_path: draft.file_path,
        command: draft.command,
        exit_code: draft.exit_code,
    });
}

static DRIVE_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^/[A-Za-z]:/").unwrap());

/// file:///c%3A/dev/app/src/x.ts → c:/dev/app/src/x.ts
fn uri_to_path(uri: &str) -> String {
    let decoded = match percent_encoding::percent_decode_str(uri).decode_utf8() {
        Ok(cow) => cow.to_string(),
        Err(_) => uri.to_string(), // keep raw
    };
    let stripped = decoded.strip_prefix("file://").unwrap_or(&decoded);
    let mut p = stripped.replace('\\', "/");
    if DRIVE_PREFIX.is_match(&p) {
        p.remove(0); // windows drive letter
    }
    p
}

/// Path of a URI value: {fsPath} first, then {external|path}.
fn path_of_uri(uri: Option<&Value>) -> String {
    let uri = match uri.filter(|u| u.is_object()) {
        Some(u) => u,
        None => return String::new(),
    };
    let fs_path = s(uri.get("fsPath"));
    if !fs_path.is_empty() {
        return uri_to_path(fs_path);
    }
    let external = {
        let e = s(uri.get("external"));
        if e.is_empty() {
            s(uri.get("path"))
        } else {
            e
        }
    };
    if external.is_empty() {
        String::new()
    } else {
        uri_to_path(external)
    }
}

/// invocationMessage / pastTenseMessage are plain strings or {value} markdown.
fn message_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(text)) => text.clone(),
        Some(obj) if obj.is_object() => s(obj.get("value")).to_string(),
        _ => String::new(),
    }
}

/// Final path component of the paths this adapter produces ('' when none).
fn basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Chat session files are plain JSON (older VS Code), or a JSONL operation
/// log (VS Code 1.10x and later): the first line `{kind:0, v}` holds the
/// initial state, then `{kind:1, k, v}` sets the value at path `k`,
/// `{kind:2, k, v, i?}` appends the items of `v` to the array at `k` (after
/// cutting it to `i` items when `i` is given), `{kind:3, k}` deletes it.
/// The log is replayed into the session it describes.
fn unwrap_session(raw: &str) -> Option<Value> {
    if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
        if !parsed.is_object() {
            return None;
        }
        if parsed.get("kind").and_then(Value::as_i64) == Some(0) {
            return parsed.get("v").filter(|v| v.is_object()).cloned();
        }
        return Some(parsed);
    }
    let mut lines = raw.split('\n').filter(|l| !l.trim().is_empty());
    let first: Value = serde_json::from_str(lines.next()?).ok()?;
    let mut state = match first.get("kind").and_then(Value::as_i64) {
        Some(0) => first.get("v").filter(|v| v.is_object())?.clone(),
        _ if first.is_object() => return Some(first),
        _ => return None,
    };
    for line in lines {
        let Ok(op) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let path = op
            .get("k")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let value = op.get("v").cloned().unwrap_or(Value::Null);
        match op.get("kind").and_then(Value::as_i64) {
            Some(0) if value.is_object() => state = value,
            Some(1) => {
                if let Some(slot) = slot_at(&mut state, &path) {
                    *slot = value;
                }
            }
            Some(2) => {
                if let Some(slot) = slot_at(&mut state, &path) {
                    if !slot.is_array() {
                        *slot = Value::Array(Vec::new());
                    }
                    if let Some(items) = slot.as_array_mut() {
                        if let Some(cut) = op.get("i").and_then(Value::as_u64) {
                            items.truncate(cut as usize);
                        }
                        match value {
                            Value::Array(new) => items.extend(new),
                            other => items.push(other),
                        }
                    }
                }
            }
            Some(3) => {
                if let Some((last, parent)) = path.split_last() {
                    if let Some(slot) = slot_at(&mut state, parent) {
                        match (slot, last) {
                            (Value::Object(map), Value::String(key)) => {
                                map.remove(key);
                            }
                            (Value::Array(items), Value::Number(n)) => {
                                if let Some(i) = n.as_u64().filter(|i| (*i as usize) < items.len())
                                {
                                    items.remove(i as usize);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Some(state)
}

/// The value at `path` (object keys and array indexes), creating missing
/// object keys on the way. None when the path runs through a scalar or past
/// the end of an array.
fn slot_at<'a>(root: &'a mut Value, path: &[Value]) -> Option<&'a mut Value> {
    let mut cur = root;
    for step in path {
        cur = match step {
            Value::String(key) => {
                if cur.is_null() {
                    *cur = Value::Object(Default::default());
                }
                cur.as_object_mut()?
                    .entry(key.clone())
                    .or_insert(Value::Null)
            }
            Value::Number(n) => cur.as_array_mut()?.get_mut(n.as_u64()? as usize)?,
            _ => return None,
        };
    }
    Some(cur)
}

pub fn parse_copilot_session(
    raw: &str,
    fallback_id: &str,
    mtime_iso_str: &str,
) -> Option<AgentSession> {
    let session = unwrap_session(raw)?;
    let empty_requests: Vec<Value> = Vec::new();
    let requests = session
        .get("requests")
        .and_then(Value::as_array)
        .unwrap_or(&empty_requests);
    if requests.is_empty() {
        return None;
    }

    let original_prompt = truncate(
        s(requests[0].get("message").and_then(|m| m.get("text"))).trim(),
        1500,
    );
    if original_prompt.is_empty() {
        return None;
    }

    let created_ms = n(session.get("creationDate"));
    let base_iso = if created_ms != 0.0 {
        mtime_iso(created_ms)
    } else {
        mtime_iso_str.to_string()
    };
    let last_ms = {
        let l = n(session.get("lastMessageDate"));
        if l != 0.0 {
            l
        } else {
            created_ms
        }
    };

    // Pass 1: the project root comes from every edited file's absolute path
    let mut known_paths: Vec<String> = Vec::new();
    for request in requests {
        let empty_parts: Vec<Value> = Vec::new();
        for part in request
            .get("response")
            .and_then(Value::as_array)
            .unwrap_or(&empty_parts)
        {
            if s(part.get("kind")) != "textEditGroup" {
                continue;
            }
            let p = path_of_uri(part.get("uri"));
            if !p.is_empty() && !known_paths.contains(&p) {
                known_paths.push(p);
            }
        }
    }
    let root = if known_paths.is_empty() {
        String::new()
    } else {
        ascend_to_project_root(&common_root(&known_paths))
    };

    // Insertion order preserved with a Vec keyed by path.
    let mut files: Vec<RawFile> = Vec::new();
    let mut created_paths: HashSet<String> = HashSet::new();
    let mut timeline: Vec<TimelineEvent> = Vec::new();
    let mut event_seq: usize = 0;

    let mut model = String::new();
    let mut last_agent_text = String::new();
    let mut commands_run: i64 = 0;
    let mut tests_run: i64 = 0;
    let mut files_analyzed: i64 = 0;
    let mut initial_plan: Vec<String> = Vec::new();
    let mut tests_failed: i64 = 0;
    let mut turns: Vec<SessionTurn> = Vec::new();
    let mut edits: Vec<RecordedEdit> = Vec::new();

    for (qi, request) in requests.iter().enumerate() {
        let req_ms = n(request.get("timestamp"));
        let req_iso = if req_ms != 0.0 {
            mtime_iso(req_ms)
        } else {
            base_iso.clone()
        };
        if !s(request.get("modelId")).is_empty() {
            model = s(request.get("modelId")).to_string();
        }
        let user_text = s(request.get("message").and_then(|m| m.get("text")))
            .trim()
            .to_string();
        if !user_text.is_empty() {
            turns.push(SessionTurn {
                index: turns.len() as u32,
                prompt: truncate(&user_text, 500),
                timestamp: req_iso.clone(),
            });
        }
        if qi > 0 && !user_text.is_empty() {
            push_event(
                &mut timeline,
                &mut event_seq,
                TimelineEventDraft::new(
                    req_iso.clone(),
                    TimelineEventKind::Note,
                    format!("User: {}", truncate(&first_line(&user_text), 70)),
                ),
            );
        }

        let mut request_text = String::new();
        let mut last_edited_path = String::new();
        let empty_parts: Vec<Value> = Vec::new();
        for part in request
            .get("response")
            .and_then(Value::as_array)
            .unwrap_or(&empty_parts)
        {
            // Markdown chunks have no kind: they are the assistant's prose
            let kind_is_string = part.get("kind").map(Value::is_string).unwrap_or(false);
            if !kind_is_string {
                if let Some(value) = part.get("value").and_then(Value::as_str) {
                    request_text.push_str(value);
                }
                continue;
            }
            let kind = s(part.get("kind"));

            if kind == "textEditGroup" {
                let abs_path = path_of_uri(part.get("uri"));
                if abs_path.is_empty() {
                    continue;
                }
                let path = relative_to(&root, &abs_path);
                let idx = match files.iter().position(|f| f.path == path) {
                    Some(i) => i,
                    None => {
                        files.push(RawFile::new(
                            path.clone(),
                            created_paths.contains(&abs_path),
                        ));
                        files.len() - 1
                    }
                };
                let empty_edits: Vec<Value> = Vec::new();
                let edits = part
                    .get("edits")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_edits);
                for group in edits {
                    let empty_group: Vec<Value> = Vec::new();
                    for edit in group.as_array().unwrap_or(&empty_group) {
                        let range = edit.get("range");
                        let start_line = {
                            let v = n(range.and_then(|r| r.get("startLineNumber"))) as i64;
                            v.max(1)
                        };
                        let end_line = {
                            let v = n(range.and_then(|r| r.get("endLineNumber"))) as i64;
                            v.max(start_line)
                        };
                        let text = s(edit.get("text"));
                        files[idx].removed += end_line - start_line;
                        if text.is_empty() {
                            continue;
                        }
                        let cleaned = text.replace('\r', "");
                        let ins_lines: Vec<&str> = cleaned.split('\n').collect();
                        files[idx].added += ins_lines.len() as i64;
                        for t in &ins_lines {
                            files[idx].added_lines.push((*t).to_string());
                        }
                    }
                }
                // Streaming produces many parts per file: one timeline event per stretch
                if files[idx].path != last_edited_path {
                    let path = files[idx].path.clone();
                    let mut ev = TimelineEventDraft::new(
                        req_iso.clone(),
                        TimelineEventKind::FileEdit,
                        format!("Edited {}", path),
                    );
                    ev.file_path = Some(path.clone());
                    push_event(&mut timeline, &mut event_seq, ev);
                    last_edited_path = path;
                }
                continue;
            }

            if kind != "toolInvocationSerialized" {
                continue;
            }
            let tool_id = s(part.get("toolId"));
            let empty_specific = Value::Object(Default::default());
            let specific = part.get("toolSpecificData").unwrap_or(&empty_specific);
            let title = {
                let past = message_of(part.get("pastTenseMessage"));
                if past.is_empty() {
                    message_of(part.get("invocationMessage"))
                } else {
                    past
                }
            };

            if tool_id == "run_in_terminal" || tool_id == "copilot_runInTerminal" {
                let command = {
                    let original = s(specific.get("commandLine").and_then(|c| c.get("original")));
                    if original.is_empty() {
                        s(specific.get("command")).to_string()
                    } else {
                        original.to_string()
                    }
                };
                let is_test = TEST_COMMAND.is_match(&command);
                commands_run += 1;
                if is_test {
                    tests_run += 1;
                }
                let fl = first_line(&command);
                let event_title = if !fl.is_empty() {
                    fl
                } else if !title.is_empty() {
                    title.clone()
                } else {
                    tool_id.to_string()
                };
                let mut ev = TimelineEventDraft::new(
                    req_iso.clone(),
                    if is_test {
                        TimelineEventKind::TestRun
                    } else {
                        TimelineEventKind::Command
                    },
                    truncate(&event_title, 80),
                );
                if !command.is_empty() {
                    ev.command = Some(truncate(&command, 300));
                }
                let exit = specific
                    .get("terminalCommandState")
                    .and_then(|st| st.get("exitCode"))
                    .and_then(Value::as_i64);
                ev.exit_code = exit;
                push_event(&mut timeline, &mut event_seq, ev);
                if let Some(code) = exit.filter(|c| *c != 0) {
                    if is_test {
                        tests_failed += 1;
                    }
                    let mut err = TimelineEventDraft::new(
                        req_iso.clone(),
                        TimelineEventKind::Error,
                        format!("Command failed (exit {})", code),
                    );
                    err.detail = Some(truncate(
                        s(specific
                            .get("terminalCommandOutput")
                            .and_then(|o| o.get("text"))),
                        500,
                    ));
                    err.exit_code = Some(code);
                    push_event(&mut timeline, &mut event_seq, err);
                }
            } else if tool_id == "copilot_readFile" {
                files_analyzed += 1;
                let event_title = if title.is_empty() {
                    "Read file".to_string()
                } else {
                    title.clone()
                };
                push_event(
                    &mut timeline,
                    &mut event_seq,
                    TimelineEventDraft::new(
                        req_iso.clone(),
                        TimelineEventKind::FileRead,
                        truncate(&event_title, 80),
                    ),
                );
            } else if tool_id == "copilot_createFile" {
                let abs_path = path_of_uri(specific.get("uri"));
                if !abs_path.is_empty() {
                    created_paths.insert(abs_path.clone());
                    let rel = relative_to(&root, &abs_path);
                    if let Some(existing) = files.iter_mut().find(|f| f.path == rel) {
                        existing.created = true;
                    }
                }
            } else if tool_id == "manage_todo_list" {
                let empty_todos: Vec<Value> = Vec::new();
                let todo_list = specific
                    .get("todoList")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_todos);
                if initial_plan.is_empty() && !todo_list.is_empty() {
                    initial_plan = todo_list
                        .iter()
                        .filter_map(|t| {
                            let text = {
                                let title = s(t.get("title"));
                                if title.is_empty() {
                                    s(t.get("description"))
                                } else {
                                    title
                                }
                            };
                            if text.is_empty() {
                                None
                            } else {
                                Some(truncate(text, 200))
                            }
                        })
                        .take(12)
                        .collect();
                }
            } else if tool_id == "copilot_findTextInFiles"
                || tool_id == "copilot_findFiles"
                || tool_id == "copilot_listDirectory"
                || tool_id == "copilot_searchCodebase"
                || tool_id == "copilot_getErrors"
            {
                let event_title = if title.is_empty() {
                    tool_id.to_string()
                } else {
                    title.clone()
                };
                push_event(
                    &mut timeline,
                    &mut event_seq,
                    TimelineEventDraft::new(
                        req_iso.clone(),
                        TimelineEventKind::Analysis,
                        truncate(&event_title, 80),
                    ),
                );
            }
        }
        let trimmed = request_text.trim();
        if utf16_len(trimmed) > 40 {
            last_agent_text = trimmed.to_string();
        }
        let turn = turns.last().map(|t| t.index).unwrap_or(0);
        record_tool_edits(request, &root, turn, &req_iso, &timeline, &mut edits);
    }

    if files.is_empty() {
        return None;
    }

    let session_id = {
        let sid = s(session.get("sessionId"));
        if sid.is_empty() {
            fallback_id.to_string()
        } else {
            sid.to_string()
        }
    };
    let declared_goal = {
        let title = s(session.get("customTitle"));
        if title.is_empty() {
            None
        } else {
            Some(title.to_string())
        }
    };
    let branch = if root.is_empty() {
        None
    } else {
        git_branch_of(&root)
    }
    .filter(|b| !b.is_empty())
    .unwrap_or_else(|| "(no git)".to_string());

    let mut session = normalize(RawSession {
        session_id,
        timestamp: base_iso,
        agent: "copilot".to_string(),
        agent_label: "GitHub Copilot".to_string(),
        model: if model.is_empty() {
            "unknown model".to_string()
        } else {
            model
        },
        project_name: if root.is_empty() {
            "(unknown)".to_string()
        } else {
            basename(&root)
        },
        project_path: if root.is_empty() {
            None
        } else {
            Some(root.clone())
        },
        branch,
        original_prompt: original_prompt.clone(),
        declared_goal,
        initial_plan,
        outcome: truncate(&last_agent_text, 700),
        timeline,
        files,
        files_analyzed,
        commands_run,
        tests_run,
        tests_failed,
        // Without a creationDate the span is unknown (not "since 1970").
        duration_minutes: if created_ms != 0.0 {
            (((last_ms - created_ms) / 60000.0).round() as i64).max(1)
        } else {
            1
        },
    })?;
    session.turns = turns;
    cap_edits(&mut edits);
    session.edits = edits;
    Some(session)
}

static PATCH_FILE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\*\*\* (Add|Update|Delete) File: (.+)$").unwrap());

/// The recorded edits of one request, from the tool calls of its rounds:
/// each round's text before the calls is the rationale. A call whose
/// arguments do not parse is skipped.
fn record_tool_edits(
    request: &Value,
    root: &str,
    turn: u32,
    ts: &str,
    timeline: &[TimelineEvent],
    edits: &mut Vec<RecordedEdit>,
) {
    let empty: Vec<Value> = Vec::new();
    let rounds = request
        .get("result")
        .and_then(|r| r.get("metadata"))
        .and_then(|m| m.get("toolCallRounds"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    for round in rounds {
        let rationale = {
            let said = s(round.get("response")).trim();
            if said.is_empty() {
                s(round.get("thinking").and_then(|t| t.get("text"))).trim()
            } else {
                said
            }
        };
        let rationale = truncate(rationale, 600);
        for call in round
            .get("toolCalls")
            .and_then(Value::as_array)
            .unwrap_or(&empty)
        {
            let Ok(args) = serde_json::from_str::<Value>(s(call.get("arguments"))) else {
                continue;
            };
            // (absolute path, added lines, removed lines)
            let mut changes: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
            match s(call.get("name")) {
                "apply_patch" => {
                    for line in s(args.get("input")).split('\n') {
                        let line = line.strip_suffix('\r').unwrap_or(line);
                        if let Some(m) = PATCH_FILE_RE.captures(line) {
                            changes.push((m[2].trim().to_string(), Vec::new(), Vec::new()));
                        } else if line.starts_with("***") || line.starts_with("@@") {
                            continue;
                        } else if let Some(last) = changes.last_mut() {
                            if let Some(rest) = line.strip_prefix('+') {
                                last.1.push(rest.to_string());
                            } else if let Some(rest) = line.strip_prefix('-') {
                                last.2.push(rest.to_string());
                            }
                        }
                    }
                }
                "replace_string_in_file" => changes.push(replacement(&args)),
                "multi_replace_string_in_file" => {
                    for one in args
                        .get("replacements")
                        .and_then(Value::as_array)
                        .unwrap_or(&empty)
                    {
                        changes.push(replacement(one));
                    }
                }
                "create_file" => changes.push((
                    s(args.get("filePath")).to_string(),
                    s(args.get("content")).lines().map(str::to_string).collect(),
                    Vec::new(),
                )),
                _ => {}
            }
            for (abs, added, removed) in changes {
                if abs.is_empty() {
                    continue;
                }
                let path = relative_to(root, &abs.replace('\\', "/"));
                let event_id = timeline
                    .iter()
                    .rev()
                    .find(|e| e.file_path.as_deref() == Some(path.as_str()))
                    .map(|e| e.id.clone());
                let mut edit = RecordedEdit {
                    seq: 0,
                    path,
                    added: Vec::new(),
                    removed: Vec::new(),
                    turn,
                    rationale: rationale.clone(),
                    timestamp: ts.to_string(),
                    event_id,
                };
                added.iter().for_each(|l| edit.push_added(l));
                removed.iter().for_each(|l| edit.push_removed(l));
                if edit.line_count() > 0 {
                    edit.seq = edits.len() as u32 + 1;
                    edits.push(edit);
                }
            }
        }
    }
}

/// One old/new string replacement: (path, new lines, old lines).
fn replacement(args: &Value) -> (String, Vec<String>, Vec<String>) {
    let lines = |key: &str| -> Vec<String> {
        s(args.get(key))
            .replace('\r', "")
            .lines()
            .map(str::to_string)
            .collect()
    };
    (
        s(args.get("filePath")).to_string(),
        lines("newString"),
        lines("oldString"),
    )
}

pub struct CopilotAdapter {
    cache: Mutex<ParseCache>,
}

impl CopilotAdapter {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(ParseCache::default()),
        }
    }
}

impl Default for CopilotAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl SessionAdapter for CopilotAdapter {
    fn id(&self) -> &'static str {
        "copilot"
    }

    fn label(&self) -> &'static str {
        "VS Code Copilot"
    }

    /// NO filesystem watch: VS Code churns workspaceStorage constantly while
    /// open. The scanner's slow poll tick covers this source instead.
    fn watch_targets(&self, _config: &AdapterConfig) -> Vec<super::WatchTarget> {
        Vec::new()
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

        let workspaces = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                status.hint = Some(
                    "No VS Code workspace storage found. Is VS Code installed on this machine?"
                        .to_string(),
                );
                return CollectResult {
                    sessions: Vec::new(),
                    status,
                };
            }
        };

        let mut candidates: Vec<(String, f64)> = Vec::new();
        for ws in workspaces.flatten() {
            let sessions_dir = ws.path().join("chatSessions");
            let names = match std::fs::read_dir(&sessions_dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in names.flatten() {
                let path = entry.path();
                let is_session = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e == "json" || e == "jsonl")
                    .unwrap_or(false);
                if !is_session {
                    continue;
                }
                if let Ok(meta) = entry.metadata() {
                    candidates.push((path.to_string_lossy().to_string(), mtime_ms(&meta)));
                }
            }
        }
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let seen: std::collections::HashSet<String> =
            candidates.iter().map(|(p, _)| p.clone()).collect();
        let mut cache = lock(&self.cache);
        cache.retain_paths(&seen);
        let mut sessions: Vec<AgentSession> = Vec::new();
        let mut chats_seen: usize = 0;
        for (path, mtime) in &candidates {
            if sessions.len() >= max_sessions {
                break;
            }
            let session = match cache.get(path, *mtime) {
                Some(cached) => cached.clone(),
                None => {
                    let parsed = std::fs::read_to_string(path).ok().and_then(|raw| {
                        let name = basename(path);
                        let name = name.trim_end_matches(".jsonl").trim_end_matches(".json");
                        parse_copilot_session(&raw, name, &mtime_iso(*mtime))
                    });
                    cache.put(path.clone(), *mtime, parsed.clone());
                    parsed
                }
            };
            chats_seen += 1;
            if let Some(session) = session {
                sessions.push(session);
            }
        }

        status.sessions = sessions.len();
        if sessions.is_empty() {
            status.hint = Some(
                if chats_seen > 0 {
                    "Copilot chats found, but none edited files. Ask and plan conversations have nothing to review."
                } else {
                    "No Copilot chat sessions found. Agent/edit-mode sessions appear here after Copilot changes files."
                }
                .to_string(),
            );
        }
        CollectResult { sessions, status }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixture::{assert_session, read as read_fixture};
    use super::*;

    #[test]
    fn parses_basic_fixture() {
        let raw = read_fixture("copilot/basic.json");
        let session = parse_copilot_session(&raw, "basic", "2026-03-10T12:00:00.000Z")
            .expect("parser returned a session");
        assert_session(&session, "copilot/expected/basic.json");
    }

    #[test]
    fn replays_the_operation_log_of_vscode_1145() {
        let raw = read_fixture("copilot/oplog-v1145.jsonl");
        let session = parse_copilot_session(&raw, "oplog", "2026-11-06T12:00:00.000Z")
            .expect("parser returned a session");
        assert_eq!(session.turns.len(), 2);
        assert_eq!(session.files.len(), 1);
        // apply_patch and multi_replace give line-level edits with the
        // round's text as the reason
        assert_eq!(session.edits.len(), 2);
        assert!(session.edits[0].rationale.contains("no upper bound"));
        assert_eq!(session.edits[1].turn, 1);
        assert_eq!(session.stats.tests_run, 2);
        assert_eq!(session.stats.tests_failed, 1);
        // `i` cut the streamed markdown before the full text was pushed
        assert!(
            !session.outcome.contains("bound; The"),
            "{}",
            session.outcome
        );
        assert_session(&session, "copilot/expected/oplog-v1145.json");
    }
}
