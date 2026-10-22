// How filesystem paths compare on this platform.
//
// Windows (NTFS) and macOS (APFS/HFS+ as formatted by default) resolve paths
// case-insensitively: `C:/Dev/App` and `c:/dev/app` are one project there.
// On Linux they are two different projects. Every comparison of filesystem
// paths (project roots, `--project`, path digests in session ids) goes
// through this module so the rule lives in one place. It is for paths only:
// agent names, file extensions, regexes and git hosts keep their own rules.
//
// Two primitives, because callers need different guarantees:
//   - `path_eq` / `strip_root` fold ASCII only. Folding keeps byte lengths,
//     so a match can be sliced at the same offset (drive letters and folder
//     names, the common case).
//   - `fold_case` lowercases fully (Unicode) for keys and digest input. The
//     digests feed persisted session ids, so on case-insensitive platforms it
//     must stay exactly the `to_lowercase` those ids were built with.

use std::borrow::Cow;

/// True where the default filesystem resolves paths case-insensitively.
pub const PATH_CASE_INSENSITIVE: bool = cfg!(any(windows, target_os = "macos"));

/// `path` as a comparison key or digest input: lowercased where paths are
/// case-insensitive, unchanged where case is significant.
pub fn fold_case(path: &str) -> Cow<'_, str> {
    if PATH_CASE_INSENSITIVE {
        Cow::Owned(path.to_lowercase())
    } else {
        Cow::Borrowed(path)
    }
}

/// Whether two paths (or path segments) name the same entry under this
/// platform's case rule. ASCII-only folding: byte lengths never change.
pub fn path_eq(a: &str, b: &str) -> bool {
    if PATH_CASE_INSENSITIVE {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// The part of forward-slash `path` below `root`, when `path` lives inside
/// `root` (a `/` must follow the root, so `/a/app2` is not inside `/a/app`).
pub fn strip_root<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    if root.is_empty() {
        return None;
    }
    let head = path.get(..root.len())?;
    if !path_eq(head, root) {
        return None;
    }
    path[root.len()..].strip_prefix('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_matches_hold_everywhere() {
        assert!(path_eq("/home/a/app", "/home/a/app"));
        assert!(!path_eq("/home/a/app", "/home/a/app2"));
        assert_eq!(
            strip_root("/home/a/app/src/x.ts", "/home/a/app"),
            Some("src/x.ts")
        );
        assert_eq!(strip_root("/home/a/app2/x.ts", "/home/a/app"), None);
        assert_eq!(strip_root("/home/a/app", "/home/a/app"), None);
        assert_eq!(strip_root("/home/a/app/x", ""), None);
        assert_eq!(fold_case("/home/a/app"), "/home/a/app");
    }

    #[test]
    fn non_ascii_case_pairs_never_mis_slice() {
        // 'İ' (2 bytes) lowercases to "i̇" (3 bytes).
        assert_eq!(strip_root("c:/i̇x/a.ts", "c:/İx"), None);
        assert_eq!(strip_root("c:/İx/a.ts", "c:/i̇x"), None);
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn case_is_ignored_where_the_filesystem_ignores_it() {
        assert!(path_eq("C:/Dev/App", "c:/dev/app"));
        assert!(path_eq("/Users/a/App", "/users/a/app"));
        assert_eq!(strip_root("C:/DEV/APP/a.ts", "c:/dev/app"), Some("a.ts"));
        assert_eq!(fold_case("/Users/a/Café"), "/users/a/café");
        // Full Unicode folding for keys, as the persisted ids were built.
        assert_eq!(fold_case("C:/ÉTÉ"), fold_case("c:/été"));
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn case_matters_where_the_filesystem_keeps_it() {
        assert!(!path_eq("/home/a/App", "/home/a/app"));
        assert_eq!(strip_root("/home/a/App/a.ts", "/home/a/app"), None);
        assert_eq!(fold_case("/home/a/App"), "/home/a/App");
        assert_ne!(fold_case("/home/a/App"), fold_case("/home/a/app"));
    }
}
