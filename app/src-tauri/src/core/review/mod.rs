// The project review: everything uncommitted in a Git work tree, hunk by
// hunk. Git says what changed; the agents' transcripts only explain it.
// Each hunk carries the recorded edit that produced it (request, the
// agent's words, when), or says that no recorded edit did.
//
// On top of that: the heuristics, what is noise (moved code, reformatting,
// generated files, repeated changes), the reviewer's decisions (bound to
// content), and what changed since the last round.
//
// Built for `ars .`: one `git diff`, one untracked listing, and only this
// project's transcripts, never a scan of every agent's history.

pub mod assess;
pub mod attribution;
pub mod changes;
pub mod noise;
pub mod package;
pub mod state;

use super::config::load_config;
use super::domain::{
    AgentSession, RiskDimension, RiskScore, SessionStats, SessionTurn, SystemArea, TimelineEvent,
};
use super::git::{checked_file, git_raw, is_safe_rel};
use super::normalize::{language_of, parse_date};
use super::{scanner, storage};
use attribution::Origin;
use changes::{hex, ChangedFile, Line, LineKind, Status};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use state::{ReviewState, Round, SaveError, Verdict};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// Sessions read for attribution and context, newest first.
const SESSION_LIMIT: usize = 8;
/// Timeline events sent per session.
const TIMELINE_MAX: usize = 1_500;
/// Rounds kept per project.
const ROUNDS_MAX: usize = 20;
/// Files larger than this get no "since your last review" diff.
const SINCE_DIFF_MAX_BYTES: usize = 2_000_000;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub root: String,
    pub name: String,
    pub branch: String,
    pub head: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewHunk {
    pub id: String,
    pub header: String,
    pub lines: Vec<Line>,
    pub origin: Origin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noise: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SinceHunk {
    pub header: String,
    pub lines: Vec<Line>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFile {
    pub id: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub change_type: &'static str,
    pub untracked: bool,
    pub binary: bool,
    pub language: String,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub area_id: String,
    pub risk_flags: Vec<RiskDimension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated: Option<&'static str>,
    pub fingerprint: String,
    pub hunks: Vec<ReviewHunk>,
    pub verification: &'static str,
    pub session_ids: Vec<String>,
    /// Against the last round: changed | unchanged | new
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_hunks: Option<Vec<SinceHunk>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_change: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditGroup {
    pub id: String,
    pub hunk_ids: Vec<String>,
    pub paths: Vec<String>,
    /// The first changed line, to recognize the change at a glance
    pub preview: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContext {
    pub session_id: String,
    pub agent: String,
    pub agent_label: String,
    pub model: String,
    pub timestamp: String,
    pub last_activity: String,
    pub project_path: Option<String>,
    pub branch: String,
    pub original_prompt: String,
    pub declared_goal: String,
    pub initial_plan: Vec<String>,
    pub outcome: String,
    pub stats: SessionStats,
    pub timeline: Vec<TimelineEvent>,
    pub turns: Vec<SessionTurn>,
    /// Hunks of the review this session's recorded edits explain
    pub attributed_hunks: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SinceReview {
    pub round: u32,
    pub at: String,
    pub changed: Vec<String>,
    pub unchanged: Vec<String>,
    pub added: Vec<String>,
    /// In the last round, not uncommitted any more (committed or reverted)
    pub gone: Vec<String>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReviewStats {
    pub files: usize,
    pub added: i64,
    pub removed: i64,
    pub hunks: usize,
    pub agent_hunks: usize,
    pub partial_hunks: usize,
    pub unattributed_hunks: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectReview {
    pub project: ProjectInfo,
    /// Changes whenever anything shown changes (files, hunks, sessions)
    pub fingerprint: String,
    pub files: Vec<ReviewFile>,
    pub sessions: Vec<SessionContext>,
    pub findings: Vec<Value>,
    pub groups: Vec<EditGroup>,
    pub risks: Vec<RiskScore>,
    pub areas: Vec<SystemArea>,
    pub state: ReviewState,
    pub state_rev: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_review: Option<SinceReview>,
    pub stats: ReviewStats,
    pub warnings: Vec<String>,
    /// Session sources switched off in Settings
    pub disabled_sources: Vec<String>,
    /// The review for the agent, from the saved state
    pub package: package::Package,
}

/// What the review for the agent is written from, kept per project so a
/// saved decision gets its updated text without reading Git again.
struct PackageBasis {
    project: ProjectInfo,
    files: Vec<ReviewFile>,
    findings: Vec<Value>,
}

/// Projects whose last review is kept (the one open, and a few before it;
/// tests build many projects at once)
const BASIS_KEPT: usize = if cfg!(test) { 1_000 } else { 4 };

/// (project key, build number, basis), most recent last
type BasisCache = Vec<(String, u64, Arc<PackageBasis>)>;

static BASIS: Mutex<BasisCache> = Mutex::new(Vec::new());
/// Numbers builds, so a slow older build never replaces a newer one
static BUILDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn remember_basis(key: String, seq: u64, basis: PackageBasis) {
    let mut cache = BASIS.lock().unwrap_or_else(|e| e.into_inner());
    if cache.iter().any(|(k, s, _)| *k == key && *s > seq) {
        return;
    }
    cache.retain(|(k, _, _)| *k != key);
    cache.push((key, seq, Arc::new(basis)));
    let excess = cache.len().saturating_sub(BASIS_KEPT);
    cache.drain(..excess);
}

fn basis_of(root: &str) -> Option<Arc<PackageBasis>> {
    let key = state::key_of(root);
    let cache = BASIS.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, _, b)| b.clone())
}

/// The review for the agent from `state`, on the project's last built
/// review (None before one was built in this process).
fn package_for(root: &str, s: &ReviewState) -> Option<package::Package> {
    let basis = basis_of(root)?;
    Some(package::build(
        &basis.project,
        &basis.files,
        &basis.findings,
        s,
    ))
}

/// The current bytes of a changed file as they are on disk, whatever Git
/// thinks of it: a file Git sees as deleted may still exist (removed from
/// the index only), and its fingerprint must say so. None: not on disk, or
/// not readable through a safe path.
pub(crate) fn read_current(root: &str, path: &str) -> Option<Vec<u8>> {
    std::fs::read(checked_file(root, path).ok()?).ok()
}

/// What a file verdict binds to: the path, HEAD's blob and the bytes now.
pub(crate) fn file_fingerprint(
    path: &str,
    old_path: Option<&str>,
    mode_change: Option<&str>,
    base_blob: Option<&str>,
    bytes: Option<&[u8]>,
) -> String {
    let mut h = Sha256::new();
    h.update(b"ars-file-v2\0");
    for part in [Some(path), old_path, mode_change, base_blob] {
        h.update(part.unwrap_or("-").as_bytes());
        h.update(b"\0");
    }
    match bytes {
        Some(b) => h.update(Sha256::digest(b)),
        None => h.update(b"deleted"),
    }
    format!("fp-{}", hex(&h.finalize()[..16]))
}

fn file_id(path: &str) -> String {
    format!("f-{}", hex(&Sha256::digest(path.as_bytes())[..6]))
}

fn change_type(status: Status) -> &'static str {
    match status {
        Status::Added | Status::Untracked => "created",
        Status::Modified => "modified",
        Status::Deleted => "deleted",
        Status::Renamed => "renamed",
    }
}

/// The diff from the version a round recorded to the current one
/// (patience, 3 lines of context), in the review's line shape.
fn since_hunks(old: &str, new: &str) -> Vec<SinceHunk> {
    use similar::{Algorithm, ChangeTag, TextDiff};
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .timeout(std::time::Duration::from_secs(1))
        .diff_lines(old, new);
    let mut out = Vec::new();
    for group in diff.grouped_ops(3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old_start = first.old_range().start;
        let new_start = first.new_range().start;
        let old_len = last.old_range().end - old_start;
        let new_len = last.new_range().end - new_start;
        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let text = change.value().trim_end_matches(['\n', '\r']).to_string();
                let num = |i: Option<usize>| i.map(|n| n as i64 + 1);
                let (kind, old_line, new_line) = match change.tag() {
                    ChangeTag::Equal => (
                        LineKind::Context,
                        num(change.old_index()),
                        num(change.new_index()),
                    ),
                    ChangeTag::Delete => (LineKind::Del, num(change.old_index()), None),
                    ChangeTag::Insert => (LineKind::Add, None, num(change.new_index())),
                };
                lines.push(Line {
                    kind,
                    text,
                    old_line,
                    new_line,
                    moved: false,
                    moved_peer: None,
                });
            }
        }
        out.push(SinceHunk {
            header: format!(
                "@@ -{},{} +{},{} @@",
                old_start + 1,
                old_len,
                new_start + 1,
                new_len
            ),
            lines,
        });
    }
    out
}

fn last_activity(s: &AgentSession) -> String {
    s.timeline
        .iter()
        .rev()
        .find(|e| parse_date(&e.timestamp).is_some())
        .map(|e| e.timestamp.clone())
        .unwrap_or_else(|| s.timestamp.clone())
}

fn context_of(s: AgentSession, attributed: usize) -> SessionContext {
    let last = last_activity(&s);
    let skip = s.timeline.len().saturating_sub(TIMELINE_MAX);
    SessionContext {
        session_id: s.session_id,
        agent: s.agent,
        agent_label: s.agent_label,
        model: s.model,
        timestamp: s.timestamp,
        last_activity: last,
        project_path: s.project_path,
        branch: s.branch,
        original_prompt: s.original_prompt,
        declared_goal: s.declared_goal,
        initial_plan: s.initial_plan,
        outcome: s.outcome,
        stats: s.stats,
        timeline: s.timeline.into_iter().skip(skip).collect(),
        turns: s.turns,
        attributed_hunks: attributed,
    }
}

fn preview_of(files: &[ReviewFile], hunk_id: &str) -> String {
    files
        .iter()
        .flat_map(|f| &f.hunks)
        .find(|h| h.id == hunk_id)
        .and_then(|h| {
            h.lines
                .iter()
                .find(|l| l.kind != LineKind::Context && !l.text.trim().is_empty())
        })
        .map(|l| {
            format!(
                "{}{}",
                if l.kind == LineKind::Add { "+ " } else { "- " },
                l.text.trim()
            )
        })
        .map(|t| t.chars().take(120).collect())
        .unwrap_or_default()
}

/// Build the review of the project containing `project`.
pub fn build(project: &str) -> Result<ProjectReview, String> {
    if !std::path::Path::new(project).is_dir() {
        return Err(format!("This folder no longer exists: {project}"));
    }
    let root = changes::toplevel(project)?;
    let seq = BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut warnings: Vec<String> = Vec::new();
    // Git and the project's transcripts are read side by side.
    let (cs, sessions) = std::thread::scope(|sc| {
        let sessions = sc.spawn(|| scanner::project_sessions(&root, SESSION_LIMIT));
        let cs = changes::collect(&root);
        (cs, sessions.join())
    });
    let sessions = sessions.unwrap_or_else(|_| {
        warnings.push(
            "The agents' sessions could not be read, so the changes come without their reasons."
                .into(),
        );
        Vec::new()
    });
    let mut cs = cs?;
    warnings.append(&mut cs.warnings);
    warnings.extend(super::rules::repo_rules_problem(&root));
    noise::mark_moved(&mut cs.files);
    let group_of = noise::edit_groups(&cs.files);
    let (attrs, attribution, assessment) = std::thread::scope(|sc| {
        let attrs = sc.spawn(|| {
            let paths: Vec<&str> = cs.files.iter().map(|f| f.path.as_str()).collect();
            noise::generated_attrs(&root, &paths)
        });
        let attribution = attribution::attribute(&root, &cs.files, &sessions);
        let assessment = assess::assess(&root, &cs.files, &load_config().finding_rules);
        (attrs.join(), attribution, assessment)
    });
    let attrs = match attrs {
        Ok(Ok(attrs)) => attrs,
        Ok(Err(e)) => {
            warnings.push(e);
            HashMap::new()
        }
        Err(_) => {
            warnings.push("Generated files could not be told apart.".into());
            HashMap::new()
        }
    };
    let (state, state_rev) = state::load(&root)?;
    let round: Option<&Round> = state.rounds.last();

    let mut stats = ReviewStats::default();
    let mut digest = Sha256::new();
    digest.update(cs.head.as_deref().unwrap_or("-").as_bytes());
    let mut files: Vec<ReviewFile> = Vec::new();
    let mut since = round.map(|r| SinceReview {
        round: r.index,
        at: r.at.clone(),
        changed: Vec::new(),
        unchanged: Vec::new(),
        added: Vec::new(),
        gone: Vec::new(),
    });
    for f in cs.files {
        let ChangedFile {
            path,
            old_path,
            status,
            binary,
            base_blob,
            hunks,
            added,
            removed,
            truncated,
            mode_change,
        } = f;
        let bytes = read_current(&root, &path);
        let fingerprint = file_fingerprint(
            &path,
            old_path.as_deref(),
            mode_change.as_deref(),
            base_blob.as_deref(),
            bytes.as_deref(),
        );
        digest.update(fingerprint.as_bytes());

        // Against the last round
        let (since_state, since_diff) = match round {
            None => (None, None),
            Some(r) => match r.files.get(&path) {
                None => {
                    if let Some(s) = since.as_mut() {
                        s.added.push(path.clone());
                    }
                    (Some("new"), None)
                }
                Some(snapshot) => {
                    let now = bytes.as_deref().map(state::blob_hash);
                    if now.as_deref() == snapshot.as_deref() {
                        if let Some(s) = since.as_mut() {
                            s.unchanged.push(path.clone());
                        }
                        (Some("unchanged"), None)
                    } else {
                        if let Some(s) = since.as_mut() {
                            s.changed.push(path.clone());
                        }
                        let old = match snapshot {
                            Some(hash) => storage::get_blob(hash).unwrap_or_else(|e| {
                                warnings.push(format!(
                                    "The last review's copy of {path} could not be read: {e}"
                                ));
                                None
                            }),
                            None => Some(Vec::new()),
                        };
                        let diff = match (old, bytes.as_deref()) {
                            (Some(old), Some(new))
                                if !binary
                                    && old.len() <= SINCE_DIFF_MAX_BYTES
                                    && new.len() <= SINCE_DIFF_MAX_BYTES =>
                            {
                                Some(since_hunks(
                                    &String::from_utf8_lossy(&old),
                                    &String::from_utf8_lossy(new),
                                ))
                            }
                            _ => None,
                        };
                        (Some("changed"), diff)
                    }
                }
            },
        };

        let mut review_hunks = Vec::new();
        for h in hunks {
            let origin = attribution
                .hunks
                .get(&h.id)
                .cloned()
                .unwrap_or_else(|| Origin::none(0));
            stats.hunks += 1;
            match origin.kind {
                "agent" => stats.agent_hunks += 1,
                "partial" => stats.partial_hunks += 1,
                _ => stats.unattributed_hunks += 1,
            }
            digest.update(h.id.as_bytes());
            digest.update(origin.kind.as_bytes());
            digest.update(origin.event_id.as_deref().unwrap_or("").as_bytes());
            review_hunks.push(ReviewHunk {
                noise: noise::hunk_noise(&h, &language_of(&path)),
                group: group_of.get(&h.id).cloned(),
                id: h.id,
                header: h.header,
                truncated: h.truncated,
                lines: h.lines,
                origin,
            });
        }
        let (area_id, risk_flags) = assessment.files.get(&path).cloned().unwrap_or_default();
        stats.files += 1;
        stats.added += added;
        stats.removed += removed;
        files.push(ReviewFile {
            id: file_id(&path),
            verification: assess::verification(&root, &path, &sessions),
            session_ids: attribution
                .file_sessions
                .get(&path)
                .cloned()
                .unwrap_or_default(),
            generated: noise::generated_reason(&path, &attrs),
            language: language_of(&path),
            change_type: change_type(status),
            untracked: status == Status::Untracked,
            binary,
            lines_added: added,
            lines_removed: removed,
            area_id,
            risk_flags,
            fingerprint,
            hunks: review_hunks,
            since: since_state,
            since_hunks: since_diff,
            truncated,
            mode_change,
            old_path,
            path,
        });
    }
    if let (Some(s), Some(r)) = (since.as_mut(), round) {
        s.gone = r
            .files
            .keys()
            .filter(|p| !files.iter().any(|f| &f.path == *p))
            .cloned()
            .collect();
    }

    // Identical changes in several places
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in &files {
        for h in &f.hunks {
            if let Some(g) = &h.group {
                groups.entry(g.clone()).or_default().push(h.id.clone());
            }
        }
    }
    let groups: Vec<EditGroup> = groups
        .into_iter()
        .map(|(id, hunk_ids)| EditGroup {
            preview: preview_of(&files, &hunk_ids[0]),
            paths: files
                .iter()
                .filter(|f| f.hunks.iter().any(|h| h.group.as_deref() == Some(&id)))
                .map(|f| f.path.clone())
                .collect(),
            id,
            hunk_ids,
        })
        .collect();

    // Context: sessions that explain something, plus the newest one.
    let newest = sessions.first().map(|s| s.session_id.clone());
    let contexts: Vec<SessionContext> = sessions
        .into_iter()
        .filter_map(|s| {
            let n = attribution
                .per_session
                .get(&s.session_id)
                .copied()
                .unwrap_or(0);
            let touches = attribution
                .file_sessions
                .values()
                .any(|ids| ids.contains(&s.session_id));
            (n > 0 || touches || Some(&s.session_id) == newest.as_ref()).then(|| context_of(s, n))
        })
        .collect();
    for s in &contexts {
        digest.update(s.session_id.as_bytes());
        digest.update(s.last_activity.as_bytes());
    }

    let name = root.rsplit('/').next().unwrap_or(&root).to_string();
    let project = ProjectInfo {
        name,
        branch: cs.branch,
        head: cs.head,
        root: root.clone(),
    };
    let findings = assessment.findings;
    let package = package::build(&project, &files, &findings, &state);
    remember_basis(
        state::key_of(&root),
        seq,
        PackageBasis {
            project: project.clone(),
            files: files.clone(),
            findings: findings.clone(),
        },
    );
    Ok(ProjectReview {
        project,
        fingerprint: format!("r-{}", hex(&digest.finalize()[..12])),
        files,
        sessions: contexts,
        findings,
        groups,
        risks: assessment.risks,
        areas: assessment.areas,
        state,
        state_rev,
        since_review: since,
        stats,
        warnings,
        disabled_sources: scanner::disabled_source_labels(),
        package,
    })
}

/// Save the client's review state over revision `expect_rev`.
pub fn save_state(project: &str, body: &Value) -> Value {
    let root = match changes::toplevel(project) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let Some(expect) = body.get("rev").and_then(Value::as_i64) else {
        return json!({ "error": "Missing rev." });
    };
    let mut incoming = match body
        .get("state")
        .cloned()
        .map(serde_json::from_value::<ReviewState>)
    {
        Some(Ok(s)) => s,
        Some(Err(e)) => return json!({ "error": format!("Invalid review state: {e}") }),
        None => return json!({ "error": "Missing state." }),
    };
    if let Some(basis) = basis_of(&root) {
        let hunks: std::collections::HashSet<&str> = basis
            .files
            .iter()
            .flat_map(|f| f.hunks.iter().map(|h| h.id.as_str()))
            .collect();
        let paths: std::collections::HashSet<&str> =
            basis.files.iter().map(|f| f.path.as_str()).collect();
        state::prune(&mut incoming, &|id| hunks.contains(id), &|p| {
            paths.contains(p)
        });
    }
    let _guard = state::lock();
    match state::save(&root, incoming, expect) {
        Ok((s, rev)) => json!({ "state": s, "stateRev": rev, "package": package_for(&root, &s) }),
        Err(SaveError::Conflict(s, rev)) => conflict(*s, rev),
        Err(SaveError::Failed(e)) => json!({ "error": e }),
    }
}

/// The bytes of every changed file now, by path (None: not on disk). The
/// hashes are a round's snapshot.
fn current_files(root: &str, paths: &[&str]) -> BTreeMap<String, Option<Vec<u8>>> {
    paths
        .iter()
        .map(|p| (p.to_string(), read_current(root, p)))
        .collect()
}

/// Record a round: the review the agent got, a snapshot of every changed
/// file (the base of "since last review") and the comments it carried.
/// Comments of the previous round the agent left untouched carry on, so
/// they keep coming back until they are resolved, approved or their code
/// changes. Nothing in the project changes.
pub fn handback(project: &str, body: &Value) -> Value {
    let root = match changes::toplevel(project) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let Some(expect) = body.get("rev").and_then(Value::as_i64) else {
        return json!({ "error": "Missing rev." });
    };
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if text.len() > state::MAX_REVIEW_TEXT {
        return json!({ "error": "The review is too long to record." });
    }
    let items = body
        .get("items")
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    let _guard = state::lock();
    let (mut s, rev) = match state::load(&root) {
        Ok(v) => v,
        Err(e) => return json!({ "error": e }),
    };
    if rev != expect {
        return conflict(s, rev);
    }
    let cs = match changes::collect(&root) {
        Ok(cs) => cs,
        Err(e) => return json!({ "error": e }),
    };
    let paths: Vec<&str> = cs.files.iter().map(|f| f.path.as_str()).collect();
    let mut snapshot: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (f, (path, bytes)) in cs.files.iter().zip(current_files(&root, &paths)) {
        let entry = bytes.map(|bytes| {
            let hash = state::blob_hash(&bytes);
            // Only what a "since last review" diff can show is kept
            let shown = !f.binary && bytes.len() <= SINCE_DIFF_MAX_BYTES;
            (hash, shown.then_some(bytes))
        });
        if let Some((hash, Some(bytes))) = &entry {
            if let Err(e) = storage::put_blob(hash, bytes) {
                return json!({ "error": e });
            }
        }
        snapshot.insert(path, entry.map(|(hash, _)| hash));
    }
    let live: std::collections::HashSet<&str> = cs
        .files
        .iter()
        .flat_map(|f| f.hunks.iter().map(|h| h.id.as_str()))
        .collect();
    let mut comments = std::mem::take(&mut s.comments);
    let carried: Vec<state::Comment> = s
        .rounds
        .last()
        .map(|r| r.comments.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|c| !c.resolved && live.contains(c.hunk_id.as_str()))
        .filter(|c| s.hunks.get(&c.hunk_id).map(|v| v.verdict) != Some(Verdict::Approved))
        .filter(|old| {
            !comments
                .iter()
                .any(|c| c.hunk_id == old.hunk_id && c.line_text == old.line_text)
        })
        .collect();
    comments.extend(carried);
    let round = Round {
        index: s.rounds.last().map(|r| r.index + 1).unwrap_or(1),
        at: state::now(),
        head: cs.head.clone(),
        text,
        items,
        files: snapshot,
        comments,
        notes: std::mem::take(&mut s.notes),
    };
    s.rounds.push(round);
    let excess = s.rounds.len().saturating_sub(ROUNDS_MAX);
    s.rounds.drain(..excess);
    s.draft = None;
    match state::write(&root, &s, rev) {
        Ok(next) => {
            // Snapshots of rounds that are gone; a failure here costs disk
            // space only, never the round just recorded.
            let _ = storage::prune_blobs();
            json!({ "ok": true, "state": s, "stateRev": next, "package": package_for(&root, &s) })
        }
        Err(Some(e)) => json!({ "error": e }),
        Err(None) => match state::load(&root) {
            Ok((s, rev)) => conflict(s, rev),
            Err(e) => json!({ "error": e }),
        },
    }
}

fn conflict(s: ReviewState, rev: i64) -> Value {
    json!({
        "error": "The review changed elsewhere. The latest version is now loaded.",
        "conflict": true,
        "state": s,
        "stateRev": rev,
    })
}

/// What `ars feedback` gives the agent
pub struct Feedback {
    pub text: String,
    pub items: usize,
    /// Files the reviewer has not decided yet
    pub undecided: usize,
}

/// The review for the agent, as the reviewer left it (their edited text
/// when they edited it), recorded as a round like the app's Copy review:
/// the next review shows only what changes from here. Asked again before
/// anything changed (the agent ran it twice), it prints the same review
/// and records nothing. Nothing to ask: no round.
pub fn feedback(project: &str) -> Result<Feedback, String> {
    let review = build(project)?;
    let s = &review.state;
    let undecided = review
        .files
        .iter()
        .filter(|f| package::file_view(s, f) == package::FileView::Pending)
        .count();
    if let Some(last) = s.rounds.last().filter(|r| nothing_new_since(&review, r)) {
        return Ok(Feedback {
            text: last.text.clone(),
            items: last.items.unwrap_or(review.package.items),
            undecided,
        });
    }
    let draft = s.draft.clone().filter(|d| !d.trim().is_empty());
    let items = review.package.items;
    if items == 0 && draft.is_none() {
        return Ok(Feedback {
            text: String::new(),
            items: 0,
            undecided,
        });
    }
    let text = draft.unwrap_or_else(|| review.package.text.clone());
    let done = handback(
        &review.project.root,
        &json!({ "rev": review.state_rev, "text": text, "items": items }),
    );
    if done["conflict"] == true {
        return Err("The review changed while it was being read. Run `ars feedback` again.".into());
    }
    if let Some(e) = done.get("error").and_then(Value::as_str) {
        return Err(e.to_string());
    }
    Ok(Feedback {
        text,
        items,
        undecided,
    })
}

/// No decision, comment or note since `round`, and every file as it was
fn nothing_new_since(review: &ProjectReview, round: &Round) -> bool {
    let s = &review.state;
    let decided_after =
        s.hunks.values().any(|v| v.at > round.at) || s.files.values().any(|v| v.at > round.at);
    if decided_after || !s.comments.is_empty() || !s.notes.is_empty() || s.draft.is_some() {
        return false;
    }
    let paths: Vec<&str> = review.files.iter().map(|f| f.path.as_str()).collect();
    let now: BTreeMap<String, Option<String>> = current_files(&review.project.root, &paths)
        .into_iter()
        .map(|(p, bytes)| (p, bytes.map(|b| state::blob_hash(&b))))
        .collect();
    now == round.files
}

/// Lines a whole-file read returns (minified bundles, dumps stop here)
const FILE_VIEW_MAX_LINES: usize = 50_000;
/// Bytes a whole-file read accepts
const FILE_VIEW_MAX_BYTES: u64 = 8_000_000;

/// The lines of one file of the review, as it is now or (`base`) as HEAD
/// has it: they feed "expand context", the whole-file view and syntax
/// colors. Only files the review shows can be read.
/// Where a file of the review is on disk, to show it in the file manager:
/// the file itself, or for a deleted file the nearest folder still there.
pub fn located(project: &str, path: &str) -> Result<std::path::PathBuf, String> {
    let root = changes::toplevel(project)?;
    let in_review = basis_of(&root).is_some_and(|b| b.files.iter().any(|f| f.path == path));
    if !in_review {
        return Err(format!("{path} is not a file of this review."));
    }
    let full = checked_file(&root, path)?;
    Ok(full
        .ancestors()
        .find(|p| p.exists())
        .map(std::path::Path::to_path_buf)
        .unwrap_or(full))
}

pub fn file_lines(project: &str, path: &str, base: bool) -> Value {
    let root = match changes::toplevel(project) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let in_review = basis_of(&root).is_some_and(|b| {
        b.files
            .iter()
            .any(|f| f.path == path || (base && f.old_path.as_deref() == Some(path)))
    });
    if !in_review || !is_safe_rel(path) {
        return json!({ "error": format!("{path} is not a file of this review.") });
    }
    let too_big =
        || json!({ "error": format!("{path} is too large to show here. Open it in your editor.") });
    let raw = if base {
        let spec = format!("HEAD:{path}");
        let size = git_raw(&root, &["cat-file", "-s", &spec], None);
        let bytes: u64 = String::from_utf8_lossy(&size.out)
            .trim()
            .parse()
            .unwrap_or(0);
        if bytes > FILE_VIEW_MAX_BYTES {
            return too_big();
        }
        let o = git_raw(&root, &["cat-file", "blob", &spec], None);
        if !o.ok {
            return json!({ "error": format!("{path} is not in HEAD.") });
        }
        o.out
    } else {
        let full = match checked_file(&root, path) {
            Ok(p) => p,
            Err(e) => return json!({ "error": e }),
        };
        if std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0) > FILE_VIEW_MAX_BYTES {
            return too_big();
        }
        match std::fs::read(full) {
            Ok(raw) => raw,
            Err(e) => return json!({ "error": format!("Could not read {path}: {e}") }),
        }
    };
    let text = String::from_utf8_lossy(&raw).replace('\r', "");
    let lines: Vec<&str> = text.split('\n').take(FILE_VIEW_MAX_LINES).collect();
    json!({ "lines": lines })
}

/// Projects for the home screen: every project with indexed agent sessions
/// or a stored review, newest activity first, with its pending changes.
/// A project whose sessions are all hidden is left out; `hidden` lists
/// those too, so they can be shown again.
pub fn projects(hidden: bool) -> Value {
    let state = scanner::ensure_scanned();
    let mut by_root: BTreeMap<String, ProjectRow> = BTreeMap::new();
    for id in &state.order {
        let Some(record) = state.by_id.get(id) else {
            continue;
        };
        let s = &record.session;
        let Some(path) = s.project_path.as_deref() else {
            continue;
        };
        let (root, git) = root_of(path);
        let row = by_root
            .entry(state::key_of(&root))
            .or_insert_with(|| ProjectRow::new(&root, git));
        row.add(s, record.archived);
    }
    let mut warning = None;
    match storage::list_project_reviews() {
        Ok(reviewed) => {
            for (root, updated) in reviewed {
                let row = by_root
                    .entry(state::key_of(&root))
                    .or_insert_with(|| ProjectRow::new(&root, true));
                row.reviewed_at = Some(updated);
            }
        }
        Err(e) => warning = Some(e),
    }
    let hidden_count = by_root.values().filter(|r| r.all_archived()).count();
    let mut rows: Vec<ProjectRow> = by_root
        .into_values()
        .filter(|r| hidden || !r.all_archived())
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.activity_key()));
    rows.truncate(40);
    // Pending changes per project, counted in parallel (one `git status` each)
    let counts: Vec<Option<usize>> = std::thread::scope(|scope| {
        let handles: Vec<_> = rows
            .iter()
            .map(|r| {
                let root = r.root.clone();
                let git = r.git;
                scope.spawn(move || git.then(|| pending_changes(&root)).flatten())
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().ok().flatten())
            .collect()
    });
    let list: Vec<Value> = rows
        .into_iter()
        .zip(counts)
        .map(|(r, changes)| r.to_value(changes))
        .collect();
    json!({ "projects": list, "hiddenCount": hidden_count, "warning": warning })
}

