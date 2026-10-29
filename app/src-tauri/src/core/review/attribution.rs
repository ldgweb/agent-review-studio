// Who wrote each hunk: the working-tree diff matched line by line against
// the edits the agents' transcripts recorded. Each changed line goes to the
// newest recorded edit that wrote it. A hunk carries the edit that explains
// most of its lines: its request and the agent's own words before it.
// Lines that no recorded edit explains were changed some other way (a shell
// command, a script, a person), and the hunk says so.
//
// Counts are what they are (lines matched / lines changed), never a score.

use super::super::domain::{AgentSession, RecordedEdit, TimelineEventKind};
use super::super::paths::{fold_case, strip_root};
use super::changes::{ChangedFile, Hunk, LineKind};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandHint {
    pub session_id: String,
    pub event_id: String,
    pub command: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Origin {
    /// "agent": every changed line comes from recorded edits; "partial":
    /// some do; "none": none does
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// Changed lines explained by recorded edits / changed lines considered
    pub matched: usize,
    pub total: usize,
    /// For unexplained lines: the newest command that names the file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_hint: Option<CommandHint>,
}

impl Origin {
    pub(crate) fn none(total: usize) -> Self {
        Origin {
            kind: "none",
            session_id: None,
            agent_label: None,
            turn: None,
            prompt: None,
            rationale: None,
            timestamp: None,
            event_id: None,
            matched: 0,
            total,
            command_hint: None,
        }
    }
}

/// One recorded edit, its lines as sets (trimmed) for matching.
struct EditRef<'a> {
    session: usize,
    edit: &'a RecordedEdit,
    added: HashSet<&'a str>,
    removed: HashSet<&'a str>,
}

pub struct Attribution {
    /// hunk id → origin
    pub hunks: HashMap<String, Origin>,
    /// path → ids of the sessions with recorded edits on it
    pub file_sessions: HashMap<String, Vec<String>>,
    /// session id → hunks it explains
    pub per_session: HashMap<String, usize>,
}

/// `path` of `session` as a path relative to `root`, or None outside it.
pub(crate) fn root_relative(path: &str, session_cwd_rel: &str, root: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    let absolute = p.starts_with('/') || p.as_bytes().get(1) == Some(&b':');
    let rel = if absolute {
        strip_root(&p, root.trim_end_matches('/'))?.to_string()
    } else if session_cwd_rel.is_empty() {
        p.trim_start_matches("./").to_string()
    } else {
        format!("{session_cwd_rel}/{}", p.trim_start_matches("./"))
    };
    (!rel.is_empty()).then_some(rel)
}

/// Where a session ran, relative to `root` ("" = the root itself).
pub fn session_cwd_rel(session: &AgentSession, root: &str) -> Option<String> {
    let cwd = session.project_path.as_deref()?.replace('\\', "/");
    let cwd = cwd.trim_end_matches('/');
    let root = root.trim_end_matches('/');
    if fold_case(cwd) == fold_case(root) {
        return Some(String::new());
    }
    strip_root(cwd, root).map(str::to_string)
}

fn line_set(lines: &[String]) -> HashSet<&str> {
    lines
        .iter()
        .map(|l| l.trim())
        .filter(|t| !t.is_empty())
        .collect()
}

fn changed_lines(h: &Hunk) -> (Vec<&str>, bool) {
    let added: Vec<&str> = h
        .changed(LineKind::Add)
        .map(|l| l.text.trim())
        .filter(|t| !t.is_empty())
        .collect();
    if !added.is_empty() {
        return (added, true);
    }
    let removed: Vec<&str> = h
        .changed(LineKind::Del)
        .map(|l| l.text.trim())
        .filter(|t| !t.is_empty())
        .collect();
    (removed, false)
}

/// The newest command of any session that names `path` (or its file name).
fn command_hint(sessions: &[AgentSession], path: &str) -> Option<CommandHint> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let needles: Vec<&str> = if name.len() >= 5 && name != path {
        vec![path, name]
    } else {
        vec![path]
    };
    for s in sessions {
        for e in s.timeline.iter().rev() {
            if !matches!(
                e.kind,
                TimelineEventKind::Command | TimelineEventKind::TestRun
            ) {
                continue;
            }
            let Some(cmd) = e.command.as_deref() else {
                continue;
            };
            if needles.iter().any(|n| cmd.contains(n)) {
                return Some(CommandHint {
                    session_id: s.session_id.clone(),
                    event_id: e.id.clone(),
                    command: cmd.to_string(),
                });
            }
        }
    }
    None
}

