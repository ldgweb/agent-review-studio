// Git integration — the review's teeth. Two jobs:
//
// 1. ENRICH (read): replace transcript-reconstructed hunks with the real
//    working-tree diff (`git diff HEAD --relative`), flag drift for changes
//    the tree no longer shows ('committed' after the session or 'missing').
//    Operates on the served session JSON only — never before session_hash
//    and never persisted, so fixtures stay stable. When hunks are replaced,
//    the per-file linesAdded/linesRemoved are recounted from the replacement
//    so the numbers describe the diff actually shown (session-level stats
//    stay transcript-based: they measure the agent's work, not the tree's
//    current state). Enrichment runs per served session (the detail route)
//    against the live repository; the review revision (transcript + HEAD +
//    file bytes) keeps the shown diff honest.
// 2. ACT (write): commit approved files / revert rejected ones (with a
//    backup patch under ~/.agent-review-studio/patches).
//
// Every git spawn goes through `git_command`: no console window on Windows,
// no terminal credential prompt (a GUI app has no terminal to answer it).

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;

const MAX_DIFF_BYTES: usize = 5_000_000;
const MAX_UNTRACKED_LINES: usize = 2000;

/// Windows process-creation flag: run console programs without a window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Keep a child process from flashing a console window (Windows only).
pub(crate) fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// A `git` command that never opens a window and never blocks on a
/// terminal prompt (credential helpers such as GCM are left alone).
pub(crate) fn git_command() -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    hide_window(&mut cmd);
    cmd
}

struct GitResult {
    ok: bool,
    out: String,
    err: String,
}

fn git(root: &str, args: &[&str]) -> GitResult {
    git_with_index(root, args, None)
}

