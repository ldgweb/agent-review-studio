// Checks that every file carrying the app version agrees, and that the version
// matches the release tag when one is given: `node scripts/check-version.mjs v0.11.0`.
import { readFileSync } from 'node:fs';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');

const versions = {
  'package.json': JSON.parse(read('package.json')).version,
  'package-lock.json': JSON.parse(read('package-lock.json')).version,
  'src-tauri/tauri.conf.json': JSON.parse(read('src-tauri/tauri.conf.json')).version,
  'src-tauri/Cargo.toml': /^version = "([^"]+)"/m.exec(read('src-tauri/Cargo.toml'))?.[1],
  // CI builds with --locked, so the lock file must carry the new version too.
  'src-tauri/Cargo.lock': /\[\[package\]\]\r?\nname = "agent-review-studio"\r?\nversion = "([^"]+)"/.exec(
    read('src-tauri/Cargo.lock'),
  )?.[1],
};

const tag = process.argv[2];
const expected = tag ? tag.replace(/^v/, '') : versions['package.json'];
const wrong = Object.entries(versions).filter(([, version]) => version !== expected);

if (wrong.length > 0) {
  for (const [file, version] of wrong) console.error(`${file}: ${version ?? 'no version'} (expected ${expected})`);
  process.exit(1);
}
console.log(`Version ${expected} everywhere.`);
