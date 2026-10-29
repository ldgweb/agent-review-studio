// The heuristics, run on what Git shows (not on what a transcript says):
// risk flags per file, folder areas, the risk scores, the built-in line
// checks, custom rules (from Settings and the repo's .ars/rules.json), and
// whether a passing test run followed the agent's last edit of a file.
// Finding ids come from the finding's content, so a stance on a finding
// survives unrelated changes.

use super::super::domain::{
    AgentSession, ChangeType, DiffHunk, DiffLine, DiffLineKind, FileChange, RiskDimension,
    RiskScore, SystemArea, TimelineEventKind,
};
use super::super::normalize::{
    area_id_of, is_test_path, language_of, line_check, risk_flags_for, score_to_level, LineCheck,
};
use super::super::paths::fold_case;
use super::super::rules;
use super::attribution::{root_relative, session_cwd_rel};
use super::changes::{hex, ChangedFile, LineKind, Status};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub struct Assessment {
    pub areas: Vec<SystemArea>,
    /// path → (area id, risk flags)
    pub files: HashMap<String, (String, Vec<RiskDimension>)>,
    pub risks: Vec<RiskScore>,
    pub findings: Vec<Value>,
}

fn finding_id(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\0");
    }
    format!("f-{}", hex(&h.finalize()[..8]))
}

fn added_lines(f: &ChangedFile) -> Vec<String> {
    f.hunks
        .iter()
        .flat_map(|h| h.changed(LineKind::Add))
        .map(|l| l.text.clone())
        .collect()
}

