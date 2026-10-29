# Repository guidance

Agent Review Studio is a beta workbench (Windows; macOS and Linux experimental) for human review of what coding agents change in a project, before commit. Keep it useful and small. Findings are deterministic heuristics; do not present them as measured probabilities or invent dependency graphs from folder names.

Read README.md for commands and SECURITY.md for trust boundaries. React/TypeScript lives in `app/src`; the shared Rust core, desktop and `ars` CLI live in `app/src-tauri`. The API is a Tauri command, not an HTTP server. A review is the project's uncommitted Git changes; transcripts only explain them (per-hunk attribution). Reviews are built on demand from Git and the project's own transcripts, never by waiting for a scan; background scanning owns the session index. The window is notified by events, never by polling. Labelled samples use a separate sandbox.

Run `npm test`, `npm run build` and `cargo test --release --locked` in their respective app/native directories. Run dependency audits and secret scans before release. Ignored smoke tests touch real user state and require explicit authorization.

Fixtures are synthetic. Change expected outputs only for intentional behavior changes. Session hashes cover full serialized content. Decisions bind to content: hunk verdicts and comments to the hunk's content id, file verdicts to HEAD's blob plus the current bytes. Preserve these checks on save and export paths.

Use literal Git paths and validate containment. ARS never writes to a repository: Git runs read-only with `GIT_OPTIONAL_LOCKS=0`, and a rejection becomes an instruction in the review for the agent, which the agent carries out. Commits stay with the user's Git tools. Copy review copies the text before it records the round. Surface persistence failures.

There is no automatic updater. Releases ship Windows, macOS (universal) and Linux packages; only Windows is tested by hand. Keep credentials, private notes, sessions, databases and generated files out of commits.
