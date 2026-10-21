// Stages the ars CLI as a Tauri sidecar. `externalBin` expects the binary at
// src-tauri/binaries/ars-<target-triple>[.exe]; Tauri then bundles it next to
// the desktop exe. `tauri build` runs this with --release through
// beforeBuildCommand; `tauri:dev` runs it without (dev only needs the file to
// exist, a debug build is enough).
import { execSync } from 'node:child_process';
import { copyFileSync, mkdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

const release = process.argv.includes('--release');
execSync(`cargo build --locked ${release ? '--release ' : ''}--bin ars --manifest-path src-tauri/Cargo.toml`, {
  stdio: 'inherit',
});
const triple = /host: (\S+)/.exec(execSync('rustc -vV').toString())[1];
const ext = process.platform === 'win32' ? '.exe' : '';
const targetDir = process.env.CARGO_TARGET_DIR ?? 'src-tauri/target';
const built = join(targetDir, release ? 'release' : 'debug', `ars${ext}`);
if (statSync(built).size === 0) throw new Error(`${built} is empty`);
mkdirSync('src-tauri/binaries', { recursive: true });
copyFileSync(built, `src-tauri/binaries/ars-${triple}${ext}`);
console.log(`staged ${release ? 'release' : 'debug'} ars as binaries/ars-${triple}${ext}`);
