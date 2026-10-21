// CLI renderers: pure functions over the typed AgentSession the shared core
// produces. Terminal + Markdown renderers, plus the same triage/evidence
// logic the frontend uses (src/lib/triage.ts, src/lib/evidence.ts). Local
// findings arrive as opaque JSON Values, so they are read by key.

use crate::app_core::domain::{AgentSession, FileChange, RiskDimension, RiskLevel, RiskScore};
use crate::app_core::policy;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::cmp::Reverse;
use std::collections::HashSet;

/// The normalizer's placeholder outcome when a transcript has no final
/// self-report (core/normalize.rs) — never rendered as a self-report.
const NO_SELF_REPORT: &str = "No final self-report found in the transcript.";

fn has_self_report(s: &AgentSession) -> bool {
    !s.outcome.is_empty() && s.outcome != NO_SELF_REPORT
}

// ===========================================================================
// the reviewer's saved review (the desktop's /approvals state)
// ===========================================================================

/// `correctionNotes` key holding the reviewer-edited correction draft
/// (DRAFT_NOTE_KEY in src/lib/approvals.ts) — never a file note.
const DRAFT_NOTE_KEY: &str = "__draft";

/// Line comments shown before "+N more".
const MAX_REVIEW_COMMENTS: usize = 30;
/// Per-file verdict rows / change requests shown before "+N more".
const MAX_REVIEW_FILES: usize = 30;

/// The reviewer's saved review of a session, as `ars` may show it.
#[derive(Debug, Clone, PartialEq)]
pub enum SavedReview {
    /// Nothing saved (or nothing recorded in what was saved).
    None,
    /// Saved at another review revision: the code changed since.
    Stale,
    /// Saved at the session's current review revision (the stored state).
    Current(Value),
}

impl SavedReview {
    /// Classify the stored state against the session's CURRENT review
    /// revision (the detail route's `reviewRevision`). Same rule the core
    /// applies before git writes: a non-empty revision, equal to the saved one.
    pub fn classify(saved: Option<&Value>, current_revision: Option<&str>) -> SavedReview {
        let Some(saved) = saved.filter(|v| has_decisions(v)) else {
            return SavedReview::None;
        };
        let revision = saved["reviewRevision"].as_str().filter(|r| !r.is_empty());
        if revision.is_some() && revision == current_revision.filter(|r| !r.is_empty()) {
            SavedReview::Current(saved.clone())
        } else {
            SavedReview::Stale
        }
    }

    pub fn current(&self) -> Option<&Value> {
        match self {
            SavedReview::Current(v) => Some(v),
            _ => None,
        }
    }
}

const STALE_REVIEW: &str =
    "A saved review exists but the code changed since; its decisions are not shown.";

/// Whether a stored state records anything `ars` renders: a verdict, a line
/// comment, a file note or a finalized decision.
fn has_decisions(v: &Value) -> bool {
    let any_verdict = v["fileStates"].as_object().is_some_and(|m| {
        m.values()
            .any(|s| s.as_str().is_some_and(|s| s != "pending"))
    });
    let any_comment = v["lineComments"].as_object().is_some_and(|m| {
        m.values()
            .any(|c| c.as_array().is_some_and(|a| !a.is_empty()))
    });
    let any_note = v["correctionNotes"].as_object().is_some_and(|m| {
        m.iter()
            .any(|(k, n)| k != DRAFT_NOTE_KEY && n.as_str().is_some_and(|n| !n.trim().is_empty()))
    });
    any_verdict || any_comment || any_note || decision_label(v).is_some()
}

/// Session decision wording, as the UI shows it (statusMeta in src/lib/risk.ts).
fn decision_label(v: &Value) -> Option<&'static str> {
    match v["sessionDecision"].as_str()? {
        "approved" => Some("Approved"),
        "partially-approved" => Some("Partially approved"),
        "corrections-requested" => Some("Corrections requested"),
        "rejected" => Some("Rejected"),
        _ => None,
    }
}

/// File verdict wording, as the UI shows it (fileApprovalMeta in src/lib/risk.ts).
fn verdict_label(state: &str) -> &'static str {
    match state {
        "approved" => "Approved",
        "rejected" => "Rejected",
        "correction-requested" => "Correction requested",
        _ => "Pending",
    }
}

fn file_verdict<'a>(review: &'a Value, file: &FileChange) -> &'a str {
    review["fileStates"][&file.id].as_str().unwrap_or("pending")
}

/// (file, line, text) for every line comment on a session file, in file order.
fn review_comments<'a>(
    s: &'a AgentSession,
    review: &'a Value,
) -> Vec<(&'a FileChange, Option<i64>, &'a str)> {
    let mut out = Vec::new();
    for f in &s.files {
        for c in review["lineComments"][&f.id]
            .as_array()
            .into_iter()
            .flatten()
        {
            let text = c["text"].as_str().unwrap_or("");
            if !text.trim().is_empty() {
                out.push((f, c["line"].as_i64(), text));
            }
        }
    }
    out
}

