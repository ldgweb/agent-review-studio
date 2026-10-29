// Cursor adapter: reads Composer/Agent sessions from
// Cursor's global key-value store (a SQLite file): `composerData:<id>` holds
// the conversation header list, `bubbleId:<composerId>:<bubbleId>` holds each
// message with its tool call. Two tool generations are supported: the legacy
// names (edit_file, search_replace, run_terminal_cmd, … with rawArgs) and the
// v2 agent names (edit_file_v2, run_terminal_command_v2, … with params),
// where edits reference content-addressed blobs (`composer.content.<sha>`)
// resolved into real before/after line diffs.
// Defensive by design: the store is an undocumented internal format that
// changes between Cursor releases, so every field access is guarded and
// unrecognized records are skipped rather than failing the scan.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};

use super::super::domain::{
    AgentSession, DiffHunk, DiffLine, DiffLineKind, RawFile, RawSession, TimelineEvent,
    TimelineEventDraft, TimelineEventKind,
};
use super::super::normalize::{content_diff, first_line, normalize, truncate, TEST_COMMAND};
use super::super::paths::fold_case;
use super::project_root::{ascend_to_project_root, common_root, git_branch_of, relative_to};
use super::{mtime_iso, mtime_ms, AdapterConfig, CollectResult, SessionAdapter};

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

/// `// ... existing code ...` in any comment syntax: Cursor's edit_file elision marker.
static EXISTING_CODE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\.\.\.\s*existing code\s*\.\.\.").unwrap());

fn resolve_path(known_paths: &[String], root: &str, target: &str) -> String {
    let clean = target.replace('\\', "/");
    let clean_key = fold_case(&clean);
    let suffix = format!("/{}", clean_key);
    let hit = known_paths.iter().find(|p| {
        let key = fold_case(p);
        key.ends_with(&suffix) || key == clean_key
    });
    relative_to(root, hit.map(String::as_str).unwrap_or(&clean))
}

/// Insertion order preserved with a Vec keyed by path.
fn file_for(
    files: &mut Vec<RawFile>,
    known_paths: &[String],
    root: &str,
    target: &str,
    created: bool,
) -> usize {
    let path = resolve_path(known_paths, root, target);
    let idx = match files.iter().position(|f| f.path == path) {
        Some(i) => i,
        None => {
            files.push(RawFile::new(path, created));
            files.len() - 1
        }
    };
    if created {
        files[idx].created = true;
    }
    // Any later write brings a deleted file back; delete_file re-marks it.
    files[idx].deleted = false;
    idx
}

/// Record an edit_file code_edit: segments between elision markers become add-only hunks.
fn apply_code_edit(file: &mut RawFile, code_edit: &str) {
    let mut hunk_lines: Vec<DiffLine> = Vec::new();
    for line in code_edit.split('\n') {
        if EXISTING_CODE.is_match(line) {
            if !hunk_lines.is_empty() {
                let mut lines = std::mem::take(&mut hunk_lines);
                lines.truncate(120);
                file.hunks.push(DiffHunk {
                    header: format!("@@ {} @@", file.path),
                    lines,
                });
            }
            continue;
        }
        file.added += 1;
        file.added_lines.push(line.to_string());
        hunk_lines.push(DiffLine {
            kind: DiffLineKind::Add,
            text: line.to_string(),
            old_line: None,
            new_line: None,
        });
    }
    if !hunk_lines.is_empty() {
        hunk_lines.truncate(120);
        file.hunks.push(DiffHunk {
            header: format!("@@ {} @@", file.path),
            lines: hunk_lines,
        });
    }
}

