// ars — the native CLI, on the same core the desktop app runs.
//
// It calls the exact router the desktop uses (app_core::api::handle) and
// deserializes the typed sessions, so the CLI can never diverge from the app.

use agent_review_studio::app_core::{api, domain::AgentSession, paths};
use agent_review_studio::cli::render::SavedReview;
use agent_review_studio::cli::{github, render};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::Value;

const USAGE: &str = "\
ars — review what your AI coding agents did, from the terminal

USAGE
  ars <command> [target] [flags]

COMMANDS
  list, ls             List reviewable agent sessions (default)
  review, r [id]       Terminal review of one session (latest when omitted)
  pr [id]              Markdown review context for the pull request

TARGET
  A 1-based index from `ars list`, a full or short session id, or nothing
  for the most recent session.

FLAGS
  --json               review/pr: print the session capsule as JSON — a
                       full-fidelity export for --from, NOT redacted: it
                       holds the raw transcript (prompts, diffs, any secrets
                       in them). Do not post or share it. It includes
                       your saved review (verdicts, line comments, notes)
                       under `review` when it was saved at the session's
                       current revision.
  --from <capsule>     review/pr: read the session from a capsule file
  --project <path>     Only sessions under this project path
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
                eprintln!("ars: unknown flag \"{other}\" — run `ars --help`");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    if pos.len() > 2 {
        eprintln!("ars: unexpected argument \"{}\" — run `ars --help`", pos[2]);
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

    // Reject unknown commands BEFORE the scan — a full source scan is not
    // free, and a typo should not pay for it.
    if !matches!(args.cmd.as_str(), "list" | "ls" | "review" | "r" | "pr") {
        eprintln!("ars: unknown command \"{}\" — run `ars --help`", args.cmd);
        std::process::exit(2);
    }

    // A capsule (--from) is a serialized session; it skips the machine scan.
    let visible: Vec<AgentSession> = if args.from.is_some() {
        Vec::new()
    } else {
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
    };

    match args.cmd.as_str() {
        "list" | "ls" => print!("{}", render::render_list(&visible)),

        "review" | "r" => {
            let (s, review) = resolve_full_session(&visible, &args).await;
            if args.json {
                print_capsule(&s, &review);
            } else {
                print!("{}", render::render_review(&s, &review));
            }
        }

        "pr" => {
            let (s, review) = resolve_full_session(&visible, &args).await;
            if args.json {
                print_capsule(&s, &review);
                return;
            }
            let body = render::render_pr_markdown(&s, &review);
            if args.post || args.dry_run {
                post_flow(&s, &body, &args).await;
            } else {
                print!("{body}");
            }
        }

        _ => unreachable!("command validated before the scan"),
    }
}

/// review/pr need the FULL session: from a capsule file, or picked from the
/// summary list and then fetched by id (the list carries no hunks/timeline).
/// Alongside it, the reviewer's saved review, classified against the
/// revision the detail route computed for this session.
async fn resolve_full_session(
    visible: &[AgentSession],
    args: &Args,
) -> (AgentSession, SavedReview) {
    if let Some(path) = &args.from {
        return load_capsule(path);
    }
    let summary = match pick(visible, args.target.as_deref()) {
        Some(s) => s,
        None => fail(args.target.as_deref()),
    };
    let encoded = utf8_percent_encode(&summary.session_id, NON_ALPHANUMERIC).to_string();
    let v = api::handle(&format!("/sessions/{encoded}"), "GET", None).await;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        eprintln!("ars: {e}");
        std::process::exit(1);
    }
    let session = match v
        .get("session")
        .cloned()
        .map(serde_json::from_value::<AgentSession>)
    {
        Some(Ok(s)) => s,
        _ => {
            eprintln!("ars: unexpected response from the core (no session detail).");
            std::process::exit(1);
        }
    };
    let revision = v["session"]["reviewRevision"].as_str();
    let review = saved_review(&session.session_id, revision).await;
    (session, review)
}

