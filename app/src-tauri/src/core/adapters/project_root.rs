// Shared path heuristics for adapters whose sources carry absolute file
// paths (Cursor, VS Code Copilot, Gemini CLI).

use super::super::paths::{path_eq, strip_root};
use std::path::Path;

/// Longest common directory prefix of a set of forward-slash paths (case
/// folded only where the platform's paths are case-insensitive).
pub fn common_root(paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let segmented: Vec<Vec<&str>> = paths
        .iter()
        .map(|p| {
            let mut segs: Vec<&str> = p.split('/').collect();
            segs.pop(); // drop the file name
            segs
        })
        .collect();
    let mut prefix: Vec<&str> = segmented[0].clone();
    for segs in segmented.iter().skip(1) {
        let mut i = 0;
        while i < prefix.len() && i < segs.len() && path_eq(prefix[i], segs[i]) {
            i += 1;
        }
        prefix.truncate(i);
    }
    prefix.join("/")
}

const PACKAGE_MARKERS: &[&str] = &[
    "package.json",
    "composer.json",
    "Cargo.toml",
    "pyproject.toml",
    "go.mod",
];

/// Walk up from a directory until a project marker is found; the git
/// repository wins over package manifests. Falls back to the input.
pub fn ascend_to_project_root(dir: &str) -> String {
    let mut ancestors: Vec<String> = Vec::new();
    let mut current = dir.to_string();
    for _ in 0..8 {
        if !current.contains('/') {
            break;
        }
        ancestors.push(current.clone());
        let parent = match current.rsplit_once('/') {
            Some((parent, _)) => parent.to_string(),
            None => break,
        };
        if parent.is_empty() || parent == current {
            break;
        }
        current = parent;
    }
    for candidate in &ancestors {
        if Path::new(candidate).join(".git").exists() {
            return candidate.clone();
        }
    }
    for candidate in &ancestors {
        if PACKAGE_MARKERS
            .iter()
            .any(|m| Path::new(candidate).join(m).exists())
        {
            return candidate.clone();
        }
    }
    dir.to_string()
}

/// Current branch of the repo at `root` (from .git/HEAD), or None.
pub fn git_branch_of(root: &str) -> Option<String> {
    let head = std::fs::read_to_string(Path::new(root).join(".git").join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(rest) = head.strip_prefix("ref: refs/heads/") {
        Some(rest.to_string())
    } else if head.is_empty() {
        None
    } else {
        Some("(detached)".to_string())
    }
}

/// Make `path` relative to `root` when it lives inside it (ASCII
/// case-insensitive where the platform's paths are, like Windows drive
/// letters and folder names; exact on Linux). Byte-wise comparison:
/// lowercasing non-ASCII text can change its byte length, so slicing at a
/// lowercased offset could mis-slice or panic.
pub fn relative_to(root: &str, path: &str) -> String {
    strip_root(path, root).unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_to_handles_non_ascii_case_pairs() {
        // 'İ' (2 bytes) lowercases to "i̇" (3 bytes): must neither panic nor
        // mis-slice.
        assert_eq!(relative_to("c:/İx", "c:/i̇x/a.ts"), "c:/i̇x/a.ts");
        assert_eq!(relative_to("c:/i̇x", "c:/İx/a.ts"), "c:/İx/a.ts");
        assert_eq!(
            relative_to("c:/dev/app", "c:/dev/app2/a.ts"),
            "c:/dev/app2/a.ts"
        );
        assert_eq!(
            relative_to("/home/a/Café", "/home/a/Café/src/a.ts"),
            "src/a.ts"
        );
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn relative_to_ignores_case_where_paths_do() {
        assert_eq!(
            relative_to("C:/Dev/Café", "c:/dev/Café/src/a.ts"),
            "src/a.ts"
        );
        assert_eq!(relative_to("c:/dev/app", "C:/DEV/APP/a.ts"), "a.ts");
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn relative_to_respects_case_on_linux() {
        assert_eq!(
            relative_to("/home/a/app", "/home/a/App/a.ts"),
            "/home/a/App/a.ts"
        );
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn common_root_is_case_insensitive() {
        let paths = vec![
            "C:/Dev/App/a.ts".to_string(),
            "c:/dev/app/b/c.ts".to_string(),
        ];
        assert_eq!(common_root(&paths), "C:/Dev/App");
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn common_root_respects_case_on_linux() {
        let paths = vec![
            "/home/a/App/a.ts".to_string(),
            "/home/a/app/b/c.ts".to_string(),
        ];
        assert_eq!(common_root(&paths), "/home/a");
    }
}