/// Attribute every hunk of `files`. `sessions` come newest first.
pub fn attribute(root: &str, files: &[ChangedFile], sessions: &[AgentSession]) -> Attribution {
    // path → edits on it, newest first
    let mut by_path: HashMap<String, Vec<EditRef>> = HashMap::new();
    for (si, s) in sessions.iter().enumerate() {
        let Some(cwd_rel) = session_cwd_rel(s, root) else {
            continue;
        };
        for edit in s.edits.iter().rev() {
            let Some(rel) = root_relative(&edit.path, &cwd_rel, root) else {
                continue;
            };
            by_path
                .entry(fold_case(&rel).into_owned())
                .or_default()
                .push(EditRef {
                    session: si,
                    edit,
                    added: line_set(&edit.added),
                    removed: line_set(&edit.removed),
                });
        }
    }

    let mut out = Attribution {
        hunks: HashMap::new(),
        file_sessions: HashMap::new(),
        per_session: HashMap::new(),
    };
    for f in files {
        let edits = by_path.get(fold_case(&f.path).as_ref());
        if let Some(edits) = edits {
            let mut ids: Vec<String> = Vec::new();
            for e in edits {
                let id = &sessions[e.session].session_id;
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
            out.file_sessions.insert(f.path.clone(), ids);
        }
        let mut hint: Option<Option<CommandHint>> = None;
        for h in &f.hunks {
            let (targets, adds) = changed_lines(h);
            let mut origin = Origin::none(targets.len());
            if let Some(edits) = edits.filter(|_| !targets.is_empty()) {
                // Each line goes to the newest edit that wrote it (or, for a
                // deletion-only hunk, removed it).
                let mut counts: Vec<usize> = vec![0; edits.len()];
                let mut matched = 0;
                for t in &targets {
                    let hit = edits.iter().position(|e| {
                        if adds {
                            e.added.contains(t)
                        } else {
                            e.removed.contains(t)
                        }
                    });
                    if let Some(i) = hit {
                        counts[i] += 1;
                        matched += 1;
                    }
                }
                if matched > 0 {
                    // The edit explaining most lines; ties go to the newest.
                    let best = (0..edits.len())
                        .max_by_key(|&i| (counts[i], std::cmp::Reverse(i)))
                        .unwrap_or(0);
                    let e = &edits[best];
                    let s = &sessions[e.session];
                    origin = Origin {
                        kind: if matched == targets.len() {
                            "agent"
                        } else {
                            "partial"
                        },
                        session_id: Some(s.session_id.clone()),
                        agent_label: Some(s.agent_label.clone()),
                        turn: Some(e.edit.turn),
                        prompt: s
                            .turns
                            .iter()
                            .find(|t| t.index == e.edit.turn)
                            .map(|t| t.prompt.clone()),
                        rationale: (!e.edit.rationale.is_empty()).then(|| e.edit.rationale.clone()),
                        timestamp: Some(e.edit.timestamp.clone()),
                        event_id: e.edit.event_id.clone(),
                        matched,
                        total: targets.len(),
                        command_hint: None,
                    };
                    *out.per_session.entry(s.session_id.clone()).or_default() += 1;
                }
            }
            if origin.kind != "agent" && !sessions.is_empty() {
                origin.command_hint = hint
                    .get_or_insert_with(|| command_hint(sessions, &f.path))
                    .clone();
            }
            out.hunks.insert(h.id.clone(), origin);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::changes::parse_diff;
    use super::*;
    use crate::app_core::domain::{SessionTurn, TimelineEvent};

    fn session(id: &str, cwd: &str, edits: Vec<RecordedEdit>, commands: &[&str]) -> AgentSession {
        let raw = super::super::super::fixture::read("claude-code/expected/basic.json");
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let mut s: AgentSession = serde_json::from_value(v["session"].clone()).unwrap();
        s.session_id = id.into();
        s.agent_label = "Claude Code".into();
        s.project_path = Some(cwd.into());
        s.edits = edits;
        s.turns = vec![SessionTurn {
            index: 0,
            prompt: "Add discount codes".into(),
            timestamp: "t".into(),
        }];
        s.timeline = commands
            .iter()
            .enumerate()
            .map(|(i, c)| {
                serde_json::from_value::<TimelineEvent>(serde_json::json!({
                    "id": format!("ev-{}", i + 1), "timestamp": "t", "kind": "command",
                    "title": "run", "command": c
                }))
                .unwrap()
            })
            .collect();
        s
    }

    fn edit(path: &str, added: &[&str], rationale: &str) -> RecordedEdit {
        RecordedEdit {
            seq: 1,
            path: path.into(),
            added: added.iter().map(|s| s.to_string()).collect(),
            removed: Vec::new(),
            turn: 0,
            rationale: rationale.into(),
            timestamp: "t".into(),
            event_id: Some("ev-9".into()),
        }
    }

    #[test]
    fn hunks_get_the_agents_words_or_say_nobody_recorded_them() {
        let files = parse_diff(concat!(
            "diff --git a/app/cart.ts b/app/cart.ts\n--- a/app/cart.ts\n+++ b/app/cart.ts\n",
            "@@ -1,2 +1,4 @@\n ctx\n+const total = round(sum);\n+  return total;\n ctx\n",
            "@@ -20,1 +22,2 @@\n ctx\n+const total = round(sum);\n+console.log(total);\n",
            "@@ -40,1 +43,2 @@\n ctx\n+// hand-written\n",
        ));
        // The session ran in app/: its edit paths are relative to it.
        let sessions = vec![session(
            "s1",
            "C:/repo/app",
            vec![edit(
                "cart.ts",
                &["const total = round(sum);", "  return total;"],
                "Round once, at the end.",
            )],
            &["python patch.py app/cart.ts"],
        )];
        let a = attribute("C:/repo", &files, &sessions);
        let o = |i: usize| &a.hunks[&files[0].hunks[i].id];
        assert_eq!(o(0).kind, "agent");
        assert_eq!(o(0).rationale.as_deref(), Some("Round once, at the end."));
        assert_eq!(o(0).prompt.as_deref(), Some("Add discount codes"));
        assert_eq!((o(1).kind, o(1).matched, o(1).total), ("partial", 1, 2));
        assert_eq!(o(2).kind, "none");
        assert_eq!(
            o(2).command_hint.as_ref().map(|h| h.command.as_str()),
            Some("python patch.py app/cart.ts")
        );
        assert_eq!(a.file_sessions["app/cart.ts"], vec!["s1".to_string()]);
    }
}
