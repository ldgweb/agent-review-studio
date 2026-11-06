// Where a name is defined, for the peek over the diff: the lines that look
// like its definition (`function name`, `fn name`, `class Name`,
// `def name`, a method header…), found by name with `git grep` in the
// work tree, read-only. Each comes whole: the comment above it and the
// definition down to the end of its body (by braces, or by indentation in
// Python), cut at MAX_BODY_LINES. Only files of the same language family
// as the file being read count.
//
// It goes by name, not by meaning: two things with the same name both
// show up, and a definition no pattern knows is not found. The peek says
// so; it never claims to have resolved the reference. Libraries are not
// searched: resolving those is what a language server does.

use super::super::git::{checked_file, git_raw};
use super::changes;
use serde::Serialize;
use serde_json::{json, Value};

/// Matches returned, best first
const MAX_MATCHES: usize = 5;
/// Lines of `git grep` output read at most
const MAX_GREP_LINES: usize = 400;
/// A longer line is minified or generated code, not a definition to read
const MAX_LINE_CHARS: usize = 300;
/// Comment lines kept above a definition
const MAX_DOC_LINES: usize = 12;
/// Lines shown of one definition, and lines read looking for its end
const MAX_BODY_LINES: usize = 150;
const MAX_SCAN_LINES: usize = 5_000;
/// A header that has not opened its body by then is a statement
const MAX_HEAD_LINES: usize = 12;
/// Files larger than this are not read for the snippet
const MAX_FILE_BYTES: u64 = 2_000_000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub path: String,
    /// 1-based
    pub line: usize,
    /// The comment right above it (doc comment, decorators)
    pub doc: Vec<String>,
    /// The definition, down to the end of its body
    pub code: Vec<String>,
    /// Lines of it past MAX_BODY_LINES, not shown
    #[serde(skip_serializing_if = "is_zero")]
    pub more: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A name worth looking up: an identifier, nothing a pattern could abuse.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && name.len() <= 80
}

/// Extended regexes (POSIX, as `git grep -E` reads them) for the lines
/// that define `name`, strongest kind first.
fn patterns(name: &str) -> [String; 3] {
    let n = name.replace('$', "\\$");
    let edge = "([^[:alnum:]_$]|$)";
    [
        // A keyword then the name: function, fn, def, class, struct, …
        format!(
            "(^|[^[:alnum:]_$])(function[*]?|fn|def|class|interface|type|struct|enum|trait|func|record|module|macro_rules!)[[:space:]]+{n}{edge}"
        ),
        // A Go method, or a method header in a class or object literal:
        // the name, its parameters and the opening brace on one line
        format!(
            "^[[:space:]]*(func[[:space:]]*\\([^)]*\\)[[:space:]]*{n}[[:space:]]*\\(|(((public|private|protected|static|async|override|abstract|virtual|export|default|pub|get|set)[[:space:]]+)*{n}[[:space:]]*(<[^>]*>)?\\([^;]*\\)[^;]*\\{{[[:space:]]*$))"
        ),
        // A constant or variable
        format!("(^|[^[:alnum:]_$])(const|let|var|val|static)[[:space:]]+(mut[[:space:]]+)?{n}{edge}"),
    ]
}

/// How strong a matched line is: 0 a definition keyword, 1 a method
/// header, 2 a variable.
fn strength(text: &str, name: &str) -> usize {
    let words: Vec<&str> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '!'))
        .filter(|w| !w.is_empty())
        .collect();
    let at = words.iter().position(|w| *w == name).unwrap_or(0);
    let before = at.checked_sub(1).map(|i| words[i]).unwrap_or("");
    match before {
        "const" | "let" | "var" | "val" | "static" | "mut" => 2,
        "function" | "fn" | "def" | "class" | "interface" | "type" | "struct" | "enum"
        | "trait" | "func" | "record" | "module" | "macro_rules!" => 0,
        _ => 1,
    }
}

/// A comment, or what sits above a definition like one: a decorator, a
/// Rust attribute
fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    ["//", "/*", "*", "#", "--", "@", "\"\"\"", "'''"]
        .iter()
        .any(|p| t.starts_with(p))
}

