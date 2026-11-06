// User configuration for the ingestion layer (~/.agent-review-studio/config.json).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AdapterConfigFile {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_dirs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub max_sessions: usize,
    pub adapters: BTreeMap<String, AdapterConfigFile>,
    /// Custom review checks (Settings → Review checks). Kept as raw JSON:
    /// rules::parse_rules validates them when they run, so a hand-edited
    /// config with an unknown field never fails to load.
    #[serde(default)]
    pub finding_rules: Vec<Value>,
}

pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".agent-review-studio")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// A VS Code-family editor's per-user folder: `%APPDATA%` on Windows,
/// `~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` (usually
/// `~/.config`) on Linux.
fn editor_user_dir(app: &str, sub: &str) -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join(app)
        .join("User")
        .join(sub)
}

fn default_cursor_dir() -> PathBuf {
    editor_user_dir("Cursor", "globalStorage")
}

fn default_copilot_dir() -> PathBuf {
    editor_user_dir("Code", "workspaceStorage")
}

fn path_str(p: PathBuf) -> String {
    p.to_string_lossy().to_string()
}

pub fn defaults() -> AppConfig {
    let home = dirs::home_dir().unwrap_or_default();
    let mut adapters = BTreeMap::new();
    adapters.insert(
        "claude-code".into(),
        AdapterConfigFile {
            enabled: true,
            dir: Some(path_str(home.join(".claude").join("projects"))),
            ..Default::default()
        },
    );
    adapters.insert(
        "codex".into(),
        AdapterConfigFile {
            enabled: true,
            dir: Some(path_str(home.join(".codex").join("sessions"))),
            ..Default::default()
        },
    );
    adapters.insert(
        "cursor".into(),
        AdapterConfigFile {
            enabled: true,
            dir: Some(path_str(default_cursor_dir())),
            ..Default::default()
        },
    );
    adapters.insert(
        "copilot".into(),
        AdapterConfigFile {
            enabled: true,
            dir: Some(path_str(default_copilot_dir())),
            ..Default::default()
        },
    );
    adapters.insert(
        "aider".into(),
        AdapterConfigFile {
            enabled: true,
            project_dirs: Some(Vec::new()),
            ..Default::default()
        },
    );
    adapters.insert(
        "gemini-cli".into(),
        AdapterConfigFile {
            enabled: true,
            dir: Some(path_str(home.join(".gemini").join("tmp"))),
            ..Default::default()
        },
    );
    adapters.insert(
        "custom".into(),
        AdapterConfigFile {
            enabled: true,
            dirs: Some(vec![path_str(config_dir().join("sessions"))]),
            ..Default::default()
        },
    );
    AppConfig {
        max_sessions: 30,
        adapters,
        finding_rules: Vec::new(),
    }
}

/// The config file merged over the defaults. Ok(None) when there is no file
/// yet; Err (naming the file) when it exists but cannot be read or is not a
/// JSON object. Callers must not overwrite such a file.
fn read_config_file() -> Result<Option<AppConfig>, String> {
    read_config_at(&config_path())
}

fn read_config_at(path: &Path) -> Result<Option<AppConfig>, String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Could not read {}: {e}", path.display())),
    };
    let parsed = match serde_json::from_str::<Value>(&raw) {
        Ok(v) if v.is_object() => v,
        Ok(_) => {
            return Err(format!(
                "{} is not a JSON object. Fix or remove it; settings are not saved over it.",
                path.display()
            ))
        }
        Err(e) => {
            return Err(format!(
                "{} is not valid JSON ({e}). Fix or remove it; settings are not saved over it.",
                path.display()
            ))
        }
    };
    let mut config = defaults();
    if let Some(n) = parsed.get("maxSessions").and_then(Value::as_u64) {
        config.max_sessions = n as usize;
    }
    if let Some(adapters) = parsed.get("adapters").and_then(Value::as_object) {
        for (id, raw) in adapters {
            if let Ok(cfg) = serde_json::from_value::<AdapterConfigFile>(raw.clone()) {
                config.adapters.insert(id.clone(), cfg);
            }
        }
    }
    if let Some(rules) = parsed.get("findingRules").and_then(Value::as_array) {
        config.finding_rules = rules.clone();
    }
    Ok(Some(config))
}

