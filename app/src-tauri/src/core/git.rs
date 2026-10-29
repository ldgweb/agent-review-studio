// Git plumbing for the project review (core/review): process spawning that
// never opens a console window, never blocks on a credential prompt and
// never writes to the repository, guards for every path a transcript or a
// client names, and the remote parsing the CLI's PR comment needs.
//
// Agent Review Studio only reads repositories: it changes no file, no
// index and no ref. Every change the reviewer wants goes to the agent as an
// instruction.
//
// Every git spawn goes through `git_command`: no console window on Windows,
// no terminal credential prompt (a GUI app has no terminal to answer it).

use std::path::Path;
use std::process::{Command, Stdio};

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
    // Not even the index's stat cache: `git status` refreshes it otherwise.
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    hide_window(&mut cmd);
    cmd
}

pub(crate) struct GitOutput {
    pub ok: bool,
    pub out: Vec<u8>,
    pub err: String,
}

/// Run git in `root` with literal pathspecs and unquoted non-ASCII paths,
/// feeding `stdin` from a separate thread (a large input must not deadlock
/// against an output pipe nobody reads yet).
pub(crate) fn git_raw(root: &str, args: &[&str], stdin: Option<Vec<u8>>) -> GitOutput {
    let mut cmd = git_command();
    cmd.arg("--literal-pathspecs")
        .args(["-c", "core.quotepath=false"])
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let failed = |e: std::io::Error| GitOutput {
        ok: false,
        out: Vec::new(),
        err: e.to_string(),
    };
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return failed(e),
    };
    let writer = match (stdin, child.stdin.take()) {
        (Some(input), Some(mut pipe)) => Some(std::thread::spawn(move || {
            use std::io::Write;
            let _ = pipe.write_all(&input);
        })),
        _ => None,
    };
    let output = child.wait_with_output();
    if let Some(w) = writer {
        let _ = w.join();
    }
    match output {
        Ok(o) => GitOutput {
            ok: o.status.success(),
            out: o.stdout,
            err: String::from_utf8_lossy(&o.stderr).to_string(),
        },
        Err(e) => failed(e),
    }
}

pub(crate) struct GitResult {
    pub ok: bool,
    pub out: String,
}

pub(crate) fn git(root: &str, args: &[&str]) -> GitResult {
    let o = git_raw(root, args, None);
    GitResult {
        ok: o.ok,
        out: String::from_utf8_lossy(&o.out).to_string(),
    }
}

pub(crate) fn norm_path(p: &str) -> String {
    p.replace('\\', "/")
}

fn is_git_short_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower
        .strip_prefix("git~")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// True only for a plain relative path that stays inside its root: no drive
/// prefix, no leading separator, no `..` component. Paths come from git,
/// transcripts and the client, so they are never trusted to join a
/// filesystem root as-is.
pub(crate) fn is_safe_rel(p: &str) -> bool {
    use std::path::Component;
    if p.is_empty() || p.contains(':') || p.chars().any(char::is_control) {
        return false;
    }
    Path::new(p).components().all(|c| match c {
        Component::Normal(s) => {
            let name = s.to_string_lossy();
            let name = name.trim_end_matches(['.', ' ']);
            // `.git`, and its Windows short name (`GIT~1`), as Git's own
            // core.protectNTFS refuses them
            !name.eq_ignore_ascii_case(".git") && !is_git_short_name(name)
        }
        _ => false,
    })
}

/// The canonical form of a project root, remembered (canonicalizing is a
/// slow system call on Windows, and every file check starts from it).
fn canonical_root(root: &str) -> Result<std::path::PathBuf, String> {
    use once_cell::sync::Lazy;
    use std::collections::HashMap;
    use std::sync::Mutex;
    static KNOWN: Lazy<Mutex<HashMap<String, std::path::PathBuf>>> =
        Lazy::new(|| Mutex::new(HashMap::new()));
    if let Some(hit) = KNOWN.lock().ok().and_then(|k| k.get(root).cloned()) {
        if hit.is_dir() {
            return Ok(hit);
        }
    }
    let canonical = Path::new(root).canonicalize().map_err(|e| e.to_string())?;
    if let Ok(mut k) = KNOWN.lock() {
        k.insert(root.to_string(), canonical.clone());
    }
    Ok(canonical)
}