/// Braces a line opens and closes, outside strings and `//` comments.
/// In Rust a single quote is a lifetime unless it is a char literal.
fn braces(line: &str, rust: bool) -> (usize, usize) {
    let chars: Vec<char> = line.chars().collect();
    let (mut open, mut close) = (0, 0);
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                if c == '\\' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '/' if chars.get(i + 1) == Some(&'/') => break,
                '"' | '`' => quote = Some(c),
                '\'' if !rust => quote = Some(c),
                // A char literal: 'x', or an escape such as '\n'
                '\'' if chars.get(i + 2) == Some(&'\'') => i += 2,
                '\'' if chars.get(i + 1) == Some(&'\\') && chars.get(i + 3) == Some(&'\'') => {
                    i += 3
                }
                '{' => open += 1,
                '}' => close += 1,
                _ => {}
            },
        }
        i += 1;
    }
    (open, close)
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// How many lines from `at` the definition takes: to the brace that closes
/// its body, or (Python) to the last line indented under it. A header that
/// opens no body is a statement: it ends at its `;`, or where the next line
/// is no deeper than it.
fn extent(lines: &[&str], at: usize, lang: &str) -> usize {
    let end = lines.len().min(at + MAX_SCAN_LINES);
    let base = indent(lines[at]);
    if lang == "py" || lang == "pyi" {
        // The header may span lines: it ends at the line ending with ':'
        let Some(head) =
            (at..end.min(at + MAX_HEAD_LINES)).find(|&j| lines[j].trim_end().ends_with(':'))
        else {
            return 1;
        };
        let mut last = head;
        for (j, l) in lines.iter().enumerate().take(end).skip(head + 1) {
            if l.trim().is_empty() {
                continue;
            }
            if indent(l) <= base {
                break;
            }
            last = j;
        }
        return last - at + 1;
    }
    let rust = lang == "rs";
    let (mut depth, mut opened) = (0usize, false);
    for j in at..end {
        let (open, close) = braces(lines[j], rust);
        opened |= open > 0;
        depth = (depth + open).saturating_sub(close);
        if opened && depth == 0 {
            return j - at + 1;
        }
        if !opened {
            let ends = lines[j].trim_end().ends_with(';');
            let next_shallow = lines
                .get(j + 1)
                .is_none_or(|n| !n.trim().is_empty() && indent(n) <= base);
            if ends
                || j - at + 1 >= MAX_HEAD_LINES
                || (next_shallow
                    && !lines[j]
                        .trim_end()
                        .ends_with(['(', ',', '=', '>', '|', '&']))
            {
                return j - at + 1;
            }
        }
    }
    end - at
}

/// The comment above line `at` (0-based), the definition (at most
/// MAX_BODY_LINES of it) and how many of its lines are left out.
fn snippet(lines: &[&str], at: usize, lang: &str) -> (Vec<String>, Vec<String>, usize) {
    let mut start = at;
    while start > 0 && at - start < MAX_DOC_LINES && is_comment(lines[start - 1]) {
        start -= 1;
    }
    let doc = lines[start..at].iter().map(|l| l.to_string()).collect();
    let len = extent(lines, at, lang);
    let shown = len.min(MAX_BODY_LINES);
    let code = lines[at..at + shown]
        .iter()
        .map(|l| l.to_string())
        .collect();
    (doc, code, len - shown)
}