/// Uncommitted files, counted as the review counts them: a rename once,
/// every untracked file on its own.
fn pending_changes(root: &str) -> Option<usize> {
    let r = super::git::git(
        root,
        &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    );
    r.ok.then(|| super::git::status_paths(&r.out).len())
}

/// Work tree root of a session's directory, cached (git per new path only).
fn root_of(path: &str) -> (String, bool) {
    use once_cell::sync::Lazy;
    use std::sync::Mutex;
    static CACHE: Lazy<Mutex<HashMap<String, (String, bool)>>> =
        Lazy::new(|| Mutex::new(HashMap::new()));
    if let Some(hit) = CACHE.lock().ok().and_then(|c| c.get(path).cloned()) {
        return hit;
    }
    let found = if std::path::Path::new(path).is_dir() {
        match changes::toplevel(path) {
            Ok(root) => (root, true),
            Err(_) => (path.replace('\\', "/"), false),
        }
    } else {
        (path.replace('\\', "/"), false)
    };
    if let Ok(mut c) = CACHE.lock() {
        c.insert(path.to_string(), found.clone());
    }
    found
}

struct ProjectRow {
    root: String,
    git: bool,
    sessions: Vec<Value>,
    /// Every session of the project: hiding the project hides them all
    session_ids: Vec<String>,
    session_count: usize,
    archived_count: usize,
    agents: Vec<String>,
    last_activity: String,
    reviewed_at: Option<String>,
}

