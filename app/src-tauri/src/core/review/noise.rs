// What a reviewer should not have to read line by line, found by fixed
// rules: code that only moved (git's --color-moved idea: runs of removed
// lines added back elsewhere, indentation ignored), hunks that only change
// whitespace or line wrapping, generated files (lockfiles, minified bundles,
// and what .gitattributes marks linguist-generated or -diff, as GitHub
// does), and the same change repeated in many places.

use super::super::git::git_raw;
use super::changes::{hex, ChangedFile, Hunk, LineKind, MovedPeer};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// A moved block (git's rule): at least two lines carrying at least this
/// many letters or digits in total.
const MOVED_MIN_ALNUM: usize = 20;

fn alnum(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

/// Mark lines that only moved: a run of removed lines whose text (trimmed)
/// is added back somewhere in the changes, and vice versa. The first line
/// of each run points at the other half (where that text first appears).
pub fn mark_moved(files: &mut [ChangedFile]) {
    let mut removed: HashMap<String, MovedPeer> = HashMap::new();
    let mut added: HashMap<String, MovedPeer> = HashMap::new();
    for f in files.iter() {
        for h in &f.hunks {
            for l in &h.lines {
                let t = l.text.trim();
                if t.is_empty() {
                    continue;
                }
                let (side, line) = match l.kind {
                    LineKind::Del => (&mut removed, l.old_line),
                    LineKind::Add => (&mut added, l.new_line),
                    LineKind::Context => continue,
                };
                side.entry(t.to_string()).or_insert_with(|| MovedPeer {
                    path: f.path.clone(),
                    line: line.unwrap_or(0),
                });
            }
        }
    }
    if removed.is_empty() || added.is_empty() {
        return;
    }
    for f in files.iter_mut() {
        for h in &mut f.hunks {
            mark_runs(h, LineKind::Add, &removed);
            mark_runs(h, LineKind::Del, &added);
        }
    }
}

/// Mark each run of consecutive `kind` lines found on the other side
/// (blank lines ride along) that is big enough to count as a moved block.
fn mark_runs(h: &mut Hunk, kind: LineKind, other_side: &HashMap<String, MovedPeer>) {
    let candidate = |l: &super::changes::Line| {
        let t = l.text.trim();
        l.kind == kind && (t.is_empty() || other_side.contains_key(t))
    };
    let mut i = 0;
    while i < h.lines.len() {
        if !candidate(&h.lines[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < h.lines.len() && candidate(&h.lines[i]) {
            i += 1;
        }
        let run = &h.lines[start..i];
        let nonblank = run.iter().filter(|l| !l.text.trim().is_empty()).count();
        let letters: usize = run.iter().map(|l| alnum(&l.text)).sum();
        if nonblank >= 2 && letters >= MOVED_MIN_ALNUM {
            let peer = run
                .iter()
                .find_map(|l| other_side.get(l.text.trim()))
                .cloned();
            for l in &mut h.lines[start..i] {
                l.moved = true;
            }
            h.lines[start].moved_peer = peer;
        }
    }
}

/// Languages where indentation is syntax: there it is never noise.
const INDENT_SYNTAX: [&str; 6] = ["py", "yaml", "yml", "mk", "makefile", "nim"];

/// Why a hunk is noise: it only moved code, or only changed whitespace
/// (the same words in the same order; in a language where indentation is
/// syntax, only the line ends may differ).
pub fn hunk_noise(h: &Hunk, language: &str) -> Option<&'static str> {
    let changed: Vec<_> = h
        .lines
        .iter()
        .filter(|l| l.kind != LineKind::Context)
        .collect();
    if changed.is_empty() || h.truncated {
        return None;
    }
    if changed.iter().all(|l| l.moved || l.text.trim().is_empty())
        && changed.iter().any(|l| l.moved)
    {
        return Some("moved");
    }
    let has_both =
        h.changed(LineKind::Del).next().is_some() && h.changed(LineKind::Add).next().is_some();
    if !has_both {
        return None;
    }
    let same = if INDENT_SYNTAX.contains(&language.to_ascii_lowercase().as_str()) {
        let ends =
            |kind: LineKind| -> Vec<&str> { h.changed(kind).map(|l| l.text.trim_end()).collect() };
        ends(LineKind::Del) == ends(LineKind::Add)
    } else {
        let words = |kind: LineKind| -> Vec<&str> {
            h.changed(kind)
                .flat_map(|l| l.text.split_whitespace())
                .collect()
        };
        words(LineKind::Del) == words(LineKind::Add)
    };
    same.then_some("whitespace")
}

const LOCKFILES: [&str; 20] = [
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "bun.lock",
    "Cargo.lock",
    "Gemfile.lock",
    "composer.lock",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
    "go.sum",
    "flake.lock",
    "mix.lock",
    "pubspec.lock",
    "Podfile.lock",
    "packages.lock.json",
    "gradle.lockfile",
    "deno.lock",
];

/// Whether any attributes file can apply to `paths`: the repo's own
/// info/attributes, or a .gitattributes at the root or in a folder on the
/// way to a changed file. Without one, `git check-attr` (a whole process)
/// has nothing to say.
fn attributes_possible(root: &str, paths: &[&str]) -> bool {
    let root = std::path::Path::new(root);
    if root.join(".git").join("info").join("attributes").is_file() {
        return true;
    }
    let mut dirs: HashSet<&str> = HashSet::new();
    dirs.insert("");
    for p in paths {
        let mut end = 0;
        while let Some(i) = p[end..].find('/') {
            end += i;
            dirs.insert(&p[..end]);
            end += 1;
        }
    }
    dirs.iter()
        .any(|d| root.join(d).join(".gitattributes").is_file())
}

/// .gitattributes verdicts for `paths`: linguist-generated, or -diff.
pub fn generated_attrs(
    root: &str,
    paths: &[&str],
) -> Result<HashMap<String, &'static str>, String> {
    let mut out = HashMap::new();
    if paths.is_empty() || !attributes_possible(root, paths) {
        return Ok(out);
    }
    let input: Vec<u8> = paths
        .iter()
        .flat_map(|p| p.bytes().chain(std::iter::once(0)))
        .collect();
    let res = git_raw(
        root,
        &["check-attr", "-z", "--stdin", "linguist-generated", "diff"],
        Some(input),
    );
    if !res.ok {
        return Err("The .gitattributes rules could not be read, so generated files may show as ordinary ones.".into());
    }
    // -z output: <path> NUL <attribute> NUL <value> NUL, repeated
    let text = String::from_utf8_lossy(&res.out).to_string();
    let parts: Vec<&str> = text.split('\0').collect();
    for chunk in parts.chunks(3) {
        if let [path, attr, value] = chunk {
            let generated = match *attr {
                "linguist-generated" => matches!(*value, "set" | "true"),
                "diff" => *value == "unset",
                _ => false,
            };
            if generated {
                out.insert(
                    path.to_string(),
                    if *attr == "diff" {
                        "no-diff"
                    } else {
                        "linguist-generated"
                    },
                );
            }
        }
    }
    Ok(out)
}

/// Why a file is generated, from its name or its .gitattributes verdict.
pub fn generated_reason(path: &str, attrs: &HashMap<String, &'static str>) -> Option<&'static str> {
    if let Some(reason) = attrs.get(path) {
        return Some(reason);
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    if LOCKFILES.contains(&name) {
        return Some("lockfile");
    }
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".min.js") || lower.ends_with(".min.css") || lower.ends_with(".min.mjs") {
        return Some("minified");
    }
    if lower.ends_with(".js.map") || lower.ends_with(".css.map") {
        return Some("source-map");
    }
    None
}

/// The same change repeated: hunk id → group id, for changes (whitespace
/// ignored at the line ends) that appear in more than one place.
pub fn edit_groups(files: &[ChangedFile]) -> HashMap<String, String> {
    let mut by_key: HashMap<String, Vec<String>> = HashMap::new();
    for f in files {
        for h in &f.hunks {
            if h.truncated {
                continue;
            }
            let mut digest = Sha256::new();
            let mut letters = 0;
            for l in h.lines.iter().filter(|l| l.kind != LineKind::Context) {
                let t = l.text.trim();
                if t.is_empty() {
                    continue;
                }
                letters += alnum(t);
                digest.update([if l.kind == LineKind::Add { b'+' } else { b'-' }]);
                digest.update(t.as_bytes());
                digest.update(b"\n");
            }
            if letters < 3 {
                continue;
            }
            let key = hex(&digest.finalize()[..10]);
            by_key.entry(key).or_default().push(h.id.clone());
        }
    }
    let mut out = HashMap::new();
    for (key, ids) in by_key {
        // Hunk ids are unique, so two ids are two places.
        if ids.len() >= 2 {
            for id in ids {
                out.insert(id, format!("g-{key}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::changes::parse_diff;
    use super::*;

    #[test]
    fn moved_blocks_and_whitespace_hunks_are_noise() {
        let mut files = parse_diff(concat!(
            "diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -1,5 +1,2 @@\n top\n",
            "-function helper(x) {\n-  return compute(x);\n-}\n",
            "+// helper moved to b.ts\n end\n",
            "diff --git a/b.ts b/b.ts\n--- a/b.ts\n+++ b/b.ts\n@@ -1,1 +1,4 @@\n start\n",
            "+  function helper(x) {\n+    return compute(x);\n+  }\n",
            "diff --git a/c.ts b/c.ts\n--- a/c.ts\n+++ b/c.ts\n@@ -1,1 +1,2 @@\n",
            "-if (a) { return b; }\n+if (a) {\n+  return b; }\n",
        ));
        mark_moved(&mut files);
        assert_eq!(
            hunk_noise(&files[1].hunks[0], "ts"),
            Some("moved"),
            "re-indented move"
        );
        assert_eq!(
            hunk_noise(&files[0].hunks[0], "ts"),
            None,
            "the comment is a real change"
        );
        assert!(files[0].hunks[0].lines[1].moved);
        let peer = |path: &str, line| {
            Some(MovedPeer {
                path: path.into(),
                line,
            })
        };
        assert_eq!(
            files[0].hunks[0].lines[1].moved_peer,
            peer("b.ts", 2),
            "went to"
        );
        assert_eq!(
            files[1].hunks[0].lines[1].moved_peer,
            peer("a.ts", 2),
            "came from"
        );
        assert_eq!(
            files[0].hunks[0].lines[2].moved_peer, None,
            "only the first line"
        );
        assert_eq!(
            hunk_noise(&files[2].hunks[0], "ts"),
            Some("whitespace"),
            "re-wrapped"
        );
    }

    #[test]
    fn generated_files_by_name() {
        let none = HashMap::new();
        assert_eq!(
            generated_reason("app/package-lock.json", &none),
            Some("lockfile")
        );
        assert_eq!(generated_reason("Cargo.lock", &none), Some("lockfile"));
        assert_eq!(generated_reason("dist/app.min.js", &none), Some("minified"));
        assert_eq!(generated_reason("src/lock.ts", &none), None);
    }

    #[test]
    fn repeated_changes_form_a_group() {
        let files = parse_diff(concat!(
            "diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -1 +1 @@\n-import { old } from 'x';\n+import { renamed } from 'x';\n",
            "diff --git a/b.ts b/b.ts\n--- a/b.ts\n+++ b/b.ts\n@@ -9 +9 @@\n-  import { old } from 'x';\n+  import { renamed } from 'x';\n",
            "diff --git a/c.ts b/c.ts\n--- a/c.ts\n+++ b/c.ts\n@@ -1 +1 @@\n-x\n+y\n",
        ));
        let groups = edit_groups(&files);
        let a = &groups[&files[0].hunks[0].id];
        assert_eq!(a, &groups[&files[1].hunks[0].id]);
        assert!(!groups.contains_key(&files[2].hunks[0].id));
    }
}
