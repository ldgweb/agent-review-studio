// The review for the agent. ARS never edits the project: every verdict
// becomes an instruction here, and the agent makes the change.
//
// The text is written for a model to act on with few tokens: one numbered
// item per thing to do, grouped by file. Each item starts with a verb
// (CHANGE, REPLACE, REMOVE, ANSWER, UNDO, DELETE, RESTORE, FIX) and points
// at the line as it is now, with the code quoted. Code is written out only
// where the agent cannot know it: what to put back, or what to write
// instead.
//
// One builder for every output: Copy review in the app, `ars feedback`,
// and the text a round records.

use super::changes::{Line, LineKind};
use super::state::{Comment, ReviewState, Verdict};
use super::{ProjectInfo, ReviewFile, ReviewHunk};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;

/// Code to put back is quoted up to this many lines; past it, a pointer to HEAD
const MAX_CODE_LINES: usize = 12;
const MAX_QUOTE: usize = 70;
const MAX_ACCEPTED_NAMES: usize = 10;

#[derive(Clone, Serialize, Debug, Default, PartialEq)]
pub struct Package {
    /// Things it asks the agent to do
    pub items: usize,
    pub text: String,
}

/// A file's verdicts taken together
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FileView {
    Approved,
    Rejected,
    Changes,
    Mixed,
    Pending,
}

fn view_of(v: Verdict) -> FileView {
    match v {
        Verdict::Approved => FileView::Approved,
        Verdict::Rejected => FileView::Rejected,
        Verdict::Changes => FileView::Changes,
    }
}

/// Pending while any change is undecided; a file without changes to show
/// binds to its fingerprint.
pub(crate) fn file_view(state: &ReviewState, f: &ReviewFile) -> FileView {
    if f.hunks.is_empty() {
        return match state.files.get(&f.path) {
            Some(v) if v.fingerprint == f.fingerprint => view_of(v.verdict),
            _ => FileView::Pending,
        };
    }
    let mut first: Option<Verdict> = None;
    for h in &f.hunks {
        let Some(v) = state.hunks.get(&h.id) else {
            return FileView::Pending;
        };
        match first {
            None => first = Some(v.verdict),
            Some(f0) if f0 != v.verdict => return FileView::Mixed,
            _ => {}
        }
    }
    first.map(view_of).unwrap_or(FileView::Pending)
}

/// "line 12", "lines 12–14", "lines 12–14, 18"
fn line_span(ns: &[i64]) -> String {
    let mut sorted: Vec<i64> = ns.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut runs: Vec<String> = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let mut j = i;
        while j + 1 < sorted.len() && sorted[j + 1] == sorted[j] + 1 {
            j += 1;
        }
        runs.push(if j > i {
            format!("{}–{}", sorted[i], sorted[j])
        } else {
            sorted[i].to_string()
        });
        i = j + 1;
    }
    let word = if sorted.len() == 1 { "line" } else { "lines" };
    format!("{word} {}", runs.join(", "))
}

/// Text from the project that goes on one line of the review: control
/// characters (a newline in a file name, say) are written escaped, so they
/// can never start a line of their own.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() && c != '\t' {
                c.escape_default().collect()
            } else {
                c.to_string()
            }
        })
        .collect()
}

fn quote(text: &str) -> String {
    let t = one_line(text.trim()).replace('`', "'");
    if t.is_empty() {
        return String::new();
    }
    if t.chars().count() > MAX_QUOTE {
        let cut: String = t.chars().take(MAX_QUOTE - 1).collect();
        format!(" `{cut}…`")
    } else {
        format!(" `{t}`")
    }
}

/// A fenced block that no backtick run inside the code can close early
fn fence(lines: &[&str], language: &str) -> String {
    let mut ticks = String::from("```");
    while lines.iter().any(|l| l.contains(&ticks)) {
        ticks.push('`');
    }
    let lang = if !language.is_empty()
        && language.len() <= 12
        && language
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '-'))
    {
        language.to_ascii_lowercase()
    } else {
        String::new()
    };
    let mut out = vec![format!("{ticks}{lang}")];
    out.extend(lines.iter().map(|l| l.to_string()));
    out.push(ticks);
    out.join("\n")
}

