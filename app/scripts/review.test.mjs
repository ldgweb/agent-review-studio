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
const { verificationStatus } = load('../src/lib/evidence.ts');
const { reducer, emptyState, effectiveStatus } = load('../src/lib/approvals.ts');

test('missing, failed and pre-edit tests never verify a later edit', () => {
  const file = { path: 'a.ts' };
  const edit = { kind: 'file-edit', filePath: 'a.ts' };
  for (const code of [undefined, null, 1]) {
    assert.equal(verificationStatus({ timeline: [edit, { kind: 'test-run', exitCode: code }] }, file), 'unverified');
  }
  assert.equal(verificationStatus({ timeline: [{ kind: 'test-run', exitCode: 0 }, edit] }, file), 'unverified');
  assert.equal(verificationStatus({ timeline: [edit, { kind: 'test-run', exitCode: 0 }] }, file), 'verified');
  assert.equal(verificationStatus({ timeline: [edit] }, file), 'no-signal');
});

test('a new review revision invalidates old file decisions, comments and finalization', () => {
  let store = reducer({}, { type: 'set-files', sessionId: 's', fileIds: ['lf-1'], state: 'approved', reviewRevision: 'v1' });
  store = reducer(store, { type: 'set-line-comment', sessionId: 's', fileId: 'lf-1', comment: { anchor: '0:0', text: 'old' }, reviewRevision: 'v1' });
  store = reducer(store, { type: 'finalize', sessionId: 's', decision: 'approved', reviewRevision: 'v1' });
  store = reducer(store, { type: 'set-files', sessionId: 's', fileIds: ['lf-2'], state: 'rejected', reviewRevision: 'v2' });
  assert.equal(store.s.fileStates['lf-1'], undefined);
  assert.equal(store.s.fileStates['lf-2'], 'rejected');
  assert.equal(store.s.sessionDecision, undefined);
  assert.equal(Object.keys(store.s.lineComments).length, 0);
  assert.equal(store.s.reviewRevision, 'v2');
});

test('legacy and changed-transcript approvals do not override inbox status', () => {
  const session = { status: 'needs-review', contentHash: 'new' };
  const approval = { ...emptyState('s'), sessionDecision: 'approved' };
  assert.equal(effectiveStatus(session, approval), 'needs-review');
  assert.equal(effectiveStatus(session, { ...approval, reviewRevision: 'r', contentHash: 'old' }), 'needs-review');
  assert.equal(effectiveStatus(session, { ...approval, reviewRevision: 'r', contentHash: 'new' }), 'approved');
});

test('consecutive edits retain both file decisions', () => {
  let store = reducer({}, { type: 'set-files', sessionId: 's', fileIds: ['a'], state: 'approved', reviewRevision: 'v1' });
  store = reducer(store, { type: 'set-files', sessionId: 's', fileIds: ['b'], state: 'rejected', reviewRevision: 'v1' });
  assert.equal(store.s.fileStates.a, 'approved');
  assert.equal(store.s.fileStates.b, 'rejected');
});

const { computeDecision } = load('../src/lib/approvals.ts');
const { fileWeight, needsAttention, groupFiles, sortFindings } = load('../src/lib/triage.ts');
// Values built inside the vm context have their own Array prototype; copy
// them into this realm before structural comparison.
const ids = (items) => Array.from(items, (x) => x.id);
const keys = (groups) => Array.from(groups, (g) => g.key);

test('computeDecision is neutral for zero files and follows the verdicts otherwise', () => {
  const none = computeDecision([], emptyState('s'));
  assert.equal(none.decision, null);
  assert.equal(none.approved + none.rejected + none.corrections + none.pending, 0);
  const at = (fileStates) => ({ ...emptyState('s'), fileStates });
  assert.equal(computeDecision(['a', 'b'], at({ a: 'approved', b: 'approved' })).decision, 'approved');
  assert.equal(computeDecision(['a', 'b'], at({ a: 'rejected', b: 'rejected' })).decision, 'rejected');
  assert.equal(computeDecision(['a', 'b'], at({ a: 'approved', b: 'rejected' })).decision, 'partially-approved');
  assert.equal(computeDecision(['a', 'b'], at({ a: 'approved' })).decision, 'partially-approved');
  assert.equal(computeDecision(['a', 'b'], at({ a: 'rejected', b: 'correction-requested' })).decision, 'corrections-requested');
  assert.equal(computeDecision(['a', 'b'], at({ a: 'approved' })).pending, 1);
});

test('review-content changes clear a finalized decision; notes do not', () => {
  const finalized = () => {
    const store = reducer({}, { type: 'set-line-comment', sessionId: 's', fileId: 'f', comment: { anchor: '0:0', text: 'x' }, reviewRevision: 'v1' });
    return reducer(store, { type: 'finalize', sessionId: 's', decision: 'corrections-requested', reviewRevision: 'v1' });
  };
  const after = (action) => reducer(finalized(), { sessionId: 's', reviewRevision: 'v1', ...action }).s;
  assert.equal(finalized().s.sessionDecision, 'corrections-requested');
  assert.equal(after({ type: 'remove-line-comment', fileId: 'f', anchor: '0:0' }).sessionDecision, undefined);
  assert.equal(after({ type: 'set-line-comment', fileId: 'f', comment: { anchor: '0:1', text: 'y' } }).sessionDecision, undefined);
  assert.equal(after({ type: 'set-files', fileIds: ['f'], state: 'approved' }).sessionDecision, undefined);
  assert.equal(after({ type: 'set-finding-state', findingId: 'x', state: 'dismissed' }).sessionDecision, undefined);
  assert.equal(after({ type: 'reject-category', dimension: 'security', fileIds: ['f'] }).sessionDecision, undefined);
  assert.equal(after({ type: 'clear-category', dimension: 'security', fileIds: ['f'] }).sessionDecision, undefined);
  const noted = after({ type: 'set-note', key: '__draft', note: 'edited' });
  assert.equal(noted.sessionDecision, 'corrections-requested');
  assert.equal(noted.correctionNotes.__draft, 'edited');
});