fn git_with_index(root: &str, args: &[&str], index: Option<&Path>) -> GitResult {
    let mut cmd = git_command();
    cmd.arg("--literal-pathspecs")
        .args(["-c", "core.quotepath=false"])
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    match cmd.output() {
        Ok(output) => GitResult {
            ok: output.status.success(),
            out: String::from_utf8_lossy(&output.stdout).to_string(),
            err: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(e) => GitResult {
            ok: false,
            out: String::new(),
            err: e.to_string(),
        },
    }
}

fn norm_path(p: &str) -> String {
    p.replace('\\', "/")
}

/// True only for a plain relative path that stays inside its root: no drive
/// prefix, no leading separator, no `..` component. Session file paths come
/// from transcripts (and ars/v1 JSON is arbitrary third-party input), so they
/// are never trusted to join a filesystem root as-is.
fn is_safe_rel(p: &str) -> bool {
    use std::path::Component;
    if p.is_empty() || p.contains(':') || p.chars().any(char::is_control) {
        return false;
    }
    Path::new(p).components().all(|c| match c {
        Component::Normal(s) => !s
            .to_string_lossy()
            .trim_end_matches(['.', ' '])
            .eq_ignore_ascii_case(".git"),
        _ => false,
    })
}

/// Strip the a/ b/ prefix and surrounding quotes from a diff header path
fn strip_diff_path(raw: &str) -> String {
    let mut p = raw.trim();
    if p.starts_with('"') && p.ends_with('"') && p.len() >= 2 {
        p = &p[1..p.len() - 1];
    }
    if p.starts_with("a/") || p.starts_with("b/") {
        p = &p[2..];
    }
    p.to_string()
}

/// Parse `git diff --unified=N --no-color` output into per-path hunk JSON
/// (same DiffHunk shape the adapters produce).
pub fn parse_unified_diff(text: &str) -> HashMap<String, Value> {
    let mut files: HashMap<String, Value> = HashMap::new();
    let mut path: Option<String> = None;
    let mut old_path: Option<String> = None;
    let mut hunks: Vec<Value> = Vec::new();
    let mut lines: Vec<Value> = Vec::new();
    let mut header: Option<String> = None;
    let mut old_line: i64 = 0;
    let mut new_line: i64 = 0;

    fn close_hunk(hunks: &mut Vec<Value>, header: &mut Option<String>, lines: &mut Vec<Value>) {
        if let Some(h) = header.take() {
            hunks.push(json!({ "header": h, "lines": std::mem::take(lines) }));
        } else {
            lines.clear();
        }
    }

    let flush = |path: &mut Option<String>,
                 old_path: &mut Option<String>,
                 hunks: &mut Vec<Value>,
                 header: &mut Option<String>,
                 lines: &mut Vec<Value>,
                 files: &mut HashMap<String, Value>| {
        close_hunk(hunks, header, lines);
        if let Some(p) = path.take() {
            if !hunks.is_empty() {
                files.insert(norm_path(&p), Value::Array(std::mem::take(hunks)));
            }
        }
        hunks.clear();
        *old_path = None;
    };

    for line in text.split('\n') {
        if line.starts_with("diff --git ") {
            flush(
                &mut path,
                &mut old_path,
                &mut hunks,
                &mut header,
                &mut lines,
                &mut files,
            );
            continue;
        }
        if let Some(rest) = line.strip_prefix("--- ") {
            let p = rest.trim();
            old_path = if p == "/dev/null" {
                None
            } else {
                Some(strip_diff_path(p))
            };
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            let p = rest.trim();
            path = if p == "/dev/null" {
                old_path.clone()
            } else {
                Some(strip_diff_path(p))
            };
            continue;
        }
        if line.starts_with("@@ ") {
            if let Some((o, n)) = parse_hunk_header(line) {
                close_hunk(&mut hunks, &mut header, &mut lines);
                header = Some(line.to_string());
                old_line = o;
                new_line = n;
                continue;
            }
        }
        if header.is_none() {
            continue; // index/mode/rename/binary metadata between files
        }
        if line.starts_with('\\') {
            continue; // "\ No newline at end of file"
        }
        // CRLF working trees: the \r travels inside the diff content — strip
        // it from the display copy
        let body = line.strip_suffix('\r').unwrap_or(line);
        if let Some(text) = body.strip_prefix('+') {
            lines.push(json!({ "kind": "add", "text": text, "newLine": new_line }));
            new_line += 1;
        } else if let Some(text) = body.strip_prefix('-') {
            lines.push(json!({ "kind": "del", "text": text, "oldLine": old_line }));
            old_line += 1;
        } else if let Some(text) = body.strip_prefix(' ') {
            // Context lines always carry the space prefix in unified format —
            // a bare empty string is only the artifact of the trailing \n split
            lines.push(json!({ "kind": "context", "text": text, "oldLine": old_line, "newLine": new_line }));
            old_line += 1;
            new_line += 1;
        }
    }
    flush(
        &mut path,
        &mut old_path,
        &mut hunks,
        &mut header,
        &mut lines,
        &mut files,
    );
    files
}

fn parse_hunk_header(line: &str) -> Option<(i64, i64)> {
    // "@@ -12,7 +12,9 @@ context" → (12, 12)
    let rest = line.strip_prefix("@@ -")?;
    let (old_part, rest) = rest.split_once(" +")?;
    let (new_part, _) = rest.split_once(" @@")?;
    let old_start: i64 = old_part.split(',').next()?.parse().ok()?;
    let new_start: i64 = new_part.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

struct RepoState {
    diffs: HashMap<String, Value>,
    untracked: HashSet<String>,
}

fn repo_state(root: &str) -> Option<RepoState> {
    let check = git(root, &["rev-parse", "--is-inside-work-tree"]);
    if !check.ok || check.out.trim() != "true" {
        return None;
    }
    // --relative: paths come out relative to projectPath (the agent's cwd),
    // matching how the adapters record FileChange.path
    let diff = git(
        root,
        &[
            "diff",
            "HEAD",
            "--relative",
            "--unified=3",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
        ],
    );
    let diffs = if diff.ok && diff.out.len() <= MAX_DIFF_BYTES {
        parse_unified_diff(&diff.out)
    } else {
        HashMap::new()
    };
    let ls = git(root, &["ls-files", "-z", "--others", "--exclude-standard"]);
    let untracked: HashSet<String> = if ls.ok {
        ls.out
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(norm_path)
            .collect()
    } else {
        HashSet::new()
    };
    Some(RepoState { diffs, untracked })
}

fn untracked_hunks(root: &str, rel_path: &str) -> Option<(Value, usize)> {
    // Callers gate rel_path on `git ls-files` output; keep this hard stop in
    // case a future caller passes a transcript path directly.
    if !is_safe_rel(rel_path) {
        return None;
    }
    let raw = std::fs::read_to_string(checked_file(root, rel_path).ok()?).ok()?;
    let mut all: Vec<&str> = raw
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    // A trailing newline yields a phantom empty last line — git doesn't count it
    if all.last() == Some(&"") {
        all.pop();
    }
    let shown = &all[..all.len().min(MAX_UNTRACKED_LINES)];
    let truncated = all.len() > shown.len();
    let header = if truncated {
        format!(
            "@@ -0,0 +1,{} @@ (first {} of {} lines)",
            shown.len(),
            MAX_UNTRACKED_LINES,
            all.len()
        )
    } else {
        format!("@@ -0,0 +1,{} @@", shown.len())
    };
    let lines: Vec<Value> = shown
        .iter()
        .enumerate()
        .map(|(i, text)| json!({ "kind": "add", "text": text, "newLine": (i + 1) as i64 }))
        .collect();
    Some((json!([{ "header": header, "lines": lines }]), all.len()))
}

/// add/del totals of a hunk-set JSON — the displayed diff's real counts
fn count_hunk_lines(hunks: &Value) -> (i64, i64) {
    let (mut adds, mut dels) = (0i64, 0i64);
    if let Some(arr) = hunks.as_array() {
        for hunk in arr {
            if let Some(lines) = hunk.get("lines").and_then(Value::as_array) {
                for line in lines {
                    match line.get("kind").and_then(Value::as_str) {
                        Some("add") => adds += 1,
                        Some("del") => dels += 1,
                        _ => {}
                    }
                }
            }
        }
    }
    (adds, dels)
}

/// Commit log entries (epoch seconds, touched paths) for commits at/after
/// `since` — the drift-detection input. Each record starts with a NUL byte,
/// so no file name (a file called `2024`, say) can pass for a timestamp.
fn fetch_log(root: &str, since: i64) -> Vec<(i64, Vec<String>)> {
    let Some(since_at) = chrono::DateTime::from_timestamp(since, 0) else {
        return Vec::new();
    };
    let since_arg = format!("--since={}", since_at.format("%Y-%m-%d %H:%M:%S +0000"));
    let res = git(
        root,
        &[
            "log",
            "--name-only",
            "--relative",
            "--format=%x00%ct",
            &since_arg,
        ],
    );
    if !res.ok {
        return Vec::new();
    }
    res.out
        .split('\0')
        .skip(1)
        .filter_map(|record| {
            let mut lines = record.lines();
            let epoch = lines.next()?.trim().parse::<i64>().ok()?;
            let paths = lines
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(norm_path)
                .collect();
            Some((epoch, paths))
        })
        .collect()
}

/// Enrich ONE served session with working-tree diffs and drift flags.
pub fn enrich_session(session: &mut Value) {
    let Some(root) = session
        .get("projectPath")
        .and_then(Value::as_str)
        .map(String::from)
    else {
        return;
    };
    if root.is_empty() || !Path::new(&root).exists() {
        return;
    }
    // No parseable session time → no cutoff → no "committed" drift claims.
    let cutoff = session
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|d| d.timestamp());
    let Some(state) = repo_state(&root) else {
        return;
    };

    let mut committed: Option<HashSet<String>> = None;
    let mut tree_matches = 0usize;
    let mut committed_matches = 0usize;
    let mut transcript_only: Vec<usize> = Vec::new();

    if let Some(files) = session.get_mut("files").and_then(Value::as_array_mut) {
        for (i, file) in files.iter_mut().enumerate() {
            let key = norm_path(file.get("path").and_then(Value::as_str).unwrap_or(""));
            let change_type = file
                .get("changeType")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let Some(map) = file.as_object_mut() else {
                continue;
            };
            if let Some(wt) = state.diffs.get(&key) {
                let (adds, dels) = count_hunk_lines(wt);
                map.insert("hunks".into(), wt.clone());
                map.insert("diffSource".into(), json!("git"));
                map.insert("linesAdded".into(), json!(adds));
                map.insert("linesRemoved".into(), json!(dels));
                tree_matches += 1;
                continue;
            }
            if state.untracked.contains(&key) {
                if let Some((hunks, total_lines)) = untracked_hunks(&root, &key) {
                    map.insert("hunks".into(), hunks);
                    map.insert("diffSource".into(), json!("git"));
                    map.insert("changeType".into(), json!("created"));
                    map.insert("linesAdded".into(), json!(total_lines as i64));
                    map.insert("linesRemoved".into(), json!(0));
                    tree_matches += 1;
                    continue;
                }
            }
            map.insert("diffSource".into(), json!("transcript"));
            if change_type != "deleted" {
                let set = committed.get_or_insert_with(|| {
                    let mut out: HashSet<String> = HashSet::new();
                    if let Some(cutoff) = cutoff {
                        for (epoch, paths) in fetch_log(&root, cutoff) {
                            if epoch >= cutoff {
                                out.extend(paths);
                            }
                        }
                    }
                    out
                });
                if set.contains(&key) {
                    map.insert("drift".into(), json!("committed"));
                    committed_matches += 1;
                } else {
                    transcript_only.push(i);
                }
            }
        }

        // Only claim "missing" when path alignment is proven by at least
        // one match — otherwise a projectPath/pathspec mismatch would cry
        // wolf on every file of the session.
        if tree_matches + committed_matches > 0 {
            for i in transcript_only {
                if let Some(map) = files.get_mut(i).and_then(Value::as_object_mut) {
                    map.insert("drift".into(), json!("missing"));
                }
            }
        }
    }

    if let Some(map) = session.as_object_mut() {
        map.insert("gitRepo".into(), json!(true));
    }
}

/// Restrict client-supplied paths to the session's own files
fn session_paths(session: &Value, requested: &[String]) -> Vec<String> {
    let known: HashSet<String> = session
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|f| f.get("path").and_then(Value::as_str))
                .map(norm_path)
                .collect()
        })
        .unwrap_or_default();
    let mut seen = HashSet::new();
    requested
        .iter()
        .map(|p| norm_path(p))
        .filter(|p| is_safe_rel(p) && known.contains(p) && seen.insert(p.clone()))
        .collect()
}