/// One shell argument that bash and PowerShell both read literally: bare
/// when plain, else in single quotes. None when it cannot be written safely
/// (a single quote or a control character in it).
fn shell_arg(arg: &str) -> Option<String> {
    if arg.chars().any(|c| c.is_control() || c == '\'') {
        return None;
    }
    let plain = arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '@' | '+' | '-' | ':'));
    Some(if plain {
        arg.to_string()
    } else {
        format!("'{arg}'")
    })
}

/// `git checkout HEAD -- <path>`, the path taken literally even when it
/// looks like a pattern (`app/[id]/page.tsx`)
fn restore_cmd(path: &str) -> Option<String> {
    let spec = if path.contains(['*', '?', '[']) {
        format!(":(literal){path}")
    } else {
        path.to_string()
    };
    shell_arg(&spec).map(|a| format!("`git checkout HEAD -- {a}`"))
}

/// `git show HEAD:<path>`
fn show_cmd(path: &str) -> Option<String> {
    shell_arg(&format!("HEAD:{path}")).map(|a| format!("`git show {a}`"))
}

#[derive(Default)]
struct Item {
    head: String,
    code: Option<String>,
    notes: Vec<String>,
}

fn item(head: String) -> Item {
    Item {
        head,
        ..Item::default()
    }
}

fn comment_spot(c: &Comment) -> String {
    if c.kind == "del" {
        return format!("the removed line{}", quote(&c.line_text));
    }
    match c.line {
        Some(n) if n != 0 => format!("line {n}{}", quote(&c.line_text)),
        _ => format!("the line{}", quote(&c.line_text)),
    }
}

fn comment_item(c: &Comment, language: &str, suffix: &str) -> Item {
    let spot = comment_spot(c);
    let text = c.text.trim();
    let notes = if text.is_empty() {
        Vec::new()
    } else {
        vec![text.to_string()]
    };
    if let (Some(code), false) = (&c.suggestion, c.kind == "del") {
        if code.trim().is_empty() {
            return Item {
                head: format!("REMOVE {spot}{suffix}"),
                code: None,
                notes,
            };
        }
        let lines: Vec<&str> = code.split('\n').collect();
        return Item {
            head: format!("REPLACE {spot} with:{suffix}"),
            code: Some(fence(&lines, language)),
            notes,
        };
    }
    let verb = if text.ends_with('?') {
        "ANSWER"
    } else {
        "CHANGE"
    };
    item(format!("{verb} {spot}: {text}{suffix}"))
}

/// Where removed lines go back: after the line before them, as it is now.
fn insertion_point(h: &ReviewHunk) -> String {
    let first = h
        .lines
        .iter()
        .position(|l| l.kind == LineKind::Del)
        .unwrap_or(0);
    for l in h.lines[..first].iter().rev() {
        if let Some(n) = l.new_line {
            return format!("after line {n}");
        }
    }
    match h.lines[first..]
        .iter()
        .find(|l| l.kind == LineKind::Context && l.new_line.is_some())
    {
        Some(l) => format!("before line {}", l.new_line.unwrap_or(0)),
        None => "at the top of the file".to_string(),
    }
}

fn numbers(lines: &[&Line], new: bool) -> Vec<i64> {
    lines
        .iter()
        .filter_map(|l| if new { l.new_line } else { l.old_line })
        .collect()
}

/// The two ranges of "@@ -a,b +c,d @@" as line spans, the old side then
/// the new one ("" for an empty side)
fn header_spans(header: &str) -> (String, String) {
    let span = |part: Option<&str>| -> String {
        let Some(part) = part else {
            return String::new();
        };
        let mut nums = part[1..].split(',').map(|n| n.parse::<i64>().unwrap_or(0));
        let start = nums.next().unwrap_or(0);
        let len = nums.next().unwrap_or(1);
        match len {
            0 => String::new(),
            1 => format!("line {start}"),
            _ => format!("lines {start}–{}", start + len - 1),
        }
    };
    let mut parts = header.split_whitespace().skip(1);
    let old = span(parts.next().filter(|p| p.starts_with('-')));
    let new = span(parts.next().filter(|p| p.starts_with('+')));
    (old, new)
}

