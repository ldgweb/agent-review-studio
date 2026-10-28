// CLI renderers. `ars list` reads the typed session index. `ars review` and
// `ars pr` read a project review as JSON: the core's own output, or a file
// saved with `ars review --json`, so a CI job without the transcripts
// renders exactly what the desktop showed.
//
// Text that comes from transcripts or the reviewer is redacted (best effort)
// and @mention-defused before it lands in a pull request.

use crate::app_core::domain::{AgentSession, RiskLevel, RiskScore};
use crate::app_core::redact;
use chrono::{DateTime, Datelike, Utc};
use serde_json::Value;
use std::collections::HashMap;

pub fn render_list(sessions: &[AgentSession]) -> String {
    if sessions.is_empty() {
        return "\n  No agent sessions found on this machine.\n\
                \n  Agent Review Studio reads Claude Code, Codex, Cursor, VS Code Copilot,\
                \n  Gemini CLI, Aider and ars/v1 sessions. Settings → Session sources in\
                \n  the app shows where it looks.\n\n"
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
        pad("when", 10),
        "goal"
    ));
    for s in sessions.iter().take(30) {
        let risk = max_risk(&s.risks)
            .map(|r| level_str(r.level))
            .unwrap_or("-");
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
            pad(&rel_time(&s.timestamp), 10),
            truncate(&first_line(goal), 50)
        ));
    }
    out.push_str("\n  Review a project: ars review [dir] · Open it in the app: ars .\n\n");
    out
}

// ===========================================================================
// The project review, read from its JSON
// ===========================================================================

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn n(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// A file's verdict: its changes' verdicts ("mixed" when they differ), or,
/// for a file without changes to show, the file verdict bound to its
/// fingerprint. None while anything is undecided.
fn file_verdict(review: &Value, file: &Value) -> Option<String> {
    let state = &review["state"];
    let hunks = arr(file, "hunks");
    if hunks.is_empty() {
        let fv = &state["files"][s(file, "path")];
        return (s(fv, "fingerprint") == s(file, "fingerprint"))
            .then(|| s(fv, "verdict").to_string())
            .filter(|v| !v.is_empty());
    }
    let verdicts: Vec<&str> = hunks
        .iter()
        .map(|h| s(&state["hunks"][s(h, "id")], "verdict"))
        .collect();
    if verdicts.iter().any(|v| v.is_empty()) {
        return None;
    }
    if verdicts.iter().all(|v| *v == verdicts[0]) {
        Some(verdicts[0].to_string())
    } else {
        Some("mixed".to_string())
    }
}

#[derive(Default)]
struct Counts {
    approved: usize,
    changes: usize,
    rejected: usize,
    mixed: usize,
    pending: usize,
}

fn counts(review: &Value) -> Counts {
    let mut c = Counts::default();
    for f in arr(review, "files") {
        match file_verdict(review, f).as_deref() {
            Some("approved") => c.approved += 1,
            Some("changes") => c.changes += 1,
            Some("rejected") => c.rejected += 1,
            Some(_) => c.mixed += 1,
            None => c.pending += 1,
        }
    }
    c
}

/// "2 approved · 1 rejected": the counts that are not zero
fn tally(c: &Counts) -> String {
    [
        (c.approved, "approved"),
        (c.changes, "to change"),
        (c.rejected, "rejected"),
        (c.mixed, "mixed"),
        (c.pending, "pending"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, label)| format!("{n} {label}"))
    .collect::<Vec<_>>()
    .join(" · ")
}

/// The overall outcome the verdicts add up to (never recorded by hand).
fn outcome(c: &Counts, files: usize) -> &'static str {
    if files == 0 {
        "Nothing uncommitted"
    } else if c.pending > 0 {
        "In review"
    } else if c.changes > 0 || c.mixed > 0 {
        "Changes requested"
    } else if c.rejected == files {
        "Rejected"
    } else if c.rejected > 0 {
        "Partially approved"
    } else {
        "Approved"
    }
}

fn verdict_label(v: Option<&str>) -> &'static str {
    match v {
        Some("approved") => "approved",
        Some("changes") => "changes requested",
        Some("rejected") => "rejected",
        Some("mixed") => "mixed verdicts",
        _ => "pending",
    }
}

fn verdict_icon(v: Option<&str>) -> &'static str {
    match v {
        Some("approved") => "✅",
        Some("changes") => "🔁",
        Some("rejected") => "❌",
        Some("mixed") => "◐",
        _ => "⏳",
    }
}

