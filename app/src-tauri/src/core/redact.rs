// Best-effort secret redaction applied to CLI exports.

use once_cell::sync::Lazy;
use regex::Regex;

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
