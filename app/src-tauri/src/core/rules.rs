// Custom review checks: user-defined rules that run on every project review
// (core/review/assess.rs) and on each scanned session, next to the built-in
// line checks.
//
// Two scopes, merged:
//   - global rules from ~/.agent-review-studio/config.json (`findingRules`),
//     edited in Settings → Review checks;
//   - repo rules from <project>/.ars/rules.json (`{ "rules": [...] }`),
//     committed with the repository.
//
// A rule matches either the ADDED LINES of a file's diff (line-anchored
// finding) or the file PATH (file-level finding). Patterns are Rust regex
// (no lookaround). Invalid or disabled rules are skipped silently: a broken
// custom rule must never break a review or a scan. Findings carry source
// "custom". In a session they land in session.findings, so they are part
// of session_hash.

use super::domain::{AgentSession, DiffLineKind, FileChange};
use super::normalize::truncate;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

/// Findings one rule may emit per review or session: a runaway pattern
/// (e.g. `.`) must not flood the review.
const MAX_FINDINGS_PER_RULE: usize = 20;

#[derive(Clone)]
pub struct Rule {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub severity: String,
    /// 'added-lines' (default) or 'path'
    pub target: String,
    pub pattern: Regex,
    pub file_glob: Option<Regex>,
    pub message: String,
}

const KINDS: [&str; 4] = ["issue", "suggestion", "question-for-agent", "potential-bug"];
const SEVERITIES: [&str; 5] = ["info", "low", "medium", "high", "critical"];

/// `src/**/*.ts`-style glob → anchored regex. `**` crosses directories,
/// `*` stays within one segment, `?` is one character.
fn glob_to_regex(glob: &str) -> Option<Regex> {
    let mut out = String::from("^");
    let normalized = glob.replace('\\', "/");
    let mut chars = normalized.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    // swallow a following '/' so "**/x" also matches "x"
                    if chars.peek() == Some(&'/') {
                        chars.next();
                        out.push_str("(?:.*/)?");
                    } else {
                        out.push_str(".*");
                    }
                } else {
                    out.push_str("[^/]*");
                }
            }
            '?' => out.push_str("[^/]"),
            c if "\\.+()[]{}^$|".contains(c) => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('$');
    Regex::new(&out).ok()
}

/// Parse raw rule JSON (from config or .ars/rules.json) into usable rules.
/// Disabled and malformed entries are dropped.
pub fn parse_rules(raw: &[Value]) -> Vec<Rule> {
    raw.iter()
        .filter_map(|r| {
            if !r.get("enabled").and_then(Value::as_bool).unwrap_or(true) {
                return None;
            }
            let pattern = compile_pattern(r.get("pattern").and_then(Value::as_str)?).ok()?;
            let id = r
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("rule")
                .to_string();
            let label = r
                .get("label")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("Custom check")
                .to_string();
            let kind = r
                .get("kind")
                .and_then(Value::as_str)
                .filter(|k| KINDS.contains(k))
                .unwrap_or("issue")
                .to_string();
            let severity = r
                .get("severity")
                .and_then(Value::as_str)
                .filter(|s| SEVERITIES.contains(s))
                .unwrap_or("medium")
                .to_string();
            let target = match r.get("target").and_then(Value::as_str) {
                Some("path") => "path".to_string(),
                _ => "added-lines".to_string(),
            };
            let file_glob = r
                .get("fileGlob")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|g| !g.is_empty())
                .and_then(glob_to_regex);
            let message = r
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            Some(Rule {
                id,
                label,
                kind,
                severity,
                target,
                pattern,
                file_glob,
                message,
            })
        })
        .collect()
}

/// (path → (rules.json mtime_ms, parsed rules)). Repo rules are read on
/// every review and scan, so they are cached by mtime.
type RepoRulesCache = HashMap<String, (Option<u128>, Vec<Rule>)>;
static REPO_RULES: Lazy<Mutex<RepoRulesCache>> = Lazy::new(|| Mutex::new(HashMap::new()));