/// (file, note) for each file whose verdict is "correction requested" and
/// that carries a note — the per-file ask the UI's correction request uses.
/// Looked up by file id, so the draft key is never one of them.
fn review_notes<'a>(s: &'a AgentSession, review: &'a Value) -> Vec<(&'a FileChange, &'a str)> {
    s.files
        .iter()
        .filter(|f| file_verdict(review, f) == "correction-requested")
        .filter_map(|f| {
            let note = review["correctionNotes"][&f.id].as_str()?.trim();
            (!note.is_empty() && f.id != DRAFT_NOTE_KEY).then_some((f, note))
        })
        .collect()
}

fn comment_loc(f: &FileChange, line: Option<i64>) -> String {
    match line {
        Some(n) => format!("{}:{n}", f.path),
        None => f.path.clone(),
    }
}

/// The "Reviewer decisions" Markdown section (empty when there is none).
fn review_markdown(s: &AgentSession, review: &SavedReview) -> Vec<String> {
    let mut m: Vec<String> = Vec::new();
    let r = match review {
        SavedReview::None => return m,
        SavedReview::Stale => {
            m.push(String::new());
            m.push("**Reviewer decisions**".to_string());
            m.push(String::new());
            m.push(format!("_{STALE_REVIEW}_"));
            return m;
        }
        SavedReview::Current(r) => r,
    };
    m.push(String::new());
    m.push("**Reviewer decisions** _(saved in Agent Review Studio at this revision)_".to_string());
    m.push(String::new());
    m.push(format!(
        "**Session decision:** {}",
        decision_label(r).unwrap_or("not finalized (in review)")
    ));
    if !s.files.is_empty() {
        m.push(String::new());
        m.push("| File | Verdict |".to_string());
        m.push("| --- | --- |".to_string());
        for f in s.files.iter().take(MAX_REVIEW_FILES) {
            m.push(format!(
                "| `{}` | {} |",
                inline(&f.path, 160),
                verdict_label(file_verdict(r, f))
            ));
        }
        if s.files.len() > MAX_REVIEW_FILES {
            m.push(format!("| +{} more | |", s.files.len() - MAX_REVIEW_FILES));
        }
    }
    let comments = review_comments(s, r);
    if !comments.is_empty() {
        m.push(String::new());
        m.push("**Line comments**".to_string());
        m.push(String::new());
        for (f, line, text) in comments.iter().take(MAX_REVIEW_COMMENTS) {
            m.push(format!(
                "- `{}` — {}",
                inline(&comment_loc(f, *line), 160),
                inline(text, 300)
            ));
        }
        if comments.len() > MAX_REVIEW_COMMENTS {
            m.push(format!(
                "- … +{} more",
                comments.len() - MAX_REVIEW_COMMENTS
            ));
        }
    }
    let notes = review_notes(s, r);
    if !notes.is_empty() {
        m.push(String::new());
        m.push("**Change requests**".to_string());
        m.push(String::new());
        for (f, note) in notes.iter().take(MAX_REVIEW_FILES) {
            m.push(format!(
                "- `{}` — {}",
                inline(&f.path, 160),
                inline(note, 400)
            ));
        }
        if notes.len() > MAX_REVIEW_FILES {
            m.push(format!("- … +{} more", notes.len() - MAX_REVIEW_FILES));
        }
    }
    m
}

/// The terminal "YOUR REVIEW" block (empty when there is none).
fn review_terminal(s: &AgentSession, review: &SavedReview) -> Vec<String> {
    let mut l: Vec<String> = Vec::new();
    let r = match review {
        SavedReview::None => return l,
        SavedReview::Stale => {
            l.push(String::new());
            l.push("  YOUR REVIEW".to_string());
            l.push(format!("    {STALE_REVIEW}"));
            return l;
        }
        SavedReview::Current(r) => r,
    };
    l.push(String::new());
    l.push("  YOUR REVIEW   [saved at this revision]".to_string());
    let mut counts = [0usize; 4];
    for f in &s.files {
        match file_verdict(r, f) {
            "approved" => counts[0] += 1,
            "correction-requested" => counts[1] += 1,
            "rejected" => counts[2] += 1,
            _ => counts[3] += 1,
        }
    }
    l.push(format!(
        "    {} · {} approved · {} correction requested · {} rejected · {} pending",
        decision_label(r).unwrap_or("Not finalized"),
        counts[0],
        counts[1],
        counts[2],
        counts[3]
    ));
    let comments = review_comments(s, r);
    for (f, line, text) in comments.iter().take(MAX_REVIEW_COMMENTS) {
        l.push(format!(
            "    ✎ {}  {}",
            comment_loc(f, *line),
            truncate(text, 80)
        ));
    }
    if comments.len() > MAX_REVIEW_COMMENTS {
        l.push(format!(
            "    … +{} more",
            comments.len() - MAX_REVIEW_COMMENTS
        ));
    }
    l
}

