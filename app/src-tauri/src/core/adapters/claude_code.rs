// Claude Code adapter — parses JSONL transcripts from ~/.claude/projects.
// The richest source: prompts, plan (TodoWrite), tool calls with structured
// patches, command outputs and errors.

use super::super::domain::*;
use super::super::normalize::{duration_minutes, first_line, normalize, truncate, TEST_COMMAND};
use super::super::paths::fold_case;
use super::{mtime_iso, mtime_ms, AdapterConfig, CollectResult, ParseCache, SessionAdapter};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

const MIN_FILE_BYTES: u64 = 5_000;
const DETAIL_MAX: usize = 600;
const HUNKS_PER_FILE: usize = 4;
const HUNK_LINES_MAX: usize = 60;

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

/// Drop the first UTF-16 code unit (a diff line's +/- marker); '' when there
/// is at most one.
fn drop_first_unit(s: &str) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    if units.len() <= 1 {
        String::new()
    } else {
        String::from_utf16_lossy(&units[1..])
    }
}

/// Length in UTF-16 code units (what the fixture contract counts).
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// A scalar as text: strings as-is, numbers and booleans formatted; '' when
/// missing, null, an array or an object.
fn scalar_or_empty(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(_) => String::new(),
    }
}

/// String content, or the joined `text` blocks of an array.
fn text_of_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .map(|b| b.get("text").and_then(Value::as_str).unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn rel_path(file_path: &str, cwd: &str) -> String {
    let norm = file_path.replace('\\', "/");
    let norm_cwd_full = cwd.replace('\\', "/");
    let norm_cwd = norm_cwd_full.strip_suffix('/').unwrap_or(&norm_cwd_full);
    if !norm_cwd.is_empty()
        && norm.len() > norm_cwd.len()
        && norm.as_bytes().get(norm_cwd.len()) == Some(&b'/')
        && fold_case(&norm[..norm_cwd.len()]) == fold_case(norm_cwd)
    {
        norm[norm_cwd.len() + 1..].to_string()
    } else {
        norm
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

pub fn parse_claude_transcript(raw: &str, file_mtime_iso: &str) -> Option<AgentSession> {
    let mut entries: Vec<Value> = Vec::new();
    for line in raw.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            entries.push(v);
        }
    }
    if entries.is_empty() {
        return None;
    }

    let main: Vec<&Value> = entries
        .iter()
        .filter(|e| !e.get("isSidechain").map(is_truthy).unwrap_or(false))
        .collect();
    let session_id = main
        .iter()
        .find_map(|e| {
            e.get("sessionId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })?
        .to_string();

    let cwd = main
        .iter()
        .find_map(|e| {
            e.get("cwd")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("")
        .to_string();
    let branch = {
        let b = main
            .iter()
            .find(|e| e.get("gitBranch").is_some())
            .and_then(|e| e.get("gitBranch"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if b.is_empty() {
            "(no git)".to_string()
        } else {
            b.to_string()
        }
    };
    let model = main
        .iter()
        .find_map(|e| {
            if e.get("type").and_then(Value::as_str) != Some("assistant") {
                return None;
            }
            e.get("message")
                .and_then(|m| m.get("model"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("unknown model")
        .to_string();
    let declared_goal: Option<String> = entries.iter().rev().find_map(|e| {
        if e.get("type").and_then(Value::as_str) != Some("summary") {
            return None;
        }
        e.get("summary")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    });

    let prompt_entry = main.iter().find(|e| {
        if e.get("type").and_then(Value::as_str) != Some("user") {
            return false;
        }
        let message = match e.get("message") {
            Some(m) if is_truthy(m) => m,
            _ => return false,
        };
        if let Some(origin) = e.get("origin") {
            if is_truthy(origin) && origin.get("kind").and_then(Value::as_str) != Some("human") {
                return false;
            }
        }
        let text = text_of_content(message.get("content"));
        let text = text.trim();
        !text.is_empty() && !text.starts_with('<') && !text.starts_with("Caveat:")
    });
    let original_prompt = truncate(
        prompt_entry
            .map(|e| text_of_content(e.get("message").and_then(|m| m.get("content"))))
            .unwrap_or_default()
            .trim(),
        1500,
    );
    if original_prompt.is_empty() {
        return None;
    }

    // tool_use id → (name, input)
    let mut tool_use_by_id: HashMap<String, (String, Value)> = HashMap::new();
    let mut timeline: Vec<TimelineEvent> = Vec::new();
    // Insertion-ordered rel-path → RawFile map.
    let mut file_map: Vec<RawFile> = Vec::new();
    let mut files_read: HashSet<String> = HashSet::new();
    let mut initial_plan: Vec<String> = Vec::new();
    let mut commands_run: i64 = 0;
    let mut tests_run: i64 = 0;
    let mut tests_failed: i64 = 0;
    let mut errors: i64 = 0;
    let mut last_assistant_text = String::new();
    let mut first_ts = String::new();
    let mut last_ts = String::new();
    let mut event_seq: u64 = 0;

    for entry in &main {
        let ts: Option<&str> = entry.get("timestamp").and_then(Value::as_str);
        if let Some(t) = ts {
            if !t.is_empty() {
                if first_ts.is_empty() {
                    first_ts = t.to_string();
                }
                last_ts = t.to_string();
            }
        }
        // Fall back to the file mtime when the record has no timestamp
        let event_ts: String = ts
            .map(str::to_string)
            .unwrap_or_else(|| file_mtime_iso.to_string());

        let entry_type = entry.get("type").and_then(Value::as_str);
        let content = entry.get("message").and_then(|m| m.get("content"));

        if entry_type == Some("assistant") {
            if let Some(blocks) = content.and_then(Value::as_array) {
                for block in blocks {
                    let btype = block.get("type").and_then(Value::as_str);
                    if btype == Some("text") {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            let trimmed = text.trim();
                            if utf16_len(trimmed) > 40 {
                                last_assistant_text = trimmed.to_string();
                            }
                        }
                    }
                    if btype != Some("tool_use") {
                        continue;
                    }
                    let tool_id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let tool_name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    tool_use_by_id.insert(tool_id, (tool_name.clone(), input.clone()));
                    let file_path = input
                        .get("file_path")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty());
                    let rel: Option<String> = file_path.map(|fp| rel_path(fp, &cwd));

                    match tool_name.as_str() {
                        "Read" => {
                            if let Some(r) = &rel {
                                files_read.insert(r.clone());
                            }
                            let mut d = TimelineEventDraft::new(
                                event_ts.clone(),
                                TimelineEventKind::FileRead,
                                format!(
                                    "Read {}",
                                    rel.clone().unwrap_or_else(|| "(file)".to_string())
                                ),
                            );
                            d.file_path = rel.clone();
                            push_event(&mut timeline, &mut event_seq, d);
                        }
                        "Edit" | "Write" | "NotebookEdit" => {
                            let verb = if tool_name == "Write" {
                                "Wrote"
                            } else {
                                "Edited"
                            };
                            let mut d = TimelineEventDraft::new(
                                event_ts.clone(),
                                TimelineEventKind::FileEdit,
                                format!(
                                    "{} {}",
                                    verb,
                                    rel.clone().unwrap_or_else(|| "(file)".to_string())
                                ),
                            );
                            d.file_path = rel.clone();
                            push_event(&mut timeline, &mut event_seq, d);
                        }
                        "Bash" | "PowerShell" => {
                            let command =
                                input.get("command").and_then(Value::as_str).unwrap_or("");
                            let description = input
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let is_test = TEST_COMMAND.is_match(command);
                            commands_run += 1;
                            if is_test {
                                tests_run += 1;
                            }
                            let title = if description.is_empty() {
                                truncate(&first_line(command), 80)
                            } else {
                                description.to_string()
                            };
                            let mut d = TimelineEventDraft::new(
                                event_ts.clone(),
                                if is_test {
                                    TimelineEventKind::TestRun
                                } else {
                                    TimelineEventKind::Command
                                },
                                title,
                            );
                            d.command = Some(truncate(command, 300));
                            push_event(&mut timeline, &mut event_seq, d);
                        }
                        "TodoWrite" => {
                            let todos = input
                                .get("todos")
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default();
                            let steps: Vec<String> = todos
                                .iter()
                                .map(|t| t.get("content").and_then(Value::as_str).unwrap_or(""))
                                .filter(|s| !s.is_empty())
                                .take(12)
                                .map(str::to_string)
                                .collect();
                            if initial_plan.is_empty() && !steps.is_empty() {
                                initial_plan = steps.clone();
                            }
                            let detail = steps
                                .iter()
                                .enumerate()
                                .map(|(i, s)| format!("{}. {}", i + 1, s))
                                .collect::<Vec<_>>()
                                .join("\n");
                            let mut d = TimelineEventDraft::new(
                                event_ts.clone(),
                                TimelineEventKind::Plan,
                                format!("Updated plan ({} steps)", steps.len()),
                            );
                            d.detail = if detail.is_empty() {
                                None
                            } else {
                                Some(detail)
                            };
                            push_event(&mut timeline, &mut event_seq, d);
                        }
                        "Grep" | "Glob" => {
                            let pattern =
                                input.get("pattern").and_then(Value::as_str).unwrap_or("");
                            push_event(
                                &mut timeline,
                                &mut event_seq,
                                TimelineEventDraft::new(
                                    event_ts.clone(),
                                    TimelineEventKind::Analysis,
                                    format!("Searched: {}", truncate(pattern, 60)),
                                ),
                            );
                        }
                        "Task" | "Agent" => {
                            let description = input
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or("subagent task");
                            push_event(
                                &mut timeline,
                                &mut event_seq,
                                TimelineEventDraft::new(
                                    event_ts.clone(),
                                    TimelineEventKind::Analysis,
                                    format!("Delegated to subagent: {}", truncate(description, 70)),
                                ),
                            );
                        }
                        _ => {
                            push_event(
                                &mut timeline,
                                &mut event_seq,
                                TimelineEventDraft::new(
                                    event_ts.clone(),
                                    TimelineEventKind::Note,
                                    format!("Used tool {}", tool_name),
                                ),
                            );
                        }
                    }
                }
            }
        }

        if entry_type == Some("user") {
            if let Some(blocks) = content.and_then(Value::as_array) {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let tool_use_id = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let tool = tool_use_by_id.get(tool_use_id);
                    if block.get("is_error").map(is_truthy).unwrap_or(false) {
                        errors += 1;
                        if let Some((name, tinput)) = tool {
                            if (name == "Bash" || name == "PowerShell")
                                && TEST_COMMAND.is_match(&scalar_or_empty(tinput.get("command")))
                            {
                                tests_failed += 1;
                            }
                        }
                        let title = match tool {
                            Some((name, _)) => format!("{} failed", name),
                            None => "Tool call failed".to_string(),
                        };
                        let rc = block.get("content");
                        let detail_text = {
                            let t = text_of_content(rc);
                            if t.is_empty() {
                                scalar_or_empty(rc)
                            } else {
                                t
                            }
                        };
                        let mut d = TimelineEventDraft::new(
                            event_ts.clone(),
                            TimelineEventKind::Error,
                            title,
                        );
                        d.detail = Some(truncate(&detail_text, DETAIL_MAX));
                        push_event(&mut timeline, &mut event_seq, d);
                    }
                }

                let tr = entry.get("toolUseResult").filter(|v| v.is_object());
                let tr_file_path = tr.and_then(|tr| {
                    tr.get("filePath")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                });
                if let (Some(tr), Some(fp)) = (tr, tr_file_path) {
                    let rel = rel_path(fp, &cwd);
                    let idx = match file_map.iter().position(|f| f.path == rel) {
                        Some(i) => i,
                        None => {
                            file_map.push(RawFile::new(rel.clone(), false));
                            file_map.len() - 1
                        }
                    };
                    let is_create = tr.get("type").and_then(Value::as_str) == Some("create");
                    if is_create {
                        file_map[idx].created = true;
                    }
                    let patches = tr
                        .get("structuredPatch")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for patch in &patches {
                        let lines: Vec<String> = patch
                            .get("lines")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .map(|l| l.as_str().unwrap_or("").to_string())
                                    .collect()
                            })
                            .unwrap_or_default();
                        let mut added: i64 = 0;
                        let mut removed: i64 = 0;
                        for l in &lines {
                            if l.starts_with('+') {
                                added += 1;
                                file_map[idx].added_lines.push(drop_first_unit(l));
                            } else if l.starts_with('-') {
                                removed += 1;
                            }
                        }
                        file_map[idx].added += added;
                        file_map[idx].removed += removed;
                        if file_map[idx].hunks.len() < HUNKS_PER_FILE {
                            let old_start =
                                patch.get("oldStart").and_then(Value::as_i64).unwrap_or(0);
                            let old_lines_n =
                                patch.get("oldLines").and_then(Value::as_i64).unwrap_or(0);
                            let new_start =
                                patch.get("newStart").and_then(Value::as_i64).unwrap_or(0);
                            let new_lines_n =
                                patch.get("newLines").and_then(Value::as_i64).unwrap_or(0);
                            let mut old_ln = old_start;
                            let mut new_ln = new_start;
                            let mut hunk_lines: Vec<DiffLine> = Vec::new();
                            for l in lines.iter().take(HUNK_LINES_MAX) {
                                let kind = if l.starts_with('+') {
                                    DiffLineKind::Add
                                } else if l.starts_with('-') {
                                    DiffLineKind::Del
                                } else {
                                    DiffLineKind::Context
                                };
                                let line = DiffLine {
                                    kind,
                                    text: drop_first_unit(l),
                                    old_line: if kind != DiffLineKind::Add {
                                        Some(old_ln)
                                    } else {
                                        None
                                    },
                                    new_line: if kind != DiffLineKind::Del {
                                        Some(new_ln)
                                    } else {
                                        None
                                    },
                                };
                                if kind != DiffLineKind::Add {
                                    old_ln += 1;
                                }
                                if kind != DiffLineKind::Del {
                                    new_ln += 1;
                                }
                                hunk_lines.push(line);
                            }
                            file_map[idx].hunks.push(DiffHunk {
                                header: format!(
                                    "@@ -{},{} +{},{} @@",
                                    old_start, old_lines_n, new_start, new_lines_n
                                ),
                                lines: hunk_lines,
                            });
                        }
                    }
                    if is_create && patches.is_empty() {
                        if let Some(content_str) = tr.get("content").and_then(Value::as_str) {
                            let clean = content_str.replace('\r', "");
                            let lines: Vec<&str> = clean.split('\n').collect();
                            file_map[idx].added = file_map[idx].added.max(lines.len() as i64);
                            for l in lines.iter().take(50) {
                                file_map[idx].added_lines.push((*l).to_string());
                            }
                            let shown: Vec<&str> = lines.iter().take(400).copied().collect();
                            let extra = if lines.len() > shown.len() {
                                format!(" (first {} shown)", shown.len())
                            } else {
                                String::new()
                            };
                            file_map[idx].hunks = vec![DiffHunk {
                                header: format!("@@ new file — {} lines{} @@", lines.len(), extra),
                                lines: shown
                                    .iter()
                                    .enumerate()
                                    .map(|(i, text)| DiffLine {
                                        kind: DiffLineKind::Add,
                                        text: (*text).to_string(),
                                        old_line: None,
                                        new_line: Some(i as i64 + 1),
                                    })
                                    .collect(),
                            }];
                        }
                    }
                }
            }
        }
    }

    let dur = if !first_ts.is_empty() && !last_ts.is_empty() {
        duration_minutes(&first_ts, &last_ts)
    } else {
        1
    };

    normalize(RawSession {
        session_id,
        timestamp: if first_ts.is_empty() {
            file_mtime_iso.to_string()
        } else {
            first_ts.clone()
        },
        agent: "claude-code".to_string(),
        agent_label: "Claude Code".to_string(),
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
        declared_goal,
        initial_plan,
        outcome: truncate(&last_assistant_text, 700),
        timeline,
        files: file_map,
        files_analyzed: files_read.len() as i64,
        commands_run,
        tests_run,
        tests_failed,
        errors,
        duration_minutes: dur,
    })
}

pub struct ClaudeCodeAdapter {
    cache: Mutex<ParseCache>,
}

impl ClaudeCodeAdapter {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(ParseCache::default()),
        }
    }
}

impl Default for ClaudeCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionAdapter for ClaudeCodeAdapter {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn label(&self) -> &'static str {
        "Claude Code"
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
        match std::fs::read_dir(&dir) {
            Err(_) => {
                status.hint = Some(format!("Claude Code transcripts not found at {}.", dir));
                return CollectResult {
                    sessions: Vec::new(),
                    status,
                };
            }
            Ok(projects) => {
                for project in projects.flatten() {
                    let project_dir = project.path();
                    let meta = match std::fs::metadata(&project_dir) {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    if !meta.is_dir() {
                        continue;
                    }
                    let names = match std::fs::read_dir(&project_dir) {
                        Ok(n) => n,
                        Err(_) => continue,
                    };
                    for name in names.flatten() {
                        let fname = name.file_name().to_string_lossy().to_string();
                        if !fname.ends_with(".jsonl") {
                            continue;
                        }
                        if let Ok(st) = std::fs::metadata(name.path()) {
                            if st.len() >= MIN_FILE_BYTES {
                                candidates.push((
                                    name.path().to_string_lossy().to_string(),
                                    mtime_ms(&st),
                                ));
                            }
                        }
                    }
                }
            }
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
            let session = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| parse_claude_transcript(&raw, &mtime_iso(ms)));
            cache.put(path, ms, session.clone());
            if let Some(s) = session {
                sessions.push(s);
            }
        }

        status.sessions = sessions.len();
        status.hint = if sessions.is_empty() {
            Some("No reviewable Claude Code sessions found yet.".to_string())
        } else {
            None
        };
        CollectResult { sessions, status }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixture::{assert_session, read as read_fixture};
    use super::parse_claude_transcript;

    #[test]
    fn parses_basic_fixture() {
        let raw = read_fixture("claude-code/basic.jsonl");
        let session = parse_claude_transcript(&raw, "2026-03-10T12:00:00.000Z").expect("session");
        assert_session(&session, "claude-code/expected/basic.json");
    }
}