/// The saved review of one session through the same /approvals route the
/// desktop uses. A store that cannot be read is reported, not hidden.
async fn saved_review(session_id: &str, current_revision: Option<&str>) -> SavedReview {
    let v = api::handle("/approvals", "GET", None).await;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        eprintln!("ars: warning: your saved review could not be read: {e}");
        return SavedReview::None;
    }
    SavedReview::classify(v["approvals"].get(session_id), current_revision)
}

/// The capsule: the session, plus the saved review under "review" only when
/// it was saved at the current revision (a stale one is never exported).
fn capsule_value(s: &AgentSession, review: &SavedReview) -> Value {
    let mut value = serde_json::to_value(s).unwrap_or_default();
    if let (Some(r), Some(map)) = (review.current(), value.as_object_mut()) {
        map.insert("review".into(), r.clone());
    }
    value
}

fn print_capsule(s: &AgentSession, review: &SavedReview) {
    println!(
        "{}",
        serde_json::to_string_pretty(&capsule_value(s, review)).unwrap_or_default()
    );
}

fn under_project(s: &AgentSession, root: &str) -> bool {
    let p = match &s.project_path {
        Some(p) => norm(p),
        None => return false,
    };
    let r = norm(root);
    p == r || p.starts_with(&format!("{r}/"))
}

/// Comparison key for a project path: forward slashes, no trailing slash,
/// case folded only where the platform's paths are case-insensitive.
fn norm(p: &str) -> String {
    let slashed = p.replace('\\', "/");
    paths::fold_case(slashed.trim_end_matches('/')).into_owned()
}

fn load_capsule(path: &str) -> (AgentSession, SavedReview) {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("ars: could not read capsule {path}: {e}");
        std::process::exit(1);
    });
    parse_capsule(&text).unwrap_or_else(|e| {
        eprintln!("ars: {path} is not a session capsule (expected `ars review <id> --json`): {e}");
        std::process::exit(1);
    })
}

/// A capsule is a serialized session with an optional "review", present
/// only when a review was saved at the session's current revision.
fn parse_capsule(text: &str) -> Result<(AgentSession, SavedReview), String> {
    let mut value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let review = value.as_object_mut().and_then(|m| m.remove("review"));
    let session = serde_json::from_value::<AgentSession>(value).map_err(|e| e.to_string())?;
    let review = review.filter(|r| {
        let same = r["sessionId"].as_str() == Some(session.session_id.as_str());
        if !same {
            eprintln!("ars: warning: the capsule's review belongs to another session; ignored.");
        }
        same
    });
    let revision = review
        .as_ref()
        .and_then(|r| r["reviewRevision"].as_str())
        .map(String::from);
    let review = SavedReview::classify(review.as_ref(), revision.as_deref());
    Ok((session, review))
}

