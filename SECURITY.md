# Security

## Supported scope

The beta targets Windows x64. It runs with the current user's permissions and does not sandbox Git hooks, filters or editor handlers. Use trusted repositories and transcripts.

Commit and revert accept literal session file paths, reject directories, Git metadata and linked paths, and require a saved decision ("approved" for commit, "rejected" for revert) at the current review fingerprint. Commits use an isolated index. Reverts require a successful binary-capable backup. Whole-file actions include all edits to selected files. Stop concurrent tools editing those files while applying a review; arbitrary external writers cannot be locked out by this application.

Fingerprints detect stale content; they are not reviewer signatures. A local program with the same user permissions can alter application data.

## Private data

SQLite state, backups, source transcripts and correction requests may contain private code or credentials. They are stored without application-level encryption. Export redaction is best effort; inspect content before publishing. Do not attach databases, real transcripts or unredacted stack traces to issues.

## Dependencies

Run `npm audit` and `npm run audit:rust` from `app/`. The Rust check queries OSV for the lockfile and distinguishes packages reachable on Windows from other-platform dependencies. An audit is a point-in-time check, not proof of absence of vulnerabilities. Resolve other-platform findings before supporting those platforms.

As of 2026-10-06, the GTK dependency chain retained by Tauri for Linux includes `glib 0.18.5` ([RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html)) and unmaintained `proc-macro-error 1.0.4` ([RUSTSEC-2024-0370](https://rustsec.org/advisories/RUSTSEC-2024-0370.html)). Neither is reachable by the Windows build. These findings remain reported by the audit; Linux release support is blocked until they are resolved upstream or replaced and tested.

## Reporting

Please report vulnerabilities privately through [GitHub private vulnerability reporting](https://github.com/ldgweb/agent-review-studio/security/advisories/new). Do not open a public issue for security problems. If private reporting is unavailable, ask for a private contact in an issue without exploit details, secrets or customer data.