fn apply_replace(file: &mut RawFile, old_string: &str, new_string: &str) {
    let mut lines: Vec<DiffLine> = Vec::new();
    if !old_string.is_empty() {
        for line in old_string.split('\n') {
            file.removed += 1;
            lines.push(DiffLine {
                kind: DiffLineKind::Del,
                text: line.to_string(),
                old_line: None,
                new_line: None,
            });
        }
    }
    if !new_string.is_empty() {
        for line in new_string.split('\n') {
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

/// A new-file entry's path: `uri.fsPath`, else `uri.external`, else the entry itself when it is a string.
fn new_file_uri(e: &Value) -> String {
    let fs_path = s(e.get("uri").and_then(|u| u.get("fsPath")));
    if !fs_path.is_empty() {
        return fs_path.to_string();
    }
    let external = s(e.get("uri").and_then(|u| u.get("external")));
    if !external.is_empty() {
        return external.to_string();
    }
    e.as_str().unwrap_or("").to_string()
}

/// Final path component of the paths this adapter produces ('' when none).
fn basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// First non-empty candidate or ''.
fn pick<'a>(candidates: &[&'a str]) -> &'a str {
    candidates
        .iter()
        .copied()
        .find(|c| !c.is_empty())
        .unwrap_or("")
}

/// Tool arguments: v1 bubbles carry JSON in rawArgs; v2 bubbles carry an
/// object (or JSON) in params.
fn tool_args(tool: &Value) -> Value {
    let raw = s(tool.get("rawArgs"));
    if !raw.is_empty() {
        if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
            if parsed.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
                return parsed;
            }
        }
    }
    match tool.get("params") {
        Some(p) if p.is_object() => p.clone(),
        Some(p) if p.is_string() => serde_json::from_str::<Value>(p.as_str().unwrap_or(""))
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Default::default())),
        _ => Value::Object(Default::default()),
    }
}

/// Tool result: the v2 result is an object or a JSON string.
fn tool_result_object(tool: &Value) -> Value {
    match tool.get("result") {
        Some(r) if r.is_object() => r.clone(),
        Some(r) if r.is_string() => serde_json::from_str::<Value>(r.as_str().unwrap_or(""))
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Default::default())),
        _ => Value::Object(Default::default()),
    }
}

static ABSOLUTE_PATH: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([A-Za-z]:[\\/]|/)").unwrap());

fn target_of(args: &Value) -> String {
    pick(&[
        s(args.get("target_file")),
        s(args.get("file_path")),
        s(args.get("path")),
        s(args.get("targetFile")),
        s(args.get("relativeWorkspacePath")),
    ])
    .to_string()
}

/// content id (`composer.content.<sha>`) → blob text
pub type ContentLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;

