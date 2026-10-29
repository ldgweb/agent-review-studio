// ars: the native CLI, on the same core the desktop app runs.
//
// `ars .` opens the project's review in the desktop app. `ars review` and
// `ars pr` build the same project review the desktop shows (the core's
// review::build), so the CLI can never diverge from the app. `ars feedback`
// is for the agent: it prints the review the desktop would copy.

use agent_review_studio::app_core::{api, domain::AgentSession, git, paths, review};
use agent_review_studio::cli::{github, render};
use serde_json::Value;
use std::path::{Path, PathBuf};

const USAGE: &str = "\
ars: review what your coding agents changed, before you commit

USAGE
  ars <dir>                       Open the review of <dir> in the desktop app
  ars <command> [target] [flags]

COMMANDS
  open [dir]           Open the review of this project (or <dir>) in the
                       desktop app; `ars .` is the short form
  list, ls             List recent agent sessions (default)
  review, r [target]   Terminal summary of the project's uncommitted changes:
                       who wrote what, your verdicts, findings, comments
  pr [target]          The same as Markdown for the pull request
  feedback [dir]       For your agent: print the review of this project as
                       instructions and start the next round (what Copy
                       review does in the app). Ask the agent to run it.

TARGET
  A folder in the project (default: the current one), or a session from
  `ars list` by its id or its position (1 is the first): that session's
  project.

FLAGS
  --json               review/pr: print the whole review as JSON, to read
                       back with --from. NOT redacted: it holds diffs and
                       transcript excerpts (prompts, any secrets in them).
                       Do not post or share it.
  --from <file>        review/pr: read the review from a file saved with
                       --json
  --project <path>     list: only sessions under this project path
  --post               pr: create/update the review comment on the PR
  --dry-run            pr: resolve the target and preview, post nothing
  --pr <n>             pr: pull request number (else resolved by branch)
  --repo <owner/name>  pr: repository (else GITHUB_REPOSITORY / git remote)
  -h, --help           This help
  -V, --version        Version
";

struct Args {
    cmd: String,
    target: Option<String>,
    post: bool,
    dry_run: bool,
    json: bool,
    pr: Option<u64>,
    repo: Option<String>,
    from: Option<String>,
    project: Option<String>,
}

fn flag_value(argv: &[String], i: &mut usize, flag: &str) -> String {
    *i += 1;
    match argv.get(*i) {
        Some(v) if !v.starts_with('-') => v.clone(),
        _ => {
            eprintln!("ars: {flag} requires a value");
            std::process::exit(2);
        }
    }
}

