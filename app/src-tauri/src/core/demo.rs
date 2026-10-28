// The sample tour: everything ARS can do, shown on the REAL machinery.
//
// One click installs:
//   - a sandbox git repo (~/.agent-review-studio/demo/demo-shop) with a base
//     commit and uncommitted working-tree changes, so its review is a real
//     project review: working-tree hunks and their attribution;
//   - TWO ars/v1 sessions that go through the real pipeline, so every hunk
//     carries the sample agent's words:
//       ars-demo-tour: the discount feature, waiting for verdicts;
//       ars-demo-fix: the retry work, ALREADY reviewed once. Round 1
//         approved client.ts and asked for changes to retry.ts (uncapped
//         backoff) and the docs. The agent has since capped the backoff,
//         so "since last review" shows just that.
//
// Nothing is mocked in the UI: re-installing resets the sandbox working tree
// and the sample review.

use super::{scanner, storage};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Stdio;

pub const FRESH_ID: &str = "ars-demo-tour";
pub const REVIEWED_ID: &str = "ars-demo-fix";

const FRESH_JSON: &str = include_str!("demo_session.json");
const REVIEWED_JSON: &str = include_str!("demo_session_reviewed.json");

fn run_git(root: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = super::git::git_command();
    cmd.arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// (path, base content committed to HEAD, modified content left uncommitted).
/// The uncommitted state matches the sessions' recorded edits, so every
/// change in the review is explained by them.
const FILES: [(&str, Option<&str>, &str); 8] = [
    (
        "src/auth/session.ts",
        Some("import { verify } from './verify';\n\nexport interface User {\n  id: string;\n  email: string;\n}\n\nexport interface Request {\n  cookies: { session: string };\n}\n\nexport function sessionUser(req: Request): User | null {\n  return verify(req.cookies.session);\n}\n"),
        "import { verify } from './verify';\n\nexport interface User {\n  id: string;\n  email: string;\n}\n\nexport interface Request {\n  cookies: { session: string };\n}\n\nexport function sessionUser(req: Request): User | null {\n  return verify(req.cookies.session);\n}\n\n// Validates a discount code for the signed-in user.\nexport async function validateDiscount(req: Request, code: string): Promise<Discount | null> {\n  const user = sessionUser(req);\n  if (!user) return null;\n  const apiKey = \"demo_not_a_real_key_0000\";\n  const found = await discountService.lookup(code, { apiKey });\n  return found && found.expiresAt > Date.now() ? found : null;\n}\n",
    ),
    (
        "db/migrations/2026_add_discounts.sql",
        None,
        "CREATE TABLE discounts (\n  id INTEGER PRIMARY KEY,\n  code TEXT NOT NULL UNIQUE,\n  kind TEXT NOT NULL CHECK (kind IN ('percent','fixed')),\n  value NUMERIC NOT NULL,\n  expires_at TIMESTAMP NOT NULL,\n  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP\n);\nCREATE INDEX idx_discounts_code ON discounts (code);\n",
    ),
    (
        "src/checkout/discount.ts",
        None,
        "import { validateDiscount } from '../auth/session';\n\n// TODO: move the rounding rules into config before launch\nexport async function applyDiscount(req: Request, cart: Cart, code: string): Promise<Cart> {\n  const discount = await validateDiscount(req, code);\n  if (!discount) return cart;\n  console.log('applying discount', code);\n  const total = discount.kind === 'percent'\n    ? cart.total * (1 - discount.value / 100)\n    : cart.total - discount.value;\n  return { ...cart, total: toCents(total), discountCode: code };\n}\n\nfunction toCents(amount: number): number {\n  return Math.round(amount * 100) / 100;\n}\n\nexport function describeDiscount(d: Discount): string {\n  return d.kind === 'percent' ? `${d.value}% off` : `${d.value} off`;\n}\n\nexport const DISCOUNT_FIELD = 'discount-code';\n",
    ),
    (
        "tests/checkout.test.ts",
        Some("import { total, cartWith, applyTestDiscount } from './helpers';\n\ndescribe('checkout totals', () => {\n  it('sums line items', () => {\n    expect(total(cartWith(2)).total).toBe(40);\n  });\n\n  it('applies tax per region', () => {\n    expect(total(cartWith(1), 'EU').total).toBe(24.4);\n  });\n\n  it('rejects negative quantities', () => {\n    expect(() => cartWith(-1)).toThrow();\n  });\n\n  it('keeps totals stable across currencies', () => {\n    expect(total(cartWith(3), 'US', 'usd').total).toBe(60);\n  });\n  it('never goes below zero', () => {\n    expect(applyTestDiscount(cartWith(1), 'HUGE').total).toBe(0);\n  });\n});\n"),
        "import { total, cartWith, applyTestDiscount } from './helpers';\n\ndescribe('checkout totals', () => {\n  it('applies a percent discount', () => {\n    expect(applyTestDiscount(cartWith(2), 'SAVE10').total).toBe(36);\n  });\n  it('never goes below zero', () => {\n    expect(applyTestDiscount(cartWith(1), 'HUGE').total).toBe(0);\n  });\n});\n",
    ),
    (
        "README.md",
        Some("# demo-shop\n\nA tiny storefront used by the Agent Review Studio sample tour.\n\n## Payments\n\nCheckout supports cards and wallets.\n"),
        "# demo-shop\n\nA tiny storefront used by the Agent Review Studio sample tour.\n\n## Payments\n\nCheckout supports cards and wallets.\n\n## Discount codes\nPercentage or fixed-amount, validated server-side, with expiry dates.\n",
    ),
    (
        "src/api/client.ts",
        Some("import { base } from './config';\n\nexport async function request(path: string, init?: RequestInit): Promise<Response> {\n  for (let attempt = 0; attempt < 3; attempt++) {\n    const res = await fetch(base + path, init);\n    if (res.ok) return res;\n  }\n  throw new Error('request failed');\n}\n"),
        "import { base } from './config';\nimport { withRetry } from './retry';\n\nexport async function request(path: string, init?: RequestInit): Promise<Response> {\n  return withRetry(() => fetch(base + path, init), {\n    attempts: 5,\n    baseDelayMs: 200,\n  });\n}\n",
    ),
    (
        "src/api/retry.ts",
        None,
        "export interface RetryOptions {\n  attempts: number;\n  baseDelayMs: number;\n}\n\n/** The docs promise at most 30 s between attempts. */\nconst MAX_DELAY_MS = 30_000;\n\nexport async function withRetry(run: () => Promise<Response>, opts: RetryOptions): Promise<Response> {\n  for (let i = 0; i < opts.attempts; i++) {\n    const res = await run().catch(() => null);\n    if (res && res.ok) return res;\n    const backoff = Math.min(MAX_DELAY_MS, opts.baseDelayMs * 2 ** i);\n    // Full jitter: clients that failed together do not retry in lockstep.\n    await sleep(Math.random() * backoff);\n  }\n  throw new Error('request failed after retries');\n}\n",
    ),
    (
        "docs/api.md",
        Some("# API client\n\n## Requests\n\nUse request(path, init) everywhere.\n\n## Errors\n\nThe client throws after the last attempt.\nRequests are retried up to 3 times.\n"),
        "# API client\n\n## Requests\n\nUse request(path, init) everywhere.\n\n## Errors\n\nThe client throws after the last attempt.\nRequests are retried up to 5 times with exponential backoff.\nBackoff never exceeds 30 seconds between attempts.\n",
    ),
];

/// retry.ts as the reviewer saw it in round 1, before the agent capped the
/// backoff: the base of "since last review".
const RETRY_AS_REVIEWED: &str = "export interface RetryOptions {\n  attempts: number;\n  baseDelayMs: number;\n}\n\nexport async function withRetry(run: () => Promise<Response>, opts: RetryOptions): Promise<Response> {\n  for (let i = 0; i < opts.attempts; i++) {\n    const res = await run().catch(() => null);\n    if (res && res.ok) return res;\n    await sleep(opts.baseDelayMs * 2 ** i);\n  }\n  throw new Error('request failed after retries');\n}\n";

fn write_file(root: &Path, rel: &str, content: &str) -> Result<(), String> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

fn ensure_sandbox() -> Result<String, String> {
    let root = super::config::config_dir().join("demo").join("demo-shop");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let root_str = root.to_string_lossy().replace('\\', "/");

    if !root.join(".git").exists() {
        run_git(&root_str, &["init", "-q"])?;
        // Repo-local identity: the sandbox must work even on machines with
        // no global git identity, and "you" should read as the sample user.
        run_git(&root_str, &["config", "user.name", "You (sample)"])?;
        run_git(&root_str, &["config", "user.email", "you@demo.local"])?;
        for (rel, base, _) in FILES.iter() {
            if let Some(content) = base {
                write_file(&root, rel, content)?;
            }
        }
        run_git(&root_str, &["add", "-A"])?;
        run_git(&root_str, &["commit", "-q", "-m", "chore: base demo-shop"])?;
    }

    // Always reset the working tree to the "agent just worked here" state so
    // re-running the tour is deterministic even after commits.
    for (rel, _, modified) in FILES.iter() {
        write_file(&root, rel, modified)?;
    }
    Ok(root_str)
}

/// The demo project's review after round 1 (see the header).
fn seed_review(root: &str) -> Result<(), String> {
    use super::review::changes::{self, LineKind};
    use super::review::state::{self, Comment, HunkVerdict, ReviewState, Round, Verdict};
    let cs = changes::collect(root)?;
    let hunks_of = |path: &str| -> Vec<&changes::Hunk> {
        cs.files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.hunks.iter().collect())
            .unwrap_or_default()
    };
    let at = "2026-07-19T16:00:00.000Z".to_string();
    let mut s = ReviewState {
        version: state::STATE_VERSION,
        ..ReviewState::default()
    };
    for (path, verdict) in [
        ("src/api/client.ts", Verdict::Approved),
        ("docs/api.md", Verdict::Changes),
    ] {
        for h in hunks_of(path) {
            s.hunks.insert(
                h.id.clone(),
                HunkVerdict {
                    verdict,
                    path: path.into(),
                    at: at.clone(),
                },
            );
        }
    }

    // Round 1's snapshot: retry.ts before the fix, the rest as it is now.
    let content = |path: &str| {
        FILES
            .iter()
            .find(|f| f.0 == path)
            .map(|f| f.2)
            .unwrap_or("")
    };
    let mut files = std::collections::BTreeMap::new();
    for (path, text) in [
        ("src/api/client.ts", content("src/api/client.ts")),
        ("docs/api.md", content("docs/api.md")),
        ("src/api/retry.ts", RETRY_AS_REVIEWED),
    ] {
        let hash = state::blob_hash(text.as_bytes());
        storage::put_blob(&hash, text.as_bytes())?;
        files.insert(path.to_string(), Some(hash));
    }
    let retry_hunk = changes::hunk_id(
        "src/api/retry.ts",
        RETRY_AS_REVIEWED.lines().map(|l| (LineKind::Add, l)),
    );
    let sleep_line = RETRY_AS_REVIEWED
        .lines()
        .position(|l| l.contains("await sleep"))
        .unwrap_or(0);
    let docs = hunks_of("docs/api.md");
    let docs_hunk = docs.first().ok_or("The sample docs change is missing.")?;
    let (docs_li, docs_line) = docs_hunk
        .lines
        .iter()
        .enumerate()
        .find(|(_, l)| l.text.starts_with("Backoff never exceeds"))
        .ok_or("The sample docs line is missing.")?;
    let comments = vec![
        Comment {
            id: "c-demo-1".into(),
            path: "src/api/retry.ts".into(),
            hunk_id: retry_hunk,
            li: sleep_line,
            kind: "add".into(),
            line_text: RETRY_AS_REVIEWED
                .lines()
                .nth(sleep_line)
                .unwrap_or("")
                .into(),
            line: Some(sleep_line as i64 + 1),
            text: "200ms · 2^i has no cap: it passes 30 s around attempt 8. Cap it.".into(),
            suggestion: None,
            at: at.clone(),
            resolved: false,
        },
        Comment {
            id: "c-demo-2".into(),
            path: "docs/api.md".into(),
            hunk_id: docs_hunk.id.clone(),
            li: docs_li,
            kind: "add".into(),
            line_text: docs_line.text.clone(),
            line: docs_line.new_line,
            text: "This promises a cap the code doesn't implement yet. Fix retry.ts first.".into(),
            suggestion: None,
            at: at.clone(),
            resolved: false,
        },
    ];
    let mut notes = std::collections::BTreeMap::new();
    notes.insert(
        "src/api/retry.ts".to_string(),
        "Cap the backoff: the docs promise at most 30 s between attempts. Add jitter too, so clients don't retry in lockstep.".to_string(),
    );
    // What round 1 sent the agent, in the review's own shape
    let project = super::review::ProjectInfo {
        root: root.to_string(),
        name: "demo-shop".into(),
        branch: cs.branch.clone(),
        head: cs.head.clone(),
    };
    let text = format!(
        "{}\n\n## src/api/retry.ts\n1. CHANGE this file: {}\n2. CHANGE line {} `{}`: {}\n\n## docs/api.md\n3. CHANGE line {} `{}`: {}\n",
        super::review::package::intro(&project, false),
        notes["src/api/retry.ts"],
        comments[0].line.unwrap_or(0),
        comments[0].line_text.trim(),
        comments[0].text,
        comments[1].line.unwrap_or(0),
        comments[1].line_text.trim(),
        comments[1].text,
    );
    s.rounds.push(Round {
        index: 1,
        at,
        head: cs.head.clone(),
        text,
        items: Some(3),
        files,
        comments,
        notes,
    });
    let _guard = state::lock();
    let (_, rev) = state::load(root)?;
    state::write(root, &s, rev)
        .map(|_| ())
        .map_err(|e| e.unwrap_or_else(|| "The sample review changed while installing.".into()))
}