pub fn parse_composer(
    composer: &Value,
    get_bubble: &dyn Fn(&str) -> Option<Value>,
    fallback_id: &str,
    fallback_iso: &str,
    get_content: Option<&ContentLookup>,
) -> Option<AgentSession> {
    let empty_headers: Vec<Value> = Vec::new();
    let headers = composer
        .get("fullConversationHeadersOnly")
        .and_then(Value::as_array)
        .unwrap_or(&empty_headers);
    if headers.is_empty() {
        return None;
    }

    // Materialize the bubbles once: the project root needs the v2 absolute
    // targets before any path can be resolved relative to it
    let mut bubbles: Vec<Value> = Vec::new();
    for header in headers {
        let bubble_id = s(header.get("bubbleId"));
        if bubble_id.is_empty() {
            continue;
        }
        if let Some(bubble) = get_bubble(bubble_id) {
            bubbles.push(bubble);
        }
    }

    // Only WRITE targets define the project: reads legitimately roam outside
    // it and would drag the common root up to the drive
    const EDIT_TOOLS: [&str; 9] = [
        "edit_file_v2",
        "edit_file",
        "search_replace",
        "replace",
        "MultiEdit",
        "write",
        "create_file",
        "write_file",
        "delete_file",
    ];
    let mut tool_targets: Vec<String> = Vec::new();
    for bubble in &bubbles {
        let tool = match bubble.get("toolFormerData").filter(|t| t.is_object()) {
            Some(t) => t,
            None => continue,
        };
        if !EDIT_TOOLS.contains(&s(tool.get("name"))) {
            continue;
        }
        let target = target_of(&tool_args(tool));
        if !target.is_empty() && ABSOLUTE_PATH.is_match(&target) {
            tool_targets.push(target);
        }
    }

    let empty_created: Vec<Value> = Vec::new();
    let newly_created = composer
        .get("newlyCreatedFiles")
        .and_then(Value::as_array)
        .unwrap_or(&empty_created);

    let mut known_uris: Vec<String> = Vec::new();
    if let Some(obj) = composer.get("codeBlockData").and_then(Value::as_object) {
        known_uris.extend(obj.keys().cloned());
    }
    if let Some(obj) = composer
        .get("originalFileStates")
        .and_then(Value::as_object)
    {
        known_uris.extend(obj.keys().cloned());
    }
    known_uris.extend(newly_created.iter().map(new_file_uri));
    known_uris.extend(tool_targets);
    known_uris.retain(|u| !u.is_empty());
    let mut known_paths: Vec<String> = Vec::new();
    for uri in &known_uris {
        let p = uri_to_path(uri);
        if !known_paths.contains(&p) {
            known_paths.push(p);
        }
    }
    let root = if known_paths.is_empty() {
        String::new()
    } else {
        ascend_to_project_root(&common_root(&known_paths))
    };
    let created_uris: HashSet<String> = newly_created
        .iter()
        .map(|e| uri_to_path(&new_file_uri(e)))
        .filter(|p| !p.is_empty())
        .collect();

    let created_at_ms = n(composer.get("createdAt"));
    let last_updated_ms = {
        let l = n(composer.get("lastUpdatedAt"));
        if l != 0.0 {
            l
        } else {
            created_at_ms
        }
    };
    // No createdAt: fall back to lastUpdatedAt, then the database file's
    // mtime (as other adapters do), never 1970.
    let base_iso = if created_at_ms != 0.0 {
        mtime_iso(created_at_ms)
    } else if last_updated_ms != 0.0 {
        mtime_iso(last_updated_ms)
    } else {
        fallback_iso.to_string()
    };
    let duration_minutes = if created_at_ms != 0.0 {
        (((last_updated_ms - created_at_ms) / 60000.0).round() as i64).max(1)
    } else {
        1
    };

    let mut files: Vec<RawFile> = Vec::new();
    let mut original_prompt = String::new();
    let mut last_agent_text = String::new();
    let mut commands_run: i64 = 0;
    let mut tests_run: i64 = 0;
    let mut tests_failed: i64 = 0;
    let mut errors: i64 = 0;
    let mut files_analyzed: i64 = 0;
    let mut timeline: Vec<TimelineEvent> = Vec::new();
    let mut event_seq: usize = 0;

    for bubble in &bubbles {
        let created_ms = n(bubble.get("createdAt"));
        let bubble_ts = if created_ms != 0.0 {
            mtime_iso(created_ms)
        } else {
            let sts = s(bubble.get("createdAt"));
            if sts.is_empty() {
                base_iso.clone()
            } else {
                sts.to_string()
            }
        };
        let text = s(bubble.get("text")).trim().to_string();
        let btype = bubble.get("type").and_then(Value::as_i64);

        if btype == Some(1) {
            if !text.is_empty() && original_prompt.is_empty() {
                original_prompt = truncate(&text, 1500);
            } else if !text.is_empty() {
                push_event(
                    &mut timeline,
                    &mut event_seq,
                    TimelineEventDraft::new(
                        bubble_ts.clone(),
                        TimelineEventKind::Note,
                        format!("User: {}", truncate(&first_line(&text), 70)),
                    ),
                );
            }
            continue;
        }
        if btype != Some(2) {
            continue;
        }
        if utf16_len(&text) > 40 {
            last_agent_text = text.clone();
        }

        let tool = match bubble.get("toolFormerData").filter(|t| t.is_object()) {
            Some(t) => t,
            None => continue,
        };
        let name = s(tool.get("name"));
        let args = tool_args(tool);
        let target = target_of(&args);
        let failed = s(tool.get("status")) == "error";
        if failed {
            errors += 1;
        }

        if name == "edit_file_v2" && !target.is_empty() {
            // v2 edits carry no patch: the result references content-addressed
            // before/after blobs that we diff line-by-line
            let result = tool_result_object(tool);
            let before = get_content.and_then(|f| f(s(result.get("beforeContentId"))));
            let after = get_content.and_then(|f| f(s(result.get("afterContentId"))));
            let created = before.as_deref() == Some("")
                || created_uris.contains(&resolve_path(&known_paths, &root, &target))
                || created_uris.contains(&target);
            let idx = file_for(&mut files, &known_paths, &root, &target, created);
            if let (Some(before), Some(after)) = (before, after) {
                let diff = content_diff(&before, &after);
                files[idx].hunks.extend(diff.hunks);
                files[idx].added += diff.added;
                files[idx].removed += diff.removed;
                files[idx].added_lines.extend(diff.added_lines);
            }
            let path = files[idx].path.clone();
            let mut ev = TimelineEventDraft::new(
                bubble_ts.clone(),
                TimelineEventKind::FileEdit,
                format!("Edited {}", path),
            );
            ev.file_path = Some(path);
            push_event(&mut timeline, &mut event_seq, ev);
        } else if name == "edit_file" && !target.is_empty() {
            let created = created_uris.contains(&resolve_path(&known_paths, &root, &target))
                || created_uris.contains(&target);
            let idx = file_for(&mut files, &known_paths, &root, &target, created);
            apply_code_edit(&mut files[idx], s(args.get("code_edit")));
            let path = files[idx].path.clone();
            let mut ev = TimelineEventDraft::new(
                bubble_ts.clone(),
                TimelineEventKind::FileEdit,
                format!("Edited {}", path),
            );
            ev.file_path = Some(path);
            let detail = truncate(s(args.get("instructions")), 200);
            if !detail.is_empty() {
                ev.detail = Some(detail);
            }
            push_event(&mut timeline, &mut event_seq, ev);
        } else if (name == "search_replace" || name == "replace" || name == "MultiEdit")
            && !target.is_empty()
        {
            let idx = file_for(&mut files, &known_paths, &root, &target, false);
            apply_replace(
                &mut files[idx],
                s(args.get("old_string")),
                s(args.get("new_string")),
            );
            let path = files[idx].path.clone();
            let mut ev = TimelineEventDraft::new(
                bubble_ts.clone(),
                TimelineEventKind::FileEdit,
                format!("Edited {}", path),
            );
            ev.file_path = Some(path);
            push_event(&mut timeline, &mut event_seq, ev);
        } else if (name == "write" || name == "create_file" || name == "write_file")
            && !target.is_empty()
        {
            let idx = file_for(&mut files, &known_paths, &root, &target, true);
            let contents = pick(&[
                s(args.get("contents")),
                s(args.get("content")),
                s(args.get("code_edit")),
            ]);
            apply_code_edit(&mut files[idx], contents);
            let path = files[idx].path.clone();
            let mut ev = TimelineEventDraft::new(
                bubble_ts.clone(),
                TimelineEventKind::FileEdit,
                format!("Created {}", path),
            );
            ev.file_path = Some(path);
            push_event(&mut timeline, &mut event_seq, ev);
        } else if name == "delete_file" && !target.is_empty() {
            let path = if failed {
                resolve_path(&known_paths, &root, &target)
            } else {
                // A successful delete is a reviewable change of its own.
                let idx = file_for(&mut files, &known_paths, &root, &target, false);
                files[idx].deleted = true;
                files[idx].path.clone()
            };
            let mut ev = TimelineEventDraft::new(
                bubble_ts.clone(),
                TimelineEventKind::FileEdit,
                format!("Deleted {}", path),
            );
            if !failed {
                ev.file_path = Some(path);
            }
            push_event(&mut timeline, &mut event_seq, ev);
        } else if name == "run_terminal_cmd"
            || name == "run_terminal_command"
            || name == "run_terminal_command_v2"
            || name == "terminal"
        {
            let command = s(args.get("command")).to_string();
            let is_test = TEST_COMMAND.is_match(&command);
            commands_run += 1;
            if is_test {
                tests_run += 1;
                if failed {
                    tests_failed += 1;
                }
            }
            let fl = first_line(&command);
            let kind = if failed {
                TimelineEventKind::Error
            } else if is_test {
                TimelineEventKind::TestRun
            } else {
                TimelineEventKind::Command
            };
            let mut ev = TimelineEventDraft::new(
                bubble_ts.clone(),
                kind,
                truncate(if fl.is_empty() { name } else { fl.as_str() }, 80),
            );
            ev.command = Some(truncate(&command, 300));
            push_event(&mut timeline, &mut event_seq, ev);
        } else if name == "read_file" || name == "read_file_v2" {
            files_analyzed += 1;
            if !target.is_empty() {
                push_event(
                    &mut timeline,
                    &mut event_seq,
                    TimelineEventDraft::new(
                        bubble_ts.clone(),
                        TimelineEventKind::FileRead,
                        format!("Read {}", resolve_path(&known_paths, &root, &target)),
                    ),
                );
            }
        } else if name == "grep_search"
            || name == "codebase_search"
            || name == "file_search"
            || name == "list_dir"
            || name == "ripgrep_raw_search"
            || name == "glob_file_search"
        {
            let query = pick(&[
                s(args.get("query")),
                s(args.get("pattern")),
                s(args.get("globPattern")),
                s(args.get("relative_workspace_path")),
                name,
            ]);
            push_event(
                &mut timeline,
                &mut event_seq,
                TimelineEventDraft::new(
                    bubble_ts.clone(),
                    TimelineEventKind::Analysis,
                    format!("Searched: {}", truncate(query, 70)),
                ),
            );
        } else if failed {
            let mut ev = TimelineEventDraft::new(
                bubble_ts.clone(),
                TimelineEventKind::Error,
                format!(
                    "Tool {} failed",
                    if name.is_empty() { "(unknown)" } else { name }
                ),
            );
            let detail = truncate(s(tool.get("result")), 300);
            if !detail.is_empty() {
                ev.detail = Some(detail);
            }
            push_event(&mut timeline, &mut event_seq, ev);
        }
    }

    if original_prompt.is_empty() {
        return None;
    }

    let empty_todos: Vec<Value> = Vec::new();
    let initial_plan: Vec<String> = composer
        .get("todos")
        .and_then(Value::as_array)
        .unwrap_or(&empty_todos)
        .iter()
        .filter_map(|t| {
            let text = pick(&[s(t.get("content")), s(t.get("text"))]);
            if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            }
        })
        .collect();

    let session_id = {
        let cid = s(composer.get("composerId"));
        if cid.is_empty() {
            fallback_id.to_string()
        } else {
            cid.to_string()
        }
    };
    let declared_goal = {
        let nm = s(composer.get("name"));
        if nm.is_empty() {
            None
        } else {
            Some(nm.to_string())
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
        agent: "cursor".to_string(),
        agent_label: "Cursor".to_string(),
        model: "unknown model".to_string(),
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
        original_prompt,
        declared_goal,
        initial_plan,
        outcome: truncate(&last_agent_text, 700),
        timeline,
        files,
        files_analyzed,
        commands_run,
        tests_run,
        tests_failed,
        errors,
        duration_minutes,
    })
}

