// Copies the packages built by `tauri build` into release-files/, with plain
// file names (no spaces), and adds a portable ZIP on Windows.
// Usage, from app/: node scripts/collect-release.mjs windows|macos|linux
import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { basename, join } from 'node:path';

const platform = process.argv[2];
if (!['windows', 'macos', 'linux'].includes(platform)) {
  console.error('Usage: node scripts/collect-release.mjs windows|macos|linux');
  process.exit(2);
}

const version = JSON.parse(readFileSync('package.json', 'utf8')).version;
// It ends up in file names and in a PowerShell command.
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/.test(version)) {
  console.error(`Unexpected version in package.json: ${version}`);
  process.exit(1);
}
const out = 'release-files';
const release = platform === 'macos' ? 'src-tauri/target/universal-apple-darwin/release' : 'src-tauri/target/release';
const bundle = join(release, 'bundle');
const plain = (name) => name.replaceAll('Agent Review Studio', 'agent-review-studio').replaceAll(' ', '-');

rmSync(out, { recursive: true, force: true });
mkdirSync(out);
const collected = [];

function take(folder, extension) {
  const dir = join(bundle, folder);
  if (!existsSync(dir)) return;
  for (const file of readdirSync(dir)) {
    if (!file.endsWith(extension)) continue;
    cpSync(join(dir, file), join(out, plain(file)));
    collected.push(plain(file));
  }
}

if (platform === 'windows') {
  take('nsis', '.exe');
  // Portable build: the app and the CLI side by side, so `ars .` finds the app
  const cli = join(release, 'ars.exe');
  if (statSync(cli).size === 0) throw new Error('ars.exe is empty: the CLI sidecar was not staged');
  const staging = 'release-portable';
  rmSync(staging, { recursive: true, force: true });
  mkdirSync(staging);
  for (const file of [join(release, 'agent-review-studio.exe'), cli, '../LICENSE.md', '../README.md']) {
    cpSync(file, join(staging, basename(file)));
  }
  const zip = `agent-review-studio_${version}_windows-x64-portable.zip`;
  const compress = `Compress-Archive -Path '${staging}/*' -DestinationPath '${join(out, zip)}' -Force`;
  execFileSync('pwsh', ['-NoProfile', '-Command', compress], { stdio: 'inherit' });
  rmSync(staging, { recursive: true, force: true });
  collected.push(zip);
} else if (platform === 'macos') {
  take('dmg', '.dmg');
} else {
  take('deb', '.deb');
  take('rpm', '.rpm');
  take('appimage', '.AppImage');
}

if (collected.length === 0) {
  console.error(`No packages found under ${bundle}`);
  process.exit(1);
}
console.log(collected.join('\n'));