impl ProjectRow {
    fn new(root: &str, git: bool) -> Self {
        Self {
            root: root.to_string(),
            git,
            sessions: Vec::new(),
            session_ids: Vec::new(),
            session_count: 0,
            archived_count: 0,
            agents: Vec::new(),
            last_activity: String::new(),
            reviewed_at: None,
        }
    }

    fn add(&mut self, s: &AgentSession, archived: bool) {
        self.session_count += 1;
        self.session_ids.push(s.session_id.clone());
        if archived {
            self.archived_count += 1;
        }
        if !self.agents.contains(&s.agent_label) {
            self.agents.push(s.agent_label.clone());
        }
        if s.timestamp > self.last_activity {
            self.last_activity = s.timestamp.clone();
        }
        if self.sessions.len() < 5 && !archived {
            self.sessions.push(json!({
                "sessionId": s.session_id,
                "agent": s.agent,
                "agentLabel": s.agent_label,
                "declaredGoal": s.declared_goal,
                "timestamp": s.timestamp,
                "filesModified": s.stats.files_modified,
            }));
        }
    }

    /// Hidden from the home screen: every session is hidden
    fn all_archived(&self) -> bool {
        self.session_count > 0 && self.archived_count == self.session_count
    }

    fn activity_key(&self) -> String {
        self.last_activity
            .clone()
            .max(self.reviewed_at.clone().unwrap_or_default())
    }

