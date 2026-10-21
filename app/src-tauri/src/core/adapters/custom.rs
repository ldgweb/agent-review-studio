// The open ingestion format (ars/v1).
// Any agent, editor, CI job or in-house tool can be reviewed in Agent Review
// Studio by dropping a JSON file into ~/.agent-review-studio/sessions/
// (or any folder listed in adapters.custom.dirs). This is the escape hatch
// for agents we don't ship a native adapter for.
//
// Minimal valid file:
// {
//   "format": "ars/v1",
//   "agentLabel": "My In-House Agent",
//   "prompt": "Refactor the billing module",
//   "files": [{ "path": "src/billing.py", "diff": "@@ -1,3 +1,4 @@\n-old\n+new\n context" }]
// }
// Optional fields: sessionId, model, timestamp, projectName, branch, outcome,
// plan[], timeline[{time,kind,title,detail}], files[].{created,added,removed,content},
// stats{commandsRun,testsRun,testsFailed,filesAnalyzed,durationMinutes}.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use super::super::domain::{
    AgentSession, DiffHunk, DiffLine, DiffLineKind, RawFile, RawSession, TimelineEvent,
    TimelineEventKind,
};
use super::super::normalize::{normalize, parse_date, to_iso, truncate};
use super::{mtime_iso, mtime_ms, AdapterConfig, CollectResult, SessionAdapter};