fn undo_hunk(f: &ReviewFile, h: &ReviewHunk) -> Item {
    let base = f.old_path.as_deref().unwrap_or(&f.path);
    let adds: Vec<&Line> = h.lines.iter().filter(|l| l.kind == LineKind::Add).collect();
    let dels: Vec<&Line> = h.lines.iter().filter(|l| l.kind == LineKind::Del).collect();
    // Too large to show: point at it instead of quoting part of it
    if h.truncated || (adds.is_empty() && dels.is_empty()) {
        let (old, new) = header_spans(&h.header);
        let at = if new.is_empty() {
            String::new()
        } else {
            format!(" {new}")
        };
        let from = match (old.is_empty(), show_cmd(base)) {
            (true, _) => "remove what you added there".to_string(),
            (false, Some(cmd)) => format!("put back HEAD's {old} ({cmd})"),
            (false, None) => format!("put back HEAD's {old}"),
        };
        return item(format!("UNDO the change at{at}: {from}."));
    }
    let added = line_span(&numbers(&adds, true));
    let first_add = quote(
        adds.iter()
            .find(|l| !l.text.trim().is_empty())
            .map(|l| l.text.as_str())
            .unwrap_or(""),
    );
    if dels.is_empty() {
        return item(format!(
            "UNDO {added}{first_add}: remove the lines you added."
        ));
    }
    let fits = dels.len() <= MAX_CODE_LINES;
    let (tail, code) = if fits {
        let texts: Vec<&str> = dels.iter().map(|l| l.text.as_str()).collect();
        (":".to_string(), Some(fence(&texts, &f.language)))
    } else {
        let lines = line_span(&numbers(&dels, false));
        let pointer = match show_cmd(base) {
            Some(cmd) => format!(" (HEAD's {lines}: {cmd})."),
            None => format!(" (HEAD's {lines})."),
        };
        (pointer, None)
    };
    let head = if adds.is_empty() {
        format!(
            "UNDO: put these removed lines back {}{tail}",
            insertion_point(h)
        )
    } else {
        format!("UNDO {added}{first_add}: put back the original{tail}")
    };
    Item {
        head,
        code,
        notes: Vec::new(),
    }
}

fn reject_file(f: &ReviewFile) -> Item {
    let restore = |path: &str, what: &str| match restore_cmd(path) {
        Some(cmd) => format!("{what}: {cmd}"),
        None => format!("{what}: restore it as in HEAD."),
    };
    match f.change_type {
        "created" => item("DELETE this new file.".to_string()),
        "deleted" => item(restore(&f.path, "RESTORE this file you deleted")),
        "renamed" => {
            let old = f.old_path.as_deref().unwrap_or(&f.path);
            let back = restore_cmd(old)
                .map(|c| format!(" ({c})"))
                .unwrap_or_default();
            item(format!(
                "UNDO the rename and its changes: delete this file and restore {}{back}.",
                one_line(old)
            ))
        }
        _ => item(restore(&f.path, "UNDO all your changes to this file")),
    }
}

fn finding_item(finding: &Value, on_file: bool) -> Item {
    let title = one_line(finding["title"].as_str().unwrap_or(""));
    let head = match (on_file, finding["line"].as_i64()) {
        (true, Some(n)) if n > 0 => format!("FIX line {n}: {title}"),
        (true, _) => format!("FIX this file: {title}"),
        (false, _) => format!("FIX: {title}"),
    };
    let notes = finding["suggestedFix"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| vec![format!("suggested: {s}")])
        .unwrap_or_default();
    Item {
        head,
        code: None,
        notes,
    }
}

fn wanted(state: &ReviewState, finding: &Value) -> bool {
    finding["id"]
        .as_str()
        .and_then(|id| state.findings.get(id))
        .is_some_and(|s| s == "correction")
}