fn install_session_file(dir: &str, id: &str, raw: &str, project_path: &str) -> Result<(), String> {
    let mut doc: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    if let Some(map) = doc.as_object_mut() {
        map.insert("projectPath".into(), json!(project_path));
    }
    let path = Path::new(dir).join(format!("{id}.json"));
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&doc).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())
}

/// Install (or reset) the whole tour. Returns the sandbox project so the UI
/// can open its review.
pub fn install() -> Value {
    let config = super::config::load_config();
    let sessions_dir = config
        .adapters
        .get("custom")
        .and_then(|c| c.dirs.as_ref())
        .and_then(|d| d.first().cloned())
        .unwrap_or_else(|| {
            super::config::config_dir()
                .join("sessions")
                .to_string_lossy()
                .to_string()
        });
    if let Err(e) = std::fs::create_dir_all(&sessions_dir) {
        return json!({ "error": format!("Could not create the sessions folder: {e}") });
    }

    let root = match ensure_sandbox() {
        Ok(r) => r,
        Err(e) => return json!({ "error": format!("Could not prepare the sandbox repo: {e}") }),
    };

    if let Err(e) = install_session_file(&sessions_dir, FRESH_ID, FRESH_JSON, &root)
        .and_then(|_| install_session_file(&sessions_dir, REVIEWED_ID, REVIEWED_JSON, &root))
    {
        return json!({ "error": format!("Could not write the sample sessions: {e}") });
    }

    // The samples arrive through the custom (ars/v1) source: switch it on
    // if the user had it disabled, or the click would do nothing.
    if !config
        .adapters
        .get("custom")
        .map(|c| c.enabled)
        .unwrap_or(false)
    {
        if let Err(e) =
            super::config::save_config(&json!({ "adapters": { "custom": { "enabled": true } } }))
        {
            return json!({ "error": format!("Could not enable the sample source: {e}") });
        }
    }

    if let Err(e) = storage::set_archived(&[FRESH_ID.to_string(), REVIEWED_ID.to_string()], false) {
        return json!({ "error": e });
    }
    scanner::rescan();
    if let Err(e) = seed_review(&root) {
        return json!({ "error": format!("Could not prepare the sample review: {e}") });
    }
    json!({ "ok": true, "project": root })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Machine-dependent smoke test: run explicitly with `cargo test -- --ignored`.
    /// Installs the real tour into ~/.agent-review-studio (same effect as the
    /// Welcome button) and asserts every seeded piece landed.
    #[test]
    #[ignore]
    fn smoke_install_tour() {
        let out = install();
        assert!(
            out.get("ok").and_then(Value::as_bool).unwrap_or(false),
            "install failed: {out}"
        );

        let root = super::super::config::config_dir()
            .join("demo")
            .join("demo-shop");
        assert!(root.join(".git").exists(), "sandbox repo missing");
        assert!(
            root.join("src/api/retry.ts").exists(),
            "modified files missing"
        );

        let review = super::super::review::build(out["project"].as_str().unwrap()).expect("review");
        assert_eq!(review.since_review.as_ref().map(|s| s.round), Some(1));
        let retry = review
            .files
            .iter()
            .find(|f| f.path == "src/api/retry.ts")
            .unwrap();
        assert_eq!(retry.since, Some("changed"));
    }
}
