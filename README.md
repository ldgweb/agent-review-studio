# Agent Review Studio

[![CI](https://github.com/ldgweb/agent-review-studio/actions/workflows/ci.yml/badge.svg)](https://github.com/ldgweb/agent-review-studio/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE.md)
![Platform: Windows](https://img.shields.io/badge/platform-Windows%20x64-lightgrey.svg)
![Status: beta](https://img.shields.io/badge/status-beta-orange.svg)

A local desktop workbench for reviewing what a coding agent changed — with the agent's own session beside the diff — and deciding, file by file, what to keep.

**Windows beta · React + TypeScript + Tauri/Rust · no account, no cloud**

![A 30-second tour: open a session, comment on a line, get a correction request for the agent](.github/assets/demo.gif)

> **Project status:** maintained in spare time. Issues and pull requests are welcome; response times are not guaranteed.

## Why

A diff tells you *what* changed, not *why*. When an agent edits fifteen files, you also need the task it was given, the plan it followed, the commands it ran and whether the tests it ran still cover the final code. Agent Review Studio reads the agent's local session transcript and puts all of that next to the real Git diff, flags where to look first, and turns your verdict into a commit, a backed-up revert or a correction request for the agent.

## Quick start

1. Download the latest Windows build from [Releases](https://github.com/ldgweb/agent-review-studio/releases) (or [build from source](#build-from-source)).
2. Start the app and click **Try the sample tour**. It creates a separate sandbox repository with two sample sessions, so you can try every step without touching your projects.
3. Use your own agent: Claude Code, Codex, Cursor, VS Code Copilot, Gemini CLI and `ars/v1` folders are read automatically from their default locations ([details](#where-sessions-come-from)); Aider only needs its project folders added under **Settings → Session sources**.

## Try it in five minutes

The sample tour runs on real machinery — a sandbox Git repository under `%USERPROFILE%\.agent-review-studio\demo` and two sample sessions — so nothing you do touches your own projects.

1. Click **Try the sample tour** on the welcome screen, or **Settings → Data → Sample tour → Install / reset** if you already have sessions. The inbox shows two sessions from an agent called *ARS Tour (sample)*.

   ![The review queue with the two sample sessions](.github/assets/inbox.png)

2. Open the first one, *"Add a discount-code feature…"*. The centre shows the real diff, file by file; the side rail has the **Session** tab (what the agent was asked, its plan and what it claims it did), the **Timeline** of commands and test runs, the **Findings** and **Git**.

   ![The session context beside the diff: prompt, plan and the agent's self-report](.github/assets/session-context.png)

3. Look at what the review flags for you:
   - `src/auth/session.ts` contains what looks like a hardcoded API key;
   - `src/checkout/discount.ts` still has a `TODO` and a `console.log`, and it was edited **after** the last green test run — the timeline shows "Refactored rounding AFTER the green run".
4. Decide: press `a` on the files that look fine, `r` on one you would throw away, and `c` on `discount.ts`. Hover a line in the diff, click the **+** that appears and write a comment, for example *"remove this console.log"* (`Ctrl+Enter` saves it).
5. Click **Corrections** in the bottom bar: your comments, notes and flagged findings are already assembled into one request for the agent, with file and line for each item. **Copy for ARS Tour (sample)** (the button is named after the agent) puts it on the clipboard, ready to paste into the agent's next prompt.

   ![A correction request assembled from line comments and flagged files](.github/assets/correction-request.png)

6. Open the second sample, which is already reviewed. Select a rejected file and click **Revert…** in the bottom bar to restore it (a backup is saved first), then open the **Git** tab in the side rail to commit the approved files in the sandbox repository and watch the commit graph update.
7. In a terminal, try the CLI on the same sample (from source: `npm run ars -- <command>` in `app/`):

   ```text
   ars list
   ars review ars-demo-tour
   ars pr ars-demo-tour
   ```

When every file has a verdict, **Finalize** records the outcome. **Install / reset** in Settings puts the tour back to the start; archiving the samples hides them.

### What line comments are for

If a change is fine, you don't need comments: approve it and commit. Comments matter when something must change. Instead of writing the agent a follow-up from memory ("in cart.ts the rounding is wrong, and also…"), you comment on the exact lines while you read, and the app turns them into a precise correction request. When the agent's next session arrives, review it the same way; because the content changed, earlier decisions don't carry over by mistake.

## How a review works

1. **Inbox** — the most recent sessions from each source (30 per source by default), newest first, with their risk flags and review progress.
2. **Read the session** — the original request, the agent's plan and final self-report, and a timeline of commands and test runs.
3. **Read the diff** — files are grouped into *needs attention* and *routine*. Hints point at what deserves a closer look: possible hardcoded secrets, leftover debug code or TODOs, test files that shrank, unusually large single-file changes, files edited after the last passing test run, and your own regex checks.
4. **Decide per file** — approve (`a`), reject (`r`) or request changes (`c`), and leave comments on specific lines. Press `?` for all shortcuts.
5. **Apply** — commit only the approved files (other staged work is left alone), revert rejected files after an automatic backup, or export a correction request built from your comments and notes and paste it into the agent's next prompt.
6. **Share context** — `ars pr` turns the session into a Markdown review context for the pull request.

![Findings, a line comment and the "not re-tested" signal on the real diff](.github/assets/review.png)

Decisions are bound to the session content, Git `HEAD` and the current file bytes: if anything changes after you decide, the review reopens instead of applying a stale verdict.

## Features

- **Local transcripts** from Claude Code, Codex, Cursor, VS Code Copilot, Aider, Gemini CLI and the open `ars/v1` JSON format. Adapter coverage depends on the transcript version; synthetic regression fixtures are included.
- **Real working-tree diffs** for local Git repositories; transcript-only changes are labelled as such.
- **Decisions and line comments** stored locally in SQLite.
- **Whole-file Git actions**: selected-file commits on an isolated index and reverts that refuse to run without a successful backup. Both require the decision to be saved at the current review revision, and both include human edits shown in the file's current diff.
- **Correction requests** to copy or save as Markdown, and **PR context** from the `ars` CLI, which uses the same native core as the desktop app.

Review hints are deterministic heuristics, not a security assessment. A recorded passing test after an edit is a process signal; it does not prove coverage. Missing exit codes never count as passes. Historical transcript evidence may not describe the current working tree.

## Where sessions come from

The app only reads files the agents already write on your machine. Default locations (all can be changed under **Settings → Session sources**, which also shows how many sessions each source found):

| Agent | Default location | Setup |
| --- | --- | --- |
| Claude Code | `~/.claude/projects` | none |
| Codex | `~/.codex/sessions` | none |
| Cursor | Cursor's `User/globalStorage` folder (`%APPDATA%\Cursor\…` on Windows) | none |
| VS Code Copilot | VS Code's `User/workspaceStorage` folder | none |
| Gemini CLI | `~/.gemini/tmp` (saved chats and checkpoints) | `/chat save` or `--checkpointing` |
| Aider | `.aider.chat.history.md` inside each project | add the project folders |
| Any agent or CI | `ars/v1` JSON files in `~/.agent-review-studio/sessions` — see the [example](fixtures/custom/basic.json) and the [parser](app/src-tauri/src/core/adapters/custom.rs) | add more folders if needed |

Sessions stay listed after the agent deletes old transcripts, because the app keeps its own copy. Turning a source off hides all of its sessions, including those copies; turning it back on brings them back with your decisions.

Agents change their transcript formats over time. If an agent you use shows no sessions or incomplete ones after an update, please open a **Transcript format changed** issue; [CONTRIBUTING.md](CONTRIBUTING.md#when-an-agent-changes-its-transcript-format) explains how adapters are updated.

## Platforms

| Platform | Status |
| --- | --- |
| Windows x64 | Supported; release builds |
| Linux, macOS | The core and CLI are built and tested in CI; the desktop app is not yet packaged or validated. Help testing is welcome. |

## Install (Windows)

Download the installer or portable ZIP from [GitHub Releases](https://github.com/ldgweb/agent-review-studio/releases). If no release is listed yet, [build from source](#build-from-source).

- **Installer:** the NSIS `*-setup.exe` installs the desktop app and CLI for the current user.
- **Portable:** extract the ZIP and run `agent-review-studio.exe`, or `ars.exe --help` in a terminal.
- The CLI sits beside the desktop executable. Run `ars.exe` from that directory or add it to your user `PATH`; the installer does not edit `PATH`.
- **Requirements:** Windows x64, Microsoft Edge WebView2 Runtime, and Git on `PATH` for repository actions.
- The session index, decisions, settings and backups go to `%USERPROFILE%\.agent-review-studio`, also for the portable build. Window position and the WebView2 cache live in `com.agentreviewstudio.desktop` folders under `%APPDATA%` and `%LOCALAPPDATA%`.
- Builds are currently unsigned, so Windows may show a publisher warning. Verify downloads against `SHA256SUMS.txt` in the release.

Updates are manual.

## Build from source

Prerequisites: Node.js 22.12 or newer, Rust via rustup with the MSVC toolchain (the pinned version in `app/src-tauri/rust-toolchain.toml` is installed automatically), Visual Studio Build Tools with **Desktop development with C++** and a Windows SDK, Git, and WebView2. Restart the terminal after installing the toolchains.

From `app/`:

```powershell
npm ci
npm run tauri:dev
```

Plain `npm run dev` serves the frontend only; the native API requires the Tauri desktop shell.

Build an installer with `npm run tauri:build`; it also builds and bundles the `ars` CLI. To build the executables without an installer:

```powershell
npm run tauri -- build --no-bundle --ci -- --locked
```

Outputs: `app/src-tauri/target/release/agent-review-studio.exe`, `ars.exe`, and, for bundled builds, `bundle/nsis/`.

## Project layout

| Path | Contents |
| --- | --- |
| `app/src/` | React + TypeScript UI |
| `app/src-tauri/src/core/` | Shared Rust core: adapters, normalization, Git, storage |
| `app/src-tauri/src/cli/`, `app/src-tauri/src/bin/ars.rs` | The `ars` command-line interface |
| `fixtures/` | Synthetic transcripts and expected normalized output |

## Checks

From `app/`:

```powershell
npm test
npm run build
npm audit
npm run audit:rust
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --release --locked --all-targets --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --release --locked --manifest-path src-tauri/Cargo.toml
```

The Rust audit queries OSV for every registry package in `Cargo.lock`, reports other-platform advisories separately, and fails on advisories reachable by the supported Windows build. Dependency audits need internet access. Two ignored Rust smoke tests intentionally read or modify real user data; normal tests use synthetic fixtures and temporary repositories.

## CLI

```text
ars list
ars review <session-id>
ars pr <session-id>                   # print context for manual inspection
ars pr <session-id> --dry-run         # preview the GitHub target; post nothing
ars pr <session-id> --post            # explicitly publish a PR comment
```

`ars pr --post` needs `GH_TOKEN` (or `GITHUB_TOKEN`) and creates or updates a single review comment on the pull request. Run `ars --help` for all flags.

When you have saved a review in the desktop app at the session's current revision, `ars review` and `ars pr` include your decisions: the session outcome, each file's verdict, line comments and change requests, with secrets redacted and @mentions defused in the PR text. If the code changed since you saved it, they say so instead of showing outdated decisions. `ars review <id> --json` carries that review in the capsule, so `ars pr --from capsule.json` in CI shows the same decisions.

## Data and trust

Transcripts, SQLite databases, correction requests and revert backups may contain private code or credentials. Keep `%USERPROFILE%\.agent-review-studio`, agent session directories and environment files out of public repositories. Backups live in `.agent-review-studio/patches/`; keep them until you have verified the result.

Ordinary local review does not call an AI service. Explicit Git push, PR publishing and external links can access the network. Secret redaction in exports is best effort: inspect output before sharing.

Use trusted projects and transcripts. The application runs with your user permissions; Git configuration, hooks and filters can execute code. See [SECURITY.md](SECURITY.md), including how to report a vulnerability.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports with a synthetic transcript or the built-in sample project are the easiest to act on. Never attach real transcripts or databases to an issue.

## License

[MIT](LICENSE.md) © 2026 Lucio (ldgweb)