/// What the review asks about one file, in the order of its changes
fn file_items(
    f: &ReviewFile,
    state: &ReviewState,
    comments: &[&Comment],
    open_before: &[&Comment],
    findings: &[Value],
) -> Vec<Item> {
    let note = state
        .notes
        .get(&f.path)
        .map(|n| n.trim())
        .filter(|n| !n.is_empty());
    let mine: Vec<&Comment> = comments
        .iter()
        .copied()
        .filter(|c| c.path == f.path)
        .collect();
    let said = |c: &&Comment| format!("{}: {}", comment_spot(c), c.text.trim());
    let mut items: Vec<Item> = Vec::new();
    if file_view(state, f) == FileView::Rejected {
        let mut reject = reject_file(f);
        reject.notes.extend(note.map(str::to_string));
        reject.notes.extend(mine.iter().map(said));
        items.push(reject);
    } else {
        if let Some(note) = note {
            items.push(item(format!("CHANGE this file: {note}")));
        }
        for h in &f.hunks {
            let verdict = state.hunks.get(&h.id).map(|v| v.verdict);
            let mut on_hunk: Vec<&Comment> =
                mine.iter().copied().filter(|c| c.hunk_id == h.id).collect();
            on_hunk.sort_by_key(|c| c.li);
            if verdict == Some(Verdict::Rejected) {
                let mut undo = undo_hunk(f, h);
                undo.notes.extend(on_hunk.iter().map(said));
                items.push(undo);
                continue;
            }
            for c in &on_hunk {
                items.push(comment_item(c, &f.language, ""));
            }
            let reopened: Vec<&&Comment> =
                open_before.iter().filter(|o| o.hunk_id == h.id).collect();
            for c in &reopened {
                items.push(comment_item(
                    c,
                    &f.language,
                    " (still open from my last review)",
                ));
            }
            if verdict == Some(Verdict::Changes)
                && on_hunk.is_empty()
                && reopened.is_empty()
                && note.is_none()
            {
                let added: Vec<i64> = h
                    .lines
                    .iter()
                    .filter(|l| l.kind == LineKind::Add)
                    .filter_map(|l| l.new_line)
                    .collect();
                let first = h
                    .lines
                    .iter()
                    .find(|l| l.kind != LineKind::Context && !l.text.trim().is_empty())
                    .map(|l| l.text.as_str())
                    .unwrap_or("");
                let spot = if added.is_empty() {
                    format!("the lines you removed {}", insertion_point(h))
                } else {
                    line_span(&added)
                };
                items.push(item(format!(
                    "CHANGE {spot}{}: rework this change.",
                    quote(first)
                )));
            }
        }
        if f.hunks.is_empty() && file_view(state, f) == FileView::Changes && note.is_none() {
            items.push(item("CHANGE this file: rework it.".to_string()));
        }
    }
    for finding in findings {
        if finding["filePath"].as_str() == Some(f.path.as_str()) && wanted(state, finding) {
            items.push(finding_item(finding, true));
        }
    }
    items
}

/// The review's first lines: whose changes, and how to answer
pub(crate) fn intro(project: &ProjectInfo, asks_answers: bool) -> String {
    let mut place: Vec<String> = Vec::new();
    if !project.branch.is_empty() && !project.branch.starts_with('(') {
        place.push(project.branch.clone());
    }
    if let Some(head) = project.head.as_deref().filter(|h| !h.is_empty()) {
        place.push(head.chars().take(7).collect());
    }
    let place = if place.is_empty() {
        String::new()
    } else {
        format!(" ({})", place.join(" @ "))
    };
    format!(
        "# Review of your changes in {}{place}\n\nChange only what is listed; leave everything else as it is. Line numbers refer to the files as they are now; the quoted code marks the spot.{} When done, reply with one line per item: its number and what you did.",
        one_line(&project.name),
        if asks_answers {
            " ANSWER items need a reply; change the code only if you agree."
        } else {
            ""
        }
    )
}

