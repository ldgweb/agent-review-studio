// Public dependency metadata only is sent to OSV: crate names and versions.
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const manifest = fileURLToPath(new URL('../src-tauri/Cargo.toml', import.meta.url));
const lock = readFileSync(new URL('../src-tauri/Cargo.lock', import.meta.url), 'utf8');
const target = 'x86_64-pc-windows-msvc';
const tree = execFileSync('cargo', ['tree', '--locked', '--manifest-path', manifest, '--target', target,
  '--edges', 'normal,build', '--prefix', 'none', '--format', '{p}'], { encoding: 'utf8' });
const reachable = new Set([...tree.matchAll(/^(\S+) v(\S+)/gm)].map((m) => `${m[1]}@${m[2]}`));
const packages = lock.split('[[package]]').slice(1).filter((b) => /^source = "registry\+/m.test(b)).map((b) => ({
  name: /^name = "([^"]+)"/m.exec(b)[1], version: /^version = "([^"]+)"/m.exec(b)[1],
}));

async function request(url, body) {
  const response = await fetch(url, { method: body ? 'POST' : 'GET',
    headers: body ? { 'Content-Type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(60_000) });
  if (!response.ok) throw new Error(`OSV returned HTTP ${response.status}; audit incomplete`);
  return response.json();
}

const findings = [];
for (let offset = 0; offset < packages.length; offset += 100) {
  const batch = packages.slice(offset, offset + 100);
  const data = await request('https://api.osv.dev/v1/querybatch', { queries: batch.map((p) => ({
    package: { name: p.name, ecosystem: 'crates.io' }, version: p.version,
  })) });
  if (data.results?.length !== batch.length) throw new Error('Incomplete OSV results');
  for (let i = 0; i < batch.length; i++) {
    for (const advisory of data.results[i].vulns ?? []) findings.push({ ...batch[i], id: advisory.id,
      windowsReachable: reachable.has(`${batch[i].name}@${batch[i].version}`) });
  }
}
const details = new Map();
for (const id of new Set(findings.map((f) => f.id))) {
  details.set(id, await request(`https://api.osv.dev/v1/vulns/${encodeURIComponent(id)}`));
}
const active = findings.filter((f) => !details.get(f.id).withdrawn).map((f) => ({ ...f,
  summary: details.get(f.id).summary,
  informational: details.get(f.id).database_specific?.informational ?? null,
  references: details.get(f.id).references,
}));
const report = { checkedAt: new Date().toISOString(), target, registryPackages: packages.length,
  windowsPackages: reachable.size, findings: active };
const outputDir = new URL('../../audit-results/', import.meta.url);
mkdirSync(outputDir, { recursive: true });
writeFileSync(new URL('rust-dependencies.json', outputDir), JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
if (active.some((f) => f.windowsReachable)) process.exitCode = 1;