    fn to_value(&self, changes: Option<usize>) -> Value {
        json!({
            "root": self.root,
            "name": self.root.rsplit('/').next().unwrap_or(&self.root),
            "git": self.git,
            "missing": !std::path::Path::new(&self.root).is_dir(),
            "hidden": self.all_archived(),
            "sessions": self.sessions,
            "sessionIds": self.session_ids,
            "sessionCount": self.session_count,
            "agents": self.agents,
            "lastActivity": self.last_activity,
            "reviewedAt": self.reviewed_at,
            "changes": changes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_core::git::{git, test_repo};

    /// Machine-dependent smoke, run explicitly:
    /// `ARS_SMOKE_PROJECT=<repo> cargo test --release --lib smoke_review -- --ignored --nocapture`.
    /// Builds the real review of that project from its own transcripts
    /// (read-only; the review state comes from the test database) and prints
    /// what the window would show, with timings.
    #[test]
    #[ignore]
    fn smoke_review() {
        let Ok(project) = std::env::var("ARS_SMOKE_PROJECT") else {
            return;
        };
        let t = std::time::Instant::now();
        let r = build(&project).expect("review");
        let cold = t.elapsed();
        let t = std::time::Instant::now();
        build(&project).expect("review");
        let warm = t.elapsed();
        println!(
            "{} files, {} hunks: {} from recorded edits, {} partly, {} with none on record",
            r.stats.files,
            r.stats.hunks,
            r.stats.agent_hunks,
            r.stats.partial_hunks,
            r.stats.unattributed_hunks
        );
        println!(
            "sessions: {} · findings: {} · groups: {}",
            r.sessions.len(),
            r.findings.len(),
            r.groups.len()
        );
        println!("build: first {cold:?}, again {warm:?}");
        for f in &r.files {
            let kinds: Vec<&str> = f.hunks.iter().map(|h| h.origin.kind).collect();
            let noise: Vec<&str> = f.hunks.iter().filter_map(|h| h.noise).collect();
            println!(
                "  {:55} +{:<4} -{:<4} {:?} {:?} {:?}",
                f.path, f.lines_added, f.lines_removed, kinds, noise, f.generated
            );
        }
    }

    #[test]
    fn copying_the_review_never_touches_the_project() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap().replace('\\', "/");
        std::fs::write(dir.0.join("a.txt"), "rejected\n").unwrap();
        let review = build(&root).expect("review");
        let f = &review.files[0];
        let mut s = review.state.clone();
        s.hunks.insert(
            f.hunks[0].id.clone(),
            state::HunkVerdict {
                verdict: Verdict::Rejected,
                path: f.path.clone(),
                at: state::now(),
            },
        );
        let rev = save_state(&root, &json!({ "rev": review.state_rev, "state": s }))["stateRev"]
            .as_i64()
            .expect("saved");
        assert_eq!(
            handback(&root, &json!({ "rev": rev - 1, "text": "p" }))["conflict"],
            true
        );
        let done = handback(&root, &json!({ "rev": rev, "text": "p" }));
        assert_eq!(done["ok"], true, "{done}");
        assert_eq!(
            std::fs::read_to_string(dir.0.join("a.txt")).unwrap(),
            "rejected\n"
        );
        assert_eq!(git(&root, &["status", "--porcelain"]).out.trim(), "M a.txt");
    }

    #[test]
    fn feedback_gives_the_agent_the_review_and_starts_the_next_round() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap().replace('\\', "/");
        std::fs::write(dir.0.join("b.txt"), "agent wrote this\n").unwrap();
        let none = feedback(&root).expect("feedback");
        assert_eq!((none.items, none.undecided), (0, 1), "nothing asked yet");
        assert!(
            state::load(&root).unwrap().0.rounds.is_empty(),
            "no round when nothing was asked"
        );

        let review = build(&root).expect("review");
        let f = &review.files[0];
        let mut s = review.state.clone();
        s.hunks.insert(
            f.hunks[0].id.clone(),
            state::HunkVerdict {
                verdict: Verdict::Rejected,
                path: f.path.clone(),
                at: state::now(),
            },
        );
        let saved = save_state(&root, &json!({ "rev": review.state_rev, "state": s }));
        assert_eq!(
            saved["package"]["items"], 1,
            "a save returns the updated review: {saved}"
        );

        let fb = feedback(&root).expect("feedback");
        assert_eq!(fb.items, 1);
        assert!(
            fb.text
                .contains("UNDO all your changes to this file: `git checkout HEAD -- b.txt`"),
            "{}",
            fb.text
        );
        let after = state::load(&root).unwrap().0;
        assert_eq!(after.rounds.len(), 1);
        assert_eq!(
            after.rounds[0].text, fb.text,
            "the round records what the agent got"
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join("b.txt")).unwrap(),
            "agent wrote this\n"
        );
    }