/// The review files in the session-file shape the rules engine reads.
fn as_file_changes(files: &[ChangedFile]) -> Vec<FileChange> {
    files
        .iter()
        .map(|f| FileChange {
            id: f.path.clone(),
            path: f.path.clone(),
            change_type: match f.status {
                Status::Added | Status::Untracked => ChangeType::Created,
                Status::Deleted => ChangeType::Deleted,
                _ => ChangeType::Modified,
            },
            language: language_of(&f.path),
            lines_added: f.added,
            lines_removed: f.removed,
            area_id: String::new(),
            risk_flags: Vec::new(),
            summary: String::new(),
            hunks: f
                .hunks
                .iter()
                .map(|h| DiffHunk {
                    header: h.header.clone(),
                    lines: h
                        .lines
                        .iter()
                        .map(|l| DiffLine {
                            kind: match l.kind {
                                LineKind::Add => DiffLineKind::Add,
                                LineKind::Del => DiffLineKind::Del,
                                LineKind::Context => DiffLineKind::Context,
                            },
                            text: l.text.clone(),
                            old_line: l.old_line,
                            new_line: l.new_line,
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

pub fn assess(root: &str, files: &[ChangedFile], rules_config: &[Value]) -> Assessment {
    let mut areas: Vec<SystemArea> = Vec::new();
    let mut per_file: HashMap<String, (String, Vec<RiskDimension>)> = HashMap::new();
    for f in files {
        let top = if f.path.contains('/') {
            f.path.split('/').next().unwrap_or("").to_string()
        } else {
            "(root)".to_string()
        };
        let area_id = area_id_of(&top);
        if !areas.iter().any(|a| a.id == area_id) {
            areas.push(SystemArea {
                id: area_id.clone(),
                label: top,
                critical: false,
                depends_on: Vec::new(), // folder groups carry no inferred dependency edges
            });
        }
        let flags = risk_flags_for(&f.path, &added_lines(f), f.added);
        if flags.contains(&RiskDimension::Security) || flags.contains(&RiskDimension::Database) {
            if let Some(area) = areas.iter_mut().find(|a| a.id == area_id) {
                area.critical = true;
            }
        }
        per_file.insert(f.path.clone(), (area_id, flags));
    }

    let flagged = |d: RiskDimension| -> Vec<&ChangedFile> {
        files
            .iter()
            .filter(|f| per_file[&f.path].1.contains(&d))
            .collect()
    };
    // `kind` names the files that raise the score ("Auth/credential-related")
    let score = |d: RiskDimension, base: i64, per: i64, kind: &str, no: &str| -> RiskScore {
        let hits = flagged(d);
        let score = if hits.is_empty() {
            base
        } else {
            (base + 20 + per * hits.len() as i64).min(90)
        };
        let shown: Vec<&str> = hits.iter().take(3).map(|f| f.path.as_str()).collect();
        RiskScore {
            dimension: d,
            score,
            level: score_to_level(score),
            rationale: if hits.is_empty() {
                no.to_string()
            } else {
                format!(
                    "{kind} {} touched: {}{}",
                    if hits.len() == 1 { "file" } else { "files" },
                    shown.join(", "),
                    if hits.len() > 3 { "…" } else { "" }
                )
            },
            contributing_file_ids: hits.iter().map(|f| f.path.clone()).collect(),
        }
    };
    let total_added: i64 = files.iter().map(|f| f.added).sum();
    let maint = ((total_added as f64 / 25.0).round() as i64 + files.len() as i64 * 2).min(85);
    let risks = vec![
        score(
            RiskDimension::Security,
            8,
            12,
            "Auth/credential-related",
            "No auth, credential or permission surface touched.",
        ),
        score(
            RiskDimension::Database,
            5,
            15,
            "Schema/migration/model",
            "No database-related files touched.",
        ),
        score(
            RiskDimension::Performance,
            5,
            8,
            "Cache/queue/worker",
            "No performance-critical files detected.",
        ),
        RiskScore {
            dimension: RiskDimension::Maintainability,
            score: maint,
            level: score_to_level(maint),
            rationale: format!(
                "{} {} changed, +{} {} added.",
                files.len(),
                if files.len() == 1 { "file" } else { "files" },
                total_added,
                if total_added == 1 { "line" } else { "lines" }
            ),
            contributing_file_ids: flagged(RiskDimension::Maintainability)
                .iter()
                .map(|f| f.path.clone())
                .collect(),
        },
    ];

    let mut findings: Vec<Value> = Vec::new();
    for f in files {
        let mut per = 0;
        let mut seen: HashMap<String, usize> = HashMap::new();
        'lines: for h in &f.hunks {
            for l in h.changed(LineKind::Add) {
                if per >= 25 {
                    break 'lines;
                }
                let (title, mut finding) = match line_check(&l.text) {
                    Some(LineCheck::Secret) => (
                        "Possible hardcoded secret",
                        json!({
                            "kind": "issue",
                            "severity": "high",
                            "dimension": "security",
                            "body": "A secret-like value appears in an added line. Move it to configuration or a secret store.",
                        }),
                    ),
                    Some(LineCheck::Leftover) => (
                        "Leftover debug code or TODO",
                        json!({
                            "kind": "suggestion",
                            "severity": "low",
                            "body": "This added line looks like leftover debug code or an unresolved TODO/FIXME.",
                        }),
                    ),
                    None => continue,
                };
                let n = seen.entry(l.text.trim().to_string()).or_default();
                *n += 1;
                let map = finding.as_object_mut().expect("object");
                map.insert(
                    "id".into(),
                    json!(finding_id(&[
                        "local",
                        title,
                        &f.path,
                        l.text.trim(),
                        &n.to_string()
                    ])),
                );
                map.insert("source".into(), json!("local"));
                map.insert("title".into(), json!(title));
                map.insert("filePath".into(), json!(f.path));
                if let Some(line) = l.new_line {
                    map.insert("line".into(), json!(line));
                }
                findings.push(finding);
                per += 1;
            }
        }
        if f.added > 400 {
            let title = format!("Large single-file change (+{} lines)", f.added);
            findings.push(json!({
                "id": finding_id(&["local", "large-change", &f.path]),
                "source": "local",
                "kind": "suggestion",
                "severity": "low",
                "dimension": "maintainability",
                "title": title,
                "body": "This file gained a lot of lines in one change. Consider splitting it, so it is easier to review.",
                "filePath": f.path,
                "line": 1,
            }));
        }
        if is_test_path(&f.path) && f.removed > f.added && f.removed > 0 {
            findings.push(json!({
                "id": finding_id(&["local", "test-shrank", &f.path]),
                "source": "local",
                "kind": "question-for-agent",
                "severity": "medium",
                "title": "Test file shrank",
                "body": format!(
                    "This test file lost more lines than it gained (−{} +{}). Check that no test coverage was dropped.",
                    f.removed, f.added
                ),
                "filePath": f.path,
                "line": 1,
            }));
        }
    }

    // Custom checks: Settings rules plus the repo's .ars/rules.json.
    let mut active = rules::parse_rules(rules_config);
    active.extend(rules::repo_rules_for(Some(root)));
    if !active.is_empty() {
        let changes = as_file_changes(files);
        let text_at: HashMap<(&str, i64), &str> = files
            .iter()
            .flat_map(|f| {
                f.hunks.iter().flat_map(move |h| {
                    h.changed(LineKind::Add).filter_map(move |l| {
                        l.new_line.map(|n| ((f.path.as_str(), n), l.text.trim()))
                    })
                })
            })
            .collect();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for mut f in rules::findings_for(&changes, &active) {
            let path = f["filePath"].as_str().unwrap_or("").to_string();
            let anchor = f["line"]
                .as_i64()
                .and_then(|n| text_at.get(&(path.as_str(), n)).copied())
                .unwrap_or("");
            // The rule's label names it (rule ids may contain dashes).
            let title = f["title"].as_str().unwrap_or("").to_string();
            let n = seen
                .entry(format!("{title}\0{path}\0{anchor}"))
                .or_default();
            *n += 1;
            f["id"] = json!(finding_id(&[
                "custom",
                &title,
                &path,
                anchor,
                &n.to_string()
            ]));
            findings.push(f);
        }
    }

    Assessment {
        areas,
        files: per_file,
        risks,
        findings,
    }
}

/// Did a passing test run follow the agent's last recorded edit of `path`?
/// The newest session that edited the file decides; a run without an exit
/// code never counts as a pass.
pub fn verification(root: &str, path: &str, sessions: &[AgentSession]) -> &'static str {
    // Paths are matched the way attribution matches them: relative to the
    // root, case folded where the file system ignores case.
    let want = fold_case(path);
    for s in sessions {
        let Some(cwd_rel) = session_cwd_rel(s, root) else {
            continue;
        };
        let edits_path =
            |p: &str| root_relative(p, &cwd_rel, root).is_some_and(|rel| fold_case(&rel) == want);
        let mut last_edit = None;
        let mut last_pass = None;
        let mut any_test = false;
        for (i, e) in s.timeline.iter().enumerate() {
            if e.kind == TimelineEventKind::FileEdit
                && e.file_path.as_deref().is_some_and(edits_path)
            {
                last_edit = Some(i);
            }
            if e.kind == TimelineEventKind::TestRun {
                any_test = true;
                if e.exit_code == Some(0) {
                    last_pass = Some(i);
                }
            }
        }
        if let Some(edit) = last_edit {
            return if !any_test {
                "no-signal"
            } else if last_pass.is_some_and(|p| p > edit) {
                "verified"
            } else {
                "unverified"
            };
        }
    }
    "no-signal"
}
