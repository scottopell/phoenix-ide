/** Offline installed-handler adapters only; no browser, server, or generated runtime body. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync, writeFileSync } from 'node:fs';
import { installAggregateRecorder, capturePolicy } from './qualify-history-opening-realserver.mjs';
import { readJsonLines, retainedResponse } from './replay-history-opening-recorder.mjs';
const flush = () => new Promise(resolve => setImmediate(resolve));
function clockAdapter() {
  let now = 0, next = 0; const timers = new Map();
  return { now: () => now, setTimeout: (fn, ms) => { const id = ++next; timers.set(id, { at: now + ms, fn }); return id; },
    clearTimeout: id => timers.delete(id), advance(ms) { now += ms; for (const [id, t] of [...timers]) if (t.at <= now) { timers.delete(id); t.fn(); } } };
}
function adapter(a, raw, { requestRow, documentRow, bodyRow } = {}) {
  const page = new EventEmitter(), clock = clockAdapter(), receipts = [], snapshots = [];
  let frameUrl = 'about:blank'; const frame = { url: () => frameUrl }; page.mainFrame = () => frame;
  const recorder = installAggregateRecorder(page, a, (file, value) => receipts.push({ file, value }), snapshots,
    { clock, requestStamp: req => req.source ?? null });
  const req = (url = 'http://127.0.0.1:1234' + a.endpoint, errorText = 'net::ERR_ABORTED', source = null) => ({
    url: () => url, method: () => 'GET', postData: () => null, failure: () => ({ errorText }), source });
  const old = req(requestRow?.value.url, requestRow?.value.failure?.errorText, requestRow?.source);
  const target = documentRow?.value.url ?? `http://127.0.0.1:1234${a.route}?source_transcript=${a.members[0]}#message-x`;
  const begin = () => recorder.beginNavigation({ kind: 'explicit_document_navigation', phase: 'pending_historical', target });
  const body = async (request = req(), text = async () => raw) => {
    request.source = bodyRow?.source; page.emit('request', request);
    page.emit('response', { request: () => request, url: request.url, status: () => 200, headers: () => ({ 'content-type': 'application/json' }), text }); await flush();
  };
  const document = ({ status = 200, url = target, navigation = true, mainFrame = true, frameEvent = true, responseEvent = true } = {}) => {
    const request = req(url, undefined, documentRow?.source); request.isNavigationRequest = () => navigation;
    request.frame = () => mainFrame ? frame : { url: () => url };
    page.emit('request', request);
    const response = { request: () => request, url: () => url, status: () => status };
    if (responseEvent) page.emit('response', response);
    if (frameEvent) { frameUrl = url; page.emit('framenavigated', frame); }
    return response;
  };
  return { page, clock, recorder, receipts, snapshots, req, old, target, begin, body, document };
}
function unitAggregate(kind) {
  const edge = { product_conversation_id: 'product', predecessor_transcript_row_id: 'root', successor_transcript_row_id: 'latest', continuation_message_id: 'root:continue' };
  const a = { kind, route: kind === 'ordinary' ? '/c/product' : '/global/root', members: ['root', 'latest'], product_conversation_id: 'product',
    endpoint: kind === 'ordinary' ? '/api/product-conversations/product' : '/api/global/coordinator/history/root',
    manual_opening: { status: 'accepted', edge, accepted_message_id: 'latest:key', opening_authority: 'user_authorized_instruction', presentation: 'separate_message' } };
  const d = { product_conversation_id: 'product', canonical_root: { transcript_row_id: 'root' }, requested_transcript_row_id: 'root', latest_transcript_row_id: 'latest',
    latest_opening: a.manual_opening, segments: a.members.map((id, i) => ({ transcript_row_id: id, segment_ordinal: i, messages: [], ...(kind === 'ordinary' ? { handoff: null } : {}) })) };
  if (kind === 'global') d.boundaries = [{ segment_ordinal: 0, predecessor_transcript_row_id: 'root', successor_transcript_row_id: 'latest', message_id: 'root:continue', represented_successor_opening_message_id: null }];
  return { a, raw: JSON.stringify(d) };
}
for (const kind of ['ordinary', 'global']) {
  test(`${kind}: confirmed main document cancellation is terminal, preserves original failure, and requires fresh full body`, async () => {
    const { a, raw } = unitAggregate(kind), f = adapter(a, raw);
    f.page.emit('request', f.old); f.clock.advance(1); const token = f.begin(); f.clock.advance(1);
    f.page.emit('requestfailed', f.old); assert.equal(f.recorder.partialInventory().pending_metadata[0].outcome, 'awaiting_document_supersession_confirmation');
    f.recorder.confirmNavigation(token, f.document({ url: f.target.split('#')[0] })); await f.body(); const drain = await f.recorder.sealAndDrain(); f.recorder.assertQualified();
    assert.equal(drain.partial, false); assert.equal(drain.canceled_inventory.length, 1);
    const canceled = drain.canceled_inventory[0]; assert.equal(canceled.navigation_scope, 0); assert.equal(canceled.navigation_epoch, 0);
    assert.equal(canceled.failure.errorText, 'net::ERR_ABORTED'); assert.equal(canceled.failure_at, 2);
    assert.equal(canceled.supersession_proof.is_navigation_request, true); assert.equal(canceled.supersession_proof.to_scope, 1);
  });
  for (const scenario of ['missing', 'unconfirmed', 'failed_document', 'wrong_url', 'wrong_query', 'foreign_frame', 'not_navigation', 'unobserved_response', 'same_document_spa', 'current_abort', 'before_intent', 'other_error', 'timeout', 'seal_cap', 'old_body_only', 'unreadable_old_body', 'current_body_wrong_owner', 'current_body_missing', 'stale_current_latest', 'deadline_not_reset']) {
    test(`${kind}: ${scenario} rejects`, async () => {
      const { a, raw } = unitAggregate(kind);
      if (scenario === 'stale_current_latest') a.members.push('newer');
      const f = adapter(a, raw);
      if (scenario === 'old_body_only') await f.body();
      f.page.emit('request', f.old);
      if (scenario === 'before_intent') f.page.emit('requestfailed', f.old);
      f.clock.advance(1); const token = f.begin(); f.clock.advance(1);
      if (scenario === 'current_abort') { const current = f.req(); f.page.emit('request', current); f.page.emit('requestfailed', current); }
      if (scenario === 'other_error') f.old.failure = () => ({ errorText: 'net::ERR_EMPTY_RESPONSE' });
      if (scenario === 'unreadable_old_body') {
        f.page.emit('response', { request: () => f.old, url: f.old.url, status: () => 200, headers: () => ({ 'content-type': 'application/json' }), text: async () => { throw new Error('old 200 unreadable'); } }); await flush();
      } else if (scenario !== 'before_intent') f.page.emit('requestfailed', f.old);
      if (scenario === 'timeout') f.clock.advance(11000);
      if (scenario === 'same_document_spa') f.page.emit('framenavigated', f.page.mainFrame());
      if (!['missing', 'unconfirmed', 'same_document_spa', 'seal_cap', 'deadline_not_reset'].includes(scenario)) {
        const options = { status: scenario === 'failed_document' ? 503 : 200,
          url: scenario === 'wrong_url' ? 'http://127.0.0.1:1234/foreign' : scenario === 'wrong_query' ? f.target.replace('root', 'latest') : f.target,
          mainFrame: scenario !== 'foreign_frame', navigation: scenario !== 'not_navigation', responseEvent: scenario !== 'unobserved_response' };
        try { f.recorder.confirmNavigation(token, f.document(options)); } catch { /* expected fail-closed document proof */ }
      }
      if (!['old_body_only', 'current_body_missing'].includes(scenario)) await f.body(undefined, scenario === 'current_body_wrong_owner' ? async () => raw.replaceAll('product', 'foreign') : undefined);
      if (scenario === 'deadline_not_reset') f.clock.advance(5000);
      const drained = f.recorder.sealAndDrain();
      if (scenario === 'deadline_not_reset') { f.clock.advance(6000); assert.equal(f.recorder.partialInventory().pending_required, 0, 'original failure deadline, not seal reset'); }
      f.clock.advance(scenario === 'seal_cap' ? 11000 : 22000); await drained;
      assert.throws(() => f.recorder.assertQualified(), undefined, scenario);
      assert.equal(f.recorder.partialInventory().pending_required, 0);
      if (scenario === 'timeout') assert.ok(f.recorder.captureErrors.some(e => e.reason === 'navigation_confirmation_deadline'));
      if (scenario === 'unreadable_old_body') assert.ok(f.recorder.captureErrors.some(e => e.reason === 'body_transport'));
    });
  }
}
const root = process.env.HISTORY_AUTHORITY_EVIDENCE;
test('retained bd request51 + main document context: trace candidate with synthetic ordering, NOT actual cause proof', { skip: !root }, async () => {
  const dir = `${root}/instrumentedbd-artifact/opening-browser-evidence/ordinary`;
  const a = JSON.parse(readFileSync(`${dir}/fixture.json`));
  const requestRow = readJsonLines(`${dir}/browser-failed.jsonl`).find(r => r.value.request_ordinal === 51);
  const documentRow = readJsonLines(`${dir}/browser-requests.jsonl`).find(r => new URL(r.value.url).pathname === a.route);
  const bodyRow = readJsonLines(`${dir}/browser-responses.jsonl`).find(r => r.value.outcome === 'body_received' && r.value.url.includes('/api/product-conversations/') && JSON.parse(r.value.raw).segments.length === a.members.length);
  assert.ok(requestRow && documentRow && bodyRow); assert.equal(requestRow.value.at, 2332.471986); assert.equal(requestRow.value.navigation_epoch, 2);
  assert.equal(Object.hasOwn(requestRow.value, 'failure_at'), false, 'old failure timing missing; no inferred real ordering');
  assert.equal(capturePolicy(a, 'GET', requestRow.value.url).kind, 'required_aggregate_json');
  assert.equal(capturePolicy({ ...a, kind: 'global', endpoint: '/api/global/coordinator/history/root' }, 'GET', requestRow.value.url).kind, 'metadata_only');
  const transport = readJsonLines(`${dir}/browser-required-transport.jsonl`);
  assert.equal(transport.find(r => r.value.request_ordinal === 51).value.expected_transport_failure, false);
  assert.equal(transport.find(r => r.value.request_ordinal === 126).value.expected_transport_failure, true);
  assert.ok(readJsonLines(`${dir}/preview-transport.jsonl`).some(r => r.value.action === 'destroy-roster'));
  const f = adapter(a, retainedResponse(bodyRow).raw, { requestRow, documentRow, bodyRow });
  f.page.emit('request', f.old); f.clock.advance(1); const intent = f.begin(); f.clock.advance(1); f.page.emit('requestfailed', f.old);
  f.recorder.confirmNavigation(intent, f.document()); await f.body(f.req(bodyRow.value.url)); await f.recorder.sealAndDrain(); f.recorder.assertQualified();
  if (process.env.HISTORY_NAVIGATION_OUTPUT) writeFileSync(process.env.HISTORY_NAVIGATION_OUTPUT, JSON.stringify({
    evidence_class: 'trace_candidate_with_synthetic_controlled_event_ordering_NOT_actual_cause_proof', runtime_executed: false,
    limitations: ['Retained bd log lacks failure_at and explicit goto intent/confirmation/document receipts.',
      'Navigation-request flag and main-frame object identity/order are offline adapters, not retained runtime facts.'],
    inputs: { request51: requestRow, main_document_request: documentRow, strict_body_source: bodyRow.source },
    receipts: f.receipts, drain: f.recorder.partialInventory() }, null, 2) + '\n');
  assert.deepEqual(f.recorder.failed[0].raw_request_stamp, requestRow.source);
  assert.equal(f.recorder.partialInventory().canceled_inventory[0].supersession_proof.raw_request_stamp.line_sha256, documentRow.source.line_sha256);
});

test('same/current context ERR_ABORTED without explicit intent is fatal; named phases and ownership reject', async () => {
  const { a, raw } = unitAggregate('ordinary'), f = adapter(a, raw);
  f.page.emit('request', f.old); f.page.emit('requestfailed', f.old); await f.body();
  assert.throws(() => f.recorder.beginNavigation({ kind: 'explicit_document_navigation', phase: 'chapter_click', target: f.target }));
  assert.throws(() => f.recorder.beginNavigation({ kind: 'explicit_document_navigation', phase: 'manual_identity', target: f.target.replace('/c/product', '/c/foreign') }));
  await f.recorder.sealAndDrain(); assert.throws(() => f.recorder.assertQualified());
});