// ===========================================================================
// list
// ===========================================================================

pub fn render_list(sessions: &[AgentSession]) -> String {
    if sessions.is_empty() {
        return "\n  No agent sessions found on this machine.\n\
                \n  Agent Review Studio reads Claude Code, Cursor, Codex, Aider, Gemini CLI and Copilot.\n\n"
            .to_string();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "\n  Agent Review Studio · {} session{}\n\n",
        sessions.len(),
        plural(sessions.len())
    ));
    out.push_str(&format!(
        "  {} {} {} {} {} {}\n",
        pad("id", 9),
        pad("agent", 12),
        pad("risk", 8),
        pad("files", 6),
        pad("age", 6),
        "goal"
    ));
    for s in sessions.iter().take(30) {
        let risk = max_risk(s).map(|r| level_str(r.level)).unwrap_or("—");
        let goal = if !s.declared_goal.is_empty() {
            &s.declared_goal
        } else {
            &s.original_prompt
        };
        out.push_str(&format!(
            "  {} {} {} {} {} {}\n",
            pad(&short_id(&s.session_id), 9),
            pad(&s.agent, 12),
            pad(risk, 8),
            pad(&s.stats.files_modified.to_string(), 6),
            pad(&rel_time(&s.timestamp), 6),
            truncate(&first_line(goal), 50)
        ));
    }
    out.push_str("\n  review one:  ars review <id>   · or  ars review  for the latest\n\n");
    out
}

// ===========================================================================
// review — the terminal review (provenance-first: plan/self-report, then risk)
// ===========================================================================

pub fn render_review(s: &AgentSession, review: &SavedReview) -> String {
    let mut l: Vec<String> = Vec::new();
    l.push(String::new());
    l.push("  ⬡ Agent Review Studio · review".to_string());
    l.push(String::new());
    l.push(format!(
        "  {} · {}   {} ago · {}",
        s.agent_label,
        s.model,
        rel_time(&s.timestamp),
        s.status
    ));
    let goal = if !s.declared_goal.is_empty() {
        first_line(&s.declared_goal)
    } else {
        first_line(&s.original_prompt)
    };
    l.push(format!("  {}", goal));
    let branch = if s.branch.is_empty() {
        "—".to_string()
    } else {
        format!("branch {}", s.branch)
    };
    l.push(format!(
        "  {}  ·  {}  ·  {} files  +{} −{}  ·  {} min",
        s.project_name,
        branch,
        s.stats.files_modified,
        s.stats.lines_added,
        s.stats.lines_removed,
        s.stats.duration_minutes
    ));
    l.push(format!("  id {}", short_id(&s.session_id)));

    // What the agent did — provenance first
    l.push(String::new());
    l.push("  WHAT THE AGENT DID".to_string());
    if !s.initial_plan.is_empty() {
        l.push("    Declared plan".to_string());
        for (i, step) in s.initial_plan.iter().take(8).enumerate() {
            l.push(format!("      {}. {}", i + 1, truncate(step, 100)));
        }
    }
    if has_self_report(s) {
        l.push("    Self-report".to_string());
        for line in wrap(&s.outcome, 92, 6).into_iter().take(4) {
            l.push(line);
        }
    }

    // Risk triage
    l.push(String::new());
    l.push("  RISK TRIAGE   [heuristic · no AI]".to_string());
    let mut risks: Vec<&RiskScore> = s.risks.iter().collect();
    risks.sort_by_key(|r| Reverse(r.score));
    for r in &risks {
        l.push(format!(
            "    {} {} {:>3} {}  {}",
            risk_dot(r.level),
            pad(dim_str(r.dimension), 15),
            r.score,
            pad(level_str(r.level), 8),
            truncate(&r.rationale, 64)
        ));
    }
    for w in &s.summary.risk_warnings {
        l.push(format!("    ⚠ {}", truncate(w, 88)));
    }

    // Files, risk-first
    let unverified = unverified_file_ids(s);
    let mut attention: Vec<&FileChange> = Vec::new();
    let mut routine: Vec<&FileChange> = Vec::new();
    for f in &s.files {
        if needs_attention(s, f, &unverified) {
            attention.push(f);
        } else {
            routine.push(f);
        }
    }
    attention.sort_by_key(|f| Reverse(file_weight(s, f)));
    render_group(
        &mut l,
        "NEEDS ATTENTION",
        "highest weight first",
        &attention,
        true,
        s,
        &unverified,
        12,
    );
    render_group(
        &mut l,
        "ROUTINE",
        "no risk signals",
        &routine,
        false,
        s,
        &unverified,
        8,
    );

    // Before you merge
    l.push(String::new());
    l.push("  BEFORE YOU MERGE".to_string());
    let list = checklist(s, &unverified);
    if list.is_empty() {
        l.push("    ✓ No risk signals — quick skim and ship.".to_string());
    } else {
        for item in list {
            l.push(format!("    ☐ {}", truncate(&item, 90)));
        }
    }

    l.extend(review_terminal(s, review));

    l.push(String::new());
    l.push(format!(
        "  next  ars pr {}   → provenance comment for the pull request",
        short_id(&s.session_id)
    ));
    l.push(String::new());
    l.join("\n")
}

