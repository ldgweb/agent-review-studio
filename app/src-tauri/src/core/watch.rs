// Watches the project under review, the way VS Code's Git extension does:
// a recursive file watcher on the work tree, debounced, and after each
// quiet period a cheap change signature (`git status` + HEAD). The window
// is told only when the signature moved, so build output churning in an
// ignored folder costs a `git status`, never a rebuild of the review.
//
// .git internals are ignored except what moves HEAD (HEAD itself, refs):
// the index changes whenever git refreshes it, and staging does not change
// a review of HEAD against the working tree.

use super::events;
use notify::Watcher;
use once_cell::sync::Lazy;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(300);

struct Watch {
    root: String,
    _watcher: notify::RecommendedWatcher,
}

static CURRENT: Lazy<Mutex<Option<Watch>>> = Lazy::new(|| Mutex::new(None));
/// Bumped on every new watch: an older loop sees it and stops.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn relevant(path: &std::path::Path) -> bool {
    let p = path.to_string_lossy().replace('\\', "/");
    match p.find("/.git/") {
        None => !p.ends_with("/.git"),
        Some(i) => {
            let inner = &p[i + 6..];
            inner == "HEAD" || inner.starts_with("refs/") || inner == "packed-refs"
        }
    }
}

fn signature(root: &str) -> Option<String> {
    let status = super::git::git(
        root,
        &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    );
    if !status.ok {
        return None;
    }
    let head = super::review::changes::head_of(root).unwrap_or_default();
    // Content edits keep a file "modified": its size and mtime tell them apart.
    let mut stamp = String::new();
    for path in super::git::status_paths(&status.out) {
        if let Ok(meta) = std::fs::metadata(std::path::Path::new(root).join(path)) {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
                .unwrap_or(0);
            stamp.push_str(&format!("{}:{}:{};", path, meta.len(), mtime));
        }
    }
    Some(format!("{head}\n{}\n{stamp}", status.out))
}

/// Watch `root` (a work tree root) for the desktop window, replacing any
/// previous watch. Idempotent for the project already watched.
pub fn watch_project(root: &str) {
    let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if current.as_ref().is_some_and(|w| w.root == root) {
        return;
    }
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let last_event: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let sink = last_event.clone();
    let watcher =
        notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
            if let Ok(event) = event {
                if event.paths.iter().any(|p| relevant(p)) {
                    *sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                }
            }
        });
    let Ok(mut watcher) = watcher else {
        *current = None;
        return;
    };
    if watcher
        .watch(std::path::Path::new(root), notify::RecursiveMode::Recursive)
        .is_err()
    {
        *current = None;
        return;
    }
    *current = Some(Watch {
        root: root.to_string(),
        _watcher: watcher,
    });
    let root = root.to_string();
    std::thread::Builder::new()
        .name("ars-project-watch".into())
        .spawn(move || {
            let mut last = signature(&root);
            loop {
                std::thread::sleep(Duration::from_millis(150));
                if GENERATION.load(Ordering::SeqCst) != generation {
                    return;
                }
                let due = {
                    let mut at = last_event.lock().unwrap_or_else(|e| e.into_inner());
                    match *at {
                        Some(t) if t.elapsed() >= DEBOUNCE => {
                            *at = None;
                            true
                        }
                        _ => false,
                    }
                };
                if !due {
                    continue;
                }
                let now = signature(&root);
                if now != last {
                    last = now;
                    events::emit("ars://review-changed", json!({ "root": root }));
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::relevant;
    use std::path::Path;

    #[test]
    fn git_internals_count_only_when_they_move_head() {
        assert!(relevant(Path::new("C:/p/src/a.ts")));
        assert!(relevant(Path::new("C:/p/.git/HEAD")));
        assert!(relevant(Path::new("C:/p/.git/refs/heads/main")));
        assert!(!relevant(Path::new("C:/p/.git/index")));
        assert!(!relevant(Path::new("C:/p/.git/objects/ab/cdef")));
        assert!(relevant(Path::new("C:/p/.github/workflows/ci.yml")));
    }
}