struct CursorCache {
    db_path: String,
    mtime_ms: f64,
    max_sessions: usize,
    sessions: Vec<AgentSession>,
    composers_seen: usize,
}

pub struct CursorAdapter {
    cache: Mutex<Option<CursorCache>>,
    /// composerId → (lastUpdatedAt, parse result): Cursor's DB changes mtime
    /// constantly while the editor is open, and re-parsing every composer
    /// JSON (hundreds, some 300 KB) on each scan would stall the scanner for
    /// seconds. Only changed composers re-parse.
    composer_cache: Mutex<std::collections::HashMap<String, (f64, Option<AgentSession>)>>,
}

impl CursorAdapter {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(None),
            composer_cache: Mutex::new(Default::default()),
        }
    }
}

impl Default for CursorAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn empty_hint(session_count: usize, composers_seen: usize) -> Option<String> {
    if session_count > 0 {
        return None;
    }
    Some(
        if composers_seen > 0 {
            "Cursor chats found but none contained reviewable file edits."
        } else {
            "No Cursor Composer/Agent chats found in the database (recent Cursor versions store them in a new internal format)."
        }
        .to_string(),
    )
}

impl SessionAdapter for CursorAdapter {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn label(&self) -> &'static str {
        "Cursor"
    }

    /// NO filesystem watch: Cursor rewrites state.vscdb continuously while
    /// the editor is open, which turns a watcher into a permanent rescan
    /// trigger. The scanner's slow poll tick covers this source instead.
    fn watch_targets(&self, _config: &AdapterConfig) -> Vec<super::WatchTarget> {
        Vec::new()
    }

    fn collect(&self, config: &AdapterConfig, max_sessions: usize) -> CollectResult {
        let dir = config.dir.clone().unwrap_or_default();
        // Tolerate a dir pointing at Cursor's User folder instead of globalStorage.
        let mut db_path = std::path::PathBuf::from(&dir)
            .join("state.vscdb")
            .to_string_lossy()
            .to_string();
        let mtime = match std::fs::metadata(&db_path) {
            Ok(meta) => Some(mtime_ms(&meta)),
            Err(_) => {
                let nested = std::path::PathBuf::from(&dir)
                    .join("globalStorage")
                    .join("state.vscdb")
                    .to_string_lossy()
                    .to_string();
                match std::fs::metadata(&nested) {
                    Ok(meta) => {
                        db_path = nested;
                        Some(mtime_ms(&meta))
                    }
                    Err(_) => None,
                }
            }
        };
        let mut status = self.base_status(config, db_path.clone());
        if !config.enabled {
            return CollectResult {
                sessions: Vec::new(),
                status,
            };
        }
        let mtime = match mtime {
            Some(m) => m,
            None => {
                status.hint =
                    Some("No Cursor data found. Is Cursor installed on this machine?".to_string());
                return CollectResult {
                    sessions: Vec::new(),
                    status,
                };
            }
        };
        {
            let cache = lock(&self.cache);
            if let Some(c) = cache.as_ref() {
                if c.db_path == db_path && c.mtime_ms == mtime && c.max_sessions == max_sessions {
                    status.sessions = c.sessions.len();
                    status.hint = empty_hint(c.sessions.len(), c.composers_seen);
                    return CollectResult {
                        sessions: c.sessions.clone(),
                        status,
                    };
                }
            }
        }

        let conn = match rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(c) => c,
            Err(_) => {
                status.hint =
                    Some("Could not open Cursor’s database (is the file locked?)".to_string());
                return CollectResult {
                    sessions: Vec::new(),
                    status,
                };
            }
        };

        // Cheap metadata pass: the full composer JSON is fetched and parsed
        // only for records whose lastUpdatedAt actually changed
        let mut candidates: Vec<(String, f64)> = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT key, COALESCE(json_extract(value, '$.lastUpdatedAt'), json_extract(value, '$.createdAt'), 0) AS stamp FROM cursorDiskKV WHERE key LIKE 'composerData:%'",
        ) {
            if let Ok(rows) = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1).unwrap_or(0.0)))) {
                for pair in rows.flatten() {
                    candidates.push(pair);
                }
            }
        }
        let composers_seen = candidates.len();
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // TEXT normally; tolerate BLOB rows (utf8-lossy)
        fn row_text(row: &rusqlite::Row<'_>) -> Option<String> {
            match row.get::<_, String>(0) {
                Ok(v) => Some(v),
                Err(_) => row
                    .get::<_, Vec<u8>>(0)
                    .ok()
                    .map(|b| String::from_utf8_lossy(&b).to_string()),
            }
        }
        let get_value = |key: &str| -> Option<String> {
            let mut stmt = conn
                .prepare_cached("SELECT value FROM cursorDiskKV WHERE key = ?")
                .ok()?;
            stmt.query_row([key], |row| Ok(row_text(row)))
                .ok()
                .flatten()
        };
        // v2 content blobs are stored under their own keys (composer.content.<sha>)
        let get_content = |content_id: &str| -> Option<String> {
            if content_id.is_empty() {
                return None;
            }
            get_value(content_id)
        };

        let mut sessions: Vec<AgentSession> = Vec::new();
        let mut composer_cache = lock(&self.composer_cache);
        // Evict composers gone from the DB: the cache otherwise grows for
        // the whole process lifetime.
        let live_ids: std::collections::HashSet<String> = candidates
            .iter()
            .map(|(key, _)| key.trim_start_matches("composerData:").to_string())
            .collect();
        composer_cache.retain(|id, _| live_ids.contains(id));
        for (key, stamp) in &candidates {
            if sessions.len() >= max_sessions {
                break;
            }
            let cache_id = key.trim_start_matches("composerData:").to_string();
            if let Some((cached_stamp, cached)) = composer_cache.get(&cache_id) {
                if cached_stamp == stamp {
                    if let Some(session) = cached {
                        sessions.push(session.clone());
                    }
                    continue;
                }
            }
            let mut session: Option<AgentSession> = None;
            if let Some(value) = get_value(key) {
                if let Ok(composer) = serde_json::from_str::<Value>(&value) {
                    let has_headers = composer
                        .get("fullConversationHeadersOnly")
                        .and_then(Value::as_array)
                        .map(|a| !a.is_empty())
                        .unwrap_or(false);
                    if has_headers {
                        let cid = s(composer.get("composerId")).to_string();
                        let get_bubble = |bubble_id: &str| -> Option<Value> {
                            let value = get_value(&format!("bubbleId:{}:{}", cid, bubble_id))?;
                            serde_json::from_str(&value).ok()
                        };
                        let fallback = if cid.is_empty() {
                            cache_id.clone()
                        } else {
                            cid.clone()
                        };
                        session = parse_composer(
                            &composer,
                            &get_bubble,
                            &fallback,
                            &mtime_iso(mtime),
                            Some(&get_content),
                        );
                    }
                }
            }
            composer_cache.insert(cache_id, (*stamp, session.clone()));
            if let Some(session) = session {
                sessions.push(session);
            }
        }
        drop(composer_cache);
        drop(conn);

        status.sessions = sessions.len();
        status.hint = empty_hint(sessions.len(), composers_seen);
        *lock(&self.cache) = Some(CursorCache {
            db_path,
            mtime_ms: mtime,
            max_sessions,
            sessions: sessions.clone(),
            composers_seen,
        });
        CollectResult { sessions, status }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixture::{assert_session, read as read_fixture};
    use super::*;

    #[test]
    fn missing_created_at_falls_back_and_delete_is_recorded() {
        let composer = serde_json::json!({
            "composerId": "c1",
            "fullConversationHeadersOnly": [{ "bubbleId": "b1" }, { "bubbleId": "b2" }],
        });
        let bubbles = serde_json::json!({
            "b1": { "type": 1, "text": "Remove the legacy helper" },
            "b2": { "type": 2, "text": "", "toolFormerData": {
                "name": "delete_file",
                "rawArgs": "{\"target_file\": \"src/legacy.ts\"}"
            } },
        });
        let get_bubble = |id: &str| -> Option<Value> { bubbles.get(id).cloned() };
        let session = parse_composer(
            &composer,
            &get_bubble,
            "fallback",
            "2026-03-10T12:00:00.000Z",
            None,
        )
        .expect("a delete-only session is reviewable");
        assert_eq!(session.timestamp, "2026-03-10T12:00:00.000Z");
        assert_eq!(session.stats.duration_minutes, 1);
        assert_eq!(session.files.len(), 1);
        assert_eq!(
            session.files[0].change_type,
            super::super::super::domain::ChangeType::Deleted
        );
    }

    #[test]
    fn parses_basic_fixture() {
        let fixture: Value =
            serde_json::from_str(&read_fixture("cursor/basic.json")).expect("fixture json");
        let bubbles = fixture.get("bubbles").cloned().expect("bubbles");
        let get_bubble = |id: &str| -> Option<Value> { bubbles.get(id).cloned() };
        let session = parse_composer(
            fixture.get("composer").expect("composer"),
            &get_bubble,
            "fallback",
            "2026-03-10T12:00:00.000Z",
            None,
        )
        .expect("parser returned a session");
        assert_session(&session, "cursor/expected/basic.json");
    }

    #[test]
    fn parses_agent_v2_fixture() {
        let fixture: Value =
            serde_json::from_str(&read_fixture("cursor/agent-v2.json")).expect("fixture json");
        let bubbles = fixture.get("bubbles").cloned().expect("bubbles");
        let contents = fixture.get("contents").cloned().expect("contents");
        let get_bubble = |id: &str| -> Option<Value> { bubbles.get(id).cloned() };
        let get_content = |id: &str| -> Option<String> {
            contents.get(id).and_then(Value::as_str).map(str::to_string)
        };
        let session = parse_composer(
            fixture.get("composer").expect("composer"),
            &get_bubble,
            "fallback",
            "2026-03-10T12:00:00.000Z",
            Some(&get_content),
        )
        .expect("parser returned a session");
        assert_session(&session, "cursor/expected/agent-v2.json");
    }
}
