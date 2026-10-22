// Test-only access to the shared fixture contract in the repo-root fixtures/.
//
// An expected file is `{"sessionHash": ..., "session": ...}`, 2-space pretty
// JSON with a trailing newline. To create or refresh expected files after an
// intentional behavior change, run from app/src-tauri:
//
//     ARS_UPDATE_FIXTURES=1 cargo test --release --locked
//
// Update mode only writes files whose content actually differs, so unchanged
// fixtures are never reformatted. Review the resulting diff before committing.

use super::domain::AgentSession;
use super::normalize::session_hash;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

const UPDATE_ENV: &str = "ARS_UPDATE_FIXTURES";

fn path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
}

/// Read `fixtures/<rel>` as text, panicking with the full path on failure.
pub(crate) fn read(rel: &str) -> String {
    let path = path(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {}", path.display(), e))
}

/// Compare `session` with the expected file `fixtures/<expected_rel>` (its
/// serialized `session` and its `sessionHash`). With ARS_UPDATE_FIXTURES=1,
/// (re)write the file instead when its content differs.
pub(crate) fn assert_session(session: &AgentSession, expected_rel: &str) {
    let actual = json!({
        "sessionHash": session_hash(session),
        "session": serde_json::to_value(session).expect("serialize session"),
    });
    if std::env::var(UPDATE_ENV).is_ok_and(|v| v == "1") {
        update(&actual, expected_rel);
        return;
    }
    let hint = format!(
        "fixtures/{expected_rel} does not match the parser output. If the fixture is \
         new or the change is intentional, regenerate it from app/src-tauri with \
         `{UPDATE_ENV}=1 cargo test --release --locked` and review the diff."
    );
    let raw = std::fs::read_to_string(path(expected_rel))
        .unwrap_or_else(|e| panic!("cannot read fixtures/{expected_rel}: {e}\n{hint}"));
    let expected: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("fixtures/{expected_rel} is not valid JSON: {e}\n{hint}"));
    assert_eq!(actual["session"], expected["session"], "{hint}");
    assert_eq!(actual["sessionHash"], expected["sessionHash"], "{hint}");
}

fn update(actual: &Value, expected_rel: &str) {
    let path = path(expected_rel);
    let current = existing(expected_rel);
    if current.as_ref() == Some(actual) {
        return;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("create {}: {}", parent.display(), e));
    }
    let text = render(actual, current.as_ref());
    std::fs::write(&path, text).unwrap_or_else(|e| panic!("write {}: {}", path.display(), e));
}

/// The expected file's current content, if it exists and parses.
fn existing(expected_rel: &str) -> Option<Value> {
    let raw = std::fs::read_to_string(path(expected_rel)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 2-space pretty JSON plus a trailing newline. Keys follow the order of the
/// file being replaced, so a rewrite only shows the values that changed.
fn render(actual: &Value, current: Option<&Value>) -> String {
    serde_json::to_string_pretty(&ordered_like(actual, current)).expect("render expected json")
        + "\n"
}

fn ordered_like(value: &Value, template: Option<&Value>) -> Value {
    match (value, template) {
        (Value::Object(map), Some(Value::Object(order))) => {
            let mut out = Map::new();
            for key in order.keys() {
                if let Some(v) = map.get(key) {
                    out.insert(key.clone(), ordered_like(v, order.get(key)));
                }
            }
            for (key, v) in map {
                if !out.contains_key(key) {
                    out.insert(key.clone(), v.clone());
                }
            }
            Value::Object(out)
        }
        (Value::Array(items), Some(Value::Array(order))) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(i, v)| ordered_like(v, order.get(i)))
                .collect(),
        ),
        _ => value.clone(),
    }
}