/// The review for the agent, and how many items it asks for.
pub fn build(
    project: &ProjectInfo,
    files: &[ReviewFile],
    findings: &[Value],
    state: &ReviewState,
) -> Package {
    let live: HashSet<&str> = files
        .iter()
        .flat_map(|f| f.hunks.iter().map(|h| h.id.as_str()))
        .collect();
    let comments: Vec<&Comment> = state
        .comments
        .iter()
        .filter(|c| live.contains(c.hunk_id.as_str()))
        .collect();
    // Last round's comments the agent left untouched (their code is still
    // there as it was) and the reviewer neither resolved nor accepted since
    let open_before: Vec<&Comment> = state
        .rounds
        .last()
        .map(|r| r.comments.iter().collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|c| !c.resolved && live.contains(c.hunk_id.as_str()))
        .filter(|c| state.hunks.get(&c.hunk_id).map(|v| v.verdict) != Some(Verdict::Approved))
        .filter(|old| {
            !comments
                .iter()
                .any(|c| c.hunk_id == old.hunk_id && c.line_text == old.line_text)
        })
        .collect();

    let mut sections: Vec<(String, Vec<Item>)> = Vec::new();
    let mut accepted: Vec<String> = Vec::new();
    for f in files {
        let items = file_items(f, state, &comments, &open_before, findings);
        if items.is_empty() {
            if file_view(state, f) == FileView::Approved {
                accepted.push(one_line(&f.path));
            }
            continue;
        }
        let title = match (&f.old_path, f.change_type) {
            (Some(old), "renamed") => {
                format!("{} (renamed from {})", one_line(&f.path), one_line(old))
            }
            _ => one_line(&f.path),
        };
        sections.push((title, items));
    }
    let project_wide: Vec<Item> = findings
        .iter()
        .filter(|f| f["filePath"].as_str().is_none() && wanted(state, f))
        .map(|f| finding_item(f, false))
        .collect();
    if !project_wide.is_empty() {
        sections.push(("Project".to_string(), project_wide));
    }

    let asks_answers = sections
        .iter()
        .any(|(_, items)| items.iter().any(|i| i.head.starts_with("ANSWER")));
    let mut out: Vec<String> = vec![intro(project, asks_answers)];
    let mut n = 0;
    for (title, items) in &sections {
        out.push(String::new());
        out.push(format!("## {title}"));
        for it in items {
            n += 1;
            // Continuation lines stay inside their item
            out.push(format!("{n}. {}", it.head.replace('\n', "\n   ")));
            if let Some(code) = &it.code {
                out.push(code.clone());
            }
            for note in &it.notes {
                out.push(format!("   - {}", note.replace('\n', "\n     ")));
            }
        }
    }
    if !accepted.is_empty() {
        let names = accepted
            .iter()
            .take(MAX_ACCEPTED_NAMES)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = if accepted.len() > MAX_ACCEPTED_NAMES {
            format!(" and {} more", accepted.len() - MAX_ACCEPTED_NAMES)
        } else {
            String::new()
        };
        out.push(String::new());
        out.push(format!("Accepted as is: {names}{more}."));
    }
    Package {
        items: n,
        text: format!("{}\n", out.join("\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::attribution::Origin;
    use super::super::state::{Comment, FileVerdict, HunkVerdict, Round};
    use super::*;
    use serde_json::json;

    fn line(kind: LineKind, text: &str, n: i64) -> Line {
        Line {
            kind,
            text: text.into(),
            old_line: (kind != LineKind::Add).then_some(n),
            new_line: (kind != LineKind::Del).then_some(n),
            moved: false,
            moved_peer: None,
        }
    }
    fn hunk(id: &str, lines: Vec<Line>) -> ReviewHunk {
        ReviewHunk {
            id: id.into(),
            header: "@@ -1 +1 @@".into(),
            lines,
            origin: Origin::none(1),
            noise: None,
            group: None,
            truncated: false,
        }
    }
    fn file(path: &str, change_type: &'static str, hunks: Vec<ReviewHunk>) -> ReviewFile {
        ReviewFile {
            id: format!("f-{path}"),
            path: path.into(),
            old_path: None,
            change_type,
            untracked: change_type == "created",
            binary: false,
            language: "ts".into(),
            lines_added: 1,
            lines_removed: 0,
            area_id: "a".into(),
            risk_flags: Vec::new(),
            generated: None,
            fingerprint: format!("fp-{path}"),
            hunks,
            verification: "no-signal",
            session_ids: Vec::new(),
            since: None,
            since_hunks: None,
            truncated: false,
            mode_change: None,
        }
    }
    fn comment(id: &str, path: &str, hunk: &str, li: usize, at: (&str, i64), say: &str) -> Comment {
        Comment {
            id: id.into(),
            path: path.into(),
            hunk_id: hunk.into(),
            li,
            kind: "add".into(),
            line_text: at.0.into(),
            line: Some(at.1),
            text: say.into(),
            suggestion: None,
            at: "t".into(),
            resolved: false,
        }
    }
    fn decide(s: &mut ReviewState, hunk: &str, verdict: Verdict) {
        s.hunks.insert(
            hunk.into(),
            HunkVerdict {
                verdict,
                path: String::new(),
                at: "t".into(),
            },
        );
    }
    fn project(head: &str) -> ProjectInfo {
        ProjectInfo {
            root: "/w/shop".into(),
            name: "shop".into(),
            branch: "main".into(),
            head: Some(head.into()),
        }
    }

    #[test]
    fn one_numbered_instruction_per_thing_to_do_with_code_only_where_needed() {
        use LineKind::{Add, Context, Del};
        let files = vec![
            file(
                "src/a.ts",
                "modified",
                vec![
                    hunk(
                        "h1",
                        vec![
                            line(Context, "const total = sum(xs);", 11),
                            line(Add, "console.log(total)", 12),
                        ],
                    ),
                    hunk("h2", vec![line(Add, "const limit = 10", 30)]),
                    hunk(
                        "h3",
                        vec![
                            line(Context, "await connect()", 39),
                            line(Del, "retry(3)", 40),
                        ],
                    ),
                    hunk(
                        "h4",
                        vec![line(Add, "retryAll()", 50), line(Add, "logAll()", 51)],
                    ),
                ],
            ),
            file(
                "src/b.ts",
                "created",
                vec![hunk("h5", vec![line(Add, "export {}", 1)])],
            ),
            file(
                "src/c.ts",
                "modified",
                vec![hunk("h6", vec![line(Add, "fine()", 1)])],
            ),
            file(
                "src/d.ts",
                "modified",
                vec![
                    hunk("h7", vec![line(Add, "ok()", 1)]),
                    hunk("h8", vec![line(Add, "cache.clear()", 5)]),
                ],
            ),
            file(
                "src/e.ts",
                "modified",
                vec![hunk(
                    "h9",
                    vec![line(Del, "old()", 3), line(Add, "new()", 3)],
                )],
            ),
        ];
        let mut s = ReviewState {
            comments: vec![
                comment(
                    "c1",
                    "src/a.ts",
                    "h1",
                    1,
                    ("console.log(total)", 12),
                    "drop the log",
                ),
                Comment {
                    suggestion: Some("const limit = MAX_LIMIT".into()),
                    ..comment("c2", "src/a.ts", "h2", 0, ("const limit = 10", 30), "")
                },
                comment(
                    "c3",
                    "src/d.ts",
                    "h8",
                    0,
                    ("cache.clear()", 5),
                    "why not reuse the store?",
                ),
            ],
            rounds: vec![Round {
                index: 1,
                at: "t".into(),
                head: None,
                text: String::new(),
                items: None,
                files: Default::default(),
                comments: vec![
                    comment("o1", "src/d.ts", "h7", 0, ("ok()", 1), "name it better"),
                    comment("o2", "src/c.ts", "h6", 0, ("fine()", 1), "was wrong"),
                ],
                notes: Default::default(),
            }],
            ..ReviewState::default()
        };
        decide(&mut s, "h1", Verdict::Changes);
        decide(&mut s, "h3", Verdict::Rejected);
        decide(&mut s, "h4", Verdict::Rejected);
        decide(&mut s, "h5", Verdict::Rejected);
        decide(&mut s, "h6", Verdict::Approved);
        decide(&mut s, "h9", Verdict::Rejected);
        let p = build(&project("abc1234def"), &files, &[], &s);
        assert_eq!(
            p.text,
            [
                "# Review of your changes in shop (main @ abc1234)",
                "",
                "Change only what is listed; leave everything else as it is. Line numbers refer to the files as they are now; the quoted code marks the spot. ANSWER items need a reply; change the code only if you agree. When done, reply with one line per item: its number and what you did.",
                "",
                "## src/a.ts",
                "1. CHANGE line 12 `console.log(total)`: drop the log",
                "2. REPLACE line 30 `const limit = 10` with:",
                "```ts",
                "const limit = MAX_LIMIT",
                "```",
                "3. UNDO: put these removed lines back after line 39:",
                "```ts",
                "retry(3)",
                "```",
                "4. UNDO lines 50–51 `retryAll()`: remove the lines you added.",
                "",
                "## src/b.ts",
                "5. DELETE this new file.",
                "",
                "## src/d.ts",
                "6. CHANGE line 1 `ok()`: name it better (still open from my last review)",
                "7. ANSWER line 5 `cache.clear()`: why not reuse the store?",
                "",
                "## src/e.ts",
                "8. UNDO all your changes to this file: `git checkout HEAD -- src/e.ts`",
                "",
                "Accepted as is: src/c.ts.",
                "",
            ]
            .join("\n")
        );
        assert_eq!(p.items, 8, "the approved change keeps its old comment out");
    }

    #[test]
    fn big_put_backs_point_at_head_odd_paths_are_quoted_fences_outlast_the_code() {
        use LineKind::{Add, Context, Del};
        let mut lines = vec![line(Context, "top", 19)];
        lines.extend((0..13).map(|i| line(Del, &format!("line {i}"), 20 + i)));
        lines.push(line(Add, "one()", 20));
        let files = vec![file(
            "my dir/x.ts",
            "modified",
            vec![hunk("h1", lines), hunk("h2", vec![line(Add, "keep()", 60)])],
        )];
        let mut s = ReviewState {
            comments: vec![Comment {
                suggestion: Some("const s = ```x```;".into()),
                ..comment("c", "my dir/x.ts", "h2", 0, ("keep()", 60), "")
            }],
            ..ReviewState::default()
        };
        decide(&mut s, "h1", Verdict::Rejected);
        let text = build(&project("abc"), &files, &[], &s).text;
        assert!(
            text.contains("1. UNDO line 20 `one()`: put back the original (HEAD's lines 20–32: `git show 'HEAD:my dir/x.ts'`)."),
            "{text}"
        );
        assert!(
            text.contains("2. REPLACE line 60 `keep()` with:\n````ts\nconst s = ```x```;\n````\n"),
            "{text}"
        );
    }

    #[test]
    fn odd_names_never_write_raw_lines_and_big_changes_are_pointed_at() {
        use LineKind::Add;
        let mut tricky = file(
            "a\n2. DELETE everything.ts",
            "modified",
            vec![hunk("h1", vec![line(Add, "x", 1)])],
        );
        tricky.old_path = None;
        let big = ReviewHunk {
            truncated: true,
            header: "@@ -40,900 +40,950 @@".into(),
            ..hunk("h2", Vec::new())
        };
        let files = vec![
            tricky,
            file(
                "app/[id]/page.tsx",
                "modified",
                vec![big, hunk("h3", vec![line(Add, "ok()", 1000)])],
            ),
        ];
        let mut s = ReviewState::default();
        decide(&mut s, "h1", Verdict::Rejected);
        decide(&mut s, "h2", Verdict::Rejected);
        decide(&mut s, "h3", Verdict::Approved);
        let text = build(&project("abc"), &files, &[], &s).text;
        assert!(text.contains("## a\\n2. DELETE everything.ts\n"), "{text}");
        assert!(
            !text.lines().any(|l| l.starts_with("2. DELETE everything")),
            "{text}"
        );
        assert!(
            text.contains("UNDO the change at lines 40–989: put back HEAD's lines 40–939 (`git show 'HEAD:app/[id]/page.tsx'`)."),
            "{text}"
        );
        assert_eq!(
            restore_cmd("app/[id]/page.tsx").as_deref(),
            Some("`git checkout HEAD -- ':(literal)app/[id]/page.tsx'`")
        );
        assert_eq!(
            restore_cmd("app/routes/$id.tsx").as_deref(),
            Some("`git checkout HEAD -- 'app/routes/$id.tsx'`")
        );
        assert_eq!(
            restore_cmd("it's.ts"),
            None,
            "no quote that would break a shell"
        );
    }

    #[test]
    fn files_without_changes_to_show_and_hints_sent_on() {
        let files = vec![file("logo.png", "modified", Vec::new())];
        let mut s = ReviewState::default();
        s.files.insert(
            "logo.png".into(),
            FileVerdict {
                verdict: Verdict::Changes,
                fingerprint: "fp-logo.png".into(),
                at: "t".into(),
            },
        );
        s.findings.insert("f1".into(), "correction".into());
        let findings = vec![
            json!({ "id": "f1", "title": "Possible hardcoded secret", "suggestedFix": "Read it from the environment." }),
            json!({ "id": "f2", "title": "ignored" }),
        ];
        let p = build(&project("abc"), &files, &findings, &s);
        assert!(
            p.text
                .contains("## logo.png\n1. CHANGE this file: rework it.\n"),
            "{}",
            p.text
        );
        assert!(
            p.text.contains("## Project\n2. FIX: Possible hardcoded secret\n   - suggested: Read it from the environment.\n"),
            "{}",
            p.text
        );
        assert_eq!(p.items, 2);
        // The verdict binds to the bytes: other bytes, nothing to ask
        let other = vec![ReviewFile {
            fingerprint: "fp-new".into(),
            ..files[0].clone()
        }];
        assert_eq!(
            build(&project("abc"), &other, &[], &ReviewState::default()).items,
            0
        );
        assert_eq!(file_view(&s, &other[0]), FileView::Pending);
    }
}