pub fn repo_rules_for(project_path: Option<&str>) -> Vec<Rule> {
    let Some(root) = project_path.filter(|r| !r.is_empty()) else {
        return Vec::new();
    };
    let file = Path::new(root).join(".ars").join("rules.json");
    let mtime: Option<u128> = std::fs::metadata(&file)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis());
    if let Ok(cache) = REPO_RULES.lock() {
        if let Some((cached_mtime, rules)) = cache.get(root) {
            if *cached_mtime == mtime {
                return rules.clone();
            }
        }
    }
    let rules = std::fs::read_to_string(&file)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("rules").and_then(Value::as_array).cloned())
        .map(|arr| parse_rules(&arr))
        .unwrap_or_default();
    if let Ok(mut cache) = REPO_RULES.lock() {
        cache.insert(root.to_string(), (mtime, rules.clone()));
    }
    rules
}

/// Why the repository's `.ars/rules.json` gives no rules, when it exists
/// but cannot be used.
pub fn repo_rules_problem(root: &str) -> Option<String> {
    let raw = std::fs::read_to_string(Path::new(root).join(".ars").join("rules.json")).ok()?;
    match serde_json::from_str::<Value>(&raw) {
        Ok(v) if v.get("rules").is_some_and(Value::is_array) => None,
        Ok(_) => Some(".ars/rules.json has no \"rules\" list, so its checks are not used.".into()),
        Err(e) => Some(format!(
            ".ars/rules.json could not be read ({e}), so its checks are not used."
        )),
    }
}

/// Evaluate the rules against one normalized session, appending findings.
pub fn apply_rules(session: &mut AgentSession, rules: &[Rule]) {
    let found = findings_for(&session.files, rules);
    session.findings.extend(found);
}

/// The findings `rules` raise on `files` (their paths and added lines).
pub fn findings_for(files: &[FileChange], rules: &[Rule]) -> Vec<Value> {
    let mut found: Vec<Value> = Vec::new();
    for rule in rules {
        let mut hits = 0usize;
        for file in files {
            if hits >= MAX_FINDINGS_PER_RULE {
                break;
            }
            // Per-file ordinal for added-line hits without a line number
            // (Codex/Gemini/Cursor v1 hunks), so their ids stay distinct.
            let mut unanchored = 0usize;
            if let Some(glob) = &rule.file_glob {
                if !glob.is_match(&file.path) {
                    continue;
                }
            }
            if rule.target == "path" {
                if rule.pattern.is_match(&file.path) {
                    found.push(finding(
                        rule,
                        format!("custom-{}-{}", rule.id, file.id),
                        &file.path,
                        None,
                        &file.path,
                    ));
                    hits += 1;
                }
                continue;
            }
            // added-lines: line-anchored findings on the diff's + lines
            'lines: for hunk in &file.hunks {
                for line in &hunk.lines {
                    if line.kind != DiffLineKind::Add {
                        continue;
                    }
                    if rule.pattern.is_match(&line.text) {
                        let id = match line.new_line {
                            Some(n) => format!("custom-{}-{}-{}", rule.id, file.id, n),
                            None => {
                                unanchored += 1;
                                format!("custom-{}-{}-m{}", rule.id, file.id, unanchored)
                            }
                        };
                        found.push(finding(
                            rule,
                            id,
                            &file.path,
                            line.new_line,
                            line.text.trim(),
                        ));
                        hits += 1;
                        if hits >= MAX_FINDINGS_PER_RULE {
                            break 'lines;
                        }
                    }
                }
            }
        }
    }
    // Global and repo rules may share an id: keep finding ids unique (review
    // state is keyed by them) with a deterministic suffix.
    let mut used: HashSet<String> = HashSet::new();
    for f in found.iter_mut() {
        let Some(base) = f.get("id").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        let mut id = base.clone();
        let mut n = 1;
        while !used.insert(id.clone()) {
            n += 1;
            id = format!("{}~{}", base, n);
        }
        if id != base {
            if let Some(map) = f.as_object_mut() {
                map.insert("id".into(), json!(id));
            }
        }
    }
    found
}