fn parse(argv: &[String]) -> Args {
    let mut a = Args {
        cmd: "list".to_string(),
        target: None,
        post: false,
        dry_run: false,
        json: false,
        pr: None,
        repo: None,
        from: None,
        project: None,
    };
    let mut pos: Vec<String> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let t = &argv[i];
        match t.as_str() {
            "--help" | "-h" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("ars {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--post" => a.post = true,
            "--dry-run" => a.dry_run = true,
            "--json" => a.json = true,
            "--pr" => {
                let raw = flag_value(argv, &mut i, "--pr");
                match raw.parse::<u64>() {
                    Ok(n) if n > 0 => a.pr = Some(n),
                    _ => {
                        eprintln!("ars: --pr expects a pull request number, got \"{raw}\"");
                        std::process::exit(2);
                    }
                }
            }
            "--repo" => a.repo = Some(flag_value(argv, &mut i, "--repo")),
            "--from" => a.from = Some(flag_value(argv, &mut i, "--from")),
            "--project" => a.project = Some(flag_value(argv, &mut i, "--project")),
            s if !s.starts_with('-') => pos.push(t.clone()),
            other => {
                eprintln!("ars: unknown flag \"{other}\". Run `ars --help`.");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    if pos.len() > 2 {
        eprintln!("ars: unexpected argument \"{}\". Run `ars --help`.", pos[2]);
        std::process::exit(2);
    }
    a.cmd = pos.first().cloned().unwrap_or_else(|| "list".to_string());
    a.target = pos.get(1).cloned();
    a
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = parse(&argv);

    if args.cmd == "help" {
        print!("{USAGE}");
        return;
    }

    // `ars open [dir]`, or a directory in place of a command (`ars .`). The
    // desktop app builds the review, so nothing is read here.
    if args.cmd == "open" {
        open_in_desktop(args.target.as_deref().unwrap_or("."));
        return;
    }
    if !is_command(&args.cmd) && Path::new(&args.cmd).is_dir() {
        if let Some(extra) = &args.target {
            eprintln!("ars: unexpected argument \"{extra}\". Run `ars --help`.");
            std::process::exit(2);
        }
        open_in_desktop(&args.cmd);
        return;
    }

    match args.cmd.as_str() {
        "list" | "ls" => print!("{}", render::render_list(&list_sessions(&args).await)),

        "review" | "r" => {
            let review = load_review(&args).await;
            if args.json {
                print_json(&review);
            } else {
                print!("{}", render::render_review(&review));
            }
        }

        "feedback" => feedback(args.target.as_deref().unwrap_or(".")).await,

        "pr" => {
            let review = load_review(&args).await;
            if args.json {
                print_json(&review);
                return;
            }
            let body = render::render_pr_markdown(&review);
            if args.post || args.dry_run {
                post_flow(&review, &body, &args).await;
            } else {
                print!("{body}");
            }
        }

        other => {
            eprintln!("ars: unknown command \"{other}\". Run `ars --help`.");
            std::process::exit(2);
        }
    }
}

fn is_command(cmd: &str) -> bool {
    matches!(cmd, "list" | "ls" | "review" | "r" | "pr" | "feedback")
}

/// The review for the agent on stdout, the way the agent reads it; notes
/// about it on stderr.
async fn feedback(dir: &str) {
    if !Path::new(dir).is_dir() {
        eprintln!("ars: {dir} is not a folder.");
        std::process::exit(2);
    }
    let dir = dir.to_string();
    let done = tokio::task::spawn_blocking(move || review::feedback(&dir))
        .await
        .unwrap_or_else(|_| Err("the review crashed".to_string()));
    match done {
        Ok(fb) if fb.text.is_empty() => {
            println!("No review to act on: the reviewer has not asked for any change yet.");
            if fb.undecided > 0 {
                println!(
                    "{} file{} still waiting for the reviewer's decision.",
                    fb.undecided,
                    if fb.undecided == 1 { " is" } else { "s are" }
                );
            }
        }
        Ok(fb) => {
            print!("{}", fb.text);
            if fb.undecided > 0 {
                eprintln!(
                    "ars: the review is not finished: {} file{} still undecided. Act only on what is listed.",
                    fb.undecided,
                    if fb.undecided == 1 { " is" } else { "s are" }
                );
            }
        }
        Err(e) => {
            eprintln!("ars: {e}");
            std::process::exit(1);
        }
    }
}

/// The scanned session index, filtered for `ars list` (or a target lookup).
async fn list_sessions(args: &Args) -> Vec<AgentSession> {
    let value = api::handle("/sessions", "GET", None).await;
    if let Some(err) = value.get("error").and_then(|e| e.as_str()) {
        eprintln!("ars: {err}");
        std::process::exit(1);
    }
    if let Some(err) = value.get("storageError").and_then(|e| e.as_str()) {
        eprintln!("ars: warning: {err}");
    }
    let Some(arr) = value.get("sessions").and_then(|s| s.as_array()) else {
        eprintln!("ars: unexpected response from the core (no session list).");
        std::process::exit(1);
    };
    let parsed: Vec<AgentSession> = arr
        .iter()
        .filter_map(|it| serde_json::from_value::<AgentSession>(it.clone()).ok())
        .collect();
    let skipped = arr.len() - parsed.len();
    if skipped > 0 {
        eprintln!(
            "ars: warning: {skipped} session{} could not be read and {} skipped.",
            if skipped == 1 { "" } else { "s" },
            if skipped == 1 { "was" } else { "were" }
        );
    }
    parsed
        .into_iter()
        .filter(|s| s.archived != Some(true))
        .filter(|s| match &args.project {
            Some(p) => under_project(s, p),
            None => true,
        })
        .collect()
}

/// The project review for review/pr: read from a file saved with --json,
/// or built for the target (a folder, or the project of a session from
/// `ars list`).
async fn load_review(args: &Args) -> Value {
    if let Some(path) = &args.from {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("ars: could not read {path}: {e}");
            std::process::exit(1);
        });
        return parse_capsule(&text).unwrap_or_else(|e| {
            eprintln!("ars: {path} is not a review saved with `ars review --json`: {e}");
            std::process::exit(1);
        });
    }
    let dir = match args.target.as_deref() {
        None => ".".to_string(),
        Some(t) if Path::new(t).is_dir() => t.to_string(),
        Some(t) => {
            let sessions = list_sessions(args).await;
            let s = pick(&sessions, Some(t)).unwrap_or_else(|| fail(Some(t)));
            match &s.project_path {
                Some(p) if Path::new(p).is_dir() => p.clone(),
                _ => {
                    eprintln!(
                        "ars: session {} has no project folder on this machine.",
                        s.session_id
                    );
                    std::process::exit(1);
                }
            }
        }
    };
    let built = tokio::task::spawn_blocking(move || review::build(&dir))
        .await
        .unwrap_or_else(|_| Err("the review crashed".to_string()));
    match built.map(serde_json::to_value) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            eprintln!("ars: could not serialize the review: {e}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("ars: {e}");
            std::process::exit(1);
        }
    }
}

/// A saved review: the JSON that `ars review --json` printed.
fn parse_capsule(text: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if !value["project"].is_object() || !value["files"].is_array() {
        return Err("it has no project and files".into());
    }
    Ok(value)
}

fn print_json(review: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(review).unwrap_or_default()
    );
}

/// The project a directory belongs to: its Git work tree root when there is
/// one (so `ars .` works from any subfolder), else the directory itself.
fn project_root_of(dir: &str) -> Result<String, String> {
    let full = std::fs::canonicalize(dir).map_err(|e| format!("{dir}: {e}"))?;
    // canonicalize returns a verbatim path on Windows; the sessions do not.
    let plain = paths::plain(&full.to_string_lossy());
    Ok(review::changes::toplevel(&plain).unwrap_or(plain))
}

