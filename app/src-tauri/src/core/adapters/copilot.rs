// VS Code Copilot adapter — reads VS Code Copilot Chat
// sessions (ask/edit/agent mode) from workspaceStorage/<hash>/chatSessions/.
// File edits arrive as `textEditGroup` response parts (uri + inserted text
// with real line numbers); tool calls as `toolInvocationSerialized` parts.
// The transcript stores no "before" content, so hunks are insert-shaped —
// the git enrichment replaces them with real working-tree diffs at serve
// time. Defensive by design: undocumented internal format, guarded field
// access, malformed files skipped.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};

use super::super::domain::{
    AgentSession, DiffHunk, DiffLine, DiffLineKind, RawFile, RawSession, TimelineEvent,
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

/// Path of a URI value — {fsPath} first, then {external|path}.
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

/// Chat session files are plain JSON or JSONL whose first line wraps the snapshot in {kind:0, v}.
fn unwrap_session(raw: &str) -> Option<Value> {
    let parsed: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => {
            let nl = raw.find('\n')?;
            serde_json::from_str(&raw[..nl]).ok()?
        }
    };
    if !parsed.is_object() {
        return None;
    }
    if parsed.get("kind").and_then(Value::as_i64) == Some(0) {
        if let Some(v) = parsed.get("v").filter(|v| v.is_object()) {
            return Some(v.clone());
        }
    }
    Some(parsed)
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
            // Markdown chunks have no kind — they are the assistant's prose
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
                let mut hunk_lines: Vec<DiffLine> = Vec::new();
                let mut header_line: i64 = 0;
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
                        if header_line == 0 {
                            header_line = start_line;
                        }
                        let cleaned = text.replace('\r', "");
                        let ins_lines: Vec<&str> = cleaned.split('\n').collect();
                        files[idx].added += ins_lines.len() as i64;
                        for (i, t) in ins_lines.iter().enumerate() {
                            files[idx].added_lines.push((*t).to_string());
                            hunk_lines.push(DiffLine {
                                kind: DiffLineKind::Add,
                                text: (*t).to_string(),
                                old_line: None,
                                new_line: Some(start_line + i as i64),
                            });
                        }
                    }
                }
                if !hunk_lines.is_empty() {
                    hunk_lines.truncate(120);
                    let header = format!("@@ {}:{} @@", files[idx].path, header_line);
                    files[idx].hunks.push(DiffHunk {
                        header,
                        lines: hunk_lines,
                    });
                }
                // Streaming produces many parts per file — one timeline event per stretch
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
                push_event(&mut timeline, &mut event_seq, ev);
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

    normalize(RawSession {
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
        tests_failed: 0,
        errors: 0,
        // Without a creationDate the span is unknown (not "since 1970").
        duration_minutes: if created_ms != 0.0 {
            (((last_ms - created_ms) / 60000.0).round() as i64).max(1)
        } else {
            1
        },
    })
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
                    "No VS Code workspace storage found — is VS Code installed on this machine?"
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
                    "Copilot chats found but none edited files — ask/plan conversations are not reviewable."
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
}