/// Resolve a target to one session: index (1-based), full/short id, or latest.
fn pick<'a>(sessions: &'a [AgentSession], target: Option<&str>) -> Option<&'a AgentSession> {
    if sessions.is_empty() {
        return None;
    }
    match target {
        None => sessions.first(),
        Some(t) => {
            if let Ok(idx) = t.parse::<usize>() {
                if idx == 0 {
                    eprintln!("ars: session indexes are 1-based — run `ars list`.");
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
        Some(t) => eprintln!("ars: no session matches \"{t}\" — run `ars list` to see the ids."),
        None => eprintln!("ars: no agent sessions found on this machine."),
    }
    std::process::exit(1);
}

async fn post_flow(s: &AgentSession, body: &str, args: &Args) {
    let token = github::gh_token();
    // Remote + branch come from the core's git status — reuses the desktop's
    // git module, so a capsule / CI runner without a work tree just yields none
    // and owner/repo falls back to --repo / GITHUB_REPOSITORY.
    let (remote_url, git_branch) = repo_remote_and_branch(&s.session_id).await;
    let branch = git_branch.filter(|b| !b.is_empty()).or_else(|| {
        if s.branch.is_empty() {
            None
        } else {
            Some(s.branch.clone())
        }
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
            .unwrap_or_else(|| "(resolve by branch at post time)".to_string());
        println!(
            "\n  ars pr --dry-run (nothing posted)\n  target  {}/{} · PR {}\n  branch  {}\n  token   {}\n\n  ── comment body ──\n",
            target.owner,
            target.repo,
            num,
            branch.as_deref().unwrap_or("(unknown)"),
            if token.is_some() { "present" } else { "missing — set GH_TOKEN to post" }
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

async fn repo_remote_and_branch(session_id: &str) -> (Option<String>, Option<String>) {
    let body = serde_json::json!({ "sessionId": session_id }).to_string();
    let v = api::handle("/git/status", "POST", Some(body.as_str())).await;
    (
        v.get("remoteUrl")
            .and_then(|x| x.as_str())
            .map(String::from),
        v.get("branch").and_then(|x| x.as_str()).map(String::from),
    )
}

#[cfg(test)]
mod tests {
    use super::{capsule_value, norm, parse_capsule, AgentSession, SavedReview};
    use serde_json::{json, Value};

    fn fixture_session() -> AgentSession {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/custom/expected/basic.json");
        let text = std::fs::read_to_string(&path).expect("fixture readable");
        let wrapper: Value = serde_json::from_str(&text).expect("fixture parses");
        serde_json::from_value(wrapper["session"].clone()).expect("fixture is a session")
    }

    fn saved(session_id: &str) -> Value {
        json!({
            "sessionId": session_id,
            "reviewRevision": "rev-1",
            "fileStates": { "lf-1": "approved" },
            "correctionNotes": {},
            "lineComments": { "lf-1": [{ "anchor": "0:0", "line": 1, "text": "ok" }] },
            "sessionDecision": "approved"
        })
    }

    #[test]
    fn capsule_carries_only_a_current_review_and_from_reads_it() {
        let s = fixture_session();
        let review = SavedReview::classify(Some(&saved(&s.session_id)), Some("rev-1"));
        let capsule = capsule_value(&s, &review);
        assert_eq!(capsule["review"]["sessionDecision"], "approved");
        let (back, back_review) = parse_capsule(&capsule.to_string()).expect("capsule parses");
        assert_eq!(back.session_id, s.session_id);
        assert_eq!(back_review, review);
        let md = agent_review_studio::cli::render::render_pr_markdown(&back, &back_review);
        assert!(md.contains("**Session decision:** Approved"), "{md}");

        // A stale review is never exported.
        let stale = SavedReview::classify(Some(&saved(&s.session_id)), Some("rev-2"));
        assert!(capsule_value(&s, &stale).get("review").is_none());
    }

    #[test]
    fn old_capsules_and_foreign_reviews_carry_no_review() {
        let s = fixture_session();
        let old = serde_json::to_string(&s).unwrap();
        let (_, review) = parse_capsule(&old).expect("old capsule parses");
        assert_eq!(review, SavedReview::None);

        let mut foreign = serde_json::to_value(&s).unwrap();
        foreign["review"] = saved("another-session");
        let (_, review) = parse_capsule(&foreign.to_string()).expect("capsule parses");
        assert_eq!(review, SavedReview::None);
    }

    #[test]
    fn project_keys_normalize_separators_and_trailing_slashes() {
        assert_eq!(norm("C:\\dev\\app\\"), norm("C:/dev/app"));
        assert_eq!(norm("/home/a/app/"), "/home/a/app");
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn project_keys_ignore_case_where_paths_do() {
        assert_eq!(norm("C:/Dev/App"), "c:/dev/app");
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn project_keys_keep_case_on_linux() {
        assert_ne!(norm("/home/a/App"), norm("/home/a/app"));
    }
}