/// Guarded access — the string value or ''.
fn s(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or("")
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

/// A plan entry as text: strings as-is, numbers and booleans formatted, null
/// as "null", arrays and objects as compact JSON.
fn entry_text(v: &Value) -> String {
    match v {
        Value::String(text) => text.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn timeline_kind(kind: &str) -> Option<TimelineEventKind> {
    Some(match kind {
        "plan" => TimelineEventKind::Plan,
        "analysis" => TimelineEventKind::Analysis,
        "file-read" => TimelineEventKind::FileRead,
        "file-edit" => TimelineEventKind::FileEdit,
        "command" => TimelineEventKind::Command,
        "test-run" => TimelineEventKind::TestRun,
        "error" => TimelineEventKind::Error,
        "decision" => TimelineEventKind::Decision,
        "note" => TimelineEventKind::Note,
        _ => return None,
    })
}

/// Short stable digest of a file path (separators normalized; case only where
/// the platform's paths ignore it).
fn path_tag(path: &str) -> String {
    use sha2::{Digest, Sha256};
    let normalized = path.replace('\\', "/");
    let normalized = super::super::paths::fold_case(&normalized);
    let digest = Sha256::digest(normalized.as_bytes());
    digest[..4].iter().map(|b| format!("{:02x}", b)).collect()
}

static HUNK_HEADER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@").unwrap());

struct ParsedDiff {
    hunks: Vec<DiffHunk>,
    added: i64,
    removed: i64,
    added_lines: Vec<String>,
}

/// Parse standard unified diff text into DiffHunks
fn parse_unified_diff(diff: &str) -> ParsedDiff {
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut old_ln: i64 = 0;
    let mut new_ln: i64 = 0;
    let mut added: i64 = 0;
    let mut removed: i64 = 0;
    let mut added_lines: Vec<String> = Vec::new();

    let cleaned = diff.replace('\r', "");
    // A diff ending in "\n" leaves an empty final segment — not a line.
    let body = cleaned.strip_suffix('\n').unwrap_or(&cleaned);
    for line in body.split('\n') {
        // "\ No newline at end of file" annotates the previous line.
        if line.starts_with('\\') {
            continue;
        }
        if let Some(caps) = HUNK_HEADER.captures(line) {
            hunks.push(DiffHunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
            old_ln = caps
                .get(1)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0);
            new_ln = caps
                .get(2)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0);
            continue;
        }
        let current = match hunks.last_mut() {
            Some(h) => h,
            None => continue,
        };
        if line.starts_with("---") || line.starts_with("+++") || line.starts_with("diff ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            added += 1;
            added_lines.push(rest.to_string());
            current.lines.push(DiffLine {
                kind: DiffLineKind::Add,
                text: rest.to_string(),
                old_line: None,
                new_line: Some(new_ln),
            });
            new_ln += 1;
        } else if let Some(rest) = line.strip_prefix('-') {
            removed += 1;
            current.lines.push(DiffLine {
                kind: DiffLineKind::Del,
                text: rest.to_string(),
                old_line: Some(old_ln),
                new_line: None,
            });
            old_ln += 1;
        } else {
            let text = line.strip_prefix(' ').unwrap_or(line);
            current.lines.push(DiffLine {
                kind: DiffLineKind::Context,
                text: text.to_string(),
                old_line: Some(old_ln),
                new_line: Some(new_ln),
            });
            old_ln += 1;
            new_ln += 1;
        }
    }
    ParsedDiff {
        hunks,
        added,
        removed,
        added_lines,
    }
}

pub fn parse_ars_session(
    input: &Value,
    fallback_id: &str,
    mtime_iso_ts: &str,
) -> Option<AgentSession> {
    if let Some(format) = input.get("format") {
        if is_truthy(format) && format.as_str() != Some("ars/v1") {
            return None;
        }
    }
    let prompt = s(input.get("prompt")).trim().to_string();
    let files_input = input.get("files").and_then(Value::as_array);
    if prompt.is_empty() || files_input.is_none() {
        return None;
    }
    let files_input = files_input?;

    let mut files: Vec<RawFile> = Vec::new();
    for f in files_input {
        let raw_path = s(f.get("path"));
        if raw_path.is_empty() {
            continue;
        }
        let path = raw_path.replace('\\', "/");
        let mut hunks: Vec<DiffHunk> = Vec::new();
        let mut added = f.get("added").and_then(Value::as_i64).unwrap_or(0);
        let mut removed = f.get("removed").and_then(Value::as_i64).unwrap_or(0);
        let mut added_lines: Vec<String> = Vec::new();
        let diff = s(f.get("diff"));
        let content = s(f.get("content"));
        if !diff.is_empty() {
            let parsed = parse_unified_diff(diff);
            hunks = parsed.hunks;
            // An explicit 0 falls back to the parsed count
            if added == 0 {
                added = parsed.added;
            }
            if removed == 0 {
                removed = parsed.removed;
            }
            added_lines = parsed.added_lines;
        } else if !content.is_empty() {
            let cleaned = content.replace('\r', "");
            let lines: Vec<&str> = cleaned.split('\n').collect();
            if added == 0 {
                added = lines.len() as i64;
            }
            added_lines = lines.iter().take(50).map(|l| l.to_string()).collect();
            hunks = vec![DiffHunk {
                header: format!("@@ new file — {} lines @@", lines.len()),
                lines: lines
                    .iter()
                    .take(400)
                    .enumerate()
                    .map(|(i, text)| DiffLine {
                        kind: DiffLineKind::Add,
                        text: text.to_string(),
                        old_line: None,
                        new_line: Some(i as i64 + 1),
                    })
                    .collect(),
            }];
        }
        // `created` when present, otherwise whether content was given
        let created = match f.get("created") {
            Some(v) if !v.is_null() => is_truthy(v),
            _ => !content.is_empty(),
        };
        files.push(RawFile {
            path,
            created,
            deleted: false,
            added,
            removed,
            hunks,
            added_lines,
        });
    }

    let mut timeline: Vec<TimelineEvent> = Vec::new();
    if let Some(events) = input.get("timeline").and_then(Value::as_array) {
        for e in events {
            let title = s(e.get("title"));
            if title.is_empty() {
                continue; // untitled events are dropped
            }
            let time = s(e.get("time"));
            let timestamp = if !time.is_empty() {
                parse_date(time)
                    .map(to_iso)
                    .unwrap_or_else(|| mtime_iso_ts.to_string())
            } else {
                mtime_iso_ts.to_string()
            };
            let detail = {
                let d = s(e.get("detail"));
                if d.is_empty() {
                    None
                } else {
                    Some(truncate(d, 600))
                }
            };
            // Optional provenance fields (ars/v1 extension): filePath ties an
            // event to a file (feeds "changed but never re-tested"), command
            // and exitCode make command/test-run events first-class.
            let file_path = {
                let p = s(e.get("filePath"));
                if p.is_empty() {
                    None
                } else {
                    Some(p.replace('\\', "/"))
                }
            };
            let command = {
                let c = s(e.get("command"));
                if c.is_empty() {
                    None
                } else {
                    Some(truncate(c, 200))
                }
            };
            timeline.push(TimelineEvent {
                id: format!("ev-{}", timeline.len() + 1),
                timestamp,
                kind: timeline_kind(s(e.get("kind"))).unwrap_or(TimelineEventKind::Note),
                title: truncate(title, 120),
                detail,
                file_path,
                command,
                exit_code: e.get("exitCode").and_then(Value::as_i64),
            });
        }
    }

    let session_id = {
        let sid = s(input.get("sessionId"));
        if sid.is_empty() {
            fallback_id.to_string()
        } else {
            sid.to_string()
        }
    };
    let timestamp = {
        let ts = s(input.get("timestamp"));
        if !ts.is_empty() {
            parse_date(ts)
                .map(to_iso)
                .unwrap_or_else(|| mtime_iso_ts.to_string())
        } else {
            mtime_iso_ts.to_string()
        }
    };
    let initial_plan: Vec<String> = input
        .get("plan")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|p| truncate(&entry_text(p), 200))
                .take(12)
                .collect()
        })
        .unwrap_or_default();
    let stats = input.get("stats");
    let stat = |key: &str, default: i64| -> i64 {
        stats
            .and_then(|st| st.get(key))
            .and_then(Value::as_i64)
            .unwrap_or(default)
    };
    let or_default = |key: &str, default: &str| -> String {
        let v = s(input.get(key));
        if v.is_empty() {
            default.to_string()
        } else {
            v.to_string()
        }
    };

    normalize(RawSession {
        session_id,
        timestamp,
        agent: "custom".to_string(),
        agent_label: or_default("agentLabel", "Custom agent"),
        model: or_default("model", "unknown model"),
        project_name: or_default("projectName", "(unknown)"),
        // ars/v1 extension: an absolute projectPath unlocks the git surface
        // (working-tree enrichment, commit/revert) exactly like native sources.
        project_path: {
            let p = s(input.get("projectPath"));
            if p.is_empty() {
                None
            } else {
                Some(p.replace('\\', "/"))
            }
        },
        branch: or_default("branch", "(no git)"),
        original_prompt: truncate(&prompt, 1500),
        declared_goal: None,
        initial_plan,
        outcome: truncate(s(input.get("outcome")), 700),
        timeline,
        files,
        files_analyzed: stat("filesAnalyzed", 0),
        commands_run: stat("commandsRun", 0),
        tests_run: stat("testsRun", 0),
        tests_failed: stat("testsFailed", 0),
        errors: 0,
        duration_minutes: stat("durationMinutes", 1),
    })
}

