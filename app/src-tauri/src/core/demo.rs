// The sample tour — everything ARS can do, demonstrated on REAL machinery.
//
// One click installs:
//   - a sandbox git repo (~/.agent-review-studio/demo/demo-shop) with a base
//     commit and uncommitted working-tree changes → the Git surface is fully
//     alive: working-tree diffs, commit of approved paths, revert, the graph;
//   - TWO ars/v1 sessions flowing through the real pipeline (normalize, risk
//     triage, local findings), so the "never re-tested" signal shows too:
//       ars-demo-tour  — fresh, waiting for the user's verdicts;
//       ars-demo-fix   — ALREADY reviewed: per-file verdicts, line comments,
//                        a corrections draft, outcome recorded (Apply stage).
//
// Nothing is mocked in the UI: archiving the samples is the ordinary archive
// action, re-installing un-archives and resets the sandbox working tree.

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

/// (path, base content committed to HEAD, modified content left uncommitted)
/// — the uncommitted state matches the sessions' transcript diffs, so the
/// working-tree enrichment shows the same story the transcripts tell.
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
        "export interface RetryOptions {\n  attempts: number;\n  baseDelayMs: number;\n}\n\nexport async function withRetry(run: () => Promise<Response>, opts: RetryOptions): Promise<Response> {\n  for (let i = 0; i < opts.attempts; i++) {\n    const res = await run().catch(() => null);\n    if (res && res.ok) return res;\n    await sleep(opts.baseDelayMs * 2 ** i);\n  }\n  throw new Error('request failed after retries');\n}\n",
    ),
    (
        "docs/api.md",
        Some("# API client\n\n## Requests\n\nUse request(path, init) everywhere.\n\n## Errors\n\nThe client throws after the last attempt.\nRequests are retried up to 3 times.\n"),
        "# API client\n\n## Requests\n\nUse request(path, init) everywhere.\n\n## Errors\n\nThe client throws after the last attempt.\nRequests are retried up to 5 times with exponential backoff.\nBackoff never exceeds 30 seconds between attempts.\n",
    ),
];

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
    // re-running the tour is deterministic even after commits/reverts.
    for (rel, _, modified) in FILES.iter() {
        write_file(&root, rel, modified)?;
    }
    Ok(root_str)
}

/// The reviewed sample's approval state — what a finished review looks like:
/// verdicts in all three colors, two line comments, a corrections note, and
/// the recorded outcome (so the workbench opens in the Apply stage).
fn seed_reviewed_approval() -> Result<(), String> {
    let session = scanner::session_by_id(REVIEWED_ID).ok_or("Sample session not indexed")?;
    let raw = serde_json::to_value(&*session).map_err(|e| e.to_string())?;
    let state = json!({
        "sessionId": REVIEWED_ID,
        "reviewRevision": super::git::review_revision(&raw)?,
        "contentHash": super::normalize::session_hash(&session),
        "fileStates": { "lf-1": "approved", "lf-2": "correction-requested", "lf-3": "rejected" },
        "correctionNotes": {
            "lf-2": "Cap the backoff — the docs promise ≤30s between attempts — and add jitter so clients don't retry in lockstep."
        },
        "lineComments": {
            "lf-2": [{ "anchor": "0:9", "line": 10, "text": "200ms · 2^i grows unbounded — this passes 30s around attempt 8. Cap it." }],
            "lf-3": [{ "anchor": "0:5", "line": 34, "text": "This promises a cap the code doesn't implement yet — fix retry.ts first." }]
        },
        "findingStates": {},
        "rejectedCategories": [],
        "sessionDecision": "corrections-requested"
    });
    storage::save_approval(REVIEWED_ID, &state)
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

/// Install (or reset) the whole tour. Returns the fresh session's id so the
/// UI can jump straight into the review.
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

    // The samples arrive through the custom (ars/v1) source — switch it on
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
    if let Err(e) = seed_reviewed_approval() {
        return json!({"error": e});
    }
    json!({ "ok": true, "sessionId": FRESH_ID, "reviewedSessionId": REVIEWED_ID })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Machine-dependent smoke — run explicitly with `cargo test -- --ignored`.
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

        let approvals = storage::get_approvals().expect("approvals readable");
        let seeded = approvals.get(REVIEWED_ID).expect("seeded approval missing");
        assert_eq!(
            seeded.get("sessionDecision").and_then(Value::as_str),
            Some("corrections-requested")
        );
    }
}
