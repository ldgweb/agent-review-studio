// Claude Code adapter: parses JSONL transcripts from ~/.claude/projects.
// The richest source: prompts, plan (TodoWrite), tool calls with structured
// patches, command outputs and errors. Transcripts are append-only JSONL,
// so they are parsed incrementally: a grown file is read from where the
// last read stopped.

use super::super::domain::*;
use super::super::normalize::{duration_minutes, first_line, normalize, truncate, TEST_COMMAND};
use super::super::paths::{fold_case, is_within, path_eq};
use super::{mtime_iso, mtime_ms, AdapterConfig, CollectResult, SessionAdapter};
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

/// The human prompt in a user entry, when it is one (not a tool result, a
/// meta caveat, a command echo or an interruption notice).
fn human_prompt(entry: &Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let message = entry.get("message").filter(|m| is_truthy(m))?;
    if let Some(origin) = entry.get("origin") {
        if is_truthy(origin) && origin.get("kind").and_then(Value::as_str) != Some("human") {
            return None;
        }
    }
    let text = text_of_content(message.get("content"));
    let text = text.trim();
    if text.is_empty() || text.starts_with('<') || text.starts_with("Caveat:") {
        return None;
    }
    Some(text.to_string())
}

/// "Exit code 2" in a failed command's output; 1 when it does not say.
fn failed_exit_code(content: Option<&Value>) -> i64 {
    let text = {
        let t = text_of_content(content);
        if t.is_empty() {
            scalar_or_empty(content)
        } else {
            t
        }
    };
    text.split("Exit code ")
        .nth(1)
        .and_then(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .unwrap_or(1)
}

/// What a later tool result needs to know about its tool call.
struct ToolCall {
    name: String,
    /// The command of a Bash/PowerShell call (test detection)
    command: String,
    /// Timeline event the call produced (exit codes, edit attribution)
    event: Option<usize>,
    turn: u32,
    rationale: String,
    timestamp: String,
}

/// Streaming Claude Code transcript parser: feed it JSONL lines as they are
/// appended, take a session snapshot whenever needed. Fed the whole file it
/// produces exactly what a one-shot parse would.
pub struct ClaudeParser {
    any_entry: bool,
    session_id: Option<String>,
    cwd: Option<String>,
    branch: Option<String>,
    model: Option<String>,
    declared_goal: Option<String>,
    original_prompt: Option<String>,
    calls: HashMap<String, ToolCall>,
    timeline: Vec<TimelineEvent>,
    // Insertion-ordered rel-path → RawFile map.
    file_map: Vec<RawFile>,
    files_read: HashSet<String>,
    initial_plan: Vec<String>,
    commands_run: i64,
    tests_run: i64,
    tests_failed: i64,
    errors: i64,
    last_assistant_text: String,
    /// The agent's latest text in the current turn (edit rationale)
    rationale: String,
    first_ts: String,
    last_ts: String,
    event_seq: u64,
    turns: Vec<SessionTurn>,
    edits: Vec<RecordedEdit>,
    /// Timestamp for records that carry none (the file mtime when read)
    fallback_ts: String,
}

impl ClaudeParser {
    pub fn new(fallback_ts: &str) -> Self {
        Self {
            any_entry: false,
            session_id: None,
            cwd: None,
            branch: None,
            model: None,
            declared_goal: None,
            original_prompt: None,
            calls: HashMap::new(),
            timeline: Vec::new(),
            file_map: Vec::new(),
            files_read: HashSet::new(),
            initial_plan: Vec::new(),
            commands_run: 0,
            tests_run: 0,
            tests_failed: 0,
            errors: 0,
            last_assistant_text: String::new(),
            rationale: String::new(),
            first_ts: String::new(),
            last_ts: String::new(),
            event_seq: 0,
            turns: Vec::new(),
            edits: Vec::new(),
            fallback_ts: fallback_ts.to_string(),
        }
    }

    fn cwd(&self) -> &str {
        self.cwd.as_deref().unwrap_or("")
    }

    fn current_turn(&self) -> u32 {
        self.turns.last().map(|t| t.index).unwrap_or(0)
    }

    pub fn feed_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            return;
        };
        self.any_entry = true;
        if entry.get("type").and_then(Value::as_str) == Some("summary") {
            if let Some(summary) = entry
                .get("summary")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                self.declared_goal = Some(summary.to_string());
            }
        }
        if entry.get("isSidechain").map(is_truthy).unwrap_or(false) {
            return;
        }
        self.session_fields(&entry);
        self.main_entry(&entry);
    }

    /// The first-seen session-level fields of the main conversation.
    fn session_fields(&mut self, e: &Value) {
        let text = |key: &str| {
            e.get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        if self.session_id.is_none() {
            self.session_id = text("sessionId");
        }
        if self.cwd.is_none() {
            self.cwd = text("cwd");
        }
        if self.branch.is_none() {
            if let Some(b) = e.get("gitBranch") {
                let b = b.as_str().unwrap_or("");
                self.branch = Some(if b.is_empty() {
                    "(no git)".to_string()
                } else {
                    b.to_string()
                });
            }
        }
        if self.model.is_none() && e.get("type").and_then(Value::as_str) == Some("assistant") {
            self.model = e
                .get("message")
                .and_then(|m| m.get("model"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }
        if self.original_prompt.is_none() {
            if let Some(prompt) = human_prompt(e) {
                self.original_prompt = Some(truncate(&prompt, 1500));
            }
        }
    }

    fn push(&mut self, d: TimelineEventDraft) -> usize {
        push_event(&mut self.timeline, &mut self.event_seq, d);
        self.timeline.len() - 1
    }

    fn main_entry(&mut self, entry: &Value) {
        let ts: Option<&str> = entry.get("timestamp").and_then(Value::as_str);
        if let Some(t) = ts {
            if !t.is_empty() {
                if self.first_ts.is_empty() {
                    self.first_ts = t.to_string();
                }
                self.last_ts = t.to_string();
            }
        }
        // Fall back to the file mtime when the record has no timestamp
        let event_ts: String = ts
            .map(str::to_string)
            .unwrap_or_else(|| self.fallback_ts.clone());

        let entry_type = entry.get("type").and_then(Value::as_str);
        let content = entry.get("message").and_then(|m| m.get("content"));

        if entry_type == Some("assistant") {
            if let Some(blocks) = content.and_then(Value::as_array) {
                for block in blocks {
                    self.assistant_block(block, &event_ts);
                }
            }
        }

        if entry_type == Some("user") {
            if let Some(prompt) = human_prompt(entry) {
                if !prompt.starts_with("[Request interrupted") {
                    self.turns.push(SessionTurn {
                        index: self.turns.len() as u32,
                        prompt: truncate(&prompt, 500),
                        timestamp: event_ts.clone(),
                    });
                    self.rationale.clear();
                }
            }
            self.user_entry(entry, content, &event_ts);
        }
    }

    fn assistant_block(&mut self, block: &Value, event_ts: &str) {
        let btype = block.get("type").and_then(Value::as_str);
        if btype == Some("text") {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                let trimmed = text.trim();
                if utf16_len(trimmed) > 40 {
                    self.last_assistant_text = trimmed.to_string();
                }
                if !trimmed.is_empty() {
                    self.rationale = truncate(trimmed, 600);
                }
            }
        }
        if btype != Some("tool_use") {
            return;
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
        let input = block.get("input").unwrap_or(&Value::Null);
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let rel: Option<String> = file_path.map(|fp| rel_path(fp, self.cwd()));
        let mut command = String::new();

        let event = match tool_name.as_str() {
            "Read" => {
                if let Some(r) = &rel {
                    self.files_read.insert(r.clone());
                }
                let mut d = TimelineEventDraft::new(
                    event_ts,
                    TimelineEventKind::FileRead,
                    format!(
                        "Read {}",
                        rel.clone().unwrap_or_else(|| "(file)".to_string())
                    ),
                );
                d.file_path = rel.clone();
                Some(self.push(d))
            }
            "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
                let verb = if tool_name == "Write" {
                    "Wrote"
                } else {
                    "Edited"
                };
                let mut d = TimelineEventDraft::new(
                    event_ts,
                    TimelineEventKind::FileEdit,
                    format!(
                        "{} {}",
                        verb,
                        rel.clone().unwrap_or_else(|| "(file)".to_string())
                    ),
                );
                d.file_path = rel.clone();
                Some(self.push(d))
            }
            "Bash" | "PowerShell" => {
                command = input
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let description = input
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let is_test = TEST_COMMAND.is_match(&command);
                self.commands_run += 1;
                if is_test {
                    self.tests_run += 1;
                }
                let title = if description.is_empty() {
                    truncate(&first_line(&command), 80)
                } else {
                    description.to_string()
                };
                let mut d = TimelineEventDraft::new(
                    event_ts,
                    if is_test {
                        TimelineEventKind::TestRun
                    } else {
                        TimelineEventKind::Command
                    },
                    title,
                );
                d.command = Some(truncate(&command, 300));
                Some(self.push(d))
            }
            "TodoWrite" => {
                let steps: Vec<String> = input
                    .get("todos")
                    .and_then(Value::as_array)
                    .map(|todos| {
                        todos
                            .iter()
                            .map(|t| t.get("content").and_then(Value::as_str).unwrap_or(""))
                            .filter(|s| !s.is_empty())
                            .take(12)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if self.initial_plan.is_empty() && !steps.is_empty() {
                    self.initial_plan = steps.clone();
                }
                let detail = steps
                    .iter()
                    .enumerate()
                    .map(|(i, s)| format!("{}. {}", i + 1, s))
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut d = TimelineEventDraft::new(
                    event_ts,
                    TimelineEventKind::Plan,
                    format!("Updated plan ({} steps)", steps.len()),
                );
                d.detail = if detail.is_empty() {
                    None
                } else {
                    Some(detail)
                };
                Some(self.push(d))
            }
            "Grep" | "Glob" => {
                let pattern = input.get("pattern").and_then(Value::as_str).unwrap_or("");
                Some(self.push(TimelineEventDraft::new(
                    event_ts,
                    TimelineEventKind::Analysis,
                    format!("Searched: {}", truncate(pattern, 60)),
                )))
            }
            "Task" | "Agent" => {
                let description = input
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("subagent task");
                Some(self.push(TimelineEventDraft::new(
                    event_ts,
                    TimelineEventKind::Analysis,
                    format!("Delegated to subagent: {}", truncate(description, 70)),
                )))
            }
            _ => Some(self.push(TimelineEventDraft::new(
                event_ts,
                TimelineEventKind::Note,
                format!("Used tool {}", tool_name),
            ))),
        };
        let call = ToolCall {
            name: tool_name,
            command,
            event,
            turn: self.current_turn(),
            rationale: self.rationale.clone(),
            timestamp: event_ts.to_string(),
        };
        self.calls.insert(tool_id, call);
    }

    fn user_entry(&mut self, entry: &Value, content: Option<&Value>, event_ts: &str) {
        let mut result_of: Option<String> = None;
        if let Some(blocks) = content.and_then(Value::as_array) {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                    continue;
                }
                let tool_use_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if result_of.is_none() {
                    result_of = Some(tool_use_id.to_string());
                }
                let failed = block.get("is_error").map(is_truthy).unwrap_or(false);
                let rc = block.get("content");
                // A shell call's outcome: the tool flags non-zero exits.
                if let Some(call) = self.calls.get(tool_use_id) {
                    if call.name == "Bash" || call.name == "PowerShell" {
                        if let Some(event) = call.event.and_then(|i| self.timeline.get_mut(i)) {
                            event.exit_code = Some(if failed { failed_exit_code(rc) } else { 0 });
                        }
                    }
                }
                if !failed {
                    continue;
                }
                self.errors += 1;
                let tool = self.calls.get(tool_use_id);
                if let Some(call) = tool {
                    if (call.name == "Bash" || call.name == "PowerShell")
                        && TEST_COMMAND.is_match(&call.command)
                    {
                        self.tests_failed += 1;
                    }
                }
                let title = match tool {
                    Some(call) => format!("{} failed", call.name),
                    None => "Tool call failed".to_string(),
                };
                let detail_text = {
                    let t = text_of_content(rc);
                    if t.is_empty() {
                        scalar_or_empty(rc)
                    } else {
                        t
                    }
                };
                let mut d = TimelineEventDraft::new(event_ts, TimelineEventKind::Error, title);
                d.detail = Some(truncate(&detail_text, DETAIL_MAX));
                self.push(d);
            }
        }

        let tr = entry.get("toolUseResult").filter(|v| v.is_object());
        let tr_file_path = tr.and_then(|tr| {
            tr.get("filePath")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        });
        if let (Some(tr), Some(fp)) = (tr, tr_file_path) {
            let rel = rel_path(fp, self.cwd());
            self.file_result(tr, rel, result_of.as_deref(), event_ts);
        }
    }

    /// An edit tool's structured result: the per-file diff and the recorded
    /// edit used for attribution.
    fn file_result(&mut self, tr: &Value, rel: String, tool_use_id: Option<&str>, event_ts: &str) {
        let call = tool_use_id.and_then(|id| self.calls.get(id));
        let mut edit = RecordedEdit {
            seq: 0,
            path: rel.clone(),
            added: Vec::new(),
            removed: Vec::new(),
            turn: call.map(|c| c.turn).unwrap_or_else(|| self.current_turn()),
            rationale: call.map(|c| c.rationale.clone()).unwrap_or_default(),
            timestamp: call
                .map(|c| c.timestamp.clone())
                .unwrap_or_else(|| event_ts.to_string()),
            event_id: call
                .and_then(|c| c.event)
                .and_then(|i| self.timeline.get(i))
                .map(|e| e.id.clone()),
        };

        let idx = match self.file_map.iter().position(|f| f.path == rel) {
            Some(i) => i,
            None => {
                self.file_map.push(RawFile::new(rel.clone(), false));
                self.file_map.len() - 1
            }
        };
        let file = &mut self.file_map[idx];
        let is_create = tr.get("type").and_then(Value::as_str) == Some("create");
        if is_create {
            file.created = true;
        }
        let patches = tr
            .get("structuredPatch")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for patch in patches {
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
                    let text = drop_first_unit(l);
                    edit.push_added(&text);
                    file.added_lines.push(text);
                } else if l.starts_with('-') {
                    removed += 1;
                    edit.push_removed(&drop_first_unit(l));
                }
            }
            file.added += added;
            file.removed += removed;
            if file.hunks.len() < HUNKS_PER_FILE {
                let old_start = patch.get("oldStart").and_then(Value::as_i64).unwrap_or(0);
                let old_lines_n = patch.get("oldLines").and_then(Value::as_i64).unwrap_or(0);
                let new_start = patch.get("newStart").and_then(Value::as_i64).unwrap_or(0);
                let new_lines_n = patch.get("newLines").and_then(Value::as_i64).unwrap_or(0);
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
                file.hunks.push(DiffHunk {
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
                file.added = file.added.max(lines.len() as i64);
                for l in lines.iter().take(50) {
                    file.added_lines.push((*l).to_string());
                }
                for l in &lines {
                    edit.push_added(l);
                }
                let shown: Vec<&str> = lines.iter().take(400).copied().collect();
                let extra = if lines.len() > shown.len() {
                    format!(" (first {} shown)", shown.len())
                } else {
                    String::new()
                };
                file.hunks = vec![DiffHunk {
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
        if edit.line_count() > 0 {
            edit.seq = self.edits.len() as u32 + 1;
            self.edits.push(edit);
        }
    }

    /// The session as of the lines fed so far.
    pub fn finish(&self, file_mtime_iso: &str) -> Option<AgentSession> {
        if !self.any_entry {
            return None;
        }
        let session_id = self.session_id.clone()?;
        let original_prompt = self.original_prompt.clone().unwrap_or_default();
        if original_prompt.is_empty() {
            return None;
        }
        let cwd = self.cwd().to_string();
        let dur = if !self.first_ts.is_empty() && !self.last_ts.is_empty() {
            duration_minutes(&self.first_ts, &self.last_ts)
        } else {
            1
        };
        let mut session = normalize(RawSession {
            session_id,
            timestamp: if self.first_ts.is_empty() {
                file_mtime_iso.to_string()
            } else {
                self.first_ts.clone()
            },
            agent: "claude-code".to_string(),
            agent_label: "Claude Code".to_string(),
            model: self
                .model
                .clone()
                .unwrap_or_else(|| "unknown model".to_string()),
            project_name: if cwd.is_empty() {
                "(unknown)".to_string()
            } else {
                basename(&cwd.replace('\\', "/"))
            },
            project_path: if cwd.is_empty() { None } else { Some(cwd) },
            branch: self
                .branch
                .clone()
                .unwrap_or_else(|| "(no git)".to_string()),
            original_prompt,
            declared_goal: self.declared_goal.clone(),
            initial_plan: self.initial_plan.clone(),
            outcome: truncate(&self.last_assistant_text, 700),
            timeline: self.timeline.clone(),
            files: self.file_map.clone(),
            files_analyzed: self.files_read.len() as i64,
            commands_run: self.commands_run,
            tests_run: self.tests_run,
            tests_failed: self.tests_failed,
            errors: self.errors,
            duration_minutes: dur,
        })?;
        session.turns = self.turns.clone();
        let mut edits = self.edits.clone();
        cap_edits(&mut edits);
        session.edits = edits;
        Some(session)
    }
}

pub fn parse_claude_transcript(raw: &str, file_mtime_iso: &str) -> Option<AgentSession> {
    let mut parser = ClaudeParser::new(file_mtime_iso);
    for line in raw.split('\n') {
        parser.feed_line(line);
    }
    parser.finish(file_mtime_iso)
}

/// Bytes compared to tell an appended transcript from a replaced one.
const HEAD_BYTES: usize = 512;

/// One transcript read so far: the parser state, where reading stopped and
/// the session it produced. Claude Code only appends to a transcript, so a
/// grown file is read from where the last read stopped (tail -F style); a
/// file that shrank or whose first bytes changed is read again from the
/// start.
struct Tail {
    len: u64,
    mtime: f64,
    head: Vec<u8>,
    read_pos: u64,
    /// Bytes after the last newline: a record still being written
    pending: Vec<u8>,
    parser: ClaudeParser,
    session: Option<AgentSession>,
}

fn read_tail(path: &str, len: u64, mtime: f64, prev: Option<Tail>) -> Option<Tail> {
    use std::io::{BufRead, Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; (len as usize).min(HEAD_BYTES)];
    file.read_exact(&mut head).ok()?;
    let iso = mtime_iso(mtime);
    let mut tail = match prev {
        Some(t) if t.len == len && t.mtime == mtime && t.head == head => return Some(t),
        Some(t) if len >= t.len && head.starts_with(&t.head) => t,
        _ => Tail {
            len: 0,
            mtime: 0.0,
            head: Vec::new(),
            read_pos: 0,
            pending: Vec::new(),
            parser: ClaudeParser::new(&iso),
            session: None,
        },
    };
    tail.parser.fallback_ts = iso.clone();
    file.seek(SeekFrom::Start(tail.read_pos)).ok()?;
    let mut reader = std::io::BufReader::with_capacity(1 << 16, file);
    let mut line = std::mem::take(&mut tail.pending);
    loop {
        let n = reader.read_until(b'\n', &mut line).ok()?;
        if n == 0 {
            break;
        }
        tail.read_pos += n as u64;
        if line.last() == Some(&b'\n') {
            tail.parser.feed_line(&String::from_utf8_lossy(&line));
            line.clear();
        }
    }
    // A last record without its newline counts once it is complete JSON.
    if !line.is_empty() {
        let text = String::from_utf8_lossy(&line).to_string();
        if serde_json::from_str::<Value>(text.trim()).is_ok() {
            tail.parser.feed_line(&text);
            line.clear();
        }
    }
    tail.pending = line;
    tail.session = tail.parser.finish(&iso);
    tail.len = len;
    tail.mtime = mtime;
    tail.head = head;
    Some(tail)
}

/// Claude Code's folder name for a project: every character other than an
/// ASCII letter or digit becomes '-'.
fn project_folder_key(root: &str) -> String {
    root.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Whether a ~/.claude/projects folder can hold sessions of `key`'s project:
/// the project itself or one of its subfolders (verified by the session's
/// own cwd afterwards, since sibling projects share the prefix).
fn folder_matches(name: &str, key: &str) -> bool {
    if name.len() < key.len() || !name.is_char_boundary(key.len()) {
        return false;
    }
    path_eq(&name[..key.len()], key)
        && (name.len() == key.len() || name.as_bytes()[key.len()] == b'-')
}

pub struct ClaudeCodeAdapter {
    cache: Mutex<HashMap<String, Tail>>,
}

impl ClaudeCodeAdapter {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// (path, mtime, len) of every transcript big enough to hold a session,
    /// in the project folders `keep` accepts.
    fn transcripts(dir: &str, keep: impl Fn(&str) -> bool) -> Option<Vec<(String, f64, u64)>> {
        let mut out = Vec::new();
        for project in std::fs::read_dir(dir).ok()?.flatten() {
            let project_dir = project.path();
            let Ok(meta) = std::fs::metadata(&project_dir) else {
                continue;
            };
            if !meta.is_dir() || !keep(&project.file_name().to_string_lossy()) {
                continue;
            }
            let Ok(names) = std::fs::read_dir(&project_dir) else {
                continue;
            };
            for name in names.flatten() {
                if !name.file_name().to_string_lossy().ends_with(".jsonl") {
                    continue;
                }
                if let Ok(st) = std::fs::metadata(name.path()) {
                    if st.len() >= MIN_FILE_BYTES {
                        out.push((
                            name.path().to_string_lossy().to_string(),
                            mtime_ms(&st),
                            st.len(),
                        ));
                    }
                }
            }
        }
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Some(out)
    }

    fn session_of(
        cache: &mut HashMap<String, Tail>,
        path: &str,
        mtime: f64,
        len: u64,
    ) -> Option<AgentSession> {
        let prev = cache.remove(path);
        let tail = read_tail(path, len, mtime, prev)?;
        let session = tail.session.clone();
        cache.insert(path.to_string(), tail);
        session
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
        let Some(candidates) = Self::transcripts(&dir, |_| true) else {
            status.hint = Some(format!("Claude Code transcripts not found at {}.", dir));
            return CollectResult {
                sessions: Vec::new(),
                status,
            };
        };

        let seen: HashSet<&str> = candidates.iter().map(|(p, _, _)| p.as_str()).collect();
        let mut sessions: Vec<AgentSession> = Vec::new();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        // Forget transcripts no longer on disk (the cache would grow forever)
        cache.retain(|path, _| seen.contains(path.as_str()));
        for (path, ms, len) in &candidates {
            if sessions.len() >= max_sessions {
                break;
            }
            if let Some(s) = Self::session_of(&mut cache, path, *ms, *len) {
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

    /// Only the project's own folders: Claude Code names them after the
    /// working directory, so no other project is read.
    fn project_sessions(
        &self,
        config: &AdapterConfig,
        root: &str,
        limit: usize,
    ) -> Option<Vec<AgentSession>> {
        if !config.enabled {
            return Some(Vec::new());
        }
        let dir = config.dir.clone().unwrap_or_default();
        let key = project_folder_key(root);
        let candidates = Self::transcripts(&dir, |name| folder_matches(name, &key))?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for (path, ms, len) in candidates.iter().take(limit * 3) {
            if out.len() >= limit {
                break;
            }
            if let Some(s) = Self::session_of(&mut cache, path, *ms, *len) {
                if s.project_path
                    .as_deref()
                    .is_some_and(|p| is_within(p, root))
                {
                    out.push(s);
                }
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixture::{assert_session, read as read_fixture};
    use super::{folder_matches, parse_claude_transcript, project_folder_key, read_tail};

    #[test]
    fn parses_basic_fixture() {
        let raw = read_fixture("claude-code/basic.jsonl");
        let session = parse_claude_transcript(&raw, "2026-03-10T12:00:00.000Z").expect("session");
        assert_session(&session, "claude-code/expected/basic.json");
        assert!(!session.turns.is_empty(), "the original prompt is turn 0");
    }

    #[test]
    fn appended_reads_produce_the_one_shot_session() {
        let raw = read_fixture("claude-code/basic.jsonl");
        let lines: Vec<&str> = raw.split_inclusive('\n').collect();
        let dir = std::env::temp_dir().join(format!("ars-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let p = path.to_str().unwrap();
        // First half, then the half of a line, then the rest.
        let half = lines.len() / 2;
        let first: String = lines[..half].concat();
        let cut = lines[half].len() / 2;
        std::fs::write(&path, format!("{first}{}", &lines[half][..cut])).unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        let tail = read_tail(p, len, 1.0, None).expect("first read");
        std::fs::write(&path, &raw).unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        let tail = read_tail(p, len, 1.0, Some(tail)).expect("appended read");
        let once = parse_claude_transcript(&raw, &super::mtime_iso(1.0)).expect("session");
        assert_eq!(
            serde_json::to_value(tail.session.expect("session")).unwrap(),
            serde_json::to_value(once).unwrap()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn project_folders_are_the_project_and_its_subfolders() {
        let key = project_folder_key("C:/dev/shop");
        assert_eq!(key, "C--dev-shop");
        assert!(folder_matches("C--dev-shop", &key));
        assert!(
            folder_matches("C--dev-shop-api", &key),
            "a subfolder (or a sibling: cwd decides)"
        );
        assert!(!folder_matches("C--dev-shopping", &key));
        assert!(!folder_matches("C--dev", &key));
    }
}
