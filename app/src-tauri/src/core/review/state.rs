// A project's review state, stored per work tree root. Decisions bind to
// content, never to positions or sessions:
//   - a hunk verdict is keyed by the hunk's content id, so it holds while
//     that exact change exists and survives everything else changing;
//   - a file verdict (files without hunks: binary, mode-only, empty) binds
//     to the file's fingerprint (HEAD's blob + the current bytes);
//   - a comment sits on a hunk id; when that hunk is gone the comment is
//     outdated (GitHub's rule) and shows as such.
// Rounds record each review sent to the agent: its text and a snapshot of
// every changed file, the base of "changes since your last review".

use super::super::paths::fold_case;
use super::super::storage;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Approved,
    Rejected,
    Changes,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HunkVerdict {
    pub verdict: Verdict,
    pub path: String,
    pub at: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileVerdict {
    pub verdict: Verdict,
    pub fingerprint: String,
    pub at: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub path: String,
    pub hunk_id: String,
    /// Line index within the hunk, and that line's kind and text: enough
    /// to find it again while the hunk exists
    pub li: usize,
    pub kind: String,
    pub line_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    pub text: String,
    /// Replacement code the reviewer proposed for the commented line
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    pub at: String,
    /// A previous round's comment the reviewer marked as dealt with
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resolved: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Round {
    pub index: u32,
    pub at: String,
    #[serde(default)]
    pub head: Option<String>,
    /// The review as the agent got it
    pub text: String,
    /// How many things it asked for
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<usize>,
    /// path → blob hash of the file as it was (None: deleted)
    pub files: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub comments: Vec<Comment>,
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReviewState {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub hunks: BTreeMap<String, HunkVerdict>,
    #[serde(default)]
    pub files: BTreeMap<String, FileVerdict>,
    /// path → note for the agent about the whole file
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
    #[serde(default)]
    pub comments: Vec<Comment>,
    /// finding id → accepted | dismissed | correction
    #[serde(default)]
    pub findings: BTreeMap<String, String>,
    /// The reviewer's edited review text (None: generated from the review)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    #[serde(default)]
    pub rounds: Vec<Round>,
}

pub const STATE_VERSION: u32 = 1;

/// One lock, in this process, around every write of a review state (saves,
/// rounds). Across processes (the app and the CLI) the revision check in
/// storage is what keeps writes from overwriting each other.
pub(crate) static STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
const MAX_COMMENTS: usize = 2_000;
const MAX_TEXT: usize = 20_000;
/// The review for the agent, as sent or as edited by hand
pub const MAX_REVIEW_TEXT: usize = 500_000;
const MAX_ENTRIES: usize = 20_000;

/// Take the state lock; it guards nothing but the order of writes, so a
/// panic elsewhere never leaves it unusable.
pub(crate) fn lock() -> std::sync::MutexGuard<'static, ()> {
    STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
/// Verdicts on changes that no longer exist are dropped after this long.
const STALE_DAYS: i64 = 60;

pub fn key_of(root: &str) -> String {
    fold_case(root.trim_end_matches('/')).into_owned()
}

/// The stored review of `root` and its revision (0: none yet).
pub fn load(root: &str) -> Result<(ReviewState, i64), String> {
    match storage::get_project_review(&key_of(root))? {
        None => Ok((ReviewState::default(), 0)),
        Some((rev, value)) => serde_json::from_value::<ReviewState>(value)
            .map(|s| (s, rev))
            .map_err(|e| format!("The stored review could not be read: {e}")),
    }
}

fn cap(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

/// Shape limits on a state the client sends. Rounds stay the server's,
/// except "done" on a comment of the last one.
fn sanitize(mut state: ReviewState, stored: &ReviewState) -> Result<ReviewState, String> {
    if state.comments.len() > MAX_COMMENTS
        || state.hunks.len() > MAX_ENTRIES
        || state.files.len() > MAX_ENTRIES
        || state.findings.len() > MAX_ENTRIES
        || state.notes.len() > MAX_ENTRIES
    {
        return Err("The review is too large to save.".into());
    }
    if state
        .draft
        .as_ref()
        .is_some_and(|d| d.len() > MAX_REVIEW_TEXT)
    {
        return Err("Your edited review is too long to save.".into());
    }
    for c in &mut state.comments {
        c.text = cap(&c.text);
        c.line_text = cap(&c.line_text);
        c.suggestion = c.suggestion.as_deref().map(cap);
        if !matches!(c.kind.as_str(), "add" | "del" | "context") {
            c.kind = "context".into();
        }
    }
    for note in state.notes.values_mut() {
        *note = cap(note);
    }
    state.notes.retain(|_, n| !n.trim().is_empty());
    state.draft = state.draft.filter(|d| !d.is_empty());
    state
        .findings
        .retain(|_, v| matches!(v.as_str(), "accepted" | "dismissed" | "correction"));
    let done: BTreeMap<String, bool> = match (state.rounds.last(), stored.rounds.last()) {
        (Some(mine), Some(theirs)) if mine.index == theirs.index => mine
            .comments
            .iter()
            .map(|c| (c.id.clone(), c.resolved))
            .collect(),
        _ => BTreeMap::new(),
    };
    state.rounds = stored.rounds.clone();
    if let Some(last) = state.rounds.last_mut() {
        for c in &mut last.comments {
            if let Some(resolved) = done.get(&c.id) {
                c.resolved = *resolved;
            }
        }
    }
    state.version = STATE_VERSION;
    Ok(state)
}

/// Drop verdicts older than STALE_DAYS whose change is gone, so a long-used
/// project never reaches the size limit. `live_hunks` and `live_paths` say
/// what the current review still shows.
pub fn prune(
    state: &mut ReviewState,
    live_hunks: &dyn Fn(&str) -> bool,
    live_paths: &dyn Fn(&str) -> bool,
) {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(STALE_DAYS);
    let old = |at: &str| {
        chrono::DateTime::parse_from_rfc3339(at)
            .map(|t| t < cutoff)
            .unwrap_or(false)
    };
    state.hunks.retain(|id, v| live_hunks(id) || !old(&v.at));
    state.files.retain(|p, v| live_paths(p) || !old(&v.at));
}

pub enum SaveError {
    /// Another writer saved first: here is what is stored now
    Conflict(Box<ReviewState>, i64),
    Failed(String),
}

/// Save a client's state over revision `expect_rev`.
pub fn save(
    root: &str,
    state: ReviewState,
    expect_rev: i64,
) -> Result<(ReviewState, i64), SaveError> {
    let (stored, rev) = load(root).map_err(SaveError::Failed)?;
    if rev != expect_rev {
        return Err(SaveError::Conflict(Box::new(stored), rev));
    }
    let state = sanitize(state, &stored).map_err(SaveError::Failed)?;
    write(root, &state, rev)
        .map_err(|e| match e {
            Some(msg) => SaveError::Failed(msg),
            None => match load(root) {
                Ok((s, r)) => SaveError::Conflict(Box::new(s), r),
                Err(msg) => SaveError::Failed(msg),
            },
        })
        .map(|next| (state, next))
}

/// Write `state` over `expect_rev`; Err(None) on a lost race.
pub fn write(root: &str, state: &ReviewState, expect_rev: i64) -> Result<i64, Option<String>> {
    let value = serde_json::to_value(state).map_err(|e| Some(e.to_string()))?;
    match storage::put_project_review(&key_of(root), root, expect_rev, &value) {
        Ok(Some(next)) => Ok(next),
        Ok(None) => Err(None),
        Err(e) => Err(Some(e)),
    }
}

pub fn blob_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    super::changes::hex(&Sha256::digest(bytes))
}

pub fn now() -> String {
    super::super::normalize::to_iso(chrono::Utc::now())
}
