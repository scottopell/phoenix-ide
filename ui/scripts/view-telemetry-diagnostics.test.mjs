import { test } from 'vitest';
import assert from 'node:assert/strict';
import { captureDiagnostic, samplingDecision, initialFailure, requestFact } from './view-telemetry-diagnostics.mjs';
const id = '00000000-0000-4000-8000-000000000001';
const admission = sampled => captureDiagnostic({ version: 1, kind: 'admission', route: 'ordinary', open_id: sampled ? id : null, sampled });
test('legacy correlated request is not proof of sampling', () => {
  const fact = requestFact({ url: () => `http://127.0.0.1:8123/api/product-conversations/${id}?open_id=${id}&secret=PRIVATE`, method: () => 'GET' }, 'http://127.0.0.1:8123');
  assert.ok(fact);
  assert.equal(samplingDecision([admission(false)], 'ordinary').sampled, false);
  assert.ok(!JSON.stringify(fact).includes('PRIVATE'));
});
test('sampled mark stall remains sampled without terminal/submission evidence', () => {
  const events = [admission(true), captureDiagnostic({ version: 1, kind: 'mark', open_id: id, mark: 'shell', ms: 0 })];
  assert.equal(samplingDecision(events, 'ordinary').sampled, true);
  assert.ok(!events.some(e => e.kind === 'terminal'));
});
test('sampled transport failure is separate from readiness marks', () => {
  const events = [admission(true), ...['attempted', 'failed'].map(state => captureDiagnostic({ version: 1, kind: 'submission', open_id: id, state, status: null }))];
  assert.equal(samplingDecision(events, 'ordinary').open_id, id);
  assert.deepEqual(events.slice(1).map(e => e.state), ['attempted', 'failed']);
});
test('missing/ambiguous/foreign admission and private or invalid fields fail closed', () => {
  assert.throws(() => samplingDecision([], 'ordinary'));
  assert.throws(() => samplingDecision([admission(false), admission(true)], 'ordinary'));
  assert.throws(() => samplingDecision([admission(true), { kind: 'terminal', open_id: 'foreign' }], 'ordinary'));
  assert.throws(() => captureDiagnostic({ ...admission(true), text: 'PRIVATE' }));
  assert.throws(() => captureDiagnostic({ version: 1, kind: 'mark', open_id: id, mark: 'shell', ms: Infinity }));
});
test('dual errors retain primary identity and independent cleanup failure', () => {
  const primary = new Error('primary missing report'); const cleanup = new Error('cleanup duplicate'); const trial = {};
  assert.equal(initialFailure(trial, primary, cleanup), primary);
  assert.deepEqual(trial, { failure: primary.message, cleanup_failure: cleanup.message });
  assert.equal(initialFailure({}, undefined, cleanup), cleanup);
});
test('explicit timeout abort is a bounded diagnostic distinct from failed submission', () => {
  assert.deepEqual(captureDiagnostic({ version: 1, kind: 'submission', open_id: id, state: 'timeout_abort', status: null }),
    { version: 1, kind: 'submission', open_id: id, state: 'timeout_abort', status: null });
  assert.throws(() => captureDiagnostic({ version: 1, kind: 'submission', open_id: id, state: 'timeout_abort', status: 204 }));
});
test('SSE ownership edge is bound to admitted journey, finite UUIDs only with no inferred network ownership', () => {
  const stream = '10000000-0000-4000-8000-000000000001';
  const edge = captureDiagnostic({ version: 1, kind: 'sse', open_id: id, stream_id: stream });
  assert.equal(samplingDecision([admission(true), edge], 'ordinary').open_id, id);
  assert.throws(() => captureDiagnostic({ ...edge, stream_id: 'PRIVATE_URL' }));
  assert.throws(() => captureDiagnostic({ ...edge, conversation_id: stream }));
  assert.throws(() => samplingDecision([admission(true), { ...edge, open_id: stream }], 'ordinary'));
});