fn project_root(session: &Value) -> Result<String, String> {
    let root = session
        .get("projectPath")
        .and_then(Value::as_str)
        .unwrap_or("");
    if root.is_empty() || !Path::new(root).exists() {
        return Err("The session has no project path on this machine.".into());
    }
    Ok(root.to_string())
}

/// Resolve only ordinary files inside the declared project. Reject symlinks,
/// Windows junctions and Git metadata, including in parent directories.
fn checked_file(root: &str, relative: &str) -> Result<std::path::PathBuf, String> {
    if !is_safe_rel(relative) {
        return Err(format!("Unsafe project path: {relative}"));
    }
    let root = Path::new(root).canonicalize().map_err(|e| e.to_string())?;
    let mut full = root.clone();
    for part in Path::new(relative).components() {
        full.push(part);
        match std::fs::symlink_metadata(&full) {
            Ok(meta) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if meta.file_type().is_symlink() || reparse {
                    return Err(format!(
                        "Linked paths cannot be changed through a review: {relative}"
                    ));
                }
                let resolved = full.canonicalize().map_err(|e| e.to_string())?;
                if !resolved.starts_with(&root) {
                    return Err("Path escapes the project.".into());
                }
                if resolved
                    .strip_prefix(&root)
                    .map_err(|e| e.to_string())?
                    .components()
                    .any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case(".git"))
                {
                    return Err("Git metadata cannot be accessed through a review.".into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    if full.is_dir() {
        return Err(format!("Expected a file, not a directory: {relative}"));
    }
    Ok(full)
}

/// A review binds transcript evidence, repository HEAD and the actual bytes
/// of its files. Counts and file names alone cannot identify reviewed code.
pub fn review_revision(session: &Value) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for key in [
        "sessionId",
        "projectPath",
        "originalPrompt",
        "files",
        "timeline",
        "findings",
    ] {
        let bytes = serde_json::to_vec(&session[key]).map_err(|e| e.to_string())?;
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    if let Ok(root) = project_root(session) {
        let head = git(&root, &["rev-parse", "HEAD"]);
        hash.update(head.out.as_bytes());
        if let Some(files) = session["files"].as_array() {
            for file in files {
                let rel = file["path"].as_str().ok_or("Missing file path")?;
                // Outside-project transcript entries remain reviewable but are
                // never eligible for filesystem actions.
                let Ok(full) = checked_file(&root, rel) else {
                    hash.update(b"unavailable");
                    continue;
                };
                match std::fs::File::open(full) {
                    Ok(mut file) => {
                        hash.update(b"present");
                        let mut file_hash = Sha256::new();
                        let mut block = [0u8; 65536];
                        loop {
                            let count = std::io::Read::read(&mut file, &mut block)
                                .map_err(|e| e.to_string())?;
                            if count == 0 {
                                break;
                            }
                            file_hash.update(&block[..count]);
                        }
                        hash.update(file_hash.finalize());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        hash.update(b"missing");
                    }
                    Err(e) => return Err(format!("Cannot verify {rel}: {e}")),
                }
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn require_revision(session: &Value, expected: Option<&str>) -> Result<(), String> {
    let expected = expected
        .filter(|s| !s.is_empty())
        .ok_or("Reload this session before reviewing it.")?;
    if review_revision(session)? != expected {
        return Err(
            "The code changed since this review. Reload the session and review the new changes."
                .into(),
        );
    }
    Ok(())
}

fn action_paths(session: &Value, root: &str, requested: &[String]) -> Result<Vec<String>, String> {
    let paths = session_paths(session, requested);
    if paths.is_empty() || paths.len() != requested.len() {
        return Err(
            "Every requested path must be a distinct file belonging to this session.".into(),
        );
    }
    for path in &paths {
        checked_file(root, path)?;
    }
    require_revision(session, session["reviewRevision"].as_str())?;
    Ok(paths)
}

static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Bound the payload for pathological files (minified bundles, dumps)
const MAX_FILE_LINES: usize = 50000;

/// Working-tree content of one session file — feeds "expand context" in the diff
pub fn read_file_lines(session: &Value, requested_path: &str) -> Value {
    let root = match project_root(session) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let paths = session_paths(session, &[requested_path.to_string()]);
    let Some(path) = paths.first() else {
        return json!({ "error": "The requested file does not belong to this session." });
    };
    let full = match checked_file(&root, path) {
        Ok(p) => p,
        Err(e) => return json!({"error": e}),
    };
    match std::fs::read_to_string(full) {
        Ok(raw) => {
            let cleaned = raw.replace('\r', "");
            let lines: Vec<String> = cleaned
                .split('\n')
                .take(MAX_FILE_LINES)
                .map(str::to_string)
                .collect();
            json!({ "lines": lines })
        }
        Err(e) => json!({ "error": format!("Could not read {}: {}", path, e) }),
    }
}

pub fn commit_files(session: &Value, requested: &[String], message: &str) -> Value {
    let _guard = match WRITE_LOCK.lock() {
        Ok(g) => g,
        Err(_) => return json!({"error": "Git operation unavailable."}),
    };
    let root = match project_root(session) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    if repo_state(&root).is_none() {
        return json!({ "error": "The project path is not a git work tree." });
    }
    let paths = match action_paths(session, &root, requested) {
        Ok(p) => p,
        Err(e) => return json!({"error": e}),
    };
    if message.trim().is_empty() {
        return json!({ "error": "Empty commit message." });
    }

    // Stage into a private index based on HEAD. Other staged work stays in
    // the user's index, and commit cannot re-read unreviewed working bytes.
    let index_dir = match TemporaryDir::new() {
        Ok(d) => d,
        Err(e) => return json!({"error": e}),
    };
    let index = index_dir.0.join("index");
    let init = git_with_index(&root, &["read-tree", "HEAD"], Some(&index));
    if !init.ok {
        return json!({"error": git_error_text(&init.err, &init.out)});
    }
    let mut add_args: Vec<&str> = vec!["add", "--"];
    add_args.extend(paths.iter().map(String::as_str));
    let add = git_with_index(&root, &add_args, Some(&index));
    if !add.ok {
        return json!({ "error": format!("git add failed: {}", git_error_text(&add.err, &add.out)) });
    }
    if let Err(e) = require_revision(session, session["reviewRevision"].as_str()) {
        return json!({"error": e});
    }
    let commit = git_with_index(&root, &["commit", "-m", message], Some(&index));
    if !commit.ok {
        return json!({ "error": format!("git commit failed: {}", git_error_text(&commit.err, &commit.out)) });
    }
    let sha = git(&root, &["rev-parse", "--short", "HEAD"]);
    let mut reset_args = vec!["reset", "-q", "HEAD", "--"];
    reset_args.extend(paths.iter().map(String::as_str));
    let reset = git(&root, &reset_args);
    json!({ "ok": true, "sha": if sha.ok { sha.out.trim().to_string() } else { "?".into() }, "committed": paths.len(),
        "warning": if reset.ok { Value::Null } else { json!("Commit succeeded, but updating the index failed. Check git status before continuing.") } })
}

pub fn revert_files(session: &Value, requested: &[String]) -> Value {
    revert_with_backup(
        session,
        requested,
        &super::config::config_dir().join("patches"),
    )
}

fn revert_with_backup(session: &Value, requested: &[String], patch_dir: &Path) -> Value {
    let _guard = match WRITE_LOCK.lock() {
        Ok(g) => g,
        Err(_) => return json!({"error": "Git operation unavailable."}),
    };
    let root = match project_root(session) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let Some(state) = repo_state(&root) else {
        return json!({ "error": "The project path is not a git work tree." });
    };
    let paths = match action_paths(session, &root, requested) {
        Ok(p) => p,
        Err(e) => return json!({"error": e}),
    };
    // Three kinds of path, each reverted differently:
    //   in HEAD          → `git checkout HEAD --` restores index + tree;
    //   staged as new    → in the index only: unstage, then delete;
    //   untracked        → delete.
    // Anything else (ignored, or already gone) fails closed before backup.
    let in_head = match listed_paths(
        &root,
        &["ls-tree", "-r", "-z", "--name-only", "HEAD"],
        &paths,
    ) {
        Ok(set) => set,
        Err(e) => return json!({"error": format!("Nothing reverted: {e}")}),
    };
    let in_index = match listed_paths(&root, &["ls-files", "-z"], &paths) {
        Ok(set) => set,
        Err(e) => return json!({"error": format!("Nothing reverted: {e}")}),
    };
    let mut tracked: Vec<&String> = Vec::new();
    let mut staged_new: Vec<&String> = Vec::new();
    let mut untracked: Vec<&String> = Vec::new();
    for p in &paths {
        if in_head.contains(p) {
            tracked.push(p);
        } else if in_index.contains(p) {
            staged_new.push(p);
        } else if state.untracked.contains(p) {
            untracked.push(p);
        } else {
            return json!({"error": format!(
                "Nothing reverted: {p} is not in HEAD, the index or the untracked files (ignored, or already gone)."
            )});
        }
    }
    // Whole-file copies of everything that will be deleted outright.
    let copies: Vec<&String> = staged_new.iter().chain(untracked.iter()).copied().collect();
    let patch_path = match backup_files(&root, &paths, &copies, patch_dir) {
        Ok(path) => norm_path(&path.to_string_lossy()),
        Err(e) => return json!({"error": format!("Nothing reverted: backup failed: {e}")}),
    };
    // From here on every error names the backup.
    let fail = |message: String| json!({ "error": format!("{message} Backup: {patch_path}"), "patch": patch_path });
    if let Err(e) = require_revision(session, session["reviewRevision"].as_str()) {
        return fail(format!("Nothing reverted: {e}"));
    }
    // Every backup has succeeded before the first destructive operation.
    // Tracked files first: if checkout fails, nothing has been deleted yet.
    if !tracked.is_empty() {
        let mut args: Vec<&str> = vec!["checkout", "HEAD", "--"];
        args.extend(tracked.iter().map(|s| s.as_str()));
        let checkout = git(&root, &args);
        if !checkout.ok {
            return fail(format!(
                "Nothing reverted: git checkout failed: {}.",
                git_error_text(&checkout.err, &checkout.out).trim_end()
            ));
        }
    }
    if !staged_new.is_empty() {
        let mut args: Vec<&str> = vec!["rm", "--cached", "-f", "-q", "--"];
        args.extend(staged_new.iter().map(|s| s.as_str()));
        let unstage = git(&root, &args);
        if !unstage.ok {
            return fail(format!(
                "Partially reverted: tracked files were restored, but unstaging new files failed: {}.",
                git_error_text(&unstage.err, &unstage.out).trim_end()
            ));
        }
    }
    for p in staged_new.iter().chain(untracked.iter()) {
        let removed = checked_file(&root, p).and_then(|full| match std::fs::remove_file(full) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        });
        if let Err(e) = removed {
            return fail(format!("Partially reverted: could not remove {p}: {e}."));
        }
    }
    json!({ "ok": true, "patch": patch_path, "reverted": paths.len() })
}

/// The subset of `paths` a NUL-separated git listing (`ls-tree -z`,
/// `ls-files -z`) reports, relative to the project root.
fn listed_paths(root: &str, args: &[&str], paths: &[String]) -> Result<HashSet<String>, String> {
    let mut full: Vec<&str> = args.to_vec();
    full.push("--");
    full.extend(paths.iter().map(String::as_str));
    let res = git(root, &full);
    if !res.ok {
        return Err(format!(
            "git {} failed: {}",
            args[0],
            git_error_text(&res.err, &res.out).trim_end()
        ));
    }
    Ok(res
        .out
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(norm_path)
        .collect())
}

fn backup_files(
    root: &str,
    paths: &[String],
    untracked: &[&String],
    base: &Path,
) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    );
    let dir = base.join(unique);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut args = vec![
        "diff",
        "--binary",
        "--no-ext-diff",
        "--no-textconv",
        "HEAD",
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    let diff = git(root, &args);
    if !diff.ok {
        return Err(diff.err);
    }
    let patch = dir.join("tracked.patch");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&patch)
        .map_err(|e| e.to_string())?;
    file.write_all(diff.out.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    // Preserve the staged version too: it may differ from both HEAD and disk.
    let mut index_args = vec![
        "diff",
        "--cached",
        "--binary",
        "--no-ext-diff",
        "--no-textconv",
        "HEAD",
        "--",
    ];
    index_args.extend(paths.iter().map(String::as_str));
    let staged = git(root, &index_args);
    if !staged.ok {
        return Err(staged.err);
    }
    let mut index_backup = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join("staged.patch"))
        .map_err(|e| e.to_string())?;
    index_backup
        .write_all(staged.out.as_bytes())
        .and_then(|_| index_backup.sync_all())
        .map_err(|e| e.to_string())?;
    for rel in untracked {
        let source = checked_file(root, rel)?;
        let target = dir.join("untracked").join(rel.as_str());
        std::fs::create_dir_all(target.parent().ok_or("Missing backup parent")?)
            .map_err(|e| e.to_string())?;
        std::fs::copy(source, &target).map_err(|e| e.to_string())?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(target)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(patch)
}

/// The first non-empty of stderr/stdout, capped for an error message.
fn git_error_text(err: &str, out: &str) -> String {
    let s = if err.trim().is_empty() { out } else { err };
    s.chars().take(400).collect()
}

struct TemporaryDir(std::path::PathBuf);
impl TemporaryDir {
    fn new() -> Result<Self, String> {
        // The counter keeps names unique within the process even where the
        // clock is coarse (macOS reports microseconds) and threads race.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("ars-git-{}-{stamp}-{seq}", std::process::id()));
        std::fs::create_dir(&path).map_err(|e| e.to_string())?;
        Ok(Self(path))
    }
}
impl Drop for TemporaryDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Post-review git state & actions — repo_status /
// push_branch / create_branch / pr_url.
// ---------------------------------------------------------------------------

fn empty_status() -> Value {
    json!({
        "gitRepo": false,
        "branch": "",
        "upstream": Value::Null,
        "ahead": 0,
        "behind": 0,
        "hasRemote": false,
        "remoteUrl": Value::Null,
        "dirtyPaths": [],
    })
}

pub fn repo_status(session: &Value) -> Value {
    let root = match project_root(session) {
        Ok(r) => r,
        Err(_) => return empty_status(),
    };
    let Some(state) = repo_state(&root) else {
        return empty_status();
    };

    let branch = git(&root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .out
        .trim()
        .to_string();
    let up_res = git(
        &root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    );
    let upstream: Option<String> = if up_res.ok {
        Some(up_res.out.trim().to_string())
    } else {
        None
    };
    let (mut ahead, mut behind) = (0i64, 0i64);
    if upstream.is_some() {
        let counts = git(
            &root,
            &["rev-list", "--left-right", "--count", "@{u}...HEAD"],
        );
        if counts.ok {
            let nums: Vec<i64> = counts
                .out
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            behind = nums.first().copied().unwrap_or(0);
            ahead = nums.get(1).copied().unwrap_or(0);
        }
    }
    let remote = git(&root, &["remote", "get-url", "origin"]);
    let has_remote = remote.ok && !remote.out.trim().is_empty();
    let remote_url = if has_remote {
        Some(remote.out.trim().to_string())
    } else {
        None
    };

    let own: HashSet<String> = session
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|f| f.get("path").and_then(Value::as_str))
                .map(norm_path)
                .collect()
        })
        .unwrap_or_default();
    let mut dirty: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for k in state.diffs.keys().chain(state.untracked.iter()) {
        if own.contains(k) && seen.insert(k.clone()) {
            dirty.push(k.clone());
        }
    }
    dirty.sort();

    json!({
        "gitRepo": true,
        "branch": branch,
        "upstream": upstream,
        "ahead": ahead,
        "behind": behind,
        "hasRemote": has_remote,
        "remoteUrl": remote_url,
        "dirtyPaths": dirty,
    })
}

pub fn push_branch(session: &Value, set_upstream: bool) -> Value {
    let root = match project_root(session) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let st = repo_status(session);
    if !st.get("gitRepo").and_then(Value::as_bool).unwrap_or(false) {
        return json!({ "error": "The project path is not a git work tree." });
    }
    if !st
        .get("hasRemote")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return json!({ "error": "No \"origin\" remote is configured for this repository." });
    }
    let branch = st
        .get("branch")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if branch.is_empty() || branch == "HEAD" {
        return json!({ "error": "Detached HEAD — check out a branch before pushing." });
    }
    let has_upstream = st.get("upstream").map(|v| !v.is_null()).unwrap_or(false);
    let needs_upstream = set_upstream || !has_upstream;
    let res = if needs_upstream {
        git(&root, &["push", "-u", "origin", &branch])
    } else {
        git(&root, &["push"])
    };
    if !res.ok {
        return json!({ "error": format!("git push failed: {}", git_error_text(&res.err, &res.out)) });
    }
    json!({ "ok": true, "pushed": branch, "setUpstream": needs_upstream })
}

pub fn create_branch(session: &Value, name: &str) -> Value {
    let root = match project_root(session) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    if repo_state(&root).is_none() {
        return json!({ "error": "The project path is not a git work tree." });
    }
    let clean = name.trim();
    if let Err(e) = validate_branch_name(&root, clean) {
        return json!({ "error": e });
    }
    let res = git(&root, &["checkout", "-b", clean]);
    if !res.ok {
        return json!({ "error": format!("git checkout -b failed: {}", git_error_text(&res.err, &res.out)) });
    }
    json!({ "ok": true, "branch": clean })
}

/// A new branch name must be a plain name (letters, digits, `. _ / -`, never
/// a leading `-` that git would read as an option) AND a valid ref per
/// `git check-ref-format --branch`, which must echo it back unchanged (so
/// `@{-1}`-style shorthands that expand to another branch are refused).
/// check-ref-format takes exactly one argument after `--branch`, so it is
/// never parsed as an option there.
fn validate_branch_name(root: &str, name: &str) -> Result<(), String> {
    let plain = !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '/' || c == '-');
    if !plain {
        return Err(
            "Invalid branch name (use letters, digits, . _ / - and do not start with -).".into(),
        );
    }
    let check = git(root, &["check-ref-format", "--branch", name]);
    if !check.ok || check.out.trim() != name {
        return Err(format!(
            "Invalid branch name: \"{name}\" is not a valid git branch name."
        ));
    }
    Ok(())
}

/// One GitHub owner/repo segment: `[A-Za-z0-9._-]+`, never `.` or `..`.
fn slug_part(s: &str) -> Option<String> {
    let ok = !s.is_empty()
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
    ok.then(|| s.to_string())
}

/// `owner/repo` (e.g. `--repo` or GITHUB_REPOSITORY), validated; a trailing
/// `.git` or `/` is tolerated.
pub fn parse_owner_repo(s: &str) -> Option<(String, String)> {
    let path = s.trim().trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    Some((slug_part(owner)?, slug_part(repo)?))
}

/// owner/repo from a GitHub remote URL, or None when it is not one. The host
/// must be exactly github.com, in one of the forms
/// `https://[user@]github.com/o/r`, `ssh://git@github.com/o/r` or
/// `git@github.com:o/r` (optional `.git`, optional trailing `/`).
pub fn github_slug(remote_url: &str) -> Option<(String, String)> {
    let t = remote_url.trim();
    let path = if let Some(rest) = t.strip_prefix("https://") {
        let (authority, path) = rest.split_once('/')?;
        // userinfo (`x-access-token:…@`) may precede the host
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path
    } else if let Some(rest) = t.strip_prefix("ssh://") {
        let (authority, path) = rest.split_once('/')?;
        if !authority.eq_ignore_ascii_case("git@github.com") {
            return None;
        }
        path
    } else {
        let (authority, path) = t.split_once(':')?;
        if !authority.eq_ignore_ascii_case("git@github.com") {
            return None;
        }
        path
    };
    parse_owner_repo(path)
}

/// A pre-filled GitHub "compare" URL for the current branch (open in a
/// browser). We deliberately do not shell out to `gh`.
pub fn pr_url(session: &Value, title: Option<&str>, body: Option<&str>) -> Value {
    let st = repo_status(session);
    if !st.get("gitRepo").and_then(Value::as_bool).unwrap_or(false) {
        return json!({ "error": "The project path is not a git work tree." });
    }
    let remote_url = match st.get("remoteUrl").and_then(Value::as_str) {
        Some(u) => u.to_string(),
        None => {
            return json!({ "error": "No \"origin\" remote is configured for this repository." })
        }
    };
    let branch = st
        .get("branch")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if branch.is_empty() || branch == "HEAD" {
        return json!({ "error": "Detached HEAD — check out a branch first." });
    }
    let Some((owner, repo)) = github_slug(&remote_url) else {
        return json!({ "error": "The \"origin\" remote is not a recognized GitHub URL." });
    };
    let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
    let mut query = String::from("expand=1");
    if let Some(t) = title {
        if !t.is_empty() {
            query.push_str(&format!("&title={}", enc(t)));
        }
    }
    if let Some(b) = body {
        if !b.is_empty() {
            query.push_str(&format!("&body={}", enc(b)));
        }
    }
    let url = format!(
        "https://github.com/{}/{}/compare/{}?{}",
        owner,
        repo,
        enc(&branch),
        query
    );
    json!({ "ok": true, "url": url })
}

/// Recent commits on the current branch, with ref decorations, for the git graph.
pub fn git_log(session: &Value, limit: usize) -> Value {
    let root = match project_root(session) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    if repo_state(&root).is_none() {
        return json!({ "error": "The project path is not a git work tree." });
    }
    let us = '\u{1f}';
    let fmt = format!("--pretty=format:%H{us}%h{us}%s{us}%cr{us}%D");
    let n = limit.min(50).to_string();
    let res = git(&root, &["log", &fmt, "-n", &n]);
    if !res.ok {
        return json!({ "error": format!("git log failed: {}", git_error_text(&res.err, &res.out)) });
    }
    let mut commits: Vec<Value> = Vec::new();
    for line in res.out.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.split(us);
        let sha = parts.next().unwrap_or("").to_string();
        let short_sha = parts.next().unwrap_or("").to_string();
        let subject = parts.next().unwrap_or("").to_string();
        let rel_time = parts.next().unwrap_or("").to_string();
        let refs_raw = parts.next().unwrap_or("");
        let refs: Vec<String> = refs_raw
            .split(',')
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .collect();
        commits.push(json!({
            "sha": sha,
            "shortSha": short_sha,
            "subject": subject,
            "relTime": rel_time,
            "refs": refs,
        }));
    }
    json!({ "commits": commits })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "diff --git a/src/alpha.ts b/src/alpha.ts\nindex 111..222 100644\n--- a/src/alpha.ts\n+++ b/src/alpha.ts\n@@ -1,4 +1,5 @@\n context one\n-removed line\n+added line\n+another added\n context two\n@@ -10,2 +11,2 @@ fn ctx\n context three\n-old\n+new\ndiff --git a/docs/beta.md b/docs/beta.md\n--- a/docs/beta.md\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-gone one\n-gone two\n";

    #[test]
    fn parses_unified_diff_sample() {
        let files = parse_unified_diff(SAMPLE);
        assert_eq!(files.len(), 2, "two files expected");

        let alpha = files.get("src/alpha.ts").expect("alpha parsed");
        let hunks = alpha.as_array().expect("hunks array");
        assert_eq!(hunks.len(), 2);
        let first = &hunks[0];
        assert_eq!(
            first.get("header").and_then(Value::as_str),
            Some("@@ -1,4 +1,5 @@")
        );
        let lines = first.get("lines").and_then(Value::as_array).expect("lines");
        assert_eq!(lines.len(), 5);
        assert_eq!(
            lines[0].get("kind").and_then(Value::as_str),
            Some("context")
        );
        assert_eq!(lines[0].get("oldLine").and_then(Value::as_i64), Some(1));
        assert_eq!(lines[0].get("newLine").and_then(Value::as_i64), Some(1));
        assert_eq!(lines[1].get("kind").and_then(Value::as_str), Some("del"));
        assert_eq!(lines[1].get("oldLine").and_then(Value::as_i64), Some(2));
        assert_eq!(lines[1].get("newLine"), None);
        assert_eq!(lines[2].get("kind").and_then(Value::as_str), Some("add"));
        assert_eq!(lines[2].get("newLine").and_then(Value::as_i64), Some(2));
        assert_eq!(lines[3].get("newLine").and_then(Value::as_i64), Some(3));
        assert_eq!(
            lines[4].get("kind").and_then(Value::as_str),
            Some("context")
        );
        assert_eq!(lines[4].get("oldLine").and_then(Value::as_i64), Some(3));
        assert_eq!(lines[4].get("newLine").and_then(Value::as_i64), Some(4));
        let second = &hunks[1];
        let lines2 = second
            .get("lines")
            .and_then(Value::as_array)
            .expect("lines2");
        assert_eq!(lines2[1].get("oldLine").and_then(Value::as_i64), Some(11));
        assert_eq!(lines2[2].get("newLine").and_then(Value::as_i64), Some(12));

        // Deleted file: path taken from the "---" side
        let beta = files.get("docs/beta.md").expect("beta parsed");
        let beta_lines = beta.as_array().unwrap()[0]
            .get("lines")
            .and_then(Value::as_array)
            .expect("beta lines");
        assert_eq!(beta_lines.len(), 2);
        assert!(beta_lines
            .iter()
            .all(|l| l.get("kind").and_then(Value::as_str) == Some("del")));
    }

    #[test]
    fn rejects_escaping_session_paths() {
        for bad in [
            "../secret.txt",
            "a/../../b",
            "/etc/passwd",
            "\\\\share\\x",
            "",
            ".git./config",
            "a.txt:stream",
            "a\nb.txt",
        ] {
            assert!(!is_safe_rel(&norm_path(bad)), "must reject {bad:?}");
        }
        // Drive-prefixed paths are absolute only under Windows path semantics
        #[cfg(windows)]
        assert!(
            !is_safe_rel(&norm_path("C:\\Windows\\win.ini")),
            "must reject drive paths"
        );
        for good in ["src/main.rs", "a/b/c.txt", "weird..name.txt", "..hidden"] {
            assert!(is_safe_rel(&norm_path(good)), "must accept {good:?}");
        }
        let session = json!({ "files": [{ "path": "../outside.txt" }, { "path": "src/ok.rs" }] });
        let picked = session_paths(&session, &["../outside.txt".into(), "src/ok.rs".into()]);
        assert_eq!(
            picked,
            vec!["src/ok.rs".to_string()],
            "escaping paths never reach a join"
        );
    }

    fn test_repo() -> TemporaryDir {
        let dir = TemporaryDir::new().unwrap();
        let root = dir.0.to_str().unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.invalid"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "core.hooksPath", "disabled-hooks"],
            // byte-exact checkouts whatever the machine's global setting
            vec!["config", "core.autocrlf", "false"],
        ] {
            assert!(git(root, &args).ok);
        }
        for name in ["a.txt", "b.txt"] {
            std::fs::write(dir.0.join(name), "before\n").unwrap();
        }
        assert!(git(root, &["add", "."]).ok);
        assert!(git(root, &["commit", "-qm", "base"]).ok);
        dir
    }

    fn test_session(root: &Path, paths: &[&str]) -> Value {
        let mut value = json!({"sessionId":"test", "projectPath":root, "files": paths.iter().map(|p| json!({"path":p})).collect::<Vec<_>>()});
        value["reviewRevision"] = json!(review_revision(&value).unwrap());
        value
    }

    #[test]
    fn commit_preserves_unrelated_staging_and_rejects_stale_review() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap();
        std::fs::write(dir.0.join("a.txt"), "approved\n").unwrap();
        std::fs::write(dir.0.join("b.txt"), "unrelated\n").unwrap();
        assert!(git(root, &["add", "b.txt"]).ok);
        let mut session = test_session(&dir.0, &["a.txt"]);
        std::fs::write(dir.0.join("a.txt"), "changed later\n").unwrap();
        assert!(commit_files(&session, &["a.txt".into()], "review")["error"].is_string());
        assert_eq!(
            git(root, &["diff", "--cached", "--name-only"]).out.trim(),
            "b.txt"
        );
        session["reviewRevision"] = json!(review_revision(&session).unwrap());
        let result = commit_files(&session, &["a.txt".into()], "review");
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(
            git(root, &["show", "--pretty=", "--name-only", "HEAD"])
                .out
                .trim(),
            "a.txt"
        );
        assert_eq!(
            git(root, &["diff", "--cached", "--name-only"]).out.trim(),
            "b.txt"
        );
    }

    #[test]
    fn wildcard_paths_cannot_expand_to_other_files() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap();
        std::fs::write(dir.0.join("a.txt"), "unreviewed\n").unwrap();
        let session = test_session(&dir.0, &["*.txt"]);
        assert!(commit_files(&session, &["*.txt".into()], "review")["error"].is_string());
        assert!(git(root, &["diff", "--cached", "--name-only"])
            .out
            .trim()
            .is_empty());
        assert!(checked_file(root, ".").is_err());
        assert!(checked_file(root, ".git/config").is_err());
    }

    #[test]
    fn backups_fail_closed_and_preserve_binary_and_untracked_bytes() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap();
        let binary = [0, 1, 2, 255, 128];
        std::fs::write(dir.0.join("a.txt"), binary).unwrap();
        std::fs::write(dir.0.join("new.bin"), binary).unwrap();
        let paths = vec!["a.txt".into(), "new.bin".into()];
        let bad_base = dir.0.join("a.txt");
        let session = test_session(&dir.0, &["a.txt", "new.bin"]);
        assert!(revert_with_backup(&session, &paths, &bad_base)["error"]
            .as_str()
            .unwrap()
            .contains("backup failed"));
        assert_eq!(std::fs::read(&bad_base).unwrap(), binary);
        assert_eq!(std::fs::read(dir.0.join("new.bin")).unwrap(), binary);
        let patch = backup_files(root, &paths, &[&paths[1]], &dir.0.join("backups")).unwrap();
        assert!(std::fs::read_to_string(&patch)
            .unwrap()
            .contains("GIT binary patch"));
        assert_eq!(
            std::fs::read(patch.parent().unwrap().join("untracked/new.bin")).unwrap(),
            binary
        );
    }

    #[test]
    fn failed_checkout_deletes_nothing_and_names_the_backup() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap();
        let patches = TemporaryDir::new().unwrap();
        std::fs::write(dir.0.join("a.txt"), "rejected\n").unwrap();
        std::fs::write(dir.0.join("new.txt"), "untracked\n").unwrap();
        std::fs::write(dir.0.join("staged.txt"), "staged new\n").unwrap();
        assert!(git(root, &["add", "staged.txt"]).ok);
        let paths: Vec<String> = vec!["a.txt".into(), "new.txt".into(), "staged.txt".into()];
        let session = test_session(&dir.0, &["a.txt", "new.txt", "staged.txt"]);

        // A held index lock makes `git checkout` fail.
        let lock = dir.0.join(".git").join("index.lock");
        std::fs::write(&lock, "").unwrap();
        let failed = revert_with_backup(&session, &paths, &patches.0);
        let message = failed["error"].as_str().expect("checkout must fail");
        let patch = failed["patch"].as_str().expect("error names the backup");
        assert!(message.contains(patch), "{message}");
        assert!(Path::new(patch).exists());
        assert_eq!(
            std::fs::read_to_string(dir.0.join("new.txt")).unwrap(),
            "untracked\n"
        );
        assert!(dir.0.join("staged.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.0.join("a.txt")).unwrap(),
            "rejected\n"
        );

        std::fs::remove_file(&lock).unwrap();
        let done = revert_with_backup(&session, &paths, &patches.0);
        assert_eq!(done["ok"], true, "{done}");
        assert_eq!(
            std::fs::read_to_string(dir.0.join("a.txt")).unwrap(),
            "before\n"
        );
        assert!(!dir.0.join("new.txt").exists());
        assert!(!dir.0.join("staged.txt").exists());
        assert!(git(root, &["diff", "--cached", "--name-only"])
            .out
            .trim()
            .is_empty());
        let backup = Path::new(done["patch"].as_str().unwrap()).parent().unwrap();
        assert_eq!(
            std::fs::read_to_string(backup.join("untracked/staged.txt")).unwrap(),
            "staged new\n"
        );
    }

    #[test]
    fn numeric_file_names_are_not_commit_timestamps() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap();
        std::fs::write(dir.0.join("2024"), "a numeric name\n").unwrap();
        assert!(git(root, &["add", "2024"]).ok);
        assert!(git(root, &["commit", "-qm", "numeric"]).ok);
        let log = fetch_log(root, 1_000_000_000);
        assert_eq!(log.len(), 2, "{log:?}");
        assert!(log.iter().all(|(epoch, _)| *epoch > 1_000_000_000));
        assert_eq!(log[0].1, vec!["2024".to_string()]);

        // The committed file shows up as drift, not as a bogus log entry.
        let mut session = json!({
            "projectPath": root,
            "timestamp": "2000-01-01T00:00:00Z",
            "files": [{ "path": "2024", "changeType": "created" }],
        });
        enrich_session(&mut session);
        assert_eq!(session["files"][0]["drift"], "committed");

        // No parseable timestamp → no cutoff → no drift claim.
        let mut undated = json!({
            "projectPath": root,
            "timestamp": "",
            "files": [{ "path": "2024", "changeType": "created" }],
        });
        enrich_session(&mut undated);
        assert!(undated["files"][0].get("drift").is_none());
    }

    #[test]
    fn branch_names_are_validated_by_git() {
        let dir = test_repo();
        let root = dir.0.to_str().unwrap();
        for bad in [
            "-x", "--force", "a..b", "foo.lock", "@{-1}", "bad name", "a/", "/a", "", "HEAD",
        ] {
            assert!(
                validate_branch_name(root, bad).is_err(),
                "must reject {bad:?}"
            );
        }
        assert!(validate_branch_name(root, "feature/ok-1.2_x").is_ok());
        let session = test_session(&dir.0, &["a.txt"]);
        assert!(create_branch(&session, "-b")["error"].is_string());
        assert_eq!(create_branch(&session, "review/ok")["ok"], true);
    }

    #[test]
    fn github_slugs_require_the_exact_host() {
        let ok = Some(("octo".to_string(), "repo.js".to_string()));
        for good in [
            "https://github.com/octo/repo.js",
            "https://github.com/octo/repo.js.git",
            "https://github.com/octo/repo.js/",
            "https://x-access-token:abc@github.com/octo/repo.js.git",
            "ssh://git@github.com/octo/repo.js.git",
            "git@github.com:octo/repo.js.git",
            "git@github.com:octo/repo.js/",
        ] {
            assert_eq!(github_slug(good), ok, "{good}");
        }
        for evil in [
            "https://github.com.evil.example/octo/repo",
            "https://evil.example/github.com/octo/repo",
            "git@github.com.evil.example:octo/repo",
            "ssh://git@evilgithub.com/octo/repo",
            "https://notgithub.com/octo/repo",
            "http://github.com/octo/repo",
            "https://github.com/octo",
            "https://github.com/octo/repo/extra",
            "https://github.com/octo/re po",
            "https://github.com/../repo",
            "https://github.com/octo/repo?x=1",
        ] {
            assert_eq!(github_slug(evil), None, "{evil}");
        }
        assert_eq!(
            parse_owner_repo("octo/repo"),
            Some(("octo".into(), "repo".into()))
        );
        assert_eq!(parse_owner_repo("octo/repo/x"), None);
        assert_eq!(parse_owner_repo("octo"), None);
        assert_eq!(parse_owner_repo("oc to/repo"), None);
    }
}
