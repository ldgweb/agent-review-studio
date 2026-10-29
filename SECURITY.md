# Security

## Supported scope

The beta runs on Windows x64, and experimentally on macOS and Linux x64. It runs with the current user's permissions and does not sandbox Git hooks, filters or editor handlers. Use trusted repositories and transcripts.

The application never writes to your repositories. It runs only read-only Git commands (`status`, `diff`, `ls-files`, `cat-file`, `check-attr`, `rev-parse`, `symbolic-ref`, `remote get-url`) with `GIT_OPTIONAL_LOCKS=0`, so Git does not even refresh the index. It reads working-tree files only at paths of the current changes (literal pathspecs; directories, Git metadata and linked paths are rejected). A rejected change becomes an instruction in the review for the agent: the agent, not this application, edits the code. The review can name Git commands for the agent to run (`git checkout HEAD -- <path>` to undo a whole file); the application never runs them. Committing, branching and pushing are left to your own Git tools. When you ask, it shows a file in File Explorer; only files of the current review can be shown. The only repository it creates and writes is the sample tour's sandbox, inside its own data folder.

`ars <dir>` passes the resolved project path to the desktop app (`--project`); the path reaches the WebView percent-encoded, and the review only runs Git in the work tree that contains it. A second launch is forwarded to the running instance.

Fingerprints detect stale content; they are not reviewer signatures. A local program with the same user permissions can alter application data.

## Private data

SQLite state (including the file snapshots of each review round), source transcripts, the review text and `ars review --json` output may contain private code or credentials. They are stored without application-level encryption. Export redaction is best effort; inspect content before publishing. Do not attach databases, real transcripts or unredacted stack traces to issues.

## Dependencies

Run `npm audit` and `npm run audit:rust` from `app/`. The Rust check queries OSV for the lockfile and fails only on a vulnerability in a package a release build ships (Windows, macOS or Linux). An exception needs an entry with its reason in `app/scripts/audit-allowlist.json`. Informational advisories (unmaintained, unsound) are listed but do not fail the check. An audit is a point-in-time check, not proof of absence of vulnerabilities. As of 2026-10-28 the allowlist holds one entry: `block-buffer 0.10.4` ([GHSA-qwgh-2vcv-g2f7](https://github.com/advisories/GHSA-qwgh-2vcv-g2f7)), whose cursor sits in a separate field rather than inside the block, so the reported corruption cannot happen there.

As of October 2026 the audit also lists five informational advisories. They are all Linux only and all inside Tauri's own dependencies, so they can't be updated from here: [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html) (`glib`) and [RUSTSEC-2024-0370](https://rustsec.org/advisories/RUSTSEC-2024-0370.html) (`proc-macro-error`) come with GTK 3, [RUSTSEC-2026-0370](https://rustsec.org/advisories/RUSTSEC-2026-0370.html), [RUSTSEC-2026-0371](https://rustsec.org/advisories/RUSTSEC-2026-0371.html) and [RUSTSEC-2026-0372](https://rustsec.org/advisories/RUSTSEC-2026-0372.html) with zbus 5, through the single-instance plugin. They'll go away when Tauri moves to newer versions.

## Reporting

Please report vulnerabilities privately through [GitHub private vulnerability reporting](https://github.com/ldgweb/agent-review-studio/security/advisories/new), not in a public issue. If private reporting isn't available for some reason, open an issue asking for a private contact, without exploit details, secrets or customer data, and I'll get back to you.
