// The project's uncommitted work as Git sees it: every change between HEAD
// and the working tree (staged or not) plus untracked files, parsed into
// typed hunks. This is what gets reviewed, not the transcript.
//
// The diff runs with flags that pin its format against user configuration
// (prefixes, quoting, colors, external tools, blank-context suppression,
// submodule format), so the parser sees one shape everywhere.

use super::super::git::{checked_file, git, git_raw, is_safe_rel, norm_path};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Git's empty tree: the base of a repository with no commit yet.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// Diff lines kept per file, and across the whole review.
const MAX_FILE_LINES: usize = 3_000;
const MAX_TOTAL_LINES: usize = 60_000;
const MAX_UNTRACKED_FILES: usize = 400;
const MAX_UNTRACKED_BYTES: u64 = 2_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum LineKind {
    Context,
    Add,
    Del,
}

impl LineKind {
    fn sign(self) -> char {
        match self {
            LineKind::Context => ' ',
            LineKind::Add => '+',
            LineKind::Del => '-',
        }
    }
}

#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub kind: LineKind,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_line: Option<i64>,
    /// The same text was removed (or added) elsewhere in the changes
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub moved: bool,
    /// On the first line of a moved block: where its other half is
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moved_peer: Option<MovedPeer>,
}

/// The other end of a moved block: the file and line it came from (for
/// added lines) or went to (for removed ones).
#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MovedPeer {
    pub path: String,
    pub line: i64,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<Line>,
    /// Content id: the path and the changed lines, context excluded, so the
    /// same change keeps its id wherever it moves in the file.
    pub id: String,
    /// Some lines were dropped by the display caps (stats stay complete)
    pub truncated: bool,
}

impl Hunk {
    pub fn changed(&self, kind: LineKind) -> impl Iterator<Item = &Line> {
        self.lines.iter().filter(move |l| l.kind == kind)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: Status,
    pub binary: bool,
    /// HEAD's blob id for the file (None when the file is new)
    pub base_blob: Option<String>,
    pub hunks: Vec<Hunk>,
    pub added: i64,
    pub removed: i64,
    pub truncated: bool,
    /// "100644 → 100755" when the file mode changed
    pub mode_change: Option<String>,
}

pub struct Changeset {
    pub head: Option<String>,
    pub branch: String,
    pub files: Vec<ChangedFile>,
    pub warnings: Vec<String>,
}

/// Running id of a hunk: the path, then every changed line with its sign.
struct HunkHasher(Sha256);

impl HunkHasher {
    fn new(path: &str) -> Self {
        let mut h = Sha256::new();
        h.update(b"ars-hunk-v1\0");
        h.update(path.as_bytes());
        h.update(b"\0");
        Self(h)
    }

    fn line(&mut self, kind: LineKind, text: &str) {
        if kind != LineKind::Context {
            self.0.update([kind.sign() as u8]);
            self.0.update(text.as_bytes());
            self.0.update(b"\n");
        }
    }