    #[test]
    fn open_comments_carry_on_and_feedback_twice_repeats_itself() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap().replace('\\', "/");
        std::fs::write(dir.0.join("a.txt"), "after\n").unwrap();
        let review = build(&root).expect("review");
        let hunk = review.files[0].hunks[0].clone();
        let mut s = review.state.clone();
        s.comments.push(state::Comment {
            id: "c1".into(),
            path: "a.txt".into(),
            hunk_id: hunk.id.clone(),
            li: 1,
            kind: "add".into(),
            line_text: "after".into(),
            line: Some(1),
            text: "say why".into(),
            suggestion: None,
            at: state::now(),
            resolved: false,
        });
        save_state(&root, &json!({ "rev": review.state_rev, "state": s }));

        let first = feedback(&root).expect("feedback");
        assert!(
            first.text.contains("CHANGE line 1 `after`: say why"),
            "{}",
            first.text
        );
        let again = feedback(&root).expect("feedback");
        assert_eq!(again.text, first.text, "nothing new: the same review");
        assert_eq!(
            state::load(&root).unwrap().0.rounds.len(),
            1,
            "and no new round"
        );

        // The agent ignored it: a new decision starts round 2, and the
        // comment is still asked for, then carried into that round.
        let (mut s, rev) = state::load(&root).unwrap();
        s.files.insert(
            "b.txt".into(),
            state::FileVerdict {
                verdict: Verdict::Approved,
                fingerprint: "x".into(),
                at: state::now(),
            },
        );
        save_state(&root, &json!({ "rev": rev, "state": s }));
        let second = feedback(&root).expect("feedback");
        assert!(
            second
                .text
                .contains("say why (still open from my last review)"),
            "{}",
            second.text
        );
        let (s, rev) = state::load(&root).unwrap();
        assert_eq!(s.rounds.len(), 2);
        assert_eq!(s.rounds[1].comments.len(), 1, "carried into round 2");

