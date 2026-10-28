// The shared normalizer of the ingestion layer. The fixture contract in the
// repo-root fixtures/ pins the behavior.

use super::domain::*;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

/// Cap `text` at `max` UTF-16 code units (what the fixture contract counts),
/// ending in '…' when cut. Carriage returns are removed first.
pub fn truncate(text: &str, max: usize) -> String {
    let clean = text.replace('\r', "");
    let units: Vec<u16> = clean.encode_utf16().collect();
    if units.len() > max {
        let mut out = String::from_utf16_lossy(&units[..max.saturating_sub(1)]);
        out.push('…');
        out
    } else {
        clean
    }
}

pub fn first_line(text: &str) -> String {
    text.split('\n').next().unwrap_or("").trim().to_string()
}

pub static TEST_COMMAND: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(test|pytest|jest|vitest|phpunit|artisan\s+test|go\s+test|cargo\s+test|npm\s+(run\s+)?test)\b").unwrap()
});

/// Result of `content_diff`.
pub struct ContentDiff {
    pub hunks: Vec<DiffHunk>,
    pub added: i64,
    pub removed: i64,
    pub added_lines: Vec<String>,
}

/// Line-level diff of two full file contents (before → after): common prefix/suffix trimmed, the changed middle becomes one del/add hunk with real line numbers.
pub fn content_diff(before: &str, after: &str) -> ContentDiff {
    let before_clean = before.replace('\r', "");
    let after_clean = after.replace('\r', "");
    let before_lines: Vec<&str> = if before.is_empty() {
        Vec::new()
    } else {
        before_clean.split('\n').collect()
    };
    let after_lines: Vec<&str> = if after.is_empty() {
        Vec::new()
    } else {
        after_clean.split('\n').collect()
    };
    let min = before_lines.len().min(after_lines.len());
    let mut prefix = 0usize;
    while prefix < min && before_lines[prefix] == after_lines[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < min - prefix
        && before_lines[before_lines.len() - 1 - suffix]
            == after_lines[after_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let dels = &before_lines[prefix..before_lines.len() - suffix];
    let adds = &after_lines[prefix..after_lines.len() - suffix];
    if dels.is_empty() && adds.is_empty() {
        return ContentDiff {
            hunks: Vec::new(),
            added: 0,
            removed: 0,
            added_lines: Vec::new(),
        };
    }
    let mut lines: Vec<DiffLine> = Vec::new();
    for (i, text) in dels.iter().enumerate() {
        lines.push(DiffLine {
            kind: DiffLineKind::Del,
            text: (*text).to_string(),
            old_line: Some((prefix + 1 + i) as i64),
            new_line: None,
        });
    }
    for (i, text) in adds.iter().enumerate() {
        lines.push(DiffLine {
            kind: DiffLineKind::Add,
            text: (*text).to_string(),
            old_line: None,
            new_line: Some((prefix + 1 + i) as i64),
        });
    }
    lines.truncate(400);
    ContentDiff {
        hunks: vec![DiffHunk {
            header: format!(
                "@@ -{},{} +{},{} @@",
                prefix + 1,
                dels.len(),
                prefix + 1,
                adds.len()
            ),
            lines,
        }],
        added: adds.len() as i64,
        removed: dels.len() as i64,
        added_lines: adds.iter().map(|s| (*s).to_string()).collect(),
    }
}

static SECURITY_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)auth|login|password|token|secret|crypt|session|permission|jwt|oauth|acl|csrf")
        .unwrap()
});
static DATABASE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)migration|schema|\.sql\b|database|models?[/\\]").unwrap());
static PERFORMANCE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)cache|queue|worker|cron|index\.(sql|php)").unwrap());
static OUTSIDE_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([A-Za-z]:|/|~)").unwrap());

// Local heuristic checks. The fixtures pin this behavior: treat it as frozen.
static SECRET_LINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)(AKIA[0-9A-Z]{16}|(api[_-]?key|secret|token|password|passwd|authorization|bearer)\s*[:=]\s*["']?[A-Za-z0-9_\-.]{8,})"#,
    )
    .unwrap()
});
static LEFTOVER_LINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"console\.(log|debug)|\bdebugger\b|println!|\bdbg!|\bTODO\b|\bFIXME\b|\bXXX\b|\bHACK\b",
    )
    .unwrap()
});
static TEST_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(^|[\\/])(tests?|spec|specs|__tests__|e2e|cypress)([\\/]|$)|\.(test|spec)\.[a-z]+$",
    )
    .unwrap()
});