#[allow(clippy::too_many_arguments)] // one call site per group; a struct would only rename them
fn render_group(
    l: &mut Vec<String>,
    label: &str,
    meta: &str,
    files: &[&FileChange],
    attention: bool,
    s: &AgentSession,
    unverified: &HashSet<String>,
    cap: usize,
) {
    if files.is_empty() {
        return;
    }
    l.push(String::new());
    l.push(format!("  {} ({})   {}", label, files.len(), meta));
    for &f in files.iter().take(cap) {
        let delta = format!("+{} −{}", f.lines_added, f.lines_removed);
        let bullet = if attention { "●" } else { "·" };
        let desc = if attention {
            describe_weight(s, f)
        } else {
            String::new()
        };
        l.push(format!(
            "    {} {} {:>10}  {}",
            bullet,
            pad(&f.path, 42),
            delta,
            desc
        ));
        if unverified.contains(&f.id) {
            l.push("        ⌁ never re-tested".to_string());
        }
        if attention {
            let mut fnd = findings_for_file(s, f);
            fnd.sort_by_key(|f| Reverse(finding_rank(f)));
            for finding in fnd.into_iter().take(2) {
                let sev = finding
                    .get("severity")
                    .and_then(|v| v.as_str())
                    .unwrap_or("info");
                let title = finding.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let loc = finding
                    .get("line")
                    .and_then(|v| v.as_i64())
                    .map(|n| format!(" (L{n})"))
                    .unwrap_or_default();
                l.push(format!("        {} {}{}", sev, truncate(title, 70), loc));
            }
        }
    }
    if files.len() > cap {
        l.push(format!("    … +{} more", files.len() - cap));
    }
}

// ===========================================================================
// pr — the Markdown provenance comment
// ===========================================================================

pub fn render_pr_markdown(s: &AgentSession, review: &SavedReview) -> String {
    let mut m: Vec<String> = Vec::new();
    m.push("### 🔍 Agent Review Studio — what the agent did".to_string());
    m.push(String::new());
    let branch = if s.branch.is_empty() {
        String::new()
    } else {
        format!(" · branch `{}`", s.branch)
    };
    m.push(format!(
        "**{}** `{}` · session `{}` · **{} files** +{} −{} · {} min{}",
        s.agent_label,
        s.model,
        short_id(&s.session_id),
        s.stats.files_modified,
        s.stats.lines_added,
        s.stats.lines_removed,
        s.stats.duration_minutes,
        branch
    ));
    m.push(String::new());
    m.push(format!("> **Task:** {}", inline(&s.original_prompt, 400)));

    if !s.initial_plan.is_empty() {
        m.push(String::new());
        m.push("**Declared plan**".to_string());
        m.push(String::new());
        for (i, step) in s.initial_plan.iter().take(10).enumerate() {
            m.push(format!("{}. {}", i + 1, inline(step, 160)));
        }
    }

    m.push(String::new());
    m.push("**Risk triage** _(local heuristics — no LLM)_".to_string());
    m.push(String::new());
    m.push("| Dimension | Score | Signal |".to_string());
    m.push("| --- | --- | --- |".to_string());
    let mut risks: Vec<&RiskScore> = s.risks.iter().collect();
    risks.sort_by_key(|r| Reverse(r.score));
    for r in &risks {
        m.push(format!(
            "| {} {} | {} · {} | {} |",
            risk_emoji(r.level),
            dim_str(r.dimension),
            r.score,
            level_str(r.level),
            inline(&r.rationale, 90)
        ));
    }
    if !s.summary.risk_warnings.is_empty() {
        m.push(String::new());
        for w in &s.summary.risk_warnings {
            m.push(format!("> ⚠️ {}", inline(w, 160)));
        }
    }

    let unverified = unverified_file_ids(s);
    let mut attention: Vec<&FileChange> = Vec::new();
    for f in &s.files {
        if needs_attention(s, f, &unverified) {
            attention.push(f);
        }
    }
    attention.sort_by_key(|f| Reverse(file_weight(s, f)));
    if !attention.is_empty() {
        m.push(String::new());
        m.push("**Needs attention** _(weighted, highest first)_".to_string());
        m.push(String::new());
        for &f in attention.iter().take(12) {
            let mut bits: Vec<String> = Vec::new();
            let dw = describe_weight(s, f);
            if dw != "no risk signals" {
                bits.push(dw);
            }
            if unverified.contains(&f.id) {
                bits.push("never re-tested".to_string());
            }
            let mut fnd = findings_for_file(s, f);
            fnd.sort_by_key(|f| Reverse(finding_rank(f)));
            if let Some(top) = fnd.first() {
                let title = top.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let loc = top
                    .get("line")
                    .and_then(|v| v.as_i64())
                    .map(|n| format!(" (L{n})"))
                    .unwrap_or_default();
                bits.push(format!("{}{loc}", no_mentions(title)));
            }
            m.push(format!(
                "- **`{}`** `+{} −{}` — {}",
                f.path,
                f.lines_added,
                f.lines_removed,
                bits.join(" · ")
            ));
        }
    }

    let list = checklist(s, &unverified);
    m.push(String::new());
    m.push("**Before merging**".to_string());
    m.push(String::new());
    if list.is_empty() {
        m.push("- [x] No risk signals surfaced — routine change.".to_string());
    } else {
        for item in list {
            m.push(format!("- [ ] {}", inline(&item, 160)));
        }
    }

    m.extend(review_markdown(s, review));

    if has_self_report(s) {
        m.push(String::new());
        m.push("<details><summary>Agent's self-report</summary>".to_string());
        m.push(String::new());
        m.push(inline(&s.outcome, 1200));
        m.push(String::new());
        m.push("</details>".to_string());
    }

    m.push(String::new());
    m.push("---".to_string());
    m.push(format!(
        "<sub>Generated by **Agent Review Studio** from the agent's session transcript — the intent, plan and risk the diff alone doesn't show. Run `ars review {}` locally for the full view.</sub>",
        short_id(&s.session_id)
    ));
    m.push(String::new());
    // This markdown leaves the machine (PR comment or copied output).
    // Prompt, plan and self-report come straight from the transcript, so DLP
    // redaction is always applied here.
    policy::redact_secrets(&m.join("\n"))
}