fn findings_by_path(review: &Value) -> HashMap<&str, Vec<&Value>> {
    let mut map: HashMap<&str, Vec<&Value>> = HashMap::new();
    for f in arr(review, "findings") {
        map.entry(s(f, "filePath")).or_default().push(f);
    }
    for list in map.values_mut() {
        list.sort_by_key(|f| -severity_rank(s(f, "severity")));
    }
    map
}

fn severity_rank(sev: &str) -> i64 {
    match sev {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

fn needs_attention(file: &Value, findings: &[&Value]) -> bool {
    !arr(file, "riskFlags").is_empty()
        || findings
            .iter()
            .any(|f| severity_rank(s(f, "severity")) >= 2)
        || s(file, "verification") == "unverified"
        || arr(file, "hunks")
            .iter()
            .any(|h| s(&h["origin"], "kind") == "none" && !h["origin"]["commandHint"].is_null())
}

fn unrecorded_hunks(file: &Value) -> usize {
    arr(file, "hunks")
        .iter()
        .filter(|h| s(&h["origin"], "kind") != "agent")
        .count()
}

fn comment_loc(c: &Value) -> String {
    match c.get("line").and_then(Value::as_i64) {
        Some(line) => format!("{}:{line}", s(c, "path")),
        None => s(c, "path").to_string(),
    }
}

/// A comment's words, with the code it suggests instead of its line.
fn comment_text(c: &Value) -> String {
    let text = s(c, "text").trim();
    let note = match c.get("suggestion").and_then(Value::as_str) {
        None => return text.to_string(),
        Some(code) if code.trim().is_empty() => "remove this line".to_string(),
        Some(code) => {
            let first = code.lines().next().unwrap_or("").trim().replace('`', "'");
            let more = if code.lines().count() > 1 { " ..." } else { "" };
            format!("suggests `{first}`{more}")
        }
    };
    if text.is_empty() {
        note
    } else {
        format!("{text} ({note})")
    }
}

/// Comments on code that still exists (a gone hunk makes a comment outdated).
fn current_comments(review: &Value) -> Vec<&Value> {
    let live: std::collections::HashSet<&str> = arr(review, "files")
        .iter()
        .flat_map(|f| arr(f, "hunks"))
        .map(|h| s(h, "id"))
        .collect();
    arr(&review["state"], "comments")
        .iter()
        .filter(|c| live.contains(s(c, "hunkId")))
        .collect()
}

pub fn render_review(review: &Value) -> String {
    let mut out = String::new();
    let project = &review["project"];
    let files = arr(review, "files");
    let stats = &review["stats"];
    out.push_str(&format!(
        "\n  {} @ {} · {} file{} changed (+{} −{}) · {} change{}\n",
        s(project, "name"),
        s(project, "branch"),
        files.len(),
        plural(files.len()),
        n(stats, "added"),
        n(stats, "removed"),
        n(stats, "hunks"),
        plural(n(stats, "hunks") as usize)
    ));
    if files.is_empty() {
        out.push_str("\n  Nothing uncommitted to review.\n\n");
        return out;
    }
    let sessions = arr(review, "sessions");
    if sessions.is_empty() {
        out.push_str(
            "  No agent session found for this project. The changes are shown without the why.\n",
        );
    } else {
        let labels: Vec<String> = sessions
            .iter()
            .map(|x| s(x, "agentLabel").to_string())
            .collect();
        out.push_str(&format!(
            "  Agents: {} · {} change{} from recorded edits, {} partly, {} with none on record\n",
            dedup(labels).join(", "),
            n(stats, "agentHunks"),
            plural(n(stats, "agentHunks") as usize),
            n(stats, "partialHunks"),
            n(stats, "unattributedHunks")
        ));
    }
    if let Some(since) = review.get("sinceReview").filter(|v| v.is_object()) {
        out.push_str(&format!(
            "  Since round {} ({}): {} changed · {} unchanged · {} new\n",
            n(since, "round"),
            rel_time(s(since, "at")),
            arr(since, "changed").len(),
            arr(since, "unchanged").len(),
            arr(since, "added").len()
        ));
    }
    let c = counts(review);
    out.push_str(&format!(
        "\n  Your review: {} ({})\n",
        outcome(&c, files.len()),
        tally(&c)
    ));

    let findings = findings_by_path(review);
    let (attention, routine): (Vec<&Value>, Vec<&Value>) = files.iter().partition(|f| {
        needs_attention(
            f,
            findings.get(s(f, "path")).map(Vec::as_slice).unwrap_or(&[]),
        )
    });
    for (title, group) in [("NEEDS ATTENTION", attention), ("ROUTINE", routine)] {
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("\n  {title}\n"));
        for f in group {
            let path = s(f, "path");
            let mut notes: Vec<String> = arr(f, "riskFlags")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if let Some(list) = findings.get(path) {
                notes.push(format!("{} finding{}", list.len(), plural(list.len())));
            }
            if s(f, "verification") == "unverified" {
                notes.push("not re-tested".into());
            }
            let unrecorded = unrecorded_hunks(f);
            if unrecorded > 0 && !sessions.is_empty() {
                notes.push(format!(
                    "{unrecorded} change{} not from a recorded edit",
                    plural(unrecorded)
                ));
            }
            if let Some(g) = f.get("generated").and_then(Value::as_str) {
                notes.push(format!("generated ({g})"));
            }
            if let Some(since) = f.get("since").and_then(Value::as_str) {
                notes.push(format!("{since} since last review"));
            }
            let verdict = file_verdict(review, f);
            out.push_str(&format!(
                "  {} {} {} {}  {}\n",
                verdict_icon(verdict.as_deref()),
                pad(&truncate(path, 44), 44),
                pad(
                    &format!("+{} −{}", n(f, "linesAdded"), n(f, "linesRemoved")),
                    10
                ),
                pad(verdict_label(verdict.as_deref()), 17),
                notes.join(" · ")
            ));
            for fd in findings.get(path).into_iter().flatten().take(3) {
                let line = fd
                    .get("line")
                    .and_then(Value::as_i64)
                    .map(|l| format!(" (line {l})"))
                    .unwrap_or_default();
                out.push_str(&format!("       ↳ {}{line}\n", s(fd, "title")));
            }
        }
    }
    let comments = current_comments(review);
    if !comments.is_empty() {
        out.push_str(&format!(
            "\n  Comments for the agent ({}):\n",
            comments.len()
        ));
        for c in comments.iter().take(20) {
            out.push_str(&format!(
                "    - {}: {}\n",
                comment_loc(c),
                truncate(&comment_text(c), 90)
            ));
        }
    }
    out.push_str("\n  Open it in the app: ars . · PR summary: ars pr\n\n");
    out
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    v.retain(|x| seen.insert(x.clone()));
    v
}

/// Markdown-safe single line: collapse whitespace, escape table pipes,
/// defuse @mentions, redact secrets.
fn inline(text: &str, max: usize) -> String {
    let flat: String = no_mentions(&redact::redact_secrets(
        &text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace('|', "\\|"),
    ));
    if flat.chars().count() <= max {
        flat
    } else {
        let t: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

/// Break `@user` / `@org/team` mentions with a zero-width joiner so posting
/// transcript text never pings anyone. E-mail addresses (`a@b.c`: `@` after
/// a word character) are not mentions and stay as they are.
fn no_mentions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev: Option<char> = None;
    let mut chars = text.chars().peekable();
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

/// The pull-request summary: what the agents were asked and reported, the
/// changes with the reviewer's verdicts, findings and open comments.
pub fn render_pr_markdown(review: &Value) -> String {
    let mut m: Vec<String> = Vec::new();
    let project = &review["project"];
    let files = arr(review, "files");
    m.push(format!(
        "### Agent Review Studio · {}",
        inline(s(project, "name"), 80)
    ));
    let c = counts(review);
    if files.is_empty() {
        m.push("No uncommitted changes at the time of this summary.".into());
    } else {
        m.push(format!(
            "**Review:** {} ({}, of {} file{})",
            outcome(&c, files.len()),
            tally(&c),
            files.len(),
            plural(files.len())
        ));
    }
    let rounds = arr(&review["state"], "rounds");
    if let Some(last) = rounds.last() {
        let day = s(last, "at").get(..10).unwrap_or("");
        m.push(match rounds.len() {
            1 => format!("_1 review round, on {day}._"),
            k => format!("_{k} review rounds, the last on {day}._"),
        });
    }

    let sessions = arr(review, "sessions");
    if !sessions.is_empty() {
        m.push(String::new());
        m.push("#### What the agents were asked".into());
        for x in sessions.iter().take(5) {
            m.push(format!(
                "- **{}** · {}: “{}”{}",
                inline(s(x, "agentLabel"), 40),
                inline(s(x, "model"), 40),
                inline(s(x, "originalPrompt"), 220),
                match n(x, "attributedHunks") {
                    0 => String::new(),
                    k => format!(" ({k} change{})", plural(k as usize)),
                }
            ));
            let report = s(x, "outcome");
            if !report.is_empty() && !report.starts_with("No final self-report") {
                m.push(format!("  > {}", inline(report, 400)));
            }
        }
    }

    if !files.is_empty() {
        let findings = findings_by_path(review);
        m.push(String::new());
        m.push("#### Files".into());
        m.push("| File | Change | Review | Signals |".into());
        m.push("|---|---|---|---|".into());
        for f in files.iter().take(60) {
            let path = s(f, "path");
            let mut signals: Vec<String> = arr(f, "riskFlags")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if let Some(list) = findings.get(path) {
                signals.push(format!("{} finding{}", list.len(), plural(list.len())));
            }
            if s(f, "verification") == "unverified" {
                signals.push("not re-tested".into());
            }
            let unrecorded = unrecorded_hunks(f);
            if unrecorded > 0 && !sessions.is_empty() {
                signals.push(format!(
                    "{unrecorded} change{} not from a recorded edit",
                    plural(unrecorded)
                ));
            }
            let verdict = file_verdict(review, f);
            m.push(format!(
                "| `{}` | +{} −{} | {} {} | {} |",
                inline(path, 90).replace('`', "'"),
                n(f, "linesAdded"),
                n(f, "linesRemoved"),
                verdict_icon(verdict.as_deref()),
                verdict_label(verdict.as_deref()),
                signals.join(" · ")
            ));
        }
        if files.len() > 60 {
            m.push(format!("| … {} more | | | |", files.len() - 60));
        }

        let mut all: Vec<&Value> = findings.values().flatten().copied().collect();
        all.sort_by_key(|f| -severity_rank(s(f, "severity")));
        if !all.is_empty() {
            m.push(String::new());
            m.push("#### Findings".into());
            for f in all.iter().take(12) {
                let loc = match f.get("line").and_then(Value::as_i64) {
                    Some(l) => format!("{}:{l}", s(f, "filePath")),
                    None => s(f, "filePath").to_string(),
                };
                m.push(format!(
                    "- {} **{}** at `{}`",
                    match s(f, "severity") {
                        "critical" => "🔴",
                        "high" => "🟠",
                        "medium" => "🟡",
                        _ => "⚪",
                    },
                    inline(s(f, "title"), 100),
                    inline(&loc, 100).replace('`', "'")
                ));
            }
        }
        let comments = current_comments(review);
        if !comments.is_empty() {
            m.push(String::new());
            m.push("#### Review comments".into());
            for c in comments.iter().take(20) {
                m.push(format!(
                    "- `{}`: {}",
                    inline(&comment_loc(c), 100).replace('`', "'"),
                    inline(&comment_text(c), 300)
                ));
            }
        }
    }
    m.push(String::new());
    m.push(
        "<sub>Generated by Agent Review Studio. Signals are fixed heuristics, not a security assessment; verdicts apply to the exact code reviewed.</sub>"
            .into(),
    );
    m.join("\n") + "\n"
}

fn max_risk(risks: &[RiskScore]) -> Option<&RiskScore> {
    risks.iter().max_by_key(|r| r.score)
}

fn level_str(l: RiskLevel) -> &'static str {
    match l {
        RiskLevel::Critical => "critical",
        RiskLevel::High => "high",
        RiskLevel::Medium => "medium",
        RiskLevel::Low => "low",
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

/// "just now", "5m ago", "3h ago", "2d ago", then the day: "on 19 Jul"
/// (with the year when it is not this one)
fn rel_time(ts: &str) -> String {
    rel_time_at(ts, Utc::now())
}

fn rel_time_at(ts: &str, now: DateTime<Utc>) -> String {
    let dt = match DateTime::parse_from_rfc3339(ts) {
        Ok(d) => d.with_timezone(&Utc),
        Err(_) => return "?".to_string(),
    };
    let mins = (now - dt).num_minutes().max(0);
    if mins < 1 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{mins}m ago")
    } else if mins < 1440 {
        format!("{}h ago", mins / 60)
    } else if mins < 10080 {
        format!("{}d ago", mins / 1440)
    } else if dt.year() == now.year() {
        dt.format("on %-d %b").to_string()
    } else {
        dt.format("on %-d %b %Y").to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comments_show_the_code_they_suggest() {
        assert_eq!(comment_text(&json!({ "text": "drop it" })), "drop it");
        assert_eq!(
            comment_text(&json!({ "text": "", "suggestion": "const a = `x`;\nmore()" })),
            "suggests `const a = 'x';` ..."
        );
        assert_eq!(
            comment_text(&json!({ "text": "dead code", "suggestion": "" })),
            "dead code (remove this line)"
        );
    }

    fn review() -> Value {
        json!({
            "project": { "name": "shop", "branch": "main", "root": "/w/shop" },
            "stats": { "added": 3, "removed": 1, "hunks": 2, "agentHunks": 1, "partialHunks": 0, "unattributedHunks": 1 },
            "files": [
                { "path": "src/a.ts", "fingerprint": "fp-a", "linesAdded": 2, "linesRemoved": 1, "riskFlags": ["security"],
                  "verification": "unverified",
                  "hunks": [{ "id": "h-1", "origin": { "kind": "agent" } }] },
                { "path": "b.png", "fingerprint": "fp-b", "linesAdded": 0, "linesRemoved": 0, "riskFlags": [], "hunks": [] },
                { "path": "c.ts", "fingerprint": "fp-c", "linesAdded": 1, "linesRemoved": 0, "riskFlags": [],
                  "hunks": [{ "id": "h-2", "origin": { "kind": "none" } }] }
            ],
            "sessions": [{ "agentLabel": "Claude Code", "model": "m", "originalPrompt": "Ask @octocat about the token=abcdefgh12345", "attributedHunks": 1, "outcome": "Done." }],
            "findings": [{ "title": "Possible hardcoded secret", "severity": "high", "filePath": "src/a.ts", "line": 2 }],
            "state": {
                "hunks": { "h-1": { "verdict": "approved" }, "h-2": { "verdict": "rejected" } },
                "files": { "b.png": { "verdict": "approved", "fingerprint": "fp-old" } },
                "comments": [
                    { "path": "src/a.ts", "hunkId": "h-1", "line": 2, "text": "why | here?" },
                    { "path": "src/a.ts", "hunkId": "h-gone", "line": 9, "text": "outdated" }
                ],
                "rounds": []
            }
        })
    }

    #[test]
    fn verdicts_bind_to_content_and_outdated_comments_drop() {
        let r = review();
        let files = arr(&r, "files");
        assert_eq!(file_verdict(&r, &files[0]).as_deref(), Some("approved"));
        assert_eq!(
            file_verdict(&r, &files[1]),
            None,
            "a file verdict on other bytes does not apply"
        );
        assert_eq!(file_verdict(&r, &files[2]).as_deref(), Some("rejected"));
        let c = counts(&r);
        assert_eq!((c.approved, c.rejected, c.pending), (1, 1, 1));
        assert_eq!(outcome(&c, 3), "In review");
        assert_eq!(current_comments(&r).len(), 1);
    }

    #[test]
    fn mixed_files_count_as_mixed_not_pending() {
        let mut r = review();
        r["files"][2]["hunks"] = json!([
            { "id": "h-2", "origin": { "kind": "none" } },
            { "id": "h-3", "origin": { "kind": "none" } }
        ]);
        r["state"]["hunks"]["h-3"] = json!({ "verdict": "approved" });
        let md = render_pr_markdown(&r);
        assert!(
            md.contains("**Review:** In review (1 approved · 1 mixed · 1 pending, of 3 files)"),
            "{md}"
        );
    }

    #[test]
    fn times_read_as_words() {
        let now = DateTime::parse_from_rfc3339("2026-10-28T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(rel_time_at("2026-10-28T11:59:40Z", now), "just now");
        assert_eq!(rel_time_at("2026-10-28T11:55:00Z", now), "5m ago");
        assert_eq!(rel_time_at("2026-10-28T09:00:00Z", now), "3h ago");
        assert_eq!(rel_time_at("2026-10-26T12:00:00Z", now), "2d ago");
        assert_eq!(rel_time_at("2026-07-19T16:00:00Z", now), "on 19 Jul");
        assert_eq!(rel_time_at("2025-07-19T16:00:00Z", now), "on 19 Jul 2025");
        assert_eq!(rel_time_at("not a time", now), "?");
    }

    #[test]
    fn the_pr_summary_is_safe_to_post() {
        let md = render_pr_markdown(&review());
        assert!(md.contains("@\u{200D}octocat"), "{md}");
        assert!(!md.contains("abcdefgh12345"), "secrets are redacted: {md}");
        assert!(md.contains("why \\| here?"), "pipes are escaped: {md}");
        assert!(!md.contains("outdated"), "{md}");
        assert!(
            md.contains(
                "| `src/a.ts` | +2 −1 | ✅ approved | security · 1 finding · not re-tested |"
            ),
            "{md}"
        );
        let text = render_review(&review());
        assert!(text.contains("NEEDS ATTENTION"), "{text}");
    }
}
