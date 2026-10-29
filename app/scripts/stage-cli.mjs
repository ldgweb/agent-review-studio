// Stages the ars CLI as a Tauri sidecar. `externalBin` expects the binary at
// src-tauri/binaries/ars-<target-triple>[.exe]; Tauri then bundles it next to
// the desktop exe. `tauri build` runs this with --release through
// beforeBuildCommand; `tauri:dev` runs it without (dev only needs the file to
// exist, a debug build is enough).
//
// The target is the one Tauri builds for (TAURI_ENV_TARGET_TRIPLE), else this
// machine. A universal macOS build gets a universal ars: both architectures,
// joined with lipo. On macOS and Linux the packages take ars straight from
// the build folder (their tauri.<os>.conf.json drops externalBin), so a
// universal ars also goes where Tauri puts its own universal binary.
//
// Cargo runs inside src-tauri, as Tauri runs it, so the toolchain pinned in
// src-tauri/rust-toolchain.toml builds ars too.
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

const release = process.argv.includes('--release');
const host = /host: (\S+)/.exec(execFileSync('rustc', ['-vV']).toString())[1];
const target = process.env.TAURI_ENV_TARGET_TRIPLE || host;
const ext = target.includes('windows') ? '.exe' : '';
const targetDir = process.env.CARGO_TARGET_DIR ?? 'src-tauri/target';
const profile = release ? 'release' : 'debug';

/** Builds ars for one triple; returns where the binary is */
function build(triple) {
  const cross = triple !== host;
  const args = ['build', '--locked', '--bin', 'ars'];
  if (release) args.push('--release');
  if (cross) args.push('--target', triple);
  execFileSync('cargo', args, { stdio: 'inherit', cwd: 'src-tauri' });
  const built = cross ? join(targetDir, triple, profile, `ars${ext}`) : join(targetDir, profile, `ars${ext}`);
  if (statSync(built).size === 0) throw new Error(`${built} is empty`);
  return built;
}

mkdirSync('src-tauri/binaries', { recursive: true });
const staged = `src-tauri/binaries/ars-${target}${ext}`;
if (target === 'universal-apple-darwin') {
  const parts = ['aarch64-apple-darwin', 'x86_64-apple-darwin'].map(build);
  const universal = join(targetDir, target, profile);
  mkdirSync(universal, { recursive: true });
  execFileSync('lipo', ['-create', '-output', join(universal, 'ars'), ...parts], { stdio: 'inherit' });
  copyFileSync(join(universal, 'ars'), staged);
} else {
  copyFileSync(build(target), staged);
}
console.log(`staged ${profile} ars as ${staged.replace('src-tauri/', '')}`);
