// Repo policy (.ars/policy.json): finalize thresholds. Also the best-effort
// secret redaction applied to CLI exports.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// (policy.json mtime_ms, parsed policy)
type CachedPolicy = (Option<u128>, Option<Value>);

/// project root → cached policy. The detail route reads the policy on every
/// fetch; this avoids a disk read per call.
static POLICY_CACHE: Lazy<Mutex<HashMap<String, CachedPolicy>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn load_policy_cached(project_path: Option<&str>) -> Option<Value> {
    let root = project_path?;
    if root.is_empty() {
        return None;
    }
    let file = Path::new(root).join(".ars").join("policy.json");
    let mtime: Option<u128> = std::fs::metadata(&file)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis());
    if let Ok(cache) = POLICY_CACHE.lock() {
        if let Some((cached_mtime, cached)) = cache.get(root) {
            if *cached_mtime == mtime {
                return cached.clone();
            }
        }
    }
    let policy = load_policy(Some(root));
    if let Ok(mut cache) = POLICY_CACHE.lock() {
        cache.insert(root.to_string(), (mtime, policy.clone()));
    }
    policy
}

fn load_policy(project_path: Option<&str>) -> Option<Value> {
    let root = project_path?;
    if root.is_empty() {
        return None;
    }
    let raw = std::fs::read_to_string(Path::new(root).join(".ars").join("policy.json")).ok()?;
    match serde_json::from_str::<Value>(&raw) {
        Ok(parsed) => {
            let mut policy = Map::new();
            if let Some(thresholds) = parsed
                .get("blockFinalizeAboveRisk")
                .and_then(Value::as_object)
            {
                let mut clean = Map::new();
                for (dim, value) in thresholds {
                    if value.is_number() {
                        clean.insert(dim.clone(), value.clone());
                    }
                }
                policy.insert("blockFinalizeAboveRisk".into(), Value::Object(clean));
            }
            if policy.is_empty() {
                None
            } else {
                Some(Value::Object(policy))
            }
        }
        // A malformed policy must be loud, not silently permissive
        Err(_) => Some(json!({ "error": "Malformed .ars/policy.json — fix it or remove it." })),
    }
}

// ---------------------------------------------------------------------------
// DLP redaction
// ---------------------------------------------------------------------------

static RE_PRIVATE_KEY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"-----BEGIN [A-Z ]+PRIVATE KEY-----[\s\S]*?-----END [A-Z ]+PRIVATE KEY-----")
        .unwrap()
});
static RE_AWS_KEY: Lazy<Regex> = Lazy::new(|| Regex::new(r"AKIA[0-9A-Z]{16}").unwrap());
static RE_BEARER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[Bb]earer\s+[A-Za-z0-9_.=-]{16,}").unwrap());
static RE_KEY_VALUE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(api[_-]?key|apikey|secret|token|password|passwd|authorization)(["']?\s*[:=]\s*["']?)([A-Za-z0-9_./+=-]{12,})"#)
        .unwrap()
});

pub fn redact_secrets(text: &str) -> String {
    let step1 = RE_PRIVATE_KEY.replace_all(text, "[REDACTED-PRIVATE-KEY]");
    let step2 = RE_AWS_KEY.replace_all(&step1, "[REDACTED-AWS-KEY]");
    let step3 = RE_BEARER.replace_all(&step2, "Bearer [REDACTED]");
    RE_KEY_VALUE
        .replace_all(&step3, "${1}${2}[REDACTED]")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "config: api_key = \"abcd1234efgh5678ijkl\"\naws AKIAABCDEFGHIJKLMNOP end\nAuthorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6\n-----BEGIN RSA PRIVATE KEY-----\nMIIEabc\n-----END RSA PRIVATE KEY-----\nplain line survives";
    const EXPECTED: &str = "config: api_key = \"[REDACTED]\"\naws [REDACTED-AWS-KEY] end\nAuthorization: Bearer [REDACTED]\n[REDACTED-PRIVATE-KEY]\nplain line survives";

    #[test]
    fn redacts_the_shared_sample_exactly() {
        assert_eq!(redact_secrets(SAMPLE), EXPECTED);
    }
}