/// Why the config file is currently being ignored (unreadable or malformed),
/// if it is.
pub fn config_error() -> Option<String> {
    read_config_file().err()
}

/// The effective configuration. Reads degrade gracefully: a malformed file
/// yields the defaults (see `config_error`), and a missing one is created.
pub fn load_config() -> AppConfig {
    match read_config_file() {
        Ok(Some(config)) => config,
        Ok(None) => {
            let _ = fs::create_dir_all(config_dir().join("sessions"));
            let defaults = defaults();
            // Best effort: the next save reports a write failure.
            let _ = write_config(&defaults);
            defaults
        }
        Err(_) => defaults(),
    }
}

/// Write the config atomically (temp file + rename), so a failed write never
/// leaves a truncated config.json behind.
fn write_config(config: &AppConfig) -> Result<(), String> {
    write_config_at(&config_path(), config)
}

fn write_config_at(path: &Path, config: &AppConfig) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or("The config path has no parent folder.")?;
    fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)
        .and_then(|_| fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("Could not save {}: {e}", path.display())
        })
}

/// Merge `patch` into the saved config and write it. Refuses to overwrite a
/// config file that exists but does not parse.
pub fn save_config(patch: &Value) -> Result<AppConfig, String> {
    save_config_at(&config_path(), patch)
}

fn save_config_at(path: &Path, patch: &Value) -> Result<AppConfig, String> {
    let mut next = read_config_at(path)?.unwrap_or_else(defaults);
    if let Some(n) = patch.get("maxSessions").and_then(Value::as_u64) {
        next.max_sessions = n as usize;
    }
    if let Some(adapters) = patch.get("adapters").and_then(Value::as_object) {
        for (id, raw) in adapters {
            let mut merged = next.adapters.get(id).cloned().unwrap_or_default();
            if let Some(enabled) = raw.get("enabled").and_then(Value::as_bool) {
                merged.enabled = enabled;
            }
            if let Some(dir) = raw.get("dir").and_then(Value::as_str) {
                merged.dir = Some(dir.to_string());
            }
            if let Some(list) = raw.get("projectDirs").and_then(Value::as_array) {
                merged.project_dirs = Some(
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect(),
                );
            }
            if let Some(list) = raw.get("dirs").and_then(Value::as_array) {
                merged.dirs = Some(
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect(),
                );
            }
            next.adapters.insert(id.clone(), merged);
        }
    }
    if let Some(rules) = patch.get("findingRules").and_then(Value::as_array) {
        next.finding_rules = rules.clone();
    }
    write_config_at(path, &next)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config() -> PathBuf {
        // Tests run in parallel and macOS clocks tick in microseconds: the
        // counter keeps each test's folder its own.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("ars-config-{}-{stamp}-{seq}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("config.json")
    }

    #[test]
    fn malformed_config_is_never_overwritten() {
        let path = temp_config();
        fs::write(&path, "{ not json").unwrap();
        let err = save_config_at(&path, &serde_json::json!({ "maxSessions": 5 })).unwrap_err();
        assert!(err.contains("config.json"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn save_merges_and_persists() {
        let path = temp_config();
        let saved = save_config_at(&path, &serde_json::json!({ "maxSessions": 7 })).unwrap();
        assert_eq!(saved.max_sessions, 7);
        let reread = read_config_at(&path).unwrap().unwrap();
        assert_eq!(reread.max_sessions, 7);
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn failed_write_is_reported() {
        let base = temp_config();
        // The config folder is actually a file: the write cannot happen.
        let blocker = base.with_file_name("not-a-folder");
        fs::write(&blocker, "").unwrap();
        let path = blocker.join("config.json");
        let err = save_config_at(&path, &serde_json::json!({ "maxSessions": 3 })).unwrap_err();
        assert!(err.starts_with("Could not"), "{err}");
        let _ = fs::remove_dir_all(base.parent().unwrap());
    }
}
