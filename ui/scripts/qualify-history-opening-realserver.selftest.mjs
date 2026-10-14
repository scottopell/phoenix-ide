import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, chmodSync, writeFileSync, readFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { sameEdge, openingMatches, faultTarget, previewTransport, updateFault, ownedFile, releaseOnce, ipc, persistedMessageSelector, acceptedWireCursor } from './qualify-history-opening-realserver.mjs';

test('fault remains active through acceptance and same-page reconnect, but new pin acquisition restores real transport', () => {
  const script = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const reconnect = script.indexOf("checked('same-page transport SSE drop/reconnect");
  const restored = script.indexOf('updateFault(fault, { failRoster: false })');
  const click = script.indexOf('await chapter.click()');
  const physical = script.indexOf("checked('accepted manual opening renders once");
  assert.ok(reconnect >= 0 && reconnect < restored && restored < click && click < physical);
  assert.match(script, /const restoredAggregate = page\.waitForResponse/);
  assert.match(script, /const restoredResponse = await restoredAggregate/);
  assert.match(script, /await rendered\.waitFor\(\{ state: 'visible' \}\); assert\.equal\(await rendered\.count\(\), 1\)/);
});

test('aggregate transport failure proof does not fabricate an alert contract for background probes', () => {
  const script = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  assert.match(script, /const failedRoster = await poll\(\(\) => failed\.find/);
  assert.match(script, /background aggregate ownership probe; no page-alert contract/);
  assert.match(script, /Accepted composer enabled despite roster failure/);
  assert.match(script, /actual composer message persisted/);
  assert.doesNotMatch(script, /page\.locator\('\[role="alert"\]'\)/);
});

test('accepted cursor uses owning Init incarnation and advancing JSON sequence, not invented transport ID', () => {
  const incarnation = '04bd47cf-71be-473f-b66c-818211975b81';
  const init = { requestId: '14019.119', eventName: 'init', eventId: '', data: JSON.stringify({ stream_incarnation: incarnation, sequence_id: 0 }) };
  const event = { requestId: init.requestId, eventName: 'message', eventId: '', data: JSON.stringify({ sequence_id: 1 }) };
  assert.deepEqual(acceptedWireCursor(event, [init, event]), { request_id: init.requestId, stream_incarnation: incarnation,
    init_sequence_id: 0, sequence_id: 1, transport_event_id: '' });
  assert.throws(() => acceptedWireCursor(event, [event]));
  assert.throws(() => acceptedWireCursor(event, [{ ...init, requestId: 'foreign' }, event]));
  for (const sequence_id of [0, -1, null, '1', 1.5]) {
    const invalid = { ...event, data: JSON.stringify({ sequence_id }) };
    assert.throws(() => acceptedWireCursor(invalid, [init, invalid]));
  }
  assert.throws(() => acceptedWireCursor(event, [event, init]));
});

test('exact persisted message selector covers real user ID and agent attribute without text fallback', () => {
  const id = 'successor:reserved-key';
  assert.equal(persistedMessageSelector(id), '#messages [id="message-successor:reserved-key"], #messages [data-message-id="successor:reserved-key"]');
  assert.throws(() => persistedMessageSelector(''));
  assert.throws(() => persistedMessageSelector(null));
  const script = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  assert.match(script, /page\.locator\(persistedMessageSelector\(openingId\)\)/);
  assert.match(script, /page\.locator\(persistedMessageSelector\(acceptedId\)\)/);
  assert.doesNotMatch(script, /page\.locator\(`[^`]*data-message-id=/);
});

const edge = { product_conversation_id: 'product', predecessor_transcript_row_id: 'predecessor', successor_transcript_row_id: 'successor', continuation_message_id: 'boundary', client_turn_key: 'key' };
const pending = { status: 'pending', edge, client_turn_key: 'key', opening_authority: 'user_authorized_instruction' };
const accepted = { status: 'accepted', edge, accepted_message_id: 'successor:key', opening_authority: 'user_authorized_instruction', presentation: 'separate_message' };

test('full edge and reserved/accepted key, never equal text or partial identity', () => {
  assert.equal(sameEdge(edge, edge), true);
  assert.equal(openingMatches(pending, edge, 'pending'), true);
  assert.equal(openingMatches(accepted, edge, 'accepted'), true);
  for (const key of ['product_conversation_id', 'predecessor_transcript_row_id', 'successor_transcript_row_id', 'continuation_message_id']) {
    assert.equal(sameEdge(edge, { ...edge, [key]: 'foreign' }), false);
    assert.equal(sameEdge(edge, { ...edge, [key]: undefined }), false);
    assert.equal(openingMatches({ ...accepted, edge: { ...edge, [key]: 'foreign' } }, edge, 'accepted'), false);
  }
  for (const change of [{ accepted_message_id: 'key' }, { accepted_message_id: 'other:key' }, { presentation: 'boundary' },
    { opening_authority: 'generated_predecessor_context' }, { status: 'pending' }]) {
    assert.equal(openingMatches({ ...accepted, ...change }, edge, 'accepted'), false);
  }
  assert.equal(openingMatches({ ...pending, client_turn_key: 'other' }, edge, 'pending'), false);
  assert.equal(sameEdge(null, edge), false);
});

test('HTTP fault filter is GET and exact owned aggregate/stream only, never another API', () => {
  const state = { aggregatePath: '/api/product-conversations/product', streamPath: '/api/conversations/successor/stream', failRoster: true };
  assert.equal(faultTarget('GET', state.aggregatePath + '?message_limit=100', state, 'roster'), true);
  assert.equal(faultTarget('GET', state.streamPath + '?since_event_seq=91', state, 'stream'), true);
  assert.equal(faultTarget('GET', state.aggregatePath, { ...state, failRoster: false }, 'roster'), false);
  for (const method of ['POST', 'PUT', 'DELETE', 'HEAD']) {
    assert.equal(faultTarget(method, state.aggregatePath, state, 'roster'), false);
    assert.equal(faultTarget(method, state.streamPath, state, 'stream'), false);
  }
  for (const url of ['/api/product-conversations/foreign', '/api/product-conversations/events', '/api/conversations/successor',
    '/api/conversations/foreign/stream', '/api/global/coordinator/history/root', '/api/models', '/api/product-conversations/product/extra']) {
    assert.equal(faultTarget('GET', url, state, 'roster'), false);
    assert.equal(faultTarget('GET', url, state, 'stream'), false);
  }
  assert.equal(faultTarget('GET', '/api/models', { ...state, aggregatePath: '/api/models' }, 'roster'), false);
  assert.equal(faultTarget('GET', '/api/product-conversations/events', { ...state, aggregatePath: '/api/product-conversations/events' }, 'roster'), false);
  for (const host of ['https://127.0.0.1:1', 'http://localhost:1', 'http://external.example:1', 'http://user:pass@127.0.0.1:1']) {
    assert.equal(faultTarget('GET', host + state.aggregatePath, state, 'roster'), false);
    assert.equal(faultTarget('GET', host + state.streamPath, state, 'stream'), false);
  }
  const global = { ...state, aggregatePath: '/api/global/coordinator/history/root' };
  assert.equal(faultTarget('GET', global.aggregatePath, global, 'roster'), true);
});

test('owned preview aborts transport only and drops exact live SSE once, not new subscriptions', async () => {
  const root = mkdtempSync(join(tmpdir(), 'opening-selftest-')); chmodSync(root, 0o700);
  const file = join(root, 'fault.json'), log = join(root, 'transport.jsonl');
  writeFileSync(file, JSON.stringify({ aggregatePath: '/api/product-conversations/product', failRoster: true,
    streamPath: '/api/conversations/successor/stream', drop: 0 }));
  const proxy = new EventEmitter();
  try {
    const transport = previewTransport('http://127.0.0.1:34567', file, log)['/api'];
    let requestDestroyed = 0, responseDestroyed = 0;
    const request = { method: 'GET', url: '/api/product-conversations/product', destroy: () => requestDestroyed++ };
    const response = { destroy: () => responseDestroyed++, end: () => assert.fail('must not fabricate response') };
    assert.equal(transport.bypass(request, response), false);
    assert.equal(requestDestroyed, 1); assert.equal(responseDestroyed, 1);
    assert.equal(transport.bypass({ ...request, method: 'POST' }, response), undefined);
    transport.configure(proxy);
    let destroyed = 0;
    const downstream = new EventEmitter(); downstream.destroy = () => destroyed++;
    proxy.emit('proxyRes', { destroy: () => destroyed++ }, { method: 'GET', url: '/api/conversations/successor/stream?open_id=owned' }, downstream);
    const foreign = new EventEmitter(); foreign.destroy = () => assert.fail('foreign stream');
    proxy.emit('proxyRes', { destroy: () => assert.fail('foreign upstream') }, { method: 'GET', url: '/api/conversations/foreign/stream' }, foreign);
    updateFault(file, { drop: 1 });
    await new Promise(r => setTimeout(r, 80)); assert.equal(destroyed, 2);
    await new Promise(r => setTimeout(r, 80)); assert.equal(destroyed, 2);
    const entries = readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line));
    assert.deepEqual(entries.filter(e => e.action === 'drop-streams').map(e => e.count), [1]);
    const newlyOpened = new EventEmitter(); let newDestroyed = 0; newlyOpened.destroy = () => newDestroyed++;
    proxy.emit('proxyRes', { destroy: () => newDestroyed++ }, { method: 'GET', url: '/api/conversations/successor/stream' }, newlyOpened);
    await new Promise(r => setTimeout(r, 50)); assert.equal(newDestroyed, 0);
    proxy.emit('close'); assert.equal(newDestroyed, 2);
  } finally { proxy.emit('close'); rmSync(root, { recursive: true, force: true }); }
});

