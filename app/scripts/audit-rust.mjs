// Looks up every crates.io package in Cargo.lock on OSV, the open
// vulnerability database. Only public metadata leaves the machine: crate
// names and versions.
//
// Fails on a vulnerability in a package that ships in a release build,
// unless scripts/audit-allowlist.json explains why it does not affect the app.
// Informational advisories (unmaintained, unsound, notice) are listed but do
// not fail the check: they usually sit deep in Tauri's own dependencies.
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const manifest = fileURLToPath(new URL('../src-tauri/Cargo.toml', import.meta.url));
const lock = readFileSync(new URL('../src-tauri/Cargo.lock', import.meta.url), 'utf8');
const allowlist = JSON.parse(readFileSync(new URL('./audit-allowlist.json', import.meta.url), 'utf8'));
// The platforms with release builds, and the targets each one ships. Only
// Windows is released today; add macOS or Linux here when they are.
const targets = {
  windows: ['x86_64-pc-windows-msvc'],
};

function shipped(target) {
  const tree = execFileSync(
    'cargo',
    [
      'tree',
      '--locked',
      '--manifest-path',
      manifest,
      '--target',
      target,
      '--edges',
      'normal,build',
      '--prefix',
      'none',
      '--format',
      '{p}',
    ],
    { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 },
  );
  return new Set([...tree.matchAll(/^(\S+) v(\S+)/gm)].map((m) => `${m[1]}@${m[2]}`));
}

const reach = Object.fromEntries(
  Object.entries(targets).map(([name, list]) => [name, new Set(list.flatMap((target) => [...shipped(target)]))]),
);
const packages = lock
  .split('[[package]]')
  .slice(1)
  .filter((block) => /^source = "registry\+/m.test(block))
  .map((block) => ({ name: /^name = "([^"]+)"/m.exec(block)[1], version: /^version = "([^"]+)"/m.exec(block)[1] }));

async function request(url, body) {
  const response = await fetch(url, {
    method: body ? 'POST' : 'GET',
    headers: body ? { 'Content-Type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(60_000),
  });
  if (!response.ok) throw new Error(`OSV returned HTTP ${response.status}; audit incomplete`);
  return response.json();
}

const hits = [];
for (let offset = 0; offset < packages.length; offset += 100) {
  const batch = packages.slice(offset, offset + 100);
  const data = await request('https://api.osv.dev/v1/querybatch', {
    queries: batch.map((p) => ({ package: { name: p.name, ecosystem: 'crates.io' }, version: p.version })),
  });
  if (data.results?.length !== batch.length) throw new Error('Incomplete OSV results');
  batch.forEach((pkg, i) => {
    for (const advisory of data.results[i].vulns ?? []) hits.push({ ...pkg, id: advisory.id });
  });
}

const details = new Map();
for (const id of new Set(hits.map((hit) => hit.id))) {
  details.set(id, await request(`https://api.osv.dev/v1/vulns/${encodeURIComponent(id)}`));
}

// RustSec keeps "unmaintained", "unsound" and "notice" under affected[].
const informational = (advisory, name) =>
  advisory.database_specific?.informational ??
  advisory.affected?.find((a) => a.package?.name === name)?.database_specific?.informational ??
  null;

// A GitHub advisory that repeats a RustSec one on the same package is the
// same finding.
const duplicate = (hit) =>
  hit.id.startsWith('GHSA-') &&
  hits.some(
    (other) =>
      other.id.startsWith('RUSTSEC-') &&
      other.name === hit.name &&
      ((details.get(other.id).aliases ?? []).includes(hit.id) ||
        (details.get(hit.id).aliases ?? []).includes(other.id)),
  );
const findings = hits
  .filter((hit) => !details.get(hit.id).withdrawn && !duplicate(hit))
  .map((hit) => {
    const advisory = details.get(hit.id);
    const key = `${hit.name}@${hit.version}`;
    const allowed = allowlist.find((entry) => entry.id === hit.id || (advisory.aliases ?? []).includes(entry.id));
    return {
      ...hit,
      summary: advisory.summary,
      informational: informational(advisory, hit.name),
      platforms: Object.keys(targets).filter((name) => reach[name].has(key)),
      allowed: allowed ? allowed.reason : null,
    };
  });

const blocking = findings.filter((f) => !f.informational && f.platforms.length > 0 && !f.allowed);
const report = {
  checkedAt: new Date().toISOString(),
  targets,
  registryPackages: packages.length,
  findings,
  blocking: blocking.length,
};
// The repository root's audit-results/ (git-ignored)
const outputDir = new URL('../../audit-results/', import.meta.url);
mkdirSync(outputDir, { recursive: true });
writeFileSync(new URL('rust-dependencies.json', outputDir), JSON.stringify(report, null, 2) + '\n');

for (const f of findings) {
  const where = f.platforms.length > 0 ? f.platforms.join(', ') : 'not shipped';
  const kind = f.informational ? `info: ${f.informational}` : 'vulnerability';
  const note = f.allowed ? ` (allowed: ${f.allowed})` : '';
  console.log(`${f.id} ${f.name} ${f.version} [${kind}; ${where}] ${f.summary ?? ''}${note}`);
}
console.log(`${packages.length} packages checked, ${findings.length} advisories, ${blocking.length} blocking.`);
if (blocking.length > 0) process.exitCode = 1;
