// Aider adapter: parses .aider.chat.history.md files.
// Aider keeps its history inside each project folder (not in a central
// directory), so this adapter requires configuration: list your project
// folders in ~/.agent-review-studio/config.json → adapters.aider.projectDirs.

use once_cell::sync::Lazy;
use regex::Regex;

use super::super::domain::{
    AgentSession, RawFile, RawSession, TimelineEvent, TimelineEventDraft, TimelineEventKind,
};
use super::super::normalize::{normalize, parse_date, to_iso, truncate, TEST_COMMAND};
use super::super::paths::fold_case;
use super::{mtime_iso, mtime_ms, AdapterConfig, CollectResult, SessionAdapter};

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

/// Final path component (std handles Windows separators on Windows).
fn basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Short stable digest of the project folder, so two projects with the same
/// folder name get distinct session ids. Separators and trailing slashes are
/// normalized; case only where the platform's paths ignore it.
fn path_tag(project_dir: &str) -> String {
    use sha2::{Digest, Sha256};
    let normalized = project_dir.replace('\\', "/");
    let normalized = fold_case(normalized.trim_end_matches('/'));
    let digest = Sha256::digest(normalized.as_bytes());
    digest[..4].iter().map(|b| format!("{:02x}", b)).collect()
}

static CHUNK_SPLIT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^# aider chat started at ").unwrap());
static MODEL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^>?\s*(?:Main model|Model):\s*([^\s,]+)").unwrap());
static APPLIED_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^>?\s*Applied edit to\s+(.+?)\s*$").unwrap());
static COMMIT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^>?\s*Commit\s+([0-9a-f]{6,})\s+(.+)$").unwrap());
static RAN_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^>?\s*(?:Running|Executing)\s+(.+)$").unwrap());

/// Drain the '#### ' buffer into the original prompt + a note event.
fn flush_prompt(
    prompt_buffer: &mut Vec<String>,
    original_prompt: &mut String,
    timeline: &mut Vec<TimelineEvent>,
    event_seq: &mut usize,
    timestamp: &str,
) {
    let text = prompt_buffer.join("\n").trim().to_string();
    prompt_buffer.clear();
    if text.is_empty() || text.starts_with('/') {
        return;
    }
    if original_prompt.is_empty() {
        *original_prompt = truncate(&text, 1500);
    }
    let first = text.split('\n').next().unwrap_or("");
    push_event(
        timeline,
        event_seq,
        TimelineEventDraft::new(
            timestamp.to_string(),
            TimelineEventKind::Note,
            format!("User: {}", truncate(first, 90)),
        ),
    );
}

pub fn parse_aider_history(raw: &str, project_dir: &str, mtime_iso_ts: &str) -> Vec<AgentSession> {
    let chunks: Vec<&str> = CHUNK_SPLIT.split(raw).skip(1).collect();
    let mut sessions: Vec<AgentSession> = Vec::new();

    for (chunk_idx, chunk) in chunks.iter().enumerate() {
        let lines: Vec<&str> = chunk.split('\n').collect();
        let started_at = lines.first().map(|l| l.trim()).unwrap_or("");
        let timestamp = parse_date(started_at)
            .map(to_iso)
            .unwrap_or_else(|| mtime_iso_ts.to_string());

        let mut model = "unknown model".to_string();
        let mut original_prompt = String::new();
        let mut last_text = String::new();
        let mut commands_run: i64 = 0;
        let mut tests_run: i64 = 0;
        let mut timeline: Vec<TimelineEvent> = Vec::new();
        let mut files: Vec<RawFile> = Vec::new();
        let mut event_seq: usize = 0;
        let mut prompt_buffer: Vec<String> = Vec::new();

        for line in lines.iter().skip(1) {
            if let Some(caps) = MODEL_RE.captures(line) {
                if let Some(m) = caps.get(1) {
                    model = m.as_str().to_string();
                }
            }

            if let Some(rest) = line.strip_prefix("#### ") {
                prompt_buffer.push(rest.to_string());
                continue;
            }
            flush_prompt(
                &mut prompt_buffer,
                &mut original_prompt,
                &mut timeline,
                &mut event_seq,
                &timestamp,
            );

            if let Some(caps) = APPLIED_RE.captures(line) {
                let path = caps
                    .get(1)
                    .map(|m| m.as_str())
                    .unwrap_or("")
                    .replace('\\', "/");
                if !files.iter().any(|f| f.path == path) {
                    files.push(RawFile::new(path.clone(), false));
                }
                let mut ev = TimelineEventDraft::new(
                    timestamp.clone(),
                    TimelineEventKind::FileEdit,
                    format!("Edited {}", path),
                );
                ev.file_path = Some(path);
                push_event(&mut timeline, &mut event_seq, ev);
                continue;
            }
            if let Some(caps) = COMMIT_RE.captures(line) {
                let hash = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let msg = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                push_event(
                    &mut timeline,
                    &mut event_seq,
                    TimelineEventDraft::new(
                        timestamp.clone(),
                        TimelineEventKind::Command,
                        format!(
                            "Commit {}: {}",
                            &hash[..hash.len().min(7)],
                            truncate(msg, 70)
                        ),
                    ),
                );
                continue;
            }
            if let Some(caps) = RAN_RE.captures(line) {
                let cmd = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let is_test = TEST_COMMAND.is_match(cmd);
                commands_run += 1;
                if is_test {
                    tests_run += 1;
                }
                push_event(
                    &mut timeline,
                    &mut event_seq,
                    TimelineEventDraft::new(
                        timestamp.clone(),
                        if is_test {
                            TimelineEventKind::TestRun
                        } else {
                            TimelineEventKind::Command
                        },
                        truncate(cmd, 80),
                    ),
                );
                continue;
            }
            let trimmed = line.trim();
            if utf16_len(trimmed) > 40 && !line.starts_with('>') && !line.starts_with("```") {
                last_text = trimmed.to_string();
            }
        }
        flush_prompt(
            &mut prompt_buffer,
            &mut original_prompt,
            &mut timeline,
            &mut event_seq,
            &timestamp,
        );

        if original_prompt.is_empty() || files.is_empty() {
            continue;
        }

        let date_part = timestamp.get(..10).unwrap_or(&timestamp);
        let session = normalize(RawSession {
            session_id: format!(
                "aider-{}-{}-{}-{}",
                basename(project_dir),
                path_tag(project_dir),
                chunk_idx,
                date_part
            ),
            timestamp: timestamp.clone(),
            agent: "aider".to_string(),
            agent_label: "Aider".to_string(),
            model,
            project_name: basename(project_dir),
            project_path: Some(project_dir.to_string()),
            branch: "(see git log)".to_string(),
            original_prompt,
            declared_goal: None,
            initial_plan: Vec::new(),
            outcome: truncate(&last_text, 700),
            timeline,
            files,
            files_analyzed: 0,
            commands_run,
            tests_run,
            tests_failed: 0,
            errors: 0,
            duration_minutes: 1,
        });
        if let Some(session) = session {
            sessions.push(session);
        }
    }
    sessions
}