/// The built-in per-line checks, shared by the session normalizer and the
/// project review.
pub(crate) enum LineCheck {
    Secret,
    Leftover,
}

pub(crate) fn line_check(text: &str) -> Option<LineCheck> {
    if SECRET_LINE.is_match(text) {
        Some(LineCheck::Secret)
    } else if LEFTOVER_LINE.is_match(text) {
        Some(LineCheck::Leftover)
    } else {
        None
    }
}

pub(crate) fn is_test_path(path: &str) -> bool {
    TEST_PATH.is_match(path)
}

pub(crate) fn score_to_level(score: i64) -> RiskLevel {
    if score >= 80 {
        RiskLevel::Critical
    } else if score >= 60 {
        RiskLevel::High
    } else if score >= 35 {
        RiskLevel::Medium
    } else {
        RiskLevel::Low
    }
}

pub(crate) fn language_of(rel: &str) -> String {
    let ext = if rel.contains('.') {
        rel.rsplit('.').next().unwrap_or("").to_lowercase()
    } else {
        String::new()
    };
    if ext.is_empty() {
        "text".to_string()
    } else {
        ext
    }
}

pub(crate) fn risk_flags_for(
    rel: &str,
    added_lines: &[String],
    lines_added: i64,
) -> Vec<RiskDimension> {
    let mut flags = Vec::new();
    let haystack = format!("{}\n{}", rel, added_lines.join("\n"));
    if SECURITY_PATTERN.is_match(&haystack) {
        flags.push(RiskDimension::Security);
    }
    if DATABASE_PATTERN.is_match(rel) {
        flags.push(RiskDimension::Database);
    }
    if PERFORMANCE_PATTERN.is_match(rel) {
        flags.push(RiskDimension::Performance);
    }
    if lines_added > 250 {
        flags.push(RiskDimension::Maintainability);
    }
    flags
}

pub(crate) fn area_id_of(top: &str) -> String {
    let sanitized: String = top
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("area-{}", sanitized.to_lowercase())
}

