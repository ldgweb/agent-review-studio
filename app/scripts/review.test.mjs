import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';

function load(relative) {
  const source = readFileSync(new URL(relative, import.meta.url), 'utf8');
  const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } });
  const exports = {};
  runInNewContext(outputText, { exports });
  return exports;
}
const R = load('../src/lib/review.ts');
const I = load('../src/lib/intraline.ts');
// Values built inside the vm context have their own prototypes; copy them
// into this realm before structural comparison.
const plain = (v) => JSON.parse(JSON.stringify(v));
const AT = '2026-10-28T10:00:00.000Z';

const emptyState = () => ({ version: 1, hunks: {}, files: {}, notes: {}, comments: [], findings: {}, rounds: [] });
const line = (kind, text, n) => ({ kind, text, ...(kind !== 'del' ? { newLine: n } : {}), ...(kind !== 'add' ? { oldLine: n } : {}) });
const hunk = (id, lines, origin = { kind: 'agent', matched: 1, total: 1 }) => ({ id, header: '@@ -1 +1 @@', lines, origin });
const file = (path, hunks, extra = {}) => ({
  id: `f-${path}`, path, changeType: 'modified', untracked: false, binary: false, language: 'ts',
  linesAdded: 1, linesRemoved: 0, areaId: 'a', riskFlags: [], fingerprint: `fp-${path}`, hunks,
  verification: 'no-signal', sessionIds: [], ...extra,
});
const review = (files, extra = {}) => ({
  project: { root: '/w/shop', name: 'shop', branch: 'main', head: 'abc' }, fingerprint: 'r', files,
  sessions: [], findings: [], groups: [], risks: [], areas: [], state: emptyState(), stateRev: 0,
  stats: {}, warnings: [], disabledSources: [], ...extra,
});

test('a file verdict goes to the undecided changes first, then to all, then toggles off', () => {
  const f = file('a.ts', [hunk('h1', [line('add', 'x', 1)]), hunk('h2', [line('add', 'y', 9)])]);
  let s = R.setHunkVerdict(emptyState(), f, 'h1', 'rejected', AT);
  s = R.setFileVerdict(s, f, 'approved', AT);
  assert.equal(s.hunks.h1.verdict, 'rejected', 'a decided change keeps its verdict');
  assert.equal(s.hunks.h2.verdict, 'approved');
  assert.equal(R.fileVerdict(s, f), 'mixed');
  s = R.setFileVerdict(s, f, 'approved', AT);
  assert.equal(R.fileVerdict(s, f), 'approved', 'nothing undecided: all of them');
  s = R.setFileVerdict(s, f, 'approved', AT);
  assert.equal(R.fileVerdict(s, f), 'pending', 'the same verdict again clears it');
});

test('verdicts bind to content: a changed hunk or other bytes drop the decision', () => {
  const before = file('a.ts', [hunk('h1', [line('add', 'x', 1)])]);
  const s = R.setFileVerdict(emptyState(), before, 'approved', AT);
  const after = file('a.ts', [hunk('h1b', [line('add', 'x2', 1)])]);
  assert.equal(R.fileVerdict(s, after), 'pending');
  const bin = file('logo.png', [], { binary: true });
  const sb = R.setFileVerdict(emptyState(), bin, 'rejected', AT);
  assert.equal(R.fileVerdict(sb, bin), 'rejected');
  assert.equal(R.fileVerdict(sb, { ...bin, fingerprint: 'fp-new' }), 'pending');
});

test('comments follow their line, and go outdated with their change', () => {
  const h = hunk('h1', [line('context', 'a', 1), line('add', 'console.log(x)', 2)]);
  const c = { id: 'c1', path: 'a.ts', hunkId: 'h1', li: 0, kind: 'add', lineText: 'console.log(x)', line: 2, text: 'drop it', at: AT };
  assert.equal(R.commentLineIndex(h, c), 1, 'found again by kind and text');
  const s = { ...emptyState(), comments: [c] };
  assert.equal(R.currentComments(s, [file('a.ts', [h])]).length, 1);
  assert.equal(R.outdatedComments(s, [file('a.ts', [hunk('h2', [])])]).length, 1);
});

test('word diff: every changed token is marked, lines are paired by similarity, rewrites stay unmarked', () => {
  const r = plain(I.pairRanges('const a = foo(1, 2);', 'const b = foo(1, 3);'));
  assert.deepEqual(r.del, [{ start: 6, end: 7 }, { start: 17, end: 18 }]);
  assert.deepEqual(r.add, [{ start: 6, end: 7 }, { start: 17, end: 18 }]);
  assert.equal(I.pairRanges('return a;', 'throw new Error("unreachable")'), null);
  assert.deepEqual(plain(I.alignRun(['a = 1', 'b = 2'], ['x()', 'a = 10', 'b = 20'])), [
    { del: null, add: 0, match: false },
    { del: 0, add: 1, match: true },
    { del: 1, add: 2, match: true },
  ]);
  const ranges = I.changedRanges(hunk('h', [line('del', 'a = 1', 1), line('add', 'x()', 1), line('add', 'a = 10', 2)]));
  assert.deepEqual(plain([...ranges.keys()]), [0, 2], 'the edited line pairs with its new version, not with the inserted one');
});