        // Marking it done is stored, and it stops coming back.
        let mut done = s.clone();
        done.rounds[1].comments[0].resolved = true;
        let saved = save_state(&root, &json!({ "rev": rev, "state": done }));
        assert!(
            state::load(&root).unwrap().0.rounds[1].comments[0].resolved,
            "{saved}"
        );
        assert_eq!(saved["package"]["items"], 0);
    }

    #[test]
    fn saving_drops_old_verdicts_on_changes_that_are_gone() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap().replace('\\', "/");
        std::fs::write(dir.0.join("a.txt"), "now\n").unwrap();
        let review = build(&root).expect("review");
        let live = review.files[0].hunks[0].id.clone();
        let mut s = review.state.clone();
        let verdict = |at: &str| state::HunkVerdict {
            verdict: Verdict::Approved,
            path: "a.txt".into(),
            at: at.into(),
        };
        s.hunks
            .insert(live.clone(), verdict("2020-01-01T00:00:00Z"));
        s.hunks
            .insert("h-gone-old".into(), verdict("2020-01-01T00:00:00Z"));
        s.hunks.insert("h-gone-new".into(), verdict(&state::now()));
        save_state(&root, &json!({ "rev": review.state_rev, "state": s }));
        let kept = state::load(&root).unwrap().0.hunks;
        assert!(kept.contains_key(&live), "the change is still there");
        assert!(kept.contains_key("h-gone-new"), "recent: kept a while");
        assert!(!kept.contains_key("h-gone-old"), "old and gone: dropped");
    }

    #[test]
    fn a_missing_or_plain_folder_says_what_it_is() {
        let dir = test_repo();
        let gone = dir
            .0
            .join("moved-away")
            .to_str()
            .unwrap()
            .replace('\\', "/");
        assert!(build(&gone)
            .err()
            .unwrap_or_default()
            .contains("no longer exists"));
        let plain = super::super::git::TemporaryDir::new().unwrap();
        let plain = plain.0.to_str().unwrap().replace('\\', "/");
        assert!(build(&plain)
            .err()
            .unwrap_or_default()
            .contains("not inside a Git repository"));
    }

    #[test]
    fn file_lines_read_the_work_tree_or_head() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap().replace('\\', "/");
        std::fs::write(dir.0.join("a.txt"), "after\r\nmore\n").unwrap();
        assert!(
            file_lines(&root, "a.txt", false)["error"].is_string(),
            "only after a review was built"
        );
        build(&root).expect("review");
        assert!(
            file_lines(&root, "b.txt", false)["error"].is_string(),
            "not a file of the review"
        );
        assert_eq!(
            file_lines(&root, "a.txt", false)["lines"],
            json!(["after", "more", ""])
        );
        assert_eq!(
            file_lines(&root, "a.txt", true)["lines"],
            json!(["before", ""])
        );
        assert!(file_lines(&root, "new.txt", true)["error"].is_string());
        assert!(file_lines(&root, "../a.txt", true)["error"].is_string());
    }

    #[test]
    fn only_files_of_the_review_are_located() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap().replace('\\', "/");
        std::fs::write(dir.0.join("a.txt"), "after\n").unwrap();
        build(&root).expect("review");
        assert!(located(&root, "a.txt").unwrap().ends_with("a.txt"));
        assert!(located(&root, "b.txt").is_err(), "not a file of the review");
        assert!(located(&root, "../a.txt").is_err());
        // A file deleted since: its folder, which is still there
        std::fs::remove_file(dir.0.join("a.txt")).unwrap();
        assert!(located(&root, "a.txt").unwrap().is_dir());
    }

    #[test]
    fn since_hunks_show_only_what_changed_after_the_last_round() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nD\ne\nf\ng\nh\n";
        let hunks = since_hunks(old, new);
        assert_eq!(hunks.len(), 1);
        let changed: Vec<(LineKind, &str)> = hunks[0]
            .lines
            .iter()
            .filter(|l| l.kind != LineKind::Context)
            .map(|l| (l.kind, l.text.as_str()))
            .collect();
        assert_eq!(changed, [(LineKind::Del, "d"), (LineKind::Add, "D")]);
        assert_eq!(hunks[0].header, "@@ -1,7 +1,7 @@");
    }

    #[test]
    fn a_review_round_trip_keeps_verdicts_on_unchanged_hunks() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap().replace('\\', "/");
        let body: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.0.join("a.txt"), &body).unwrap();
        assert!(git(&root, &["commit", "-qam", "thirty"]).ok);
        std::fs::write(
            dir.0.join("a.txt"),
            body.replace("line 2\n", "two\n")
                .replace("line 28\n", "x\n"),
        )
        .unwrap();

        let review = build(&root).expect("review");
        assert_eq!(review.files.len(), 1);
        let ids: Vec<String> = review.files[0].hunks.iter().map(|h| h.id.clone()).collect();
        let mut s = review.state.clone();
        for id in &ids {
            s.hunks.insert(
                id.clone(),
                state::HunkVerdict {
                    verdict: Verdict::Approved,
                    path: "a.txt".into(),
                    at: state::now(),
                },
            );
        }
        let saved = save_state(&root, &json!({ "rev": review.state_rev, "state": s }));
        let rev = saved["stateRev"].as_i64().expect("saved");
        // A stale revision is refused with the stored state.
        assert_eq!(
            save_state(&root, &json!({ "rev": 0, "state": s }))["conflict"],
            true
        );

        let back = handback(&root, &json!({ "rev": rev, "text": "review" }));
        assert_eq!(back["ok"], true, "{back}");

        // The agent changes the second hunk only.
        std::fs::write(
            dir.0.join("a.txt"),
            body.replace("line 2\n", "two\n")
                .replace("line 28\n", "y\n"),
        )
        .unwrap();
        let again = build(&root).expect("review");
        let f = &again.files[0];
        assert_eq!(f.since, Some("changed"));
        assert!(f.since_hunks.as_ref().is_some_and(|h| h.len() == 1));
        assert!(
            again.state.hunks.contains_key(&f.hunks[0].id),
            "unchanged hunk keeps its verdict"
        );
        assert!(
            !again.state.hunks.contains_key(&f.hunks[1].id),
            "the changed hunk is new"
        );
        assert_eq!(again.since_review.as_ref().map(|s| s.round), Some(1));
    }
}
