// GitHub posting for `ars pr --post`. Upserts one review-context comment per
// PR through the REST API, without depending on `gh`. Token from GH_TOKEN /
// GITHUB_TOKEN (GitHub Actions injects GITHUB_TOKEN).

use crate::app_core::git::{github_slug, parse_owner_repo};
use once_cell::sync::OnceCell;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::Method;
use serde_json::{json, Value};
use std::time::Duration;

pub const ARS_MARKER: &str = "<!-- ars-review -->";

/// GitHub rejects comment bodies over 65536 characters; stay safely below
/// it (counted in bytes, which is never fewer than characters).
const MAX_COMMENT_BYTES: usize = 65_000;
const PER_PAGE: usize = 100;
/// Comment pages scanned for our existing comment (100 per page).
const MAX_COMMENT_PAGES: usize = 50;

pub struct PrTarget {
    pub owner: String,
    pub repo: String,
    pub number: Option<u64>,
}

pub fn gh_token() -> Option<String> {
    std::env::var("GH_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("GITHUB_TOKEN").ok().filter(|s| !s.is_empty()))
}

fn event_pr_number() -> Option<u64> {
    let path = std::env::var("GITHUB_EVENT_PATH").ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("pull_request")
        .and_then(|p| p.get("number"))
        .and_then(|n| n.as_u64())
        .or_else(|| v.get("number").and_then(|n| n.as_u64()))
}

/// One shared client; every request times out after 30 seconds.
fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceCell<reqwest::Client> = OnceCell::new();
    CLIENT.get_or_try_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Could not start the HTTP client: {e}"))
    })
}

