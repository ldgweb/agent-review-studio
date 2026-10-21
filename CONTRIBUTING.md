# Contributing

Use the Windows setup in [README.md](README.md). Fork the repository, work on a topic branch and open a pull request against `main`; for larger changes, open an issue first to agree on the approach. Keep changes small and focused on reviewing actual agent work. New visualizations should expose evidence the application can substantiate.

The native core is in `app/src-tauri/src/core/`; both the desktop API and `ars` CLI use it. React features live in `app/src/features/`, pure logic in `app/src/lib/`, and persistence hooks in `app/src/state/`.

Before a pull request, run the README checks. Security changes need regression tests. Adapter changes need synthetic transcripts; never copy private sessions into fixtures. Expected outputs must change only when behavior intentionally changes. Full-content hashes are part of that contract.

Never include credentials, environment files, private transcripts, databases, signing keys or business notes. Gitleaks permits only exact synthetic test credentials in specific test source files.

Describe the problem, changed behavior, validation and remaining limitations. Do not claim support for an operating system or transcript version that has not been tested.

Release tags must match the versions in `app/package.json`, `app/src-tauri/Cargo.toml` and `app/src-tauri/tauri.conf.json`. The release workflow reruns checks and creates a draft Windows release. Inspect the installer and portable ZIP before publishing it. No automatic updater or private signing key is required.

## When an agent changes its transcript format

Agents change how they store sessions without notice, so an adapter can silently start missing prompts, commands, test runs or file edits. This is the most likely way the app breaks, and the easiest place to help.

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