#[derive(Default)]
pub struct CustomAdapter;

impl CustomAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl SessionAdapter for CustomAdapter {
    fn id(&self) -> &'static str {
        "custom"
    }

    fn label(&self) -> &'static str {
        "Custom (ars/v1)"
    }

    /// The watched folders hold flat *.json drops — non-recursive is enough.
    fn watch_targets(&self, config: &AdapterConfig) -> Vec<super::WatchTarget> {
        config
            .dirs
            .iter()
            .map(|dir| super::WatchTarget {
                path: dir.clone(),
                recursive: false,
            })
            .collect()
    }

    fn collect(&self, config: &AdapterConfig, max_sessions: usize) -> CollectResult {
        let dirs = &config.dirs;
        let source = {
            let joined = dirs.join(", ");
            if joined.is_empty() {
                "(not configured)".to_string()
            } else {
                joined
            }
        };
        let mut status = self.base_status(config, source);
        if !config.enabled {
            return CollectResult {
                sessions: Vec::new(),
                status,
            };
        }

        let mut sessions: Vec<AgentSession> = Vec::new();
        for dir in dirs {
            let entries = match std::fs::read_dir(dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".json") {
                    continue;
                }
                let full = entry.path();
                let full_str = full.to_string_lossy().to_string();
                let meta = match std::fs::metadata(&full) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let parsed: Value = match std::fs::read_to_string(&full)
                    .ok()
                    .and_then(|raw| serde_json::from_str(&raw).ok())
                {
                    Some(v) => v,
                    None => continue, // malformed file — skip
                };
                let nested = parsed.get("sessions").and_then(Value::as_array).cloned();
                let inputs: Vec<Value> = match nested {
                    Some(arr) => arr,
                    None => vec![parsed],
                };
                let stem = name.strip_suffix(".json").unwrap_or(&name).to_string();
                // Same-named files in different folders must not share ids.
                let tag = path_tag(&full_str);
                let ts = mtime_iso(mtime_ms(&meta));
                for (i, input) in inputs.iter().enumerate() {
                    let fallback = format!("{}-{}-{}", stem, tag, i);
                    if let Some(session) = parse_ars_session(input, &fallback, &ts) {
                        sessions.push(session);
                    }
                }
            }
        }
        let total = sessions.len();
        // Newest first before capping — read_dir order is arbitrary.
        sessions.sort_by_cached_key(|s| {
            std::cmp::Reverse(parse_date(&s.timestamp).map(|d| d.timestamp_millis()))
        });
        sessions.truncate(max_sessions);
        status.sessions = sessions.len();
        status.hint = if total == 0 {
            Some("Drop ars/v1 JSON files into this folder to review any custom agent (see the ars/v1 example linked from the README).".to_string())
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

    #[test]
    fn unified_diff_ignores_trailing_newline_and_no_newline_marker() {
        let parsed = parse_unified_diff(
            "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n keep\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n",
        );
        assert_eq!(parsed.hunks.len(), 1);
        let lines = &parsed.hunks[0].lines;
        assert_eq!(lines.len(), 3, "no phantom context line");
        assert_eq!(lines[0].kind, DiffLineKind::Context);
        assert_eq!(lines[1].kind, DiffLineKind::Del);
        assert_eq!(lines[1].old_line, Some(2));
        assert_eq!(lines[2].kind, DiffLineKind::Add);
        assert_eq!(lines[2].text, "new");
        assert_eq!(lines[2].new_line, Some(2));
        assert_eq!((parsed.added, parsed.removed), (1, 1));
        // A blank context line inside a hunk is still a line.
        let blank = parse_unified_diff("@@ -1,3 +1,3 @@\n a\n\n+b\n");
        assert_eq!(blank.hunks[0].lines.len(), 3);
        assert_eq!(blank.hunks[0].lines[2].new_line, Some(3));
    }

    #[test]
    fn parses_basic_fixture() {
        let input: Value =
            serde_json::from_str(&read_fixture("custom/basic.json")).expect("fixture json");
        let session =
            parse_ars_session(&input, "basic-0", MTIME).expect("parser returned a session");
        assert_session(&session, "custom/expected/basic.json");
    }
}