pub fn normalize(raw: RawSession) -> Option<AgentSession> {
    if raw.files.is_empty() {
        return None; // nothing to review
    }

    // Insertion-ordered area registry
    let mut area_order: Vec<String> = Vec::new();
    let mut areas: Vec<SystemArea> = Vec::new();
    let area_of = |rel: &str, areas: &mut Vec<SystemArea>, order: &mut Vec<String>| -> String {
        let outside = OUTSIDE_PATTERN.is_match(rel);
        let top = if outside {
            "(outside project)".to_string()
        } else if rel.contains('/') {
            rel.split('/').next().unwrap_or("").to_string()
        } else {
            "(root)".to_string()
        };
        let id = area_id_of(&top);
        if !order.contains(&id) {
            order.push(id.clone());
            areas.push(SystemArea {
                id: id.clone(),
                label: top,
                critical: false,
                depends_on: Vec::new(), // folder groups carry no inferred dependency edges
            });
        }
        id
    };

    let mut files: Vec<FileChange> = Vec::new();
    for (i, f) in raw.files.iter().enumerate() {
        let area_id = area_of(&f.path, &mut areas, &mut area_order);
        let outside = areas
            .iter()
            .find(|a| a.id == area_id)
            .map(|a| a.label == "(outside project)")
            .unwrap_or(false);
        let mut flags = risk_flags_for(&f.path, &f.added_lines, f.added);
        if outside {
            flags.retain(|d| *d == RiskDimension::Maintainability);
        }
        if flags.contains(&RiskDimension::Security) || flags.contains(&RiskDimension::Database) {
            if let Some(area) = areas.iter_mut().find(|a| a.id == area_id) {
                area.critical = true;
            }
        }
        let (change_type, verb) = if f.deleted {
            (ChangeType::Deleted, "Deleted")
        } else if f.created {
            (ChangeType::Created, "Created")
        } else {
            (ChangeType::Modified, "Modified")
        };
        files.push(FileChange {
            id: format!("lf-{}", i + 1),
            path: f.path.clone(),
            change_type,
            language: language_of(&f.path),
            lines_added: f.added,
            lines_removed: f.removed,
            area_id,
            risk_flags: flags,
            summary: format!(
                "{} — +{} −{} lines in this session.",
                verb, f.added, f.removed
            ),
            hunks: f.hunks.clone(),
        });
    }

    let total_added: i64 = files.iter().map(|f| f.lines_added).sum();
    let total_removed: i64 = files.iter().map(|f| f.lines_removed).sum();

    let files_with = |d: RiskDimension| -> Vec<&FileChange> {
        files.iter().filter(|f| f.risk_flags.contains(&d)).collect()
    };
    let risk_score = |d: RiskDimension,
                      base: i64,
                      per_file: i64,
                      rationale_yes: &str,
                      rationale_no: &str|
     -> RiskScore {
        let hits = files_with(d);
        let score = if hits.is_empty() {
            base
        } else {
            (base + 20 + per_file * hits.len() as i64).min(90)
        };
        let rationale = if !hits.is_empty() {
            let shown: Vec<&str> = hits.iter().take(3).map(|f| f.path.as_str()).collect();
            format!(
                "{}: {}{}",
                rationale_yes,
                shown.join(", "),
                if hits.len() > 3 { "…" } else { "" }
            )
        } else {
            rationale_no.to_string()
        };
        RiskScore {
            dimension: d,
            score,
            level: score_to_level(score),
            rationale,
            contributing_file_ids: hits.iter().map(|f| f.id.clone()).collect(),
        }
    };

    let maint_score = ((total_added as f64 / 25.0).round() as i64 + files.len() as i64 * 2).min(85);
    let risks = vec![
        risk_score(
            RiskDimension::Security,
            8,
            12,
            "Auth/credential-related files touched",
            "No auth, credential or permission surface touched.",
        ),
        risk_score(
            RiskDimension::Database,
            5,
            15,
            "Schema/migration/model files touched",
            "No database-related files touched.",
        ),
        risk_score(
            RiskDimension::Performance,
            5,
            8,
            "Cache/queue/worker files touched",
            "No performance-critical files detected.",
        ),
        RiskScore {
            dimension: RiskDimension::Maintainability,
            score: maint_score,
            level: score_to_level(maint_score),
            rationale: format!(
                "{} files changed, +{} lines added in one session.",
                files.len(),
                total_added
            ),
            contributing_file_ids: files_with(RiskDimension::Maintainability)
                .iter()
                .map(|f| f.id.clone())
                .collect(),
        },
    ];

    let mut risk_warnings: Vec<String> = Vec::new();
    if !files_with(RiskDimension::Security).is_empty() {
        risk_warnings.push(
            "Security-sensitive files were modified — review auth/credential handling.".to_string(),
        );
    }
    if !files_with(RiskDimension::Database).is_empty() {
        risk_warnings.push("Database schema or model files were modified.".to_string());
    }
    if raw.tests_run == 0 && total_added > 50 {
        risk_warnings.push("No test runs detected in this session.".to_string());
    }
    if raw.errors > 0 {
        risk_warnings.push(format!(
            "{} tool call{} failed during the session.",
            raw.errors,
            if raw.errors > 1 { "s" } else { "" }
        ));
    }

    // Local heuristic checks.
    let mut findings: Vec<Value> = Vec::new();
    for file in &files {
        let mut per_file = 0;
        'lines: for h in &file.hunks {
            for l in &h.lines {
                if !matches!(l.kind, DiffLineKind::Add) {
                    continue;
                }
                if per_file >= 25 {
                    break 'lines;
                }
                // Adapters without real line numbers (Codex patches, Gemini
                // and Cursor v1 edits) still get the finding, just unanchored.
                let mut finding = if SECRET_LINE.is_match(&l.text) {
                    serde_json::json!({
                        "id": format!("lc-{}", findings.len() + 1),
                        "source": "local",
                        "kind": "issue",
                        "severity": "high",
                        "dimension": "security",
                        "title": "Possible hardcoded secret",
                        "body": "A secret-like value appears in an added line. Move it to configuration or a secret store.",
                        "filePath": file.path,
                    })
                } else if LEFTOVER_LINE.is_match(&l.text) {
                    serde_json::json!({
                        "id": format!("lc-{}", findings.len() + 1),
                        "source": "local",
                        "kind": "suggestion",
                        "severity": "low",
                        "title": "Debug/diagnostic or TODO left in the change",
                        "body": "This added line looks like a leftover debug statement or an unresolved TODO/FIXME.",
                        "filePath": file.path,
                    })
                } else {
                    continue;
                };
                if let (Some(line), Some(map)) = (l.new_line, finding.as_object_mut()) {
                    map.insert("line".into(), serde_json::json!(line));
                }
                findings.push(finding);
                per_file += 1;
            }
        }
        if file.lines_added > 400 {
            findings.push(serde_json::json!({
                "id": format!("lc-{}", findings.len() + 1),
                "source": "local",
                "kind": "suggestion",
                "severity": "low",
                "dimension": "maintainability",
                "title": format!("Large single-file change (+{} lines)", file.lines_added),
                "body": "This file gained a lot of lines in one session. Consider splitting it, so it is easier to review.",
                "filePath": file.path,
                "line": 1,
            }));
        }
        if TEST_PATH.is_match(&file.path)
            && file.lines_removed > file.lines_added
            && file.lines_removed > 0
        {
            findings.push(serde_json::json!({
                "id": format!("lc-{}", findings.len() + 1),
                "source": "local",
                "kind": "question-for-agent",
                "severity": "medium",
                "title": "Test file shrank",
                "body": format!(
                    "This test file lost more lines than it gained (−{} +{}). Check that no test coverage was dropped.",
                    file.lines_removed, file.lines_added
                ),
                "filePath": file.path,
                "line": 1,
            }));
        }
    }

    let area_labels: Vec<&str> = areas.iter().map(|a| a.label.as_str()).collect();
    let declared_goal = match &raw.declared_goal {
        Some(g) if !g.is_empty() => g.clone(),
        _ => truncate(&first_line(&raw.original_prompt), 110),
    };
    let outcome = if raw.outcome.is_empty() {
        "No final self-report found in the transcript.".to_string()
    } else {
        raw.outcome.clone()
    };

    Some(AgentSession {
        session_id: raw.session_id,
        source: Some("live".to_string()),
        timestamp: raw.timestamp,
        agent: raw.agent,
        agent_label: raw.agent_label.clone(),
        model: raw.model,
        project_name: raw.project_name,
        project_path: raw.project_path,
        branch: raw.branch,
        original_prompt: raw.original_prompt,
        declared_goal,
        initial_plan: raw.initial_plan,
        status: "needs-review".to_string(),
        outcome,
        stats: SessionStats {
            files_analyzed: raw.files_analyzed,
            files_modified: files.len() as i64,
            lines_added: total_added,
            lines_removed: total_removed,
            commands_run: raw.commands_run,
            tests_run: raw.tests_run,
            tests_failed: raw.tests_failed,
            duration_minutes: raw.duration_minutes,
        },
        summary: SessionSummary {
            text: format!(
                "Ingested from a live {} transcript. The agent changed {} file{} (+{} −{}) across {}, ran {} command{} and {} test run{}.",
                raw.agent_label,
                files.len(),
                if files.len() > 1 { "s" } else { "" },
                total_added,
                total_removed,
                area_labels.join(", "),
                raw.commands_run,
                if raw.commands_run == 1 { "" } else { "s" },
                raw.tests_run,
                if raw.tests_run == 1 { "" } else { "s" },
            ),
            impacted_areas: area_labels.iter().map(|s| s.to_string()).collect(),
            risk_warnings,
        },
        system_areas: areas,
        files,
        timeline: raw.timeline,
        risks,
        findings,
        turns: Vec::new(),
        edits: Vec::new(),
        archived: None,
    })
}

