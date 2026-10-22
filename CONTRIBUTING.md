# Contributing

Thanks for taking a look! Small fixes are welcome as they are. For anything bigger, please open an issue first so we can agree on the approach before you spend time on it.

## Getting started

Set up the project as described in [README.md](README.md#build-from-source), fork the repository, work on a branch and open a pull request against `main`.

A quick map of the code: the native core is in `app/src-tauri/src/core/`, and both the desktop API and the `ars` CLI use it. React features are in `app/src/features/`, pure logic in `app/src/lib/` and persistence hooks in `app/src/state/`.

I'd like to keep the app small and focused on reviewing real agent work. A new view or hint should show something the app can actually back up from the transcript or the repository.

## Before you open a pull request

- Run the checks listed in the README.
- Security fixes come with a regression test.
- Adapter changes come with a synthetic transcript. Never copy a real session into `fixtures/`.
- Expected outputs in `fixtures/` only change when the behavior is meant to change. Session hashes cover the full content, so they're part of that contract too.
- No credentials, environment files, real transcripts, databases, signing keys or private notes. Gitleaks only allows the exact fake credentials used in specific test files.
- In the description, say what the problem was, what changes, how you tested it and what's still missing. Please don't claim support for an operating system or transcript version you haven't tested.

## Releases

Release tags must match the version in `app/package.json`, `app/src-tauri/Cargo.toml` and `app/src-tauri/tauri.conf.json`. The release workflow runs the checks again and creates a draft Windows release; I check the installer and the portable ZIP by hand before publishing it.

## When an agent changes its transcript format

Agents change how they store sessions without warning, and when that happens an adapter can quietly start missing prompts, commands, test runs or file edits. It's the most likely way for the app to break, and also the easiest place to help.

**Spotting it**

- **Settings → Session sources** shows each source's session count and, when something is off, a hint. A count of zero for an agent you use, or sessions with no files or no timeline, usually means the format moved.
- Users report it with the **Transcript format changed** issue template: agent name and version, what is missing, and a *minimal, anonymised* excerpt. Never paste a real transcript: it can contain private code and credentials.

**Fixing it**

1. Find the adapter in `app/src-tauri/src/core/adapters/` (one file per agent). Keep the old format working: users upgrade agents at different times, so add the new shape next to the old one instead of replacing it.
2. Write a **synthetic** transcript in the new format under `fixtures/<agent>/`, named after what it covers (for example `cursor/agent-v3.json`). Invent the content; keep it small but include every record type the change touched.
3. Add a test next to the existing fixture tests in the adapter (they call `fixture::assert_session`) and generate its expected output from `app/src-tauri`:

   ```powershell
   $env:ARS_UPDATE_FIXTURES = "1"; cargo test --release --locked; Remove-Item Env:ARS_UPDATE_FIXTURES
   ```

   (On Linux or macOS: `ARS_UPDATE_FIXTURES=1 cargo test --release --locked`.)

   This writes only files whose content changed. Read the generated `fixtures/<agent>/expected/*.json` like any other diff: it is the contract.
4. Run the full checks from the README. Existing expected files must not change unless the fix intentionally changes their behavior; say so in the pull request if they do.
5. In the pull request, state the agent version you modelled the fixture on.