/// The last n elements (history files append, so the newest chunks). As in
/// every other adapter, a cap of 0 keeps nothing.
fn last_n(mut v: Vec<AgentSession>, n: usize) -> Vec<AgentSession> {
    let len = v.len();
    if len > n {
        v.drain(..len - n);
    }
    v
}

/// history path → (mtime_ms, max_sessions, parsed sessions)
type HistoryCache = std::collections::HashMap<String, (f64, usize, Vec<AgentSession>)>;

#[derive(Default)]
pub struct AiderAdapter {
    /// The history file is re-read and re-parsed only when it actually changed.
    cache: std::sync::Mutex<HistoryCache>,
}

impl AiderAdapter {
    pub fn new() -> Self {
        Self {
            cache: std::sync::Mutex::new(Default::default()),
        }
    }
}

impl SessionAdapter for AiderAdapter {
    fn id(&self) -> &'static str {
        "aider"
    }

    fn label(&self) -> &'static str {
        "Aider"
    }

    /// The history file sits at each project root, so there is no reason
    /// to watch the whole project tree recursively.
    fn watch_targets(&self, config: &AdapterConfig) -> Vec<super::WatchTarget> {
        config
            .project_dirs
            .iter()
            .map(|dir| super::WatchTarget {
                path: dir.clone(),
                recursive: false,
            })
            .collect()
    }

    fn collect(&self, config: &AdapterConfig, max_sessions: usize) -> CollectResult {
        let dirs = &config.project_dirs;
        let source = if dirs.is_empty() {
            "(not configured)".to_string()
        } else {
            dirs.join(", ")
        };
        let mut status = self.base_status(config, source);
        if !config.enabled {
            return CollectResult {
                sessions: Vec::new(),
                status,
            };
        }
        if dirs.is_empty() {
            status.hint =
                Some("Add your Aider project folders in Settings → Session sources.".to_string());
            return CollectResult {
                sessions: Vec::new(),
                status,
            };
        }

        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut sessions: Vec<AgentSession> = Vec::new();
        for dir in dirs {
            let history_path = std::path::PathBuf::from(dir).join(".aider.chat.history.md");
            let key = history_path.to_string_lossy().to_string();
            let meta = match std::fs::metadata(&history_path) {
                Ok(m) => m,
                Err(_) => continue, // no history in this project
            };
            let ms = mtime_ms(&meta);
            seen.insert(key.clone());
            if let Some((cached_ms, cached_max, cached)) = cache.get(&key) {
                if *cached_ms == ms && *cached_max == max_sessions {
                    sessions.extend(cached.iter().cloned());
                    continue;
                }
            }
            let raw = match std::fs::read_to_string(&history_path) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let parsed = last_n(parse_aider_history(&raw, dir, &mtime_iso(ms)), max_sessions);
            cache.insert(key, (ms, max_sessions, parsed.clone()));
            sessions.extend(parsed);
        }
        cache.retain(|key, _| seen.contains(key));
        let total = sessions.len();
        // Across projects, keep the newest sessions, not whichever folder
        // happens to be listed last.
        sessions.sort_by_cached_key(|s| {
            std::cmp::Reverse(parse_date(&s.timestamp).map(|d| d.timestamp_millis()))
        });
        sessions.truncate(max_sessions);
        status.sessions = sessions.len();
        status.hint = if total == 0 {
            Some("No .aider.chat.history.md found in the configured folders.".to_string())
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
    fn parses_basic_history_fixture() {
        let sessions = parse_aider_history(
            &read_fixture("aider/basic.history.md"),
            "/fixtures/acme-shop",
            MTIME,
        );
        assert_eq!(sessions.len(), 2, "expected 2 sessions");
        for (i, session) in sessions.iter().enumerate() {
            assert_session(session, &format!("aider/expected/basic-{}.json", i));
        }
    }
}