    fn finish(self) -> String {
        let digest = self.0.finalize();
        format!("h-{}", hex(&digest[..12]))
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The id a hunk with these changed lines has, as `collect` computes it.
pub fn hunk_id<'a>(path: &str, lines: impl IntoIterator<Item = (LineKind, &'a str)>) -> String {
    let mut h = HunkHasher::new(path);
    for (kind, text) in lines {
        h.line(kind, text);
    }
    h.finish()
}

/// Undo git's C-style quoting of a path ("a\"b", "\303\251" octal bytes).
fn unquote(raw: &str) -> String {
    let Some(inner) = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) else {
        return raw.to_string();
    };
    let mut bytes: Vec<u8> = Vec::new();
    let mut chars = inner.bytes().peekable();
    while let Some(b) = chars.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        match chars.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'a') => bytes.push(7),
            Some(b'b') => bytes.push(8),
            Some(b'f') => bytes.push(12),
            Some(b'v') => bytes.push(11),
            Some(d @ b'0'..=b'7') => {
                let mut v = (d - b'0') as u32;
                for _ in 0..2 {
                    match chars.peek() {
                        Some(&n @ b'0'..=b'7') => {
                            v = v * 8 + (n - b'0') as u32;
                            chars.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(v as u8);
            }
            Some(other) => bytes.push(other),
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).to_string()
}

/// The path of a `--- a/x` / `+++ b/x` line, or None for /dev/null.
fn side_path(raw: &str, prefix: &str) -> Option<String> {
    let raw = raw.trim_end_matches(['\r', '\t']);
    if raw == "/dev/null" {
        return None;
    }
    let p = unquote(raw);
    Some(norm_path(p.strip_prefix(prefix).unwrap_or(&p)))
}

/// Both paths of `diff --git a/x b/y` when unambiguous (quoted, or the same
/// path twice), for a header without --- / +++ lines.
fn header_paths(rest: &str) -> Option<(String, String)> {
    if let Some(inner) = rest.strip_prefix('"') {
        let end = inner.find("\" ")? + 1;
        let a = unquote(&rest[..=end]);
        let b = unquote(rest[end + 2..].trim());
        return Some((
            norm_path(a.strip_prefix("a/")?),
            norm_path(b.strip_prefix("b/")?),
        ));
    }
    let body = rest.strip_prefix("a/")?;
    // "a/P b/P": the two halves are equal
    let n = body.len().checked_sub(3)? / 2;
    if body.is_char_boundary(n) && body[n..].starts_with(" b/") && body[..n] == body[n + 3..] {
        let p = norm_path(&body[..n]);
        return Some((p.clone(), p));
    }
    None
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

/// Parser state for one `git diff` output.
struct DiffParser {
    files: Vec<ChangedFile>,
    total_lines: usize,
    current: Option<ChangedFile>,
    hunk: Option<(Hunk, HunkHasher)>,
    file_lines: usize,
    old_line: i64,
    new_line: i64,
    old_mode: Option<String>,
    header_path: Option<(String, String)>,
}

impl DiffParser {
    fn close_hunk(&mut self) {
        if let (Some(file), Some((mut hunk, hasher))) = (self.current.as_mut(), self.hunk.take()) {
            hunk.id = hasher.finish();
            file.truncated |= hunk.truncated;
            file.hunks.push(hunk);
        }
    }

    fn close_file(&mut self) {
        self.close_hunk();
        if let Some(mut file) = self.current.take() {
            if file.path.is_empty() {
                if let Some((a, b)) = self.header_path.take() {
                    file.path = b;
                    if file.status == Status::Renamed && file.old_path.is_none() {
                        file.old_path = Some(a);
                    }
                }
            }
            if !file.path.is_empty() {
                self.files.push(file);
            }
        }
        self.header_path = None;
        self.old_mode = None;
        self.file_lines = 0;
    }

    fn line(&mut self, raw: &str) {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            self.close_file();
            self.header_path = header_paths(rest);
            self.current = Some(ChangedFile {
                path: String::new(),
                old_path: None,
                status: Status::Modified,
                binary: false,
                base_blob: None,
                hunks: Vec::new(),
                added: 0,
                removed: 0,
                truncated: false,
                mode_change: None,
            });
            return;
        }
        let Some(file) = self.current.as_mut() else {
            return;
        };
        if self.hunk.is_none() || raw.starts_with("@@ ") {
            // Extended header lines (before the first hunk)
            if let Some(m) = raw.strip_prefix("new file mode ") {
                file.status = Status::Added;
                let _ = m;
            } else if raw.starts_with("deleted file mode ") {
                file.status = Status::Deleted;
            } else if let Some(m) = raw.strip_prefix("old mode ") {
                self.old_mode = Some(m.trim().to_string());
            } else if let Some(m) = raw.strip_prefix("new mode ") {
                if let Some(old) = &self.old_mode {
                    file.mode_change = Some(format!("{old} → {}", m.trim()));
                }
            } else if let Some(p) = raw.strip_prefix("rename from ") {
                file.status = Status::Renamed;
                file.old_path = Some(norm_path(&unquote(p.trim_end())));
            } else if let Some(p) = raw.strip_prefix("rename to ") {
                file.status = Status::Renamed;
                file.path = norm_path(&unquote(p.trim_end()));
            } else if let Some(ids) = raw.strip_prefix("index ") {
                let old = ids.split("..").next().unwrap_or("");
                if !old.is_empty() && old.bytes().any(|b| b != b'0') {
                    file.base_blob = Some(old.to_string());
                }
            } else if let Some(p) = raw.strip_prefix("--- ") {
                if file.old_path.is_none() && file.status == Status::Renamed {
                    file.old_path = side_path(p, "a/");
                }
                if let Some(path) = side_path(p, "a/") {
                    if file.path.is_empty() {
                        file.path = path;
                    }
                }
                return;
            } else if let Some(p) = raw.strip_prefix("+++ ") {
                if let Some(path) = side_path(p, "b/") {
                    file.path = path;
                }
                return;
            } else if raw.starts_with("Binary files ") || raw.starts_with("GIT binary patch") {
                file.binary = true;
            }
            if raw.starts_with("@@ ") {
                if let Some((o, n)) = parse_hunk_header(raw) {
                    self.close_hunk();
                    let file = self.current.as_ref().expect("current file");
                    self.old_line = o;
                    self.new_line = n;
                    self.hunk = Some((
                        Hunk {
                            header: raw.trim_end_matches('\r').to_string(),
                            lines: Vec::new(),
                            id: String::new(),
                            truncated: false,
                        },
                        HunkHasher::new(&file.path),
                    ));
                }
            }
            return;
        }
        if raw.starts_with('\\') {
            return; // "\ No newline at end of file"
        }
        // CRLF working trees: the \r travels inside the diff content, so the
        // display copy drops it
        let body = raw.strip_suffix('\r').unwrap_or(raw);
        let (kind, text) = if let Some(t) = body.strip_prefix('+') {
            (LineKind::Add, t)
        } else if let Some(t) = body.strip_prefix('-') {
            (LineKind::Del, t)
        } else if let Some(t) = body.strip_prefix(' ') {
            (LineKind::Context, t)
        } else {
            return; // the empty tail of the final split
        };
        let (old_line, new_line) = match kind {
            LineKind::Add => {
                file.added += 1;
                self.new_line += 1;
                (None, Some(self.new_line - 1))
            }
            LineKind::Del => {
                file.removed += 1;
                self.old_line += 1;
                (Some(self.old_line - 1), None)
            }
            LineKind::Context => {
                self.old_line += 1;
                self.new_line += 1;
                (Some(self.old_line - 1), Some(self.new_line - 1))
            }
        };
        let keep = self.file_lines < MAX_FILE_LINES && self.total_lines < MAX_TOTAL_LINES;
        if let Some((hunk, hasher)) = self.hunk.as_mut() {
            hasher.line(kind, text);
            if keep {
                hunk.lines.push(Line {
                    kind,
                    text: text.to_string(),
                    old_line,
                    new_line,
                    moved: false,
                    moved_peer: None,
                });
                self.file_lines += 1;
                self.total_lines += 1;
            } else {
                hunk.truncated = true;
            }
        }
    }
}

/// Parse `git diff` output (see `diff_args`).
pub fn parse_diff(text: &str) -> Vec<ChangedFile> {
    let mut p = DiffParser {
        files: Vec::new(),
        total_lines: 0,
        current: None,
        hunk: None,
        file_lines: 0,
        old_line: 0,
        new_line: 0,
        old_mode: None,
        header_path: None,
    };
    for line in text.split('\n') {
        p.line(line);
    }
    p.close_file();
    // A hunk header seen before the +++ line knows its path late: re-id
    // every hunk with the file's final path.
    for f in &mut p.files {
        for h in &mut f.hunks {
            if !h.truncated {
                h.id = hunk_id(&f.path, h.lines.iter().map(|l| (l.kind, l.text.as_str())));
            }
        }
        let mut ids: Vec<String> = f.hunks.iter().map(|h| h.id.clone()).collect();
        number_duplicates(&mut ids);
        for (h, id) in f.hunks.iter_mut().zip(ids) {
            h.id = id;
        }
    }
    p.files
}

/// The same change twice in one file: the later ones get an ordinal
/// ("h-…~2"), so each can be decided on its own.
pub fn number_duplicates(ids: &mut [String]) {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for id in ids.iter_mut() {
        let n = seen.entry(id.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            *id = format!("{id}~{n}");
        }
    }
}

/// The flags every review diff runs with (base, then pinned format).
pub fn diff_args(base: &str) -> Vec<&str> {
    vec![
        "-c",
        "diff.suppressBlankEmpty=false",
        "diff",
        base,
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--full-index",
        "-M",
        "--histogram",
        "--unified=3",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--submodule=short",
    ]
}

/// HEAD's commit id, None for a repository without commits.
pub fn head_of(root: &str) -> Option<String> {
    let r = git(root, &["rev-parse", "--verify", "-q", "HEAD"]);
    let sha = r.out.trim().to_string();
    (r.ok && !sha.is_empty()).then_some(sha)
}

/// The work tree root of `dir`, with forward slashes (remembered: the
/// window asks for the same project on every refresh).
pub fn toplevel(dir: &str) -> Result<String, String> {
    use once_cell::sync::Lazy;
    use std::collections::HashMap;
    use std::sync::Mutex;
    static KNOWN: Lazy<Mutex<HashMap<String, String>>> = Lazy::new(|| Mutex::new(HashMap::new()));
    if let Some(root) = KNOWN.lock().ok().and_then(|k| k.get(dir).cloned()) {
        if std::path::Path::new(&root).join(".git").exists() {
            return Ok(root);
        }
    }
    let r = git(dir, &["rev-parse", "--show-toplevel"]);
    let root = r.out.trim().to_string();
    if !r.ok || root.is_empty() {
        return Err(format!("{dir} is not inside a Git repository."));
    }
    let root = norm_path(&root);
    if let Ok(mut k) = KNOWN.lock() {
        k.insert(dir.to_string(), root.clone());
    }
    Ok(root)
}

/// An untracked file as one all-added hunk, or marked binary (no hunk).
/// Its shown lines come out of `budget`, the lines left for the review.
fn untracked_file(
    root: &str,
    rel: &str,
    budget: &mut usize,
    warnings: &mut Vec<String>,
) -> ChangedFile {
    let mut file = ChangedFile {
        path: rel.to_string(),
        old_path: None,
        status: Status::Untracked,
        binary: false,
        base_blob: None,
        hunks: Vec::new(),
        added: 0,
        removed: 0,
        truncated: false,
        mode_change: None,
    };
    let unreadable = |file: &mut ChangedFile, warnings: &mut Vec<String>| {
        file.binary = true;
        warnings.push(format!(
            "{rel} could not be read, so its content is not shown."
        ));
    };
    let Ok(full) = checked_file(root, rel) else {
        unreadable(&mut file, warnings);
        return file;
    };
    let size = std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
    if size > MAX_UNTRACKED_BYTES {
        file.binary = true;
        file.truncated = true;
        return file;
    }
    let Ok(bytes) = std::fs::read(&full) else {
        unreadable(&mut file, warnings);
        return file;
    };
    // git's heuristic: a NUL byte in the first 8000 bytes means binary
    if bytes.iter().take(8000).any(|b| *b == 0) {
        file.binary = true;
        return file;
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    // A trailing newline yields an empty last line that git doesn't count
    if lines.last() == Some(&"") {
        lines.pop();
    }
    file.added = lines.len() as i64;
    let shown = lines.len().min(MAX_FILE_LINES).min(*budget);
    *budget -= shown;
    let id = hunk_id(rel, lines.iter().map(|l| (LineKind::Add, *l)));
    if !lines.is_empty() {
        file.truncated = shown < lines.len();
        file.hunks.push(Hunk {
            header: format!("@@ -0,0 +1,{} @@", lines.len()),
            lines: lines[..shown]
                .iter()
                .enumerate()
                .map(|(i, t)| Line {
                    kind: LineKind::Add,
                    text: (*t).to_string(),
                    old_line: None,
                    new_line: Some(i as i64 + 1),
                    moved: false,
                    moved_peer: None,
                })
                .collect(),
            id,
            truncated: file.truncated,
        });
    }
    file
}

/// Every uncommitted change of the repository at `root` (a work tree root).
pub fn collect(root: &str) -> Result<Changeset, String> {
    // Independent git calls run side by side: each costs a process start
    // (tens of milliseconds on Windows) before it does any work.
    let (head, branch, diff, ls) = std::thread::scope(|sc| {
        let head = sc.spawn(|| head_of(root));
        let branch = sc.spawn(|| git(root, &["symbolic-ref", "--short", "-q", "HEAD"]));
        let diff = sc.spawn(|| git_raw(root, &diff_args("HEAD"), None));
        let ls = git(root, &["ls-files", "-z", "--others", "--exclude-standard"]);
        fn joined<T>(r: std::thread::Result<T>) -> Result<T, String> {
            r.map_err(|_| "A Git command stopped unexpectedly.".to_string())
        }
        Ok::<_, String>((
            joined(head.join())?,
            joined(branch.join())?,
            joined(diff.join())?,
            ls,
        ))
    })?;
    let branch = {
        let b = branch.out.trim().to_string();
        if branch.ok && !b.is_empty() {
            b
        } else {
            "(detached HEAD)".to_string()
        }
    };
    // A repository without commits diffs against the empty tree.
    let diff = if head.is_none() {
        git_raw(root, &diff_args(EMPTY_TREE), None)
    } else {
        diff
    };
    if !diff.ok {
        return Err(format!(
            "git diff failed: {}",
            diff.err.chars().take(400).collect::<String>()
        ));
    }
    let mut files = parse_diff(&String::from_utf8_lossy(&diff.out));
    let mut warnings = Vec::new();
    let shown: usize = files
        .iter()
        .flat_map(|f| &f.hunks)
        .map(|h| h.lines.len())
        .sum();
    let mut budget = MAX_TOTAL_LINES.saturating_sub(shown);
    // `git rm --cached` leaves a file both deleted in the diff and untracked
    // on disk: the diff already shows it.
    let in_diff: std::collections::HashSet<String> = files.iter().map(|f| f.path.clone()).collect();

    if ls.ok {
        let untracked: Vec<String> = ls
            .out
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(norm_path)
            .filter(|p| is_safe_rel(p) && !in_diff.contains(p))
            .collect();
        if untracked.len() > MAX_UNTRACKED_FILES {
            warnings.push(format!(
                "{} untracked files: only the first {} are shown. Add the rest to .gitignore if they are build output.",
                untracked.len(),
                MAX_UNTRACKED_FILES
            ));
        }
        for rel in untracked.iter().take(MAX_UNTRACKED_FILES) {
            files.push(untracked_file(root, rel, &mut budget, &mut warnings));
        }
    } else {
        warnings.push("Untracked files could not be listed.".into());
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Changeset {
        head,
        branch,
        files,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "diff --git a/src/alpha.ts b/src/alpha.ts\nindex 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n--- a/src/alpha.ts\n+++ b/src/alpha.ts\n@@ -1,4 +1,5 @@\n context one\n-removed line\n+added line\n+another added\n context two\n@@ -10,2 +11,2 @@ fn ctx\n context three\n-old\n+new\ndiff --git a/docs/beta.md b/docs/beta.md\ndeleted file mode 100644\nindex 3333333333333333333333333333333333333333..0000000000000000000000000000000000000000\n--- a/docs/beta.md\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-gone one\n-gone two\ndiff --git a/old name.txt b/new name.txt\nsimilarity index 100%\nrename from old name.txt\nrename to new name.txt\ndiff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\ndiff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251.txt\"\nnew file mode 100644\nindex 0000000000000000000000000000000000000000..4444444444444444444444444444444444444444\n--- /dev/null\n+++ \"b/caf\\303\\251.txt\"\n@@ -0,0 +1 @@\n+hello\n";

    #[test]
    fn parses_modified_deleted_renamed_mode_and_quoted_files() {
        let files = parse_diff(SAMPLE);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "src/alpha.ts",
                "docs/beta.md",
                "new name.txt",
                "run.sh",
                "café.txt"
            ]
        );

        let alpha = &files[0];
        assert_eq!(alpha.status, Status::Modified);
        assert_eq!(
            alpha.base_blob.as_deref(),
            Some("1111111111111111111111111111111111111111")
        );
        assert_eq!((alpha.added, alpha.removed), (3, 2));
        assert_eq!(alpha.hunks.len(), 2);
        let first = &alpha.hunks[0].lines;
        assert_eq!(first[0].kind, LineKind::Context);
        assert_eq!((first[0].old_line, first[0].new_line), (Some(1), Some(1)));
        assert_eq!((first[1].kind, first[1].old_line), (LineKind::Del, Some(2)));
        assert_eq!((first[2].kind, first[2].new_line), (LineKind::Add, Some(2)));
        assert_eq!(first[4].new_line, Some(4));
        assert_eq!(alpha.hunks[1].lines[1].old_line, Some(11));

        assert_eq!(files[1].status, Status::Deleted);
        assert_eq!(files[1].hunks[0].lines.len(), 2);
        assert_eq!(files[2].status, Status::Renamed);
        assert_eq!(files[2].old_path.as_deref(), Some("old name.txt"));
        assert!(files[2].hunks.is_empty());
        assert_eq!(files[3].mode_change.as_deref(), Some("100644 → 100755"));
        assert_eq!(files[4].status, Status::Added);
        assert!(files[4].base_blob.is_none());
    }

    #[test]
    fn hunk_ids_follow_the_change_not_its_position_or_context() {
        let a = parse_diff(
            "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n ctx\n-old\n+new\n ctx2\n",
        );
        let b = parse_diff("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -40,3 +40,3 @@\n other\n-old\n+new\n other2\n");
        let c = parse_diff(
            "diff --git a/y b/y\n--- a/y\n+++ b/y\n@@ -1,3 +1,3 @@\n ctx\n-old\n+new\n ctx2\n",
        );
        assert_eq!(a[0].hunks[0].id, b[0].hunks[0].id);
        assert_ne!(
            a[0].hunks[0].id, c[0].hunks[0].id,
            "the path is part of the id"
        );
        // An untracked file and the same file once staged share their id.
        assert_eq!(
            hunk_id("n.txt", [(LineKind::Add, "hello")]),
            parse_diff("diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1 @@\n+hello\n")[0].hunks[0].id
        );
    }

    #[test]
    fn collects_tracked_staged_and_untracked_changes() {
        let dir = super::super::super::git::test_repo();
        let root = dir.0.to_str().unwrap();
        std::fs::write(dir.0.join("a.txt"), "after\n").unwrap();
        std::fs::write(dir.0.join("staged.txt"), "staged\n").unwrap();
        assert!(git(root, &["add", "staged.txt"]).ok);
        std::fs::write(dir.0.join("new.txt"), "one\r\ntwo\r\n").unwrap();
        std::fs::remove_file(dir.0.join("b.txt")).unwrap();
        let cs = collect(root).expect("changeset");
        let got: Vec<(&str, Status)> = cs
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.status))
            .collect();
        assert_eq!(
            got,
            [
                ("a.txt", Status::Modified),
                ("b.txt", Status::Deleted),
                ("new.txt", Status::Untracked),
                ("staged.txt", Status::Added),
            ]
        );
        let new = &cs.files[2];
        assert_eq!(new.added, 2);
        assert_eq!(new.hunks[0].lines[1].text, "two", "CR stripped for display");
        assert!(cs.head.is_some());
    }
}