test('fault control rejects shared permissions and symlink files', () => {
  const root = mkdtempSync(join(tmpdir(), 'opening-selftest-'));
  const file = join(root, 'fault'); writeFileSync(file, '{}');
  try {
    chmodSync(root, 0o755); assert.throws(() => ownedFile(file));
    chmodSync(root, 0o700); assert.equal(ownedFile(file), file);
    const link = join(root, 'link'); symlinkSync(file, link); assert.throws(() => ownedFile(link));
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('release is exact and attempted once even after child/control failure', async () => {
  for (const fail of [false, true]) {
    const state = { armed: true, releaseAttempted: false, predecessor: 'p', key: 'k' };
    const calls = [];
    const send = async body => { calls.push(body); if (fail) throw new Error('child/control failed'); return { state: 'released' }; };
    if (fail) await assert.rejects(releaseOnce(state, send)); else await releaseOnce(state, send);
    await releaseOnce(state, send);
    assert.deepEqual(calls, [{ command: 'release', predecessor: 'p', client_turn_key: 'k' }]);
  }
  await releaseOnce({ armed: false }, () => assert.fail('unarmed release'));
});

test('IPC command bounds reject before touching a socket', async () => {
  await assert.rejects(ipc('/never-accessed', { command: 'arm', predecessor: 'p', client_turn_key: 'x'.repeat(1024) }));
});

// Controllable mocks exercise the same response producer installed by run().
const { installAggregateRecorder, capturePolicy, REQUIRED_BODY_DEADLINE_MS, REQUIRED_BODY_MAX_BYTES } =
  await import('./qualify-history-opening-realserver.mjs');
const aggregate = { kind: 'ordinary', endpoint: '/api/product-conversations/product', members: ['root', 'successor'],
  manual_opening: { edge: { product_conversation_id: 'product' } } };
const aggregateBody = { segments: [], boundaries: [], canonical_root: { transcript_row_id: 'root' },
  product_conversation_id: 'product', requested_transcript_row_id: 'root', latest_transcript_row_id: 'successor' };
function recorderFixture(options = {}, a = aggregate) {
  const page = new EventEmitter(), receipts = [], snapshots = [];
  page.mainFrame = () => page;
  const recorder = installAggregateRecorder(page, a, (file, value) => receipts.push({ file, value: structuredClone(value) }), snapshots, options);
  const request = (path = a.endpoint, method = 'GET', body = null) => ({
    url: () => 'http://127.0.0.1:1234' + path, method: () => method, postData: () => body,
    failure: () => ({ errorText: 'net::ERR_EMPTY_RESPONSE' }),
  });
  const respond = (req, text = async () => JSON.stringify(aggregateBody), status = 200, type = 'application/json') => {
    let textCalls = 0;
    page.emit('response', { request: () => req, url: req.url, status: () => status,
      headers: () => ({ 'content-type': type }), text: () => { textCalls++; return text(); } });
    return () => textCalls;
  };
  return { page, recorder, receipts, snapshots, request, respond };
}

test('typed owned policy covers exact aggregate and source aliases, including Global, not list/events/foreign APIs', () => {
  assert.equal(REQUIRED_BODY_DEADLINE_MS, 11000); assert.equal(REQUIRED_BODY_MAX_BYTES, 2 * 1024 * 1024);
  for (const a of [aggregate, { ...aggregate, kind: 'global', endpoint: '/api/global/coordinator/history/root' }]) {
    const prefix = a.kind === 'global' ? '/api/global/coordinator/history/' : '/api/product-conversations/';
    for (const path of [a.endpoint, prefix + 'successor']) assert.equal(capturePolicy(a, 'GET', 'http://x' + path + '?message_limit=1').kind, 'required_aggregate_json');
    for (const path of ['/api/product-conversations', prefix + 'foreign', '/api/product-conversations/events', '/api/models', a.endpoint + '/extra']) {
      assert.equal(capturePolicy(a, 'GET', 'http://x' + path).kind, 'metadata_only');
    }
    assert.equal(capturePolicy(a, 'POST', 'http://x' + a.endpoint).kind, 'metadata_only');
  }
});

test('unavailable unrelated/telemetry bodies are metadata-only: text never called, request payload retained', async () => {
  const f = recorderFixture();
  for (const [path, method, body] of [['/api/models', 'GET', null], ['/api/telemetry', 'POST', '{"journey":"owned"}']]) {
    const req = f.request(path, method, body); f.page.emit('request', req);
    const calls = f.respond(req, () => assert.fail('unnecessary body retrieval'));
    assert.equal(calls(), 0); assert.equal(f.recorder.requests.at(-1).body, body);
  }
  const drain = await f.recorder.sealAndDrain(); assert.equal(drain.partial, false); f.recorder.assertQualified();
  assert.equal(f.snapshots.length, 0);
});

test('required 200 unavailable body fails with immutable full metadata before body access, not pageerror', async () => {
  const f = recorderFixture(); const req = f.request(); f.page.emit('request', req);
  f.page.emit('framenavigated', f.page);
  const actual = 'Protocol error (Network.getResponseBody): No data found for resource with given identifier';
  f.respond(req, async () => {
    assert.equal(f.receipts.at(-1).value.outcome, 'response_metadata');
    f.page.emit('framenavigated', f.page); throw new Error(actual);
  });
  const drain = await f.recorder.sealAndDrain();
  assert.equal(drain.partial, false); assert.equal(drain.pending_required, 0);
  assert.equal(f.recorder.applicationErrors.length, 0); assert.throws(() => f.recorder.assertQualified());
  const error = f.recorder.captureErrors[0];
  assert.equal(error.reason, 'body_transport'); assert.match(error.error, /No data found/);
  assert.equal(error.url, req.url()); assert.equal(error.method, 'GET'); assert.equal(error.status, 200);
  assert.equal(error.type, 'application/json'); assert.equal(error.request_ordinal, 1);
  assert.equal(error.navigation_epoch, 0); assert.equal(error.response_navigation_epoch, 1);
  assert.ok(Number.isFinite(error.at) && Number.isFinite(error.response_at));
  assert.equal(error.identity_source, 'playwright_request_local_ordinal'); assert.equal('requestId' in error, false);
});

test('required invalid JSON, wrong aggregate, byte overlimit, HTTP/content-type failures all fail qualification', async () => {
  for (const [reason, text, status, type, options] of [
    ['parse', '{oops', 200, 'application/json', {}],
    ['wrong_aggregate', JSON.stringify({ ...aggregateBody, product_conversation_id: 'foreign' }), 200, 'application/json', {}],
    ['wrong_aggregate', JSON.stringify({ ...aggregateBody, canonical_root: { transcript_row_id: 'foreign' } }), 200, 'application/json', {}],
    ['wrong_aggregate', '{}', 200, 'application/json', {}],
    ['over_limit', 'é'.repeat(6), 200, 'application/json', { maxBytes: 10 }],
    ['required_contract', '{}', 503, 'application/json', {}],
    ['required_contract', '{}', 200, 'text/html', {}],
  ]) {
    const f = recorderFixture(options); const req = f.request(); f.page.emit('request', req);
    f.respond(req, async () => text, status, type); const drain = await f.recorder.sealAndDrain();
    assert.equal(drain.partial, false); assert.equal(f.recorder.captureErrors[0].reason, reason);
    assert.equal(f.recorder.captureErrors[0].url, req.url()); assert.throws(() => f.recorder.assertQualified());
    if (['parse', 'wrong_aggregate'].includes(reason)) assert.equal(f.receipts.find(r => r.value.outcome === 'body_received').value.raw, text);
    if (reason === 'over_limit') assert.equal(f.recorder.captureErrors[0].body_bytes, 12);
  }
});

test('required deadline and disposal terminate bounded sealed drain without late snapshots', async () => {
  for (const reason of ['deadline', 'disposed']) {
    const f = recorderFixture({ deadlineMs: 10 }); const req = f.request(); f.page.emit('request', req);
    let complete;
    f.respond(req, () => new Promise(resolve => { complete = resolve; }));
    await new Promise(resolve => setImmediate(resolve));
    const drained = f.recorder.sealAndDrain(); assert.equal(f.page.listenerCount('response'), 0);
    if (reason === 'disposed') f.page.emit('close');
    const drain = await drained; assert.equal(drain.partial, false); assert.equal(drain.pending_required, 0);
    assert.equal(f.recorder.captureErrors[0].reason, reason); assert.throws(() => f.recorder.assertQualified());
    complete(JSON.stringify(aggregateBody)); await new Promise(resolve => setImmediate(resolve));
    assert.equal(f.snapshots.length, 0);
  }
});

test('sealed drain includes all outstanding required responses, preserves raw and parsed identity', async () => {
  const f = recorderFixture(); let complete;
  const req = f.request(); f.page.emit('request', req);
  f.respond(req, () => new Promise(resolve => { complete = resolve; }));
  await new Promise(resolve => setImmediate(resolve));
  const drained = f.recorder.sealAndDrain(); assert.equal(f.page.listenerCount('response'), 0);
  f.respond(f.request('/api/models'), () => assert.fail('sealed listener'));
  complete(JSON.stringify(aggregateBody)); const drain = await drained;
  assert.equal(drain.partial, false); assert.equal(drain.required_outcomes[0].outcome, 'captured');
  assert.equal(f.snapshots[0].raw, JSON.stringify(aggregateBody));
  assert.equal(f.snapshots[0].parsed_identity.product_conversation_id, 'product');
  f.recorder.assertQualified(); assert.equal(await f.recorder.sealAndDrain(), drain);
});

test('missing required outcome fails; requestfailed transport remains distinct from 200 capture failure; app errors always fail', async () => {
  const f = recorderFixture(); const req = f.request(); f.page.emit('request', req);
  await f.recorder.sealAndDrain(); assert.equal(f.recorder.captureErrors[0].reason, 'missing_response');
  assert.throws(() => f.recorder.assertQualified());
  const fault = recorderFixture({ expectedRequestFailure: entry => entry.url === 'http://127.0.0.1:1234' + aggregate.endpoint && entry.method === 'GET' }); const failedReq = fault.request(); fault.page.emit('request', failedReq);
  fault.page.emit('requestfailed', failedReq); const drain = await fault.recorder.sealAndDrain();
  assert.equal(drain.required_outcomes[0].outcome, 'request_failed'); assert.equal(fault.recorder.captureErrors.length, 0);
  assert.equal(fault.recorder.failed[0].url, failedReq.url()); fault.recorder.assertQualified();
  fault.page.emit('pageerror', new Error('application bug')); assert.throws(() => fault.recorder.assertQualified());
  assert.equal(fault.recorder.applicationErrors[0].error, 'Error: application bug');
  const unexpected = recorderFixture(); const other = unexpected.request(); unexpected.page.emit('request', other);
  unexpected.page.emit('requestfailed', other); await unexpected.recorder.sealAndDrain();
  assert.throws(() => unexpected.recorder.assertQualified(), /exact expected transport failure/);
});

test('actual Global producer captures owned source alias and a registered committed successor', async () => {
  const a = { ...aggregate, kind: 'global', endpoint: '/api/global/coordinator/history/root' };
  const f = recorderFixture({}, a);
  f.recorder.registerMember('new-successor');
  for (const id of ['successor', 'new-successor']) {
    const req = f.request('/api/global/coordinator/history/' + id); f.page.emit('request', req); f.respond(req);
  }
  await f.recorder.sealAndDrain(); f.recorder.assertQualified(); assert.equal(f.snapshots.length, 2);
  assert.equal(a.members.includes('new-successor'), false, 'recorder does not mutate runtime fixture');
});

test('byte bound accepts exact 2MiB raw JSON and rejects one extra byte without truncating a successful receipt', async () => {
  const base = JSON.stringify(aggregateBody);
  for (const extra of [0, 1]) {
    const f = recorderFixture(); const req = f.request(); f.page.emit('request', req);
    const raw = base + ' '.repeat(REQUIRED_BODY_MAX_BYTES - Buffer.byteLength(base) + extra);
    f.respond(req, async () => raw); await f.recorder.sealAndDrain();
    if (extra) { assert.throws(() => f.recorder.assertQualified()); assert.equal(f.recorder.captureErrors[0].reason, 'over_limit'); }
    else { f.recorder.assertQualified(); assert.equal(f.snapshots[0].raw, raw); }
  }
});

test('out-of-order concurrent response completions keep owning request identities and seal drains both', async () => {
  const f = recorderFixture(); const completions = [];
  for (const cursor of ['first', 'second']) {
    const req = f.request(aggregate.endpoint + '?before=' + cursor); f.page.emit('request', req);
    f.respond(req, () => new Promise(resolve => completions.push(resolve)));
  }
  await new Promise(resolve => setImmediate(resolve)); const drained = f.recorder.sealAndDrain();
  completions[1](JSON.stringify(aggregateBody)); await new Promise(resolve => setImmediate(resolve));
  completions[0](JSON.stringify(aggregateBody)); const drain = await drained; f.recorder.assertQualified();
  assert.deepEqual(f.snapshots.map(s => s.request_ordinal), [2, 1]);
  assert.deepEqual(drain.required_outcomes.map(s => s.request_ordinal), [1, 2]);
});

test('runtime seals before qualification, rechecks after cleanup and preserves failed cleanup semantics', () => {
  const script = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  assert.match(script, /await recorder\.sealAndDrain\(\); recorder\.assertQualified\(\);/);
  assert.match(script, /recorder\?\.assertQualified\(\)/);
  assert.match(script, /result\.application_page_errors = recorder\?\.applicationErrors/);
  assert.match(script, /result\.cdp_capture_errors = recorder\?\.captureErrors/);
  assert.match(script, /result\.cleanup_error = String\(error\); process\.exitCode = 1/);
});