/// The desktop executable shipped next to ars (installer and portable ZIP).
fn desktop_exe() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let name = format!("agent-review-studio{}", std::env::consts::EXE_SUFFIX);
    Some(dir.join(name)).filter(|p| p.is_file())
}

fn open_in_desktop(dir: &str) {
    let project = project_root_of(dir).unwrap_or_else(|e| {
        eprintln!("ars: {e}");
        std::process::exit(1);
    });
    let Some(exe) = desktop_exe() else {
        eprintln!(
            "ars: the desktop app was not found next to ars ({}). Install Agent Review Studio, or run the ars that ships with it.",
            std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        );
        std::process::exit(1);
    };
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--project")
        .arg(&project)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Detached: the window outlives this terminal command.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    match cmd.spawn() {
        Ok(_) => println!("ars: opening the review of {project}"),
        Err(e) => {
            eprintln!("ars: could not start {}: {e}", exe.display());
            std::process::exit(1);
        }
    }
}

fn under_project(s: &AgentSession, root: &str) -> bool {
    s.project_path
        .as_deref()
        .is_some_and(|p| paths::is_within(p, root))
}

/// Resolve a target to one session: index (1-based) or full/short id.
fn pick<'a>(sessions: &'a [AgentSession], target: Option<&str>) -> Option<&'a AgentSession> {
    if sessions.is_empty() {
        return None;
    }
    match target {
        None => sessions.first(),
        Some(t) => {
            if let Ok(idx) = t.parse::<usize>() {
                if idx == 0 {
                    eprintln!("ars: positions in `ars list` start at 1.");
                    std::process::exit(2);
                }
                return sessions.get(idx - 1);
            }
            if let Some(exact) = sessions.iter().find(|s| s.session_id == t) {
                return Some(exact);
            }
            // short_id is a prefix of the id, so a prefix match covers it.
            let matches: Vec<&AgentSession> = sessions
                .iter()
                .filter(|s| s.session_id.starts_with(t))
                .collect();
            if matches.len() > 1 {
                eprintln!("ars: \"{t}\" matches {} sessions:", matches.len());
                for s in &matches {
                    eprintln!("  {}  {}", s.session_id, s.project_name);
                }
                eprintln!("ars: use a longer id prefix.");
                std::process::exit(2);
            }
            matches.first().copied()
        }
    }
}

fn fail(target: Option<&str>) -> ! {
    match target {
        Some(t) => eprintln!(
            "ars: \"{t}\" is neither a folder nor a session. Run `ars list` to see the ids."
        ),
        None => eprintln!("ars: no agent sessions found on this machine."),
    }
    std::process::exit(1);
}

async fn post_flow(review: &Value, body: &str, args: &Args) {
    let token = github::gh_token();
    // Remote and branch from the work tree when it is here. A saved review
    // on a CI runner without it falls back to --repo / GITHUB_REPOSITORY
    // and the branch the review recorded.
    let root = review["project"]["root"].as_str().unwrap_or("");
    let (remote_url, git_branch) = if !root.is_empty() && Path::new(root).is_dir() {
        git::remote_and_branch(root)
    } else {
        (None, None)
    };
    let branch = git_branch.or_else(|| {
        review["project"]["branch"]
            .as_str()
            .filter(|b| !b.is_empty() && !b.starts_with('('))
            .map(str::to_string)
    });
    let target = match github::resolve_target(
        args.repo.as_deref(),
        args.pr,
        remote_url.as_deref(),
        branch.as_deref(),
        token.as_deref(),
        args.dry_run,
    )
    .await
    {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ars: {e}");
            std::process::exit(1);
        }
    };

    if args.dry_run {
        let num = target
            .number
            .map(|n| format!("#{n}"))
            .unwrap_or_else(|| "(found from the branch when posting)".to_string());
        println!(
            "\n  ars pr --dry-run (nothing posted)\n  target  {}/{} · PR {}\n  branch  {}\n  token   {}\n\n  ── comment body ──\n",
            target.owner,
            target.repo,
            num,
            branch.as_deref().unwrap_or("(unknown)"),
            if token.is_some() { "present" } else { "missing (set GH_TOKEN to post)" }
        );
        print!("{body}");
        return;
    }

    let token = match token {
        Some(t) => t,
        None => {
            eprintln!(
                "ars: no token to post with. Set GH_TOKEN (or GITHUB_TOKEN), or use --dry-run."
            );
            std::process::exit(1);
        }
    };
    match github::upsert_comment(&token, &target, body).await {
        Ok((action, url)) => println!("ars: {action} the review comment → {url}"),
        Err(e) => {
            eprintln!("ars: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_capsule;

    #[test]
    fn capsules_are_review_json() {
        assert!(parse_capsule(r#"{"project":{"name":"x"},"files":[]}"#).is_ok());
        assert!(parse_capsule(r#"{"sessionId":"old session capsule"}"#).is_err());
        assert!(parse_capsule("not json").is_err());
    }
}