test('previous round comments: open while their code is untouched, changed once it moved on', () => {
  const live = file('a.ts', [hunk('h1', [line('add', 'x', 1)])]);
  const mk = (id, hunkId, resolved) => ({ id, path: 'a.ts', hunkId, li: 0, kind: 'add', lineText: 'x', text: 't', at: AT, resolved });
  const s = { ...emptyState(), rounds: [{ index: 1, at: AT, text: '', files: {}, comments: [mk('a', 'h1'), mk('b', 'gone'), mk('c', 'gone', true)], notes: {} }] };
  assert.deepEqual(plain(R.previousRoundItems(review([live]), s).map((i) => i.status)), ['open', 'changed', 'resolved']);
  const resolved = R.setRoundCommentResolved(s, 'a', true);
  assert.equal(resolved.rounds[0].comments[0].resolved, true);
});

test('one reading order: what needs a closer look (weightiest first), the routine rest, generated files last', () => {
  const plainFile = file('b.ts', [hunk('h1', [line('add', 'x', 1)])]);
  const risky = file('a.ts', [hunk('h2', [line('add', 'y', 1)])], { riskFlags: ['security'] });
  const riskier = file('c.ts', [hunk('h3', [line('add', 'z', 1)])], { riskFlags: ['security', 'database'] });
  const lock = file('package-lock.json', [hunk('h4', [line('add', '{', 1)])], { generated: 'lockfile' });
  const groups = R.groupFiles([plainFile, risky, lock, riskier], review([plainFile, risky, lock, riskier]));
  assert.deepEqual(plain(groups.map((g) => [g.key, g.files.map((f) => f.path)])), [
    ['attention', ['c.ts', 'a.ts']],
    ['routine', ['b.ts']],
    ['generated', ['package-lock.json']],
  ]);
  assert.deepEqual(plain(R.attentionReasons(risky, review([risky]))), ['touches security']);
});

test('a comment asks for a change; removing the last one takes that back', () => {
  const f = file('a.ts', [hunk('h1', [line('add', 'x', 1), line('add', 'y', 2)]), hunk('h2', [line('add', 'z', 9)])]);
  const c = (id, li) => ({ id, path: 'a.ts', hunkId: 'h1', li, kind: 'add', lineText: li ? 'y' : 'x', line: li + 1, text: 't', at: AT });
  let s = R.commentOnHunk(emptyState(), c('c1', 0), AT);
  assert.equal(s.hunks.h1.verdict, 'changes');
  s = R.commentOnHunk(s, c('c2', 1), AT);
  s = R.uncommentHunk(s, 'c1');
  assert.equal(s.hunks.h1.verdict, 'changes', 'another comment still asks');
  s = R.uncommentHunk(s, 'c2');
  assert.equal(s.hunks.h1, undefined);
  const approved = R.setHunkVerdict(emptyState(), f, 'h2', 'approved', AT);
  const commented = R.commentOnHunk(approved, { ...c('c3', 0), hunkId: 'h2' }, AT);
  assert.equal(commented.hunks.h2.verdict, 'approved', 'a decided change keeps its verdict');
  assert.equal(R.uncommentHunk(commented, 'c3').hunks.h2.verdict, 'approved');
});

test('needs attention: risk, real findings, untested edits, changes since review, unrecorded changes', () => {
  const f = file('a.ts', [hunk('h1', [line('add', 'x', 1)], { kind: 'none', matched: 0, total: 1 })]);
  assert.equal(R.needsAttention(f, review([f])), false, 'no sessions: nothing to compare with');
  assert.equal(R.needsAttention(f, review([f], { sessions: [{ sessionId: 's' }] })), true);
  assert.equal(R.needsAttention({ ...f, generated: 'lockfile' }, review([f], { sessions: [{ sessionId: 's' }] })), false);
  const plainFile = file('b.ts', [hunk('h2', [line('add', 'y', 1)])]);
  assert.equal(R.needsAttention({ ...plainFile, since: 'changed' }, review([plainFile])), true);
  assert.equal(R.needsAttention(plainFile, review([plainFile], { findings: [{ id: 'f', severity: 'medium', filePath: 'b.ts' }] })), true);
});

test('sortFindings ranks by severity, then potential bugs, without mutating the input', () => {
  const findings = [
    { id: 'low', severity: 'low', kind: 'issue' },
    { id: 'high-issue', severity: 'high', kind: 'issue' },
    { id: 'high-bug', severity: 'high', kind: 'potential-bug' },
    { id: 'critical', severity: 'critical', kind: 'suggestion' },
  ];
  assert.deepEqual(plain(R.sortFindings(findings).map((f) => f.id)), ['critical', 'high-bug', 'high-issue', 'low']);
  assert.equal(findings[0].id, 'low');
});