fn risk_emoji(l: RiskLevel) -> &'static str {
    match l {
        RiskLevel::Critical => "🔴",
        RiskLevel::High => "🟠",
        RiskLevel::Medium => "🟡",
        RiskLevel::Low => "🟢",
    }
}

/// Break `@user` / `@org/team` mentions with a zero-width joiner so posting
/// transcript text never pings anyone. E-mail addresses (`a@b.c`: `@` after
/// a word character) are not mentions and stay as they are.
fn no_mentions(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev: Option<char> = None;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '@'
            && !prev.is_some_and(|p| p.is_alphanumeric() || p == '_')
            && chars.peek().is_some_and(|n| n.is_ascii_alphanumeric())
        {
            out.push('\u{200D}');
        }
        prev = Some(c);
    }
    out
}

/// Markdown-safe single line: collapse whitespace, escape table pipes,
/// defuse @mentions.
fn inline(s: &str, n: usize) -> String {
    let flat: String = no_mentions(
        &s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace('|', "\\|"),
    );
    if flat.chars().count() <= n {
        flat
    } else {
        let t: String = flat.chars().take(n.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

// ===========================================================================
// triage (mirrors src/lib/triage.ts) + evidence (src/lib/evidence.ts)
// ===========================================================================

fn severity_rank(sev: &str) -> i64 {
    match sev {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

fn finding_rank(f: &Value) -> i64 {
    f.get("severity")
        .and_then(|v| v.as_str())
        .map(severity_rank)
        .unwrap_or(0)
}

pub fn findings_for_file<'a>(session: &'a AgentSession, file: &FileChange) -> Vec<&'a Value> {
    session
        .findings
        .iter()
        .filter(|f| f.get("filePath").and_then(|v| v.as_str()) == Some(file.path.as_str()))
        .collect()
}

fn is_critical_area(session: &AgentSession, file: &FileChange) -> bool {
    session
        .system_areas
        .iter()
        .any(|a| a.id == file.area_id && a.critical)
}

fn file_weight(session: &AgentSession, file: &FileChange) -> i64 {
    let mut finding_sev = 0;
    for f in findings_for_file(session, file) {
        let rank = finding_rank(f);
        let contribution = if rank >= 4 {
            3
        } else if rank >= 3 {
            2
        } else if rank >= 2 {
            1
        } else {
            0
        };
        if contribution > finding_sev {
            finding_sev = contribution;
        }
    }
    1 + file.risk_flags.len() as i64
        + if is_critical_area(session, file) {
            1
        } else {
            0
        }
        + finding_sev
}

fn describe_weight(session: &AgentSession, file: &FileChange) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !file.risk_flags.is_empty() {
        parts.push(format!(
            "{} risk flag{}",
            file.risk_flags.len(),
            plural(file.risk_flags.len())
        ));
    }
    if is_critical_area(session, file) {
        parts.push("critical area".to_string());
    }
    if findings_for_file(session, file)
        .iter()
        .any(|f| finding_rank(f) >= 2)
    {
        parts.push("reviewer findings".to_string());
    }
    if parts.is_empty() {
        "no risk signals".to_string()
    } else {
        parts.join(" · ")
    }
}

fn needs_attention(
    session: &AgentSession,
    file: &FileChange,
    unverified: &HashSet<String>,
) -> bool {
    if !file.risk_flags.is_empty() {
        return true;
    }
    if findings_for_file(session, file)
        .iter()
        .any(|f| finding_rank(f) >= 2)
    {
        return true;
    }
    if is_critical_area(session, file) {
        return true;
    }
    unverified.contains(&file.id)
}

/// Files edited after the last passing test run — "changed but never re-tested".
pub fn unverified_file_ids(session: &AgentSession) -> HashSet<String> {
    use crate::app_core::domain::TimelineEventKind;
    let mut out = HashSet::new();
    for file in &session.files {
        let mut last_edit: i64 = -1;
        let mut last_ok_test: i64 = -1;
        let mut has_test = false;
        for (i, e) in session.timeline.iter().enumerate() {
            match e.kind {
                TimelineEventKind::FileEdit => {
                    if e.file_path.as_deref() == Some(file.path.as_str()) {
                        last_edit = i as i64;
                    }
                }
                TimelineEventKind::TestRun => {
                    has_test = true;
                    if e.exit_code == Some(0) {
                        last_ok_test = i as i64;
                    }
                }
                _ => {}
            }
        }
        if last_edit != -1 && has_test && last_ok_test <= last_edit {
            out.insert(file.id.clone());
        }
    }
    out
}

/// The pre-merge checklist.
fn checklist(s: &AgentSession, unverified: &HashSet<String>) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for w in &s.summary.risk_warnings {
        items.push(w.clone());
    }
    let unv: Vec<&FileChange> = s
        .files
        .iter()
        .filter(|f| unverified.contains(&f.id))
        .collect();
    if !unv.is_empty() {
        let names: Vec<String> = unv.iter().take(3).map(|f| file_name(&f.path)).collect();
        items.push(format!(
            "{} file{} changed but never re-tested after the last passing test ({}{}) — run the suite.",
            unv.len(),
            plural(unv.len()),
            names.join(", "),
            if unv.len() > 3 { "…" } else { "" }
        ));
    }
    let secret_count = s
        .findings
        .iter()
        .filter(|f| {
            f.get("title")
                .and_then(|v| v.as_str())
                .map(|t| t.to_lowercase().contains("secret"))
                .unwrap_or(false)
        })
        .count();
    if secret_count > 0 {
        items.push(format!(
            "Resolve {} possible hardcoded secret{} before merging.",
            secret_count,
            plural(secret_count)
        ));
    }
    for f in &s.findings {
        let sev = f.get("severity").and_then(|v| v.as_str()).unwrap_or("");
        if sev != "critical" && sev != "high" {
            continue;
        }
        let title = f.get("title").and_then(|v| v.as_str()).unwrap_or("");
        if title.to_lowercase().contains("secret") {
            continue;
        }
        let loc = match (
            f.get("filePath").and_then(|v| v.as_str()),
            f.get("line").and_then(|v| v.as_i64()),
        ) {
            (Some(p), Some(n)) => format!(" ({}:{})", file_name(p), n),
            (Some(p), None) => format!(" ({})", file_name(p)),
            _ => String::new(),
        };
        items.push(format!("{title}{loc}."));
    }
    // De-dup preserving order, cap at 6.
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for i in items {
        if seen.insert(i.clone()) {
            out.push(i);
        }
    }
    out.truncate(6);
    out
}

// ===========================================================================
// small helpers
// ===========================================================================

fn max_risk(s: &AgentSession) -> Option<&RiskScore> {
    s.risks.iter().max_by_key(|r| r.score)
}

fn level_str(l: RiskLevel) -> &'static str {
    match l {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
        RiskLevel::Critical => "critical",
    }
}