test('displayed status shows "in review" once a saved verdict applies to this transcript', () => {
  const session = { sessionId: 's', status: 'needs-review', contentHash: 'h' };
  assert.equal(effectiveStatus(session), 'needs-review');
  const touched = { ...emptyState('s'), reviewRevision: 'r', contentHash: 'h', fileStates: { a: 'approved' } };
  assert.equal(effectiveStatus(session, touched), 'in-review');
  assert.equal(effectiveStatus(session, { ...touched, contentHash: 'old' }), 'needs-review');
  assert.equal(effectiveStatus(session, { ...touched, fileStates: { a: 'pending' } }), 'needs-review');
  assert.equal(effectiveStatus(session, { ...touched, sessionDecision: 'rejected' }), 'rejected');
});

const triageFile = (id, extra = {}) => ({ id, path: `${id}.ts`, areaId: 'misc', riskFlags: [], ...extra });
const triageSession = (files, findings = []) => ({
  files,
  findings,
  systemAreas: [
    { id: 'misc', label: 'Misc', critical: false },
    { id: 'auth', label: 'Auth', critical: true },
  ],
});

test('fileWeight adds risk flags, critical area and the worst finding on the file', () => {
  const file = triageFile('a', { areaId: 'auth', riskFlags: ['security', 'database'] });
  const session = triageSession([file], [
    { id: 'f1', filePath: 'a.ts', severity: 'medium', kind: 'issue' },
    { id: 'f2', filePath: 'a.ts', severity: 'high', kind: 'issue' },
    { id: 'f3', filePath: 'other.ts', severity: 'critical', kind: 'issue' },
  ]);
  const w = fileWeight(file, session);
  assert.equal(w.riskFlags, 2);
  assert.equal(w.criticalArea, true);
  assert.equal(w.findingSeverity, 2);
  assert.equal(w.total, 6);
  assert.equal(fileWeight(triageFile('plain'), session).total, 1);
});

test('needsAttention flags risk, medium+ findings, critical areas and unverified files only', () => {
  const session = triageSession([], [
    { id: 'm', filePath: 'medium.ts', severity: 'medium', kind: 'issue' },
    { id: 'l', filePath: 'low.ts', severity: 'low', kind: 'issue' },
  ]);
  const none = new Set();
  assert.equal(needsAttention(triageFile('plain'), session, none), false);
  assert.equal(needsAttention(triageFile('low'), session, none), false);
  assert.equal(needsAttention(triageFile('medium'), session, none), true);
  assert.equal(needsAttention(triageFile('flagged', { riskFlags: ['performance'] }), session, none), true);
  assert.equal(needsAttention(triageFile('crit', { areaId: 'auth' }), session, none), true);
  assert.equal(needsAttention(triageFile('plain'), session, new Set(['plain'])), true);
});

test('groupFiles orders risk groups by weight and puts critical areas first', () => {
  const files = [
    triageFile('routine'),
    triageFile('one', { riskFlags: ['security'] }),
    triageFile('three', { areaId: 'auth', riskFlags: ['security', 'database'] }),
    triageFile('nested', { path: 'dir/nested.ts' }),
  ];
  const session = triageSession(files);
  const risk = groupFiles(files, session, 'risk', new Set());
  assert.deepEqual(keys(risk), ['attention', 'routine']);
  assert.deepEqual(ids(risk[0].files), ['three', 'one']);
  assert.deepEqual(ids(risk[1].files), ['routine', 'nested']);

  const area = groupFiles(files, session, 'area', new Set());
  assert.deepEqual(keys(area), ['area:auth', 'area:misc']);
  assert.equal(area[0].meta, 'critical area');

  const dir = groupFiles(files, session, 'dir', new Set());
  assert.deepEqual(Array.from(dir, (g) => g.label), ['(root)', 'dir/']);
  assert.deepEqual(ids(dir[0].files), ['routine', 'one', 'three']);
});

test('sortFindings ranks by severity, then potential bugs, without mutating the input', () => {
  const findings = [
    { id: 'low', severity: 'low', kind: 'issue' },
    { id: 'high-issue', severity: 'high', kind: 'issue' },
    { id: 'high-bug', severity: 'high', kind: 'potential-bug' },
    { id: 'critical', severity: 'critical', kind: 'suggestion' },
    { id: 'info', severity: 'info', kind: 'question-for-agent' },
  ];
  assert.deepEqual(ids(sortFindings(findings)), ['critical', 'high-bug', 'high-issue', 'low', 'info']);
  assert.equal(findings[0].id, 'low');
});