/// The definitions of `name` in the project, best first: in `from` (the
/// file being read) before others, then by kind, then by path.
pub fn definitions(project: &str, name: &str, from: &str) -> Value {
    if !valid_name(name) {
        return json!({ "error": "Not a name to look up." });
    }
    let root = match changes::toplevel(project) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let [p0, p1, p2] = patterns(name);
    let out = git_raw(
        &root,
        &[
            "grep",
            "-n",
            "-z",
            "-I",
            "-E",
            "--untracked",
            "--no-color",
            "-e",
            &p0,
            "-e",
            &p1,
            "-e",
            &p2,
        ],
        None,
    );
    // Exit 1 is "nothing found"; anything else that failed is an error
    if !out.ok && !out.err.trim().is_empty() {
        return json!({ "error": format!("Could not search the project: {}", out.err.trim()) });
    }
    let text = String::from_utf8_lossy(&out.out);
    let mut hits: Vec<(usize, String, usize)> = Vec::new();
    for raw in text.lines().take(MAX_GREP_LINES) {
        let mut parts = raw.splitn(3, '\0');
        let (Some(path), Some(line), Some(code)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Ok(line) = line.parse::<usize>() else {
            continue;
        };
        if code.chars().count() > MAX_LINE_CHARS || path.ends_with(".min.js") {
            continue;
        }
        // A name in a TypeScript file is not defined by a Rust file
        if family(path) != family(from) && family(from).is_some() {
            continue;
        }
        hits.push((strength(code, name), path.to_string(), line));
    }
    let total = hits.len();
    hits.sort_by(|a, b| (a.1 != from, a.0, &a.1, a.2).cmp(&(b.1 != from, b.0, &b.1, b.2)));
    let mut found: Vec<Definition> = Vec::new();
    for (_, path, line) in hits.into_iter().take(MAX_MATCHES) {
        let Ok(full) = checked_file(&root, &path) else {
            continue;
        };
        if std::fs::metadata(&full)
            .map(|m| m.len())
            .unwrap_or(u64::MAX)
            > MAX_FILE_BYTES
        {
            continue;
        }
        let Ok(raw) = std::fs::read(&full) else {
            continue;
        };
        let content = String::from_utf8_lossy(&raw).replace('\r', "");
        let lines: Vec<&str> = content.split('\n').collect();
        if line == 0 || line > lines.len() {
            continue;
        }
        let lang = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let (doc, code, more) = snippet(&lines, line - 1, &lang);
        found.push(Definition {
            path,
            line,
            doc,
            code,
            more,
        });
    }
    json!({ "name": name, "total": total, "matches": found })
}

/// Languages that can define each other's names: TypeScript and
/// JavaScript, C and C++… None for anything else (no filter).
fn family(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    Some(match ext.as_str() {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" | "vue" | "svelte" => "js",
        "py" | "pyi" => "py",
        "rs" => "rs",
        "go" => "go",
        "java" | "kt" | "kts" | "scala" => "jvm",
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" => "c",
        "cs" => "cs",
        "rb" => "rb",
        "php" => "php",
        "swift" => "swift",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_identifiers_only() {
        assert!(valid_name("toCents"));
        assert!(valid_name("$store"));
        assert!(valid_name("_private1"));
        assert!(!valid_name("1abc"));
        assert!(!valid_name("a.b"));
        assert!(!valid_name("a|b"));
        assert!(!valid_name(""));
    }

    #[test]
    fn a_keyword_definition_ranks_before_a_method_and_a_variable() {
        assert_eq!(strength("export function toCents(amount) {", "toCents"), 0);
        assert_eq!(
            strength("pub fn handback(project: &str) -> Value {", "handback"),
            0
        );
        assert_eq!(strength("  async retry(opts) {", "retry"), 1);
        assert_eq!(strength("const limit = 10;", "limit"), 2);
    }

    #[test]
    fn the_snippet_is_the_comment_above_and_the_whole_body() {
        let lines = [
            "import x from 'y';",
            "",
            "/**",
            " * Rounds to cents.",
            " */",
            "export function toCents(",
            "  amount: number,",
            "): number {",
            "  const s = '}';",
            "  if (amount) { return 0; }",
            "  return Math.round(amount * 100) / 100; // }",
            "}",
            "",
            "export const other = 1;",
        ];
        let (doc, code, more) = snippet(&lines, 5, "ts");
        assert_eq!(doc, vec!["/**", " * Rounds to cents.", " */"]);
        assert_eq!(
            code.len(),
            7,
            "header and body, braces in strings and comments ignored"
        );
        assert_eq!(code.last().map(String::as_str), Some("}"));
        assert_eq!(more, 0);
        // A statement ends at its semicolon
        assert_eq!(snippet(&lines, 13, "ts").1, vec!["export const other = 1;"]);
    }

    #[test]
    fn rust_lifetimes_and_char_literals_do_not_hide_braces() {
        let lines = [
            "pub fn first<'a>(s: &'a str) -> &'a str {",
            "    let open = '{';",
            "    s.split(' ').next().unwrap_or(\"\")",
            "}",
            "fn after() {}",
        ];
        assert_eq!(snippet(&lines, 0, "rs").1.len(), 4);
    }

    #[test]
    fn python_bodies_end_where_the_indentation_does() {
        let lines = [
            "def total(",
            "    items,",
            "):",
            "    \"\"\"Sum them.\"\"\"",
            "",
            "    return sum(items)",
            "",
            "def other():",
            "    pass",
        ];
        assert_eq!(snippet(&lines, 0, "py").1.len(), 6);
    }

    #[test]
    fn a_long_body_is_cut_and_says_how_much_is_left() {
        let mut lines = vec!["function big() {".to_string()];
        lines.extend((0..200).map(|i| format!("  step({i});")));
        lines.push("}".into());
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let (_, code, more) = snippet(&refs, 0, "js");
        assert_eq!(code.len(), MAX_BODY_LINES);
        assert_eq!(more, 202 - MAX_BODY_LINES);
    }

    #[test]
    fn it_finds_definitions_by_name_in_the_work_tree() {
        let dir = std::env::temp_dir().join(format!("ars-def-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let root = dir.to_string_lossy().replace('\\', "/");
        assert!(git_raw(&root, &["init", "-q"], None).ok);
        std::fs::write(
            dir.join("src/cart.ts"),
            "// Rounds to cents.\nexport function toCents(a: number): number {\n  return a;\n}\nconst total = toCents(3);\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src/other.ts"),
            "class Cart {\n  toCents(a) {\n    return a;\n  }\n}\n",
        )
        .unwrap();
        // Same name in another language: not a definition for a .ts file
        std::fs::write(dir.join("src/money.rs"), "pub fn toCents() {}\n").unwrap();
        let v = definitions(&root, "toCents", "src/other.ts");
        let matches = v["matches"].as_array().unwrap();
        assert_eq!(v["total"], 2, "a call is not a definition: {v}");
        // The file being read comes first
        assert_eq!(matches[0]["path"], "src/other.ts");
        assert_eq!(matches[1]["path"], "src/cart.ts");
        assert_eq!(matches[1]["line"], 2);
        assert_eq!(matches[1]["doc"][0], "// Rounds to cents.");
        assert!(definitions(&root, "nothingLikeThis", "")["matches"]
            .as_array()
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