/// SHA256 of the full session with recursively sorted object keys. Changing
/// evidence with unchanged counts must still invalidate indexed content.
pub fn session_hash(session: &AgentSession) -> String {
    use sha2::{Digest, Sha256};
    // Include every field, notably hunk text, command outcomes and findings.
    fn canonical(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.sort_keys();
                for value in map.values_mut() {
                    canonical(value);
                }
            }
            Value::Array(values) => {
                for value in values {
                    canonical(value);
                }
            }
            _ => {}
        }
    }
    let mut value =
        serde_json::to_value(session).expect("AgentSession serialization is infallible");
    canonical(&mut value);
    let payload = serde_json::to_vec(&value).expect("JSON serialization is infallible");
    format!("{:x}", Sha256::digest(payload))
}

/// ISO-8601 UTC with milliseconds: `YYYY-MM-DDTHH:MM:SS.sssZ`.
pub fn to_iso(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Parse an RFC 3339 timestamp, or a naive `YYYY-MM-DD HH:MM:SS` read as UTC;
/// None for anything else.
pub fn parse_date(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
    if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
        return Some(dt.with_timezone(&Utc));
    }
    // Naive "YYYY-MM-DD HH:MM:SS" (aider's local timestamps), treated as UTC
    if let Ok(naive) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S") {
        return Some(Utc.from_utc_datetime(&naive));
    }
    None
}

/// Whole minutes between two timestamps, rounded to nearest (halves up), at
/// least 1; 1 when either does not parse.
pub fn duration_minutes(first: &str, last: &str) -> i64 {
    match (parse_date(first), parse_date(last)) {
        (Some(a), Some(b)) => {
            let ms = (b - a).num_milliseconds() as f64;
            ((ms / 60_000.0).round() as i64).max(1)
        }
        _ => 1,
    }
}