fn dim_str(d: RiskDimension) -> &'static str {
    match d {
        RiskDimension::Security => "security",
        RiskDimension::Database => "database",
        RiskDimension::Performance => "performance",
        RiskDimension::Maintainability => "maintainability",
    }
}

fn risk_dot(l: RiskLevel) -> &'static str {
    match l {
        RiskLevel::Low => "○",
        _ => "●",
    }
}

pub fn short_id(id: &str) -> String {
    if id.chars().count() > 12 {
        id.chars().take(8).collect()
    } else {
        id.to_string()
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn rel_time(ts: &str) -> String {
    let dt = match DateTime::parse_from_rfc3339(ts) {
        Ok(d) => d.with_timezone(&Utc),
        Err(_) => return "?".to_string(),
    };
    let mins = (Utc::now() - dt).num_minutes().max(0);
    if mins < 1 {
        "now".to_string()
    } else if mins < 60 {
        format!("{mins}m")
    } else if mins < 1440 {
        format!("{}h", mins / 60)
    } else if mins < 10080 {
        format!("{}d", mins / 1440)
    } else {
        dt.format("%-d %b").to_string()
    }
}

fn pad(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len >= n {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(n - len))
    }
}

fn truncate(s: &str, n: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= n {
        flat
    } else {
        let t: String = flat.chars().take(n.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}

/// Wrap prose to `width`, each line indented, for the self-report block.
fn wrap(s: &str, width: usize, indent: usize) -> Vec<String> {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut lines = Vec::new();
    let mut line = String::new();
    for w in flat.split(' ') {
        if w.is_empty() {
            continue;
        }
        if !line.is_empty() && line.chars().count() + 1 + w.chars().count() > width {
            lines.push(format!("{}{}", " ".repeat(indent), line));
            line = w.to_string();
        } else if line.is_empty() {
            line = w.to_string();
        } else {
            line.push(' ');
            line.push_str(w);
        }
    }
    if !line.is_empty() {
        lines.push(format!("{}{}", " ".repeat(indent), line));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_session() -> AgentSession {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/custom/expected/basic.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
        let wrapper: Value = serde_json::from_str(&text).expect("expected fixture parses");
        serde_json::from_value(
            wrapper
                .get("session")
                .cloned()
                .expect("fixture has a session"),
        )
        .expect("expected fixture deserializes into AgentSession")
    }

    #[test]
    fn pr_markdown_redacts_transcript_secrets() {
        let mut s = fixture_session();
        s.original_prompt =
            "Rotate the key: api_key = \"abcd1234efgh5678ijkl\" and redeploy".into();
        s.outcome = "Done. Used AKIAABCDEFGHIJKLMNOP and Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6".into();
        let md = render_pr_markdown(&s, &SavedReview::None);
        assert!(
            !md.contains("abcd1234efgh5678ijkl"),
            "prompt secret must not reach the PR body"
        );
        assert!(
            !md.contains("AKIAABCDEFGHIJKLMNOP"),
            "AWS key must not reach the PR body"
        );
        assert!(
            !md.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6"),
            "bearer token must not reach the PR body"
        );
        assert!(
            md.contains("[REDACTED]"),
            "redaction placeholders should be present"
        );
    }

    #[test]
    fn pr_markdown_never_pings_mentioned_users() {
        let mut s = fixture_session();
        s.original_prompt = "Ask @octocat and @acme/core-team; mail dev@example.com".into();
        s.outcome = "Done, cc @someone.".into();
        let md = render_pr_markdown(&s, &SavedReview::None);
        for mention in ["@octocat", "@acme/core-team", "@someone"] {
            assert!(
                !md.contains(mention),
                "{mention} must not reach the PR body"
            );
        }
        assert!(md.contains("@\u{200D}octocat"));
        assert!(
            md.contains("dev@example.com"),
            "e-mail addresses stay intact"
        );
        s.outcome = NO_SELF_REPORT.into();
        assert!(!has_self_report(&s));
        assert!(!render_pr_markdown(&s, &SavedReview::None).contains("self-report</summary>"));
    }

    /// A saved review of the fixture (files lf-1..lf-3) at revision "rev-1".
    fn saved_review_value() -> Value {
        serde_json::json!({
            "sessionId": fixture_session().session_id,
            "reviewRevision": "rev-1",
            "fileStates": { "lf-1": "approved", "lf-2": "correction-requested", "lf-3": "rejected" },
            "correctionNotes": {
                "lf-2": "Round half-up, not banker's rounding.",
                "lf-1": "note left before approving",
                "__draft": "EDITED DRAFT TEXT must stay in the app"
            },
            "lineComments": {
                "lf-2": [{ "anchor": "0:3", "line": 12, "text": "Ask @someone: why is AKIAABCDEFGHIJKLMNOP here?" }],
                "lf-3": [{ "anchor": "0:1", "text": "Drop this | flag" }]
            },
            "findingStates": {},
            "rejectedCategories": [],
            "sessionDecision": "corrections-requested"
        })
    }

    #[test]
    fn saved_review_applies_only_at_the_current_revision() {
        let saved = saved_review_value();
        assert!(matches!(
            SavedReview::classify(Some(&saved), Some("rev-1")),
            SavedReview::Current(_)
        ));
        assert_eq!(
            SavedReview::classify(Some(&saved), Some("rev-2")),
            SavedReview::Stale
        );
        assert_eq!(
            SavedReview::classify(Some(&saved), None),
            SavedReview::Stale
        );
        assert_eq!(
            SavedReview::classify(None, Some("rev-1")),
            SavedReview::None
        );
        let mut unrevisioned = saved.clone();
        unrevisioned["reviewRevision"] = serde_json::json!("");
        assert_eq!(
            SavedReview::classify(Some(&unrevisioned), Some("")),
            SavedReview::Stale
        );
        // Only a draft (or nothing) recorded: nothing to show.
        let draft_only = serde_json::json!({
            "reviewRevision": "rev-1",
            "fileStates": { "lf-1": "pending" },
            "correctionNotes": { "__draft": "x" },
            "lineComments": { "lf-1": [] }
        });
        assert_eq!(
            SavedReview::classify(Some(&draft_only), Some("rev-1")),
            SavedReview::None
        );
    }

    #[test]
    fn pr_markdown_shows_reviewer_decisions_at_the_current_revision() {
        let s = fixture_session();
        let review = SavedReview::classify(Some(&saved_review_value()), Some("rev-1"));
        let md = render_pr_markdown(&s, &review);
        assert!(md.contains("**Reviewer decisions**"), "{md}");
        assert!(md.contains("**Session decision:** Corrections requested"));
        assert!(md.contains("| `src/billing/tax.py` | Approved |"));
        assert!(md.contains("| `src/services/tax.py` | Correction requested |"));
        assert!(md.contains("| `src/config/settings.py` | Rejected |"));
        assert!(md.contains("- `src/services/tax.py:12` — Ask @\u{200D}someone"));
        assert!(md.contains(r"- `src/config/settings.py` — Drop this \| flag"));
        assert!(md.contains("- `src/services/tax.py` — Round half-up, not banker's rounding."));
        // A note on a file no longer flagged for correction, and the draft,
        // stay in the app.
        assert!(!md.contains("note left before approving"));
        assert!(!md.contains("EDITED DRAFT TEXT"));
        assert!(!md.contains(STALE_REVIEW));
        // Reviewer text gets the same redaction + mention escaping.
        assert!(
            !md.contains("AKIAABCDEFGHIJKLMNOP"),
            "AWS key must not reach the PR body"
        );
        assert!(md.contains("[REDACTED-AWS-KEY]"));
        assert!(
            !md.contains("@someone"),
            "mentions in comments must not ping"
        );

        let term = render_review(&s, &review);
        assert!(
            term.contains("YOUR REVIEW   [saved at this revision]"),
            "{term}"
        );
        assert!(term.contains(
            "Corrections requested · 1 approved · 1 correction requested · 1 rejected · 0 pending"
        ));
        assert!(term.contains("✎ src/services/tax.py:12"));
        assert!(!term.contains("EDITED DRAFT TEXT"));
    }

    #[test]
    fn stale_review_shows_only_a_notice_and_none_omits_the_section() {
        let s = fixture_session();
        let stale = SavedReview::classify(Some(&saved_review_value()), Some("rev-2"));
        let md = render_pr_markdown(&s, &stale);
        assert!(md.contains("**Reviewer decisions**"));
        assert!(md.contains(STALE_REVIEW));
        for hidden in [
            "Corrections requested",
            "| Verdict |",
            "Round half-up",
            "Drop this",
        ] {
            assert!(
                !md.contains(hidden),
                "{hidden} must not show for a stale review"
            );
        }
        let term = render_review(&s, &stale);
        assert!(term.contains("YOUR REVIEW") && term.contains(STALE_REVIEW));
        assert!(!term.contains("1 approved"));

        let md = render_pr_markdown(&s, &SavedReview::None);
        assert!(!md.contains("Reviewer decisions"));
        assert!(!md.contains(STALE_REVIEW));
        assert!(!render_review(&s, &SavedReview::None).contains("YOUR REVIEW"));
    }

    #[test]
    fn reviewer_comments_are_capped() {
        let s = fixture_session();
        let mut saved = saved_review_value();
        let many: Vec<Value> = (1..=35)
            .map(|i| serde_json::json!({ "anchor": format!("0:{i}"), "line": i, "text": format!("comment {i}") }))
            .collect();
        saved["lineComments"] = serde_json::json!({ "lf-1": many });
        let review = SavedReview::classify(Some(&saved), Some("rev-1"));
        let md = render_pr_markdown(&s, &review);
        assert!(md.contains("comment 30"));
        assert!(!md.contains("comment 31"));
        assert!(md.contains("- … +5 more"));
        assert!(render_review(&s, &review).contains("… +5 more"));
    }

    #[test]
    fn unknown_test_exit_does_not_verify_and_full_hash_detects_changed_evidence() {
        let mut s = fixture_session();
        let hash = crate::app_core::normalize::session_hash(&s);
        s.files[0].hunks[0].lines[0].text.push_str(" changed");
        assert_ne!(hash, crate::app_core::normalize::session_hash(&s));
        let path = s.files[0].path.clone();
        s.timeline = serde_json::from_value(serde_json::json!([
            {"id":"edit", "kind":"file-edit", "timestamp":"00:00", "title":"edit", "detail":"", "filePath":path},
            {"id":"test", "kind":"test-run", "timestamp":"00:01", "title":"test", "detail":""}
        ]))
        .unwrap();
        assert!(unverified_file_ids(&s).contains(&s.files[0].id));
        s.timeline[1].exit_code = Some(0);
        assert!(!unverified_file_ids(&s).contains(&s.files[0].id));
    }
}