/// Resolve only ordinary files inside the project. Reject symlinks, Windows
/// junctions and Git metadata, including in parent directories. A plain
/// relative path (no `..`, no root, no `.git`: `is_safe_rel`) whose every
/// existing component is neither a link nor a junction cannot leave the
/// canonical root, so no component needs resolving.
pub(crate) fn checked_file(root: &str, relative: &str) -> Result<std::path::PathBuf, String> {
    if !is_safe_rel(relative) {
        return Err(format!("Unsafe project path: {relative}"));
    }
    let mut full = canonical_root(root)?;
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
                        "Linked paths (symlinks, junctions) are not read: {relative}"
                    ));
                }
            }
            // The rest of the path does not exist yet: nothing to follow.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    let mut full = canonical_root(root)?;
    full.extend(Path::new(relative).components());
    if full.is_dir() {
        return Err(format!("Expected a file, not a directory: {relative}"));
    }
    Ok(full)
}

/// The paths of `git status --porcelain=v2 -z` output, one per changed
/// file: the fields before a path are fixed per entry kind, and the path
/// itself may contain spaces. A rename's original path is skipped.
pub(crate) fn status_paths(out: &str) -> Vec<&str> {
    let mut paths = Vec::new();
    let mut entries = out.split('\0');
    while let Some(entry) = entries.next() {
        let fields = match entry.as_bytes().first() {
            Some(b'1') => 8,
            Some(b'2') => {
                entries.next();
                9
            }
            Some(b'u') => 10,
            Some(b'?') => 1,
            _ => continue,
        };
        if let Some(path) = entry
            .splitn(fields + 1, ' ')
            .nth(fields)
            .filter(|p| !p.is_empty())
        {
            paths.push(path);
        }
    }
    paths
}

/// origin's URL and the current branch of the repository at `root`: the
/// CLI finds the pull request with them.
pub fn remote_and_branch(root: &str) -> (Option<String>, Option<String>) {
    let remote = git(root, &["remote", "get-url", "origin"]);
    let branch = git(root, &["symbolic-ref", "--short", "-q", "HEAD"]);
    let text = |r: GitResult| {
        let t = r.out.trim().to_string();
        (r.ok && !t.is_empty()).then_some(t)
    };
    (text(remote), text(branch))
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

/// A throwaway directory, removed on drop (tests).
#[cfg(test)]
pub(crate) struct TemporaryDir(pub std::path::PathBuf);
#[cfg(test)]
impl TemporaryDir {
    pub(crate) fn new() -> Result<Self, String> {
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
#[cfg(test)]
impl Drop for TemporaryDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A repository with `a.txt` and `b.txt` committed as "before\n" (tests).
#[cfg(test)]
pub(crate) fn test_repo() -> TemporaryDir {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escaping_paths() {
        for bad in [
            "../secret.txt",
            "a/../../b",
            "/etc/passwd",
            "\\\\share\\x",
            "",
            ".git./config",
            "GIT~1/config",
            "a/git~12/HEAD",
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
    }

    #[test]
    fn status_paths_may_contain_spaces() {
        let out = [
            "1 .M N... 100644 100644 100644 abc abc my dir/a b.ts",
            "2 R. N... 100644 100644 100644 abc abc R100 new name.ts",
            "old name.ts",
            "? notes/to do.md",
            "",
        ]
        .join("\0");
        assert_eq!(
            status_paths(&out),
            vec!["my dir/a b.ts", "new name.ts", "notes/to do.md"]
        );
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
