// The domain model — only what the ingestion pipeline produces. It mirrors
// src/types/domain.ts (camelCase keys, optional fields omitted when absent);
// the serialization is pinned by the fixture contract in the repo-root
// fixtures/, verified by `cargo test`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Strip a serialized session down to its list shape: the inbox needs
/// metadata, stats and risk — not the diff hunks or the full timeline.
/// Keys stay present (empty arrays) so the summary still fits the frontend's
/// `AgentSession` type (src/types/domain.ts).
pub fn strip_to_summary(v: &mut Value) {
    let Some(map) = v.as_object_mut() else { return };
    if map.contains_key("timeline") {
        map.insert("timeline".into(), json!([]));
    }
    if let Some(files) = map.get_mut("files").and_then(Value::as_array_mut) {
        for file in files {
            if let Some(f) = file.as_object_mut() {
                if f.contains_key("hunks") {
                    f.insert("hunks".into(), json!([]));
                }
            }
        }
    }
}

/// The summary (list) serialization of a session.
pub fn summary_value(session: &AgentSession) -> Value {
    let mut v = serde_json::to_value(session).unwrap_or(Value::Null);
    strip_to_summary(&mut v);
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_line: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimelineEventKind {
    Plan,
    Analysis,
    FileRead,
    FileEdit,
    Command,
    TestRun,
    Error,
    Decision,
    Note,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEvent {
    pub id: String,
    pub timestamp: String,
    pub kind: TimelineEventKind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
}

/// Timeline event without the id — `push_event` assigns `ev-<n>`.
#[derive(Debug, Clone)]
pub struct TimelineEventDraft {
    pub timestamp: String,
    pub kind: TimelineEventKind,
    pub title: String,
    pub detail: Option<String>,
    pub file_path: Option<String>,
    pub command: Option<String>,
    pub exit_code: Option<i64>,
}

impl TimelineEventDraft {
    pub fn new(
        timestamp: impl Into<String>,
        kind: TimelineEventKind,
        title: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: timestamp.into(),
            kind,
            title: title.into(),
            detail: None,
            file_path: None,
            command: None,
            exit_code: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RiskDimension {
    Security,
    Database,
    Performance,
    Maintainability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskScore {
    pub dimension: RiskDimension,
    pub score: i64,
    pub level: RiskLevel,
    pub rationale: String,
    pub contributing_file_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemArea {
    pub id: String,
    pub label: String,
    pub critical: bool,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub id: String,
    pub path: String,
    pub change_type: ChangeType,
    pub language: String,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub area_id: String,
    pub risk_flags: Vec<RiskDimension>,
    pub summary: String,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub files_analyzed: i64,
    pub files_modified: i64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub commands_run: i64,
    pub tests_run: i64,
    pub tests_failed: i64,
    pub duration_minutes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub text: String,
    pub impacted_areas: Vec<String>,
    pub risk_warnings: Vec<String>,
}

/// THE primary entity: the full lifecycle of an agent working on a task.
/// `findings` holds the line-anchored local findings the normalizer emits
/// (opaque `Value`s — the frontend owns their shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub timestamp: String,
    pub agent: String,
    pub agent_label: String,
    pub model: String,
    pub project_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    pub branch: String,
    pub original_prompt: String,
    pub declared_goal: String,
    pub initial_plan: Vec<String>,
    pub status: String,
    pub outcome: String,
    pub stats: SessionStats,
    pub summary: SessionSummary,
    pub system_areas: Vec<SystemArea>,
    pub files: Vec<FileChange>,
    pub timeline: Vec<TimelineEvent>,
    pub risks: Vec<RiskScore>,
    pub findings: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}

/// What an adapter extracts from its agent's native format.
#[derive(Debug, Clone)]
pub struct RawFile {
    pub path: String,
    pub created: bool,
    /// The transcript says the file was deleted (wins over `created`).
    pub deleted: bool,
    pub added: i64,
    pub removed: i64,
    pub hunks: Vec<DiffHunk>,
    /// Added source lines, used for content-based risk detection
    pub added_lines: Vec<String>,
}

impl RawFile {
    pub fn new(path: impl Into<String>, created: bool) -> Self {
        Self {
            path: path.into(),
            created,
            deleted: false,
            added: 0,
            removed: 0,
            hunks: Vec::new(),
            added_lines: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawSession {
    pub session_id: String,
    pub timestamp: String,
    pub agent: String,
    pub agent_label: String,
    pub model: String,
    pub project_name: String,
    pub project_path: Option<String>,
    pub branch: String,
    pub original_prompt: String,
    pub declared_goal: Option<String>,
    pub initial_plan: Vec<String>,
    pub outcome: String,
    pub timeline: Vec<TimelineEvent>,
    pub files: Vec<RawFile>,
    pub files_analyzed: i64,
    pub commands_run: i64,
    pub tests_run: i64,
    pub tests_failed: i64,
    pub errors: i64,
    pub duration_minutes: i64,
}