async fn api(
    token: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut req = client()?
        .request(method, format!("https://api.github.com{path}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "ars-cli");
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.map_err(|e| e.to_string())?;
    let status = res.status();
    let text = res.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let snippet: String = text.chars().take(200).collect();
        return Err(format!("GitHub API {path} → {status}: {snippet}"));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

async fn resolve_pr_number(
    token: &str,
    owner: &str,
    repo: &str,
    branch: &str,
) -> Result<Option<u64>, String> {
    let head = utf8_percent_encode(&format!("{owner}:{branch}"), NON_ALPHANUMERIC).to_string();
    let path = format!("/repos/{owner}/{repo}/pulls?head={head}&state=open&per_page=1");
    let v = api(token, Method::GET, &path, None).await?;
    Ok(v.as_array()
        .and_then(|a| a.first())
        .and_then(|p| p.get("number"))
        .and_then(|n| n.as_u64()))
}

/// owner/repo from `--repo`, else GITHUB_REPOSITORY, else the GitHub
/// "origin" remote, each validated by the same parser as the desktop app.
fn resolve_slug(
    repo_flag: Option<&str>,
    remote_url: Option<&str>,
) -> Result<(String, String), String> {
    if let Some(flag) = repo_flag {
        return parse_owner_repo(flag)
            .ok_or_else(|| format!("Invalid --repo \"{flag}\": expected <owner>/<name>."));
    }
    if let Ok(env) = std::env::var("GITHUB_REPOSITORY") {
        if !env.is_empty() {
            return parse_owner_repo(&env).ok_or_else(|| {
                format!("Invalid GITHUB_REPOSITORY \"{env}\": expected <owner>/<name>.")
            });
        }
    }
    remote_url.and_then(github_slug).ok_or_else(|| {
        "Could not determine owner/repo. Pass --repo <owner/name>, set GITHUB_REPOSITORY, or run inside a repo with a GitHub \"origin\" remote.".to_string()
    })
}

pub async fn resolve_target(
    repo_flag: Option<&str>,
    pr_flag: Option<u64>,
    remote_url: Option<&str>,
    branch: Option<&str>,
    token: Option<&str>,
    dry_run: bool,
) -> Result<PrTarget, String> {
    let (owner, repo) = resolve_slug(repo_flag, remote_url)?;
    let mut number = pr_flag.or_else(event_pr_number);
    if number.is_none() && !dry_run {
        let token = token.ok_or(
            "No PR number given and no token to resolve one. Pass --pr <n> or set GH_TOKEN.",
        )?;
        let branch = branch.ok_or("No branch to find the PR from. Pass --pr <n>.")?;
        number = resolve_pr_number(token, &owner, &repo, branch).await?;
        if number.is_none() {
            return Err(format!("No open PR found for branch \"{branch}\" in {owner}/{repo}. Push the branch and open a PR, or pass --pr <n>."));
        }
    }
    Ok(PrTarget {
        owner,
        repo,
        number,
    })
}

/// Login this token acts as: the PAT's user, or github-actions[bot] for the
/// installation token Actions injects (whose /user lookup fails).
async fn author_login(token: &str) -> Option<String> {
    if let Ok(v) = api(token, Method::GET, "/user", None).await {
        if let Some(login) = v.get("login").and_then(|l| l.as_str()) {
            return Some(login.to_string());
        }
    }
    if std::env::var("GITHUB_ACTIONS").ok().as_deref() == Some("true") {
        return Some("github-actions[bot]".to_string());
    }
    None
}

/// The marked comment body, cut (at a line break, on a char boundary) to
/// fit GitHub's size limit, with a note when anything was dropped.
pub fn comment_body(body: &str) -> String {
    const NOTE: &str = "\n\n_… truncated to fit GitHub's comment size limit. Run `ars review` locally for the full view._\n";
    let full = format!("{ARS_MARKER}\n{body}");
    if full.len() <= MAX_COMMENT_BYTES {
        return full;
    }
    let mut cut = MAX_COMMENT_BYTES - NOTE.len();
    while !full.is_char_boundary(cut) {
        cut -= 1;
    }
    let kept = &full[..cut];
    let kept = kept.rfind('\n').map_or(kept, |i| &kept[..i]);
    format!("{kept}{NOTE}")
}

/// Is this comment ours: carries the marker AND was written by `login`?
/// The marker alone can be spoofed by anyone on the PR to hijack the update.
fn is_ours(comment: &Value, login: Option<&str>) -> bool {
    let marked = comment
        .get("body")
        .and_then(|b| b.as_str())
        .is_some_and(|b| b.contains(ARS_MARKER));
    let author = comment
        .get("user")
        .and_then(|u| u.get("login"))
        .and_then(|l| l.as_str());
    marked && login.is_some() && author == login
}

/// Our existing comment's id, searching every page of the PR's comments.
async fn find_our_comment(
    token: &str,
    target: &PrTarget,
    number: u64,
) -> Result<Option<u64>, String> {
    let login = author_login(token).await;
    for page in 1..=MAX_COMMENT_PAGES {
        let path = format!(
            "/repos/{}/{}/issues/{}/comments?per_page={PER_PAGE}&page={page}",
            target.owner, target.repo, number
        );
        let listed = api(token, Method::GET, &path, None).await?;
        let comments = listed.as_array().cloned().unwrap_or_default();
        if let Some(c) = comments.iter().find(|c| is_ours(c, login.as_deref())) {
            return c
                .get("id")
                .and_then(|v| v.as_u64())
                .map(Some)
                .ok_or_else(|| "comment id missing".to_string());
        }
        if comments.len() < PER_PAGE {
            return Ok(None);
        }
    }
    Ok(None)
}

/// Create or update the single review-context comment on the PR.
pub async fn upsert_comment(
    token: &str,
    target: &PrTarget,
    body: &str,
) -> Result<(String, String), String> {
    let number = target.number.ok_or("No PR number resolved.")?;
    let payload = json!({ "body": comment_body(body) });
    let (action, response) = match find_our_comment(token, target, number).await? {
        Some(id) => {
            let path = format!(
                "/repos/{}/{}/issues/comments/{}",
                target.owner, target.repo, id
            );
            (
                "updated",
                api(token, Method::PATCH, &path, Some(payload)).await?,
            )
        }
        None => {
            let path = format!(
                "/repos/{}/{}/issues/{}/comments",
                target.owner, target.repo, number
            );
            (
                "created",
                api(token, Method::POST, &path, Some(payload)).await?,
            )
        }
    };
    Ok((
        action.to_string(),
        response
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_bodies_are_cut_below_the_limit_with_a_note() {
        let short = comment_body("hello");
        assert_eq!(short, format!("{ARS_MARKER}\nhello"));
        let line = "é".repeat(99) + "\n";
        let long = line.repeat(1000);
        let cut = comment_body(&long);
        assert!(cut.len() <= MAX_COMMENT_BYTES);
        assert!(cut.starts_with(ARS_MARKER));
        assert!(cut.contains("truncated"));
        // cut at a line break: every kept line is whole
        let kept = cut.split("\n\n_…").next().unwrap();
        assert!(kept.lines().skip(1).all(|l| l.chars().count() == 99));
    }

    #[test]
    fn only_marked_comments_by_this_login_are_ours() {
        let mine = json!({ "body": format!("{ARS_MARKER}\nx"), "user": { "login": "me" } });
        let spoof = json!({ "body": format!("{ARS_MARKER}\nx"), "user": { "login": "other" } });
        let plain = json!({ "body": "x", "user": { "login": "me" } });
        assert!(is_ours(&mine, Some("me")));
        assert!(!is_ours(&spoof, Some("me")));
        assert!(!is_ours(&plain, Some("me")));
        assert!(!is_ours(&mine, None));
    }

    #[test]
    fn repo_flag_is_validated() {
        assert_eq!(
            resolve_slug(Some("octo/repo"), None).unwrap(),
            ("octo".to_string(), "repo".to_string())
        );
        assert!(resolve_slug(Some("octo"), None).is_err());
        assert!(resolve_slug(Some("octo/re po"), None).is_err());
        assert!(resolve_slug(Some("../x"), None).is_err());
    }
}
