# Repository guidance

Agent Review Studio is a Windows beta workbench for human review of coding-agent sessions. Keep it useful and small. Findings are deterministic heuristics; do not present them as measured probabilities or invent dependency graphs from folder names.

Read README.md for commands and SECURITY.md for trust boundaries. React/TypeScript lives in `app/src`; the shared Rust core, desktop and `ars` CLI live in `app/src-tauri`. The API is a Tauri command, not an HTTP server. Serving uses indexed sessions; background scanning owns ingestion. Labelled samples use a separate sandbox.

Run `npm test`, `npm run build` and `cargo test --release --locked` in their respective app/native directories. Run dependency audits and secret scans before release. Ignored smoke tests touch real user state and require explicit authorization.

Fixtures are synthetic. Change expected outputs only for intentional behavior changes. Session hashes cover full serialized content. Approvals bind transcript content, HEAD and current file bytes; preserve revision checks on write and export paths.

Use literal Git paths and validate containment. Failed backups prevent reverts. Selected-file commits preserve unrelated staged work. Surface persistence failures.

There is no automatic updater. Only Windows artifacts are currently released. Keep credentials, private notes, sessions, databases and generated files out of commits.