fn finding(rule: &Rule, id: String, path: &str, line: Option<i64>, matched: &str) -> Value {
    let body = if rule.message.is_empty() {
        format!("Custom check matched: `{}`", truncate(matched, 160))
    } else {
        rule.message.clone()
    };
    let mut f = json!({
        "id": id,
        "kind": rule.kind,
        "severity": rule.severity,
        "title": rule.label,
        "body": body,
        "filePath": path,
        "source": "custom",
    });
    if let Some(n) = line {
        if let Some(map) = f.as_object_mut() {
            map.insert("line".into(), json!(n));
        }
    }
    f
}

/// Compile a rule pattern the way every rule is compiled: trimmed, Rust
/// regex defaults. An empty pattern is an error so callers can say why.
fn compile_pattern(source: &str) -> Result<Regex, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("The pattern is empty, so this check would never run.".into());
    }
    Regex::new(source).map_err(|e| e.to_string())
}

/// Settings → Review checks tester: test one sample (an added line or a file
/// path) with the same compiled pattern and `is_match` the scan uses.
pub fn test_pattern(pattern: &str, sample: &str) -> Result<bool, String> {
    Ok(compile_pattern(pattern)?.is_match(sample))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_ids_are_unique_without_line_numbers() {
        // The Codex fixture's hunks carry no line numbers.
        let expected: Value =
            serde_json::from_str(&super::super::fixture::read("codex/expected/basic.json"))
                .expect("json");
        let mut session: AgentSession =
            serde_json::from_value(expected["session"].clone()).expect("session");
        let before = session.findings.len();
        // The same rule id twice (global + repo scope) must not collide either.
        let rules = parse_rules(&[
            json!({ "id": "any", "pattern": "." }),
            json!({ "id": "any", "pattern": "." }),
        ]);
        apply_rules(&mut session, &rules);
        let ids: Vec<&str> = session.findings[before..]
            .iter()
            .filter_map(|f| f.get("id").and_then(Value::as_str))
            .collect();
        assert!(ids.len() > 2);
        let unique: HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "duplicate ids: {:?}", ids);
    }

    #[test]
    fn body_truncation_respects_max() {
        let rules = parse_rules(&[json!({ "id": "x", "pattern": "a" })]);
        let long = "a".repeat(400);
        let f = finding(&rules[0], "id".into(), "p", None, &long);
        let body = f["body"].as_str().unwrap();
        let inner = body
            .strip_prefix("Custom check matched: `")
            .and_then(|b| b.strip_suffix('`'))
            .unwrap();
        assert_eq!(inner.chars().count(), 160);
        assert!(inner.ends_with('…'));
    }

    #[test]
    fn glob_matches_segments_and_crossings() {
        let g = glob_to_regex("src/**/*.ts").unwrap();
        assert!(g.is_match("src/lib/api.ts"));
        assert!(g.is_match("src/a/b/c.ts"));
        assert!(!g.is_match("app/src/lib/api.rs"));
        let one = glob_to_regex("*.md").unwrap();
        assert!(one.is_match("README.md"));
        assert!(!one.is_match("docs/x.md"));
        let cross = glob_to_regex("**/*.sql").unwrap();
        assert!(cross.is_match("migrations/001.sql"));
        assert!(cross.is_match("001.sql"));
    }

    #[test]
    fn parses_and_skips_invalid() {
        let raw = vec![
            json!({ "id": "no-eval", "label": "No eval", "pattern": "\\beval\\(", "severity": "critical" }),
            json!({ "id": "broken", "pattern": "([" }),
            json!({ "id": "off", "pattern": "x", "enabled": false }),
        ];
        let rules = parse_rules(&raw);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "no-eval");
        assert_eq!(rules[0].kind, "issue");
        assert_eq!(rules[0].target, "added-lines");
    }
}
