import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, chmodSync, writeFileSync, readFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { assertBackgroundReport } from './qualify-view-telemetry.mjs';
import { prepareJointFailureDiagnostic, TELEMETRY_MODULE_SHA256, TELEMETRY_DIAGNOSTICS_SHA256 } from './history-joint-telemetry.mjs';
import { sameEdge, openingMatches, faultTarget, previewTransport, updateFault, ownedFile, releaseOnce, ipc, persistedMessageSelector, acceptedWireCursor, retainCleanupFailure } from './qualify-history-opening-realserver.mjs';

test('fault remains active through acceptance and same-page reconnect, but new pin acquisition restores real transport', () => {
  const script = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const reconnect = script.indexOf("checked('same-page transport SSE drop/reconnect");
  const restored = script.indexOf('updateFault(fault, { failRoster: false })');
  const click = script.indexOf('await chapter.click()');
  const physical = script.indexOf("checked('accepted manual opening renders once");
  assert.ok(reconnect >= 0 && reconnect < restored && restored < click && click < physical);
  assert.match(script, /const restoredAggregate = a\.kind === 'ordinary' \? page\.waitForResponse/);
  assert.match(script, /const restoredResponse = await restoredAggregate/);
  assert.match(script, /await rendered\.waitFor\(\{ state: 'visible' \}\); assert\.equal\(await rendered\.count\(\), 1\)/);
});

test('aggregate transport failure proof does not fabricate an alert contract for background probes', () => {
  const script = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  assert.match(script, /const failedRoster = globalFaultObservation\?\.request \?\? await poll\(\(\) => failed\.slice\(failureStart\)\.find/);
  assert.match(script, /background aggregate ownership probe; no page-alert contract/);
  assert.match(script, /Accepted composer enabled despite roster failure/);
  assert.match(script, /actual composer message persisted/);
  assert.doesNotMatch(script, /page\.locator\('\[role="alert"\]'\)/);
});

test('accepted cursor uses owning Init incarnation and advancing JSON sequence, not invented transport ID', () => {
  const incarnation = '04bd47cf-71be-473f-b66c-818211975b81';
  const init = { requestId: '14019.119', eventName: 'init', eventId: '', data: JSON.stringify({ stream_incarnation: incarnation, sequence_id: 0, conversation: { id: 'successor' }, opening: { edge } }) };
  const event = { requestId: init.requestId, eventName: 'message', eventId: '', data: JSON.stringify({ sequence_id: 1, opening: accepted, message: { message_id: accepted.accepted_message_id } }) };
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
  product_conversation_id: 'product', manual_opening: { ...accepted, edge: { ...edge, predecessor_transcript_row_id: 'root' } } };
const aggregateBody = { segments: aggregate.members.map((id, i) => ({ transcript_row_id: id, segment_ordinal: i, messages: [], handoff: null })),
  canonical_root: { transcript_row_id: 'root' }, latest_opening: aggregate.manual_opening,
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

test('sealed drain includes all outstanding required responses, preserves raw and parsed identity', async () => {
  const f = recorderFixture(); let complete;
  const req = f.request(); f.page.emit('request', req);
  f.respond(req, () => new Promise(resolve => { complete = resolve; }));
  await new Promise(resolve => setImmediate(resolve));
  const drained = f.recorder.sealAndDrain(); assert.equal(f.page.listenerCount('response'), 1);
  f.respond(f.request('/api/models'), () => assert.fail('sealed listener'));
  complete(JSON.stringify(aggregateBody)); const drain = await drained;
  assert.equal(drain.partial, false); assert.equal(drain.required_outcomes[0].outcome, 'captured');
  assert.equal(f.snapshots[0].raw, JSON.stringify(aggregateBody));
  assert.equal(f.snapshots[0].parsed_identity.product_conversation_id, 'product');
  f.recorder.assertQualified(); assert.equal(await f.recorder.sealAndDrain(), drain);
});

test('missing required outcome fails; requestfailed transport remains distinct from 200 capture failure; app errors always fail', async () => {
  const clock = controlledClock();
  const f = recorderFixture({ clock }); const req = f.request(); f.page.emit('request', req);
  const drained = f.recorder.sealAndDrain(); clock.advance(11000);
  await drained; assert.equal(f.recorder.captureErrors[0].reason, 'deadline');
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

test('synthetic Global alias uses its own selected row and registered committed evidence (not runtime)', async () => {
  const a = { ...aggregate, kind: 'global', endpoint: '/api/global/coordinator/history/root' };
  const f = recorderFixture({}, a);
  const body = { ...aggregateBody, requested_transcript_row_id: 'successor',
    segments: aggregateBody.segments.map(({ handoff, ...s }) => s), boundaries: [{ message_id: 'boundary', segment_ordinal: 0,
      predecessor_transcript_row_id: 'root', successor_transcript_row_id: 'successor', represented_successor_opening_message_id: null }] };
  const req = f.request('/api/global/coordinator/history/successor'); f.page.emit('request', req); f.respond(req, async () => JSON.stringify(body));
  await f.recorder.sealAndDrain(); f.recorder.assertQualified(); assert.equal(f.snapshots.length, 1);
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
  assert.match(script, /retainCleanupFailure\(result, 'browser_cleanup', error\); process\.exitCode = 1/);
});

for (const failure of ['browser_close', 'finalizer', 'inventory']) for (const withPrimary of [true, false])
test(`OFFLINE UNIT ONLY actual runner catch retains ${failure} secondary with primary=${withPrimary}`, async () => {
  const script = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const start = script.indexOf('  } catch (error) {\n    result.error = ', script.indexOf('async function run()'));
  const end = script.lastIndexOf('\n}\nif (process.argv');
  assert.ok(start > 0 && end > start);
  const tail = script.slice(start, end);
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  const execute = new AsyncFunction('ctx', `
    const { primary, result, process, page, recorder, gate, command, releaseOnce, record,
      jointTelemetry, finalizeHistoryTelemetry, diagnoseHistoryFinalization, historyTelemetry, updateFault, fault,
      clearTimeout, hardStop, writeFileSync, resolve, output, retainCleanupFailure } = ctx;
    let browser = ctx.browser;
    try { if (primary) throw primary; result.status = 'passed_assertions';
    ${tail}
  `);
  const primary = withPrimary ? new Error('ORIGINAL_DONOR_FAILURE') : null, result = { status: 'failed' }, writes = [];
  const modulePath = fileURLToPath(new URL('./qualify-view-telemetry.mjs', import.meta.url));
  const helper = fileURLToPath(new URL('./view-telemetry-diagnostics.mjs', import.meta.url));
  const diagnose = prepareJointFailureDiagnostic({ modulePath, dependencies: {
    [modulePath]: TELEMETRY_MODULE_SHA256, [helper]: TELEMETRY_DIAGNOSTICS_SHA256 } });
  let actualError; try { assertBackgroundReport({}, {}); } catch (error) { actualError = error; }
  assert.ok(actualError instanceof Error);
  const process = { exitCode: 0 };
  await execute({ primary, result, process, page: null, recorder: null, gate: { armed: false },
    command: async () => ({}), releaseOnce: async () => {}, record: () => {}, jointTelemetry: true,
    browser: { close: async () => { if (failure === 'browser_close') throw new Error('OUTER_BROWSER_CLEANUP_FAILURE'); } },
    finalizeHistoryTelemetry: () => { assert.equal(failure, 'finalizer'); throw actualError; },
    diagnoseHistoryFinalization: error => { assert.equal(error, actualError); return diagnose(error, 'history_finalization'); },
    historyTelemetry: { snapshot: () => { if (failure === 'inventory') throw actualError; return {}; } },
    updateFault: () => {}, fault: {},
    clearTimeout: () => {}, hardStop: null, writeFileSync: (path, body) => writes.push(JSON.parse(body)),
    resolve: (...parts) => parts.join('/'), output: '/UNIT_ONLY', retainCleanupFailure });
  assert.equal(result.error, primary?.stack);
  assert.equal(process.exitCode, 1);
  assert.equal(result.status, 'failed');
  assert.deepEqual(result.cleanup_failures.map(f => f.stage), failure === 'browser_close'
    ? ['joint_browser_cleanup', 'browser_cleanup'] : ['joint_browser_cleanup']);
  assert.equal(writes[0].error, primary?.stack);
  const secondary = result.cleanup_failures[0];
  if (failure === 'finalizer') {
    assert.equal(secondary.error, actualError.stack.slice(0, 2048));
    assert.deepEqual(secondary.diagnostic, diagnose(actualError, 'history_finalization'));
    assert.equal(secondary.diagnostic.resolution, 'verified_assertion_site');
    assert.equal(secondary.diagnostic.line, 530);
    assert.equal(writes[0].cleanup_failures[0].diagnostic.phase, 'history_finalization');
  } else assert.equal(secondary.diagnostic, undefined);
});

test('cleanup-only runner failures fail closed and remain bounded (UNIT ONLY)', () => {
  const result = { status: 'passed_assertions' };
  for (let index = 0; index < 10; index++) retainCleanupFailure(result, 'browser_cleanup', new Error('x'.repeat(4096)));
  assert.equal(result.status, 'failed'); assert.equal(result.error, undefined);
  assert.equal(result.cleanup_failures.length, 8); assert.equal(result.cleanup_overflow, true);
  assert.equal(result.cleanup_failures[0].error.length, 2048); assert.equal(result.cleanup_failures[0].truncated, true);
});

test('authenticated opening session precedes observers/navigation and protects direct HTTP/SSE (source control)', () => {
  const script = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const run = script.slice(script.indexOf('async function run()'));
  assert.ok(run.indexOf('await authenticateOwnedContext(context, api)') < run.indexOf('page = await context.newPage()'));
  const auth = run.indexOf('await authenticateOwnedContext(context, api)');
  const ownership = run.indexOf('await installHistoryRequestOwnership(context, jointTelemetry)');
  const observed = run.indexOf('installObservedHistoryTelemetry(page, ui, requestOwnership)');
  assert.ok(auth > 0 && auth < ownership && ownership < observed);
  assert.match(run, /fetch\(api \+ path, \{ method, headers: \{ Cookie: sessionCookie/);
  assert.match(run, /\/stream`, \{ headers: \{ Cookie: sessionCookie \}/);
  assert.doesNotMatch(run, /record\([^\n]*sessionCookie/);
});

function controlledClock() {
  let time = 0, serial = 0;
  const timers = new Map();
  return { now: () => time,
    setTimeout(fn, delay) { const id = ++serial; timers.set(id, { fn, at: time + delay }); return id; },
    clearTimeout(id) { timers.delete(id); },
    advance(ms) {
      const end = time + ms;
      for (;;) {
        const next = [...timers].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
        if (!next) break;
        time = next[1].at; timers.delete(next[0]); next[1].fn();
      }
      time = end;
    }, pending: () => [...timers.values()].map(t => t.at) };
}
const flush = () => new Promise(resolve => setImmediate(resolve));

test('preseal response arrivals captured during drain; unknown postseal identities typed outside capture', { timeout: 2000 }, async () => {
  const clock = controlledClock(), f = recorderFixture({ clock });
  const req = f.request(); f.page.emit('request', req);
  const drained = f.recorder.sealAndDrain(); assert.equal(f.recorder.sealAndDrain(), drained);
  assert.throws(() => f.recorder.assertQualified(), /completed drain/);
  const partial = f.recorder.partialInventory(); assert.equal(partial.partial, true);
  assert.equal(partial.pending_metadata[0].request_ordinal, 1);
  const unknown = f.request(); f.page.emit('request', unknown);
  f.respond(unknown, () => assert.fail('outside body')); f.page.emit('requestfailed', unknown);
  assert.equal(f.recorder.requests.length, 1); assert.equal(f.recorder.failed.length, 0);
  assert.deepEqual(f.receipts.filter(r => r.file === 'browser-outside-capture.jsonl').map(r => r.value.policy.kind), ['outside_capture', 'outside_capture']);
  clock.advance(5000); f.respond(req); const summary = await drained;
  assert.equal(summary.partial, false); assert.equal(summary.pending_required, 0);
  assert.equal(f.snapshots.length, 1); assert.equal(clock.pending().length, 0); f.recorder.assertQualified();
});

test('one absolute seal deadline caps late bodies and awaiting responses, no timer reset or late capture', { timeout: 2000 }, async () => {
  const clock = controlledClock(), f = recorderFixture({ clock }); const requests = [f.request(), f.request()];
  requests.forEach(req => f.page.emit('request', req));
  const drained = f.recorder.sealAndDrain(); clock.advance(10990); let complete;
  f.respond(requests[0], () => new Promise(resolve => { complete = resolve; })); await flush();
  assert.deepEqual(clock.pending(), [11000, 11000]);
  clock.advance(10); const summary = await drained;
  assert.equal(summary.pending_body_tasks, 0); assert.equal(summary.pending_required, 0);
  assert.deepEqual(f.recorder.captureErrors.map(e => e.reason), ['deadline', 'deadline']);
  assert.throws(() => f.recorder.assertQualified());
  complete(JSON.stringify(aggregateBody)); f.respond(requests[1]); await flush(); assert.equal(f.snapshots.length, 0);
});

test('postseal close fails awaiting response and reading body immediately as disposed', { timeout: 2000 }, async () => {
  const clock = controlledClock(), f = recorderFixture({ clock }); const req = f.request(), other = f.request();
  f.page.emit('request', req); f.page.emit('request', other); let complete;
  f.respond(req, () => new Promise(resolve => { complete = resolve; })); await flush();
  const drained = f.recorder.sealAndDrain(); f.page.emit('close'); await drained;
  assert.deepEqual(f.recorder.captureErrors.map(e => e.reason), ['disposed', 'disposed']);
  assert.equal(clock.now(), 0); assert.equal(clock.pending().length, 0);
  complete(JSON.stringify(aggregateBody)); await flush(); assert.equal(f.snapshots.length, 0); assert.throws(() => f.recorder.assertQualified());
});

test('postseal known requestfailed completes bounded drain; only exact expected fault qualifies', { timeout: 2000 }, async () => {
  for (const expected of [false, true]) {
    const clock = controlledClock(), f = recorderFixture({ clock, expectedRequestFailure: () => expected });
    const req = f.request(); f.page.emit('request', req); const drained = f.recorder.sealAndDrain();
    f.page.emit('requestfailed', req); const summary = await drained;
    assert.equal(summary.required_outcomes[0].outcome, 'request_failed'); assert.equal(clock.pending().length, 0);
    if (expected) f.recorder.assertQualified(); else assert.throws(() => f.recorder.assertQualified());
  }
  const f = recorderFixture({ clock: controlledClock() }); const req = f.request(); f.page.emit('request', req);
  const drained = f.recorder.sealAndDrain(); f.respond(req, async () => { throw new Error('preseal body rejected'); });
  await drained; assert.equal(f.recorder.captureErrors[0].reason, 'body_transport'); assert.throws(() => f.recorder.assertQualified());
});

test('body capture deadline applies before seal too (real clock only guards test liveness)', { timeout: 2000 }, async () => {
  const clock = controlledClock(), f = recorderFixture({ clock }); const req = f.request(); f.page.emit('request', req);
  f.respond(req, () => new Promise(() => {})); await flush(); clock.advance(11000); await flush();
  await f.recorder.sealAndDrain(); assert.equal(f.recorder.captureErrors[0].reason, 'deadline'); assert.throws(() => f.recorder.assertQualified());
});

test('chat ACK retains exact actual URL/status/body/canonical ID and rejects duplicate or steering', async () => {
  const { validateChatAck } = await import('./qualify-history-opening-realserver.mjs');
  const body = { queued: true, steering: false, already_persisted: false };
  const receipt = { url: 'http://127.0.0.1:1/api/conversations/successor/chat', status: 200,
    raw: JSON.stringify(body), body, canonical_message_id: 'successor:key' };
  validateChatAck(receipt, 'successor', 'key');
  const pinnedBody = { queued: true };
  validateChatAck({ ...receipt, raw: JSON.stringify(pinnedBody), body: pinnedBody }, 'successor', 'key');
  for (const mutation of [{ url: receipt.url.replace('successor', 'foreign') }, { status: 201 },
    { canonical_message_id: 'foreign:key' }, { body: { ...body, already_persisted: true } },
    { body: { ...body, steering: true } }, { body: { ...body, queued: false } }]) {
    assert.throws(() => validateChatAck({ ...receipt, ...mutation }, 'successor', 'key'));
  }
});


test('manual full-edge oracle rejects each field independently for fixture and registered pending/accepted evidence', async () => {
  const { validateOwnedAggregate } = await import('./qualify-history-opening-realserver.mjs');
  const fields = ['product_conversation_id', 'predecessor_transcript_row_id', 'successor_transcript_row_id', 'continuation_message_id'];
  for (const status of ['fixture', 'pending', 'accepted']) {
    const a = structuredClone(aggregate), data = structuredClone(aggregateBody);
    if (status !== 'fixture') {
      a.opening_evidence = { successor: { ...a.manual_opening.edge, client_turn_key: 'key' } };
      data.latest_opening = status === 'pending' ? { status, edge: { ...a.manual_opening.edge },
        opening_authority: 'user_authorized_instruction', client_turn_key: 'key' }
        : { ...a.manual_opening, accepted_message_id: 'successor:key' };
    }
    validateOwnedAggregate(a, data, 'http://127.0.0.1:1' + a.endpoint);
    for (const field of fields) {
      const bad = structuredClone(data); bad.latest_opening.edge[field] = 'foreign';
      assert.throws(() => validateOwnedAggregate(a, bad, 'http://127.0.0.1:1' + a.endpoint), field);
    }
  }
  const script = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  assert.match(script, /validateOwnedAggregate\([^;]*fetchedLatestAccepted, restoredResponse.url\(\)\)/);
  assert.match(script, /openingMatches\(fetchedLatestAccepted.latest_opening, entered, 'accepted'\)/);
});

function documentDispatch(f, target, before = () => {}) {
  return () => {
    before();
    f.page.url = () => target;
    const req = { ...f.request('/c/product'), url: () => target,
      isNavigationRequest: () => true, frame: () => f.page };
    const response = { request: () => req, url: () => target, status: () => 200 };
    f.page.emit('request', req); f.page.emit('response', response); f.page.emit('framenavigated', f.page);
    return response;
  };
}
const navigationIntent = { kind: 'explicit_document_navigation', phase: 'pending_historical',
  target: 'http://127.0.0.1:1234/c/product?source_transcript=root' };

test('driver navigation waits for live required headers/body and new admissions, retaining actual raw receipts', async () => {
  const clock = controlledClock(), f = recorderFixture({ clock }, { ...aggregate, route: '/c/product' });
  const a = f.request(), b = f.request(); f.page.emit('request', a);
  let navigations = 0, finishA, finishB;
  const nav = f.recorder.navigateAfterRequiredBodies(navigationIntent,
    documentDispatch(f, navigationIntent.target, () => {
      navigations++; assert.equal(f.snapshots.length, 2);
      assert.ok(f.snapshots.every(s => s.raw_sha256 && s.raw === JSON.stringify(aggregateBody)));
    }));
  await flush(); assert.equal(navigations, 0);
  assert.ok(!f.receipts.some(r => r.value.outcome === 'navigation_intent'));
  f.respond(a, () => new Promise(resolve => { finishA = resolve; })); await flush();
  f.page.emit('request', b); finishA(JSON.stringify(aggregateBody)); await flush();
  assert.equal(navigations, 0);
  f.respond(b, () => new Promise(resolve => { finishB = resolve; })); await flush();
  assert.equal(navigations, 0); finishB(JSON.stringify(aggregateBody)); await nav;
  assert.equal(navigations, 1); assert.equal(clock.pending().length, 0);
  const current = f.request(); f.page.emit('request', current); f.respond(current); await flush();
  await f.recorder.sealAndDrain(); f.recorder.assertQualified();
});

test('pre-navigation absolute deadline and close fail without goto, reset or late body admission', async () => {
  for (const mode of ['headers', 'body', 'close']) {
    const clock = controlledClock(), f = recorderFixture({ clock }, { ...aggregate, route: '/c/product' });
    const req = f.request(); f.page.emit('request', req); let complete, navigations = 0;
    if (mode === 'body') { f.respond(req, () => new Promise(resolve => { complete = resolve; })); await flush(); }
    const nav = f.recorder.navigateAfterRequiredBodies(navigationIntent, () => { navigations++; });
    const rejected = assert.rejects(nav);
    if (mode === 'close') f.page.emit('close');
    else { clock.advance(10000); f.page.emit('request', f.request()); clock.advance(1000); }
    await rejected; await flush(); assert.equal(navigations, 0);
    complete?.(JSON.stringify(aggregateBody)); await flush();
    assert.equal(f.snapshots.length, 0); assert.equal(clock.pending().length, 0);
    await f.recorder.sealAndDrain(); assert.throws(() => f.recorder.assertQualified());
  }
});

test('required body failure still blocks driver navigation; no supersession or replacement receipt rescue', async () => {
  const f = recorderFixture({}, { ...aggregate, route: '/c/product' });
  const req = f.request(); f.page.emit('request', req); let navigations = 0;
  const nav = f.recorder.navigateAfterRequiredBodies(navigationIntent, () => { navigations++; });
  const rejected = assert.rejects(nav, /pre-navigation capture errors/);
  f.respond(req, async () => { throw new Error('Network.getResponseBody: No resource with given identifier found'); });
  await rejected; assert.equal(navigations, 0);
  const later = f.request(); f.page.emit('request', later); f.respond(later); await flush();
  await f.recorder.sealAndDrain(); assert.throws(() => f.recorder.assertQualified());
  assert.equal(f.recorder.captureErrors[0].reason, 'body_transport');
});

test('request admitted at navigation handoff remains required and failed body cannot disappear', async () => {
  const clock = controlledClock(), f = recorderFixture({ clock }, { ...aggregate, route: '/c/product' });
  let finish;
  await f.recorder.navigateAfterRequiredBodies(navigationIntent, documentDispatch(f, navigationIntent.target, () => {
    const late = f.request(); f.page.emit('request', late);
    f.respond(late, () => new Promise(resolve => { finish = resolve; }));
  }));
  await flush(); finish(Promise.reject(new Error('body lost'))); await flush();
  await f.recorder.sealAndDrain(); assert.throws(() => f.recorder.assertQualified());
  assert.equal(f.recorder.captureErrors[0].reason, 'body_transport');
});

test('both actual driver document navigations use tested body-settling wrapper without a capture exception', () => {
  const script = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  assert.equal((script.match(/await recorder\.navigateAfterRequiredBodies\(/g) ?? []).length, 2);
  assert.doesNotMatch(script, /const (?:manual_identity|pending_historical)Navigation = recorder\.beginNavigation/);
  assert.match(script, /const response = await dispatch\(\);\s*this\.confirmNavigation\(token, response\)/);
});

// Fabricated wire UNIT ONLY: execute the installed coverage block, not a replay model.
function reconnectCoverage(init, overrides = {}) {
  const source = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const start = source.indexOf('  assert.equal(init.pending_truncated');
  const end = source.indexOf('  assert.ok(published.message', start);
  assert.ok(start >= 0 && end > start, 'installed reconnect coverage block required');
  const message = (message_id, text, sequence_id) => ({ message_id, conversation_id: 'successor', message_type: 'user', content: { text }, sequence_id });
  const context = {
    assert, init, requestCursor: 5, latest: 'successor', id: 'successor:key', instruction: 'opening', requestGeneration: 1,
    ack: { canonical_message_id: 'successor:chat' }, chatRequest: { text: 'submitted' },
    preDropRow: { conversation: { id: 'successor', transcript_generation: 1 },
      messages: [message('successor:key', 'opening', 1), message('successor:chat', 'submitted', 2)] }, ...overrides,
  };
  return new Function(...Object.keys(context), source.slice(start, end))(...Object.values(context));
}
const reconnectSnapshot = coverage => ({ transcript_coverage: coverage, pending_truncated: false,
  pending_anchor_sequence_id: 5, last_sequence_id: 8, pending_events: [6, 7, 8].map(sequence_id => ({ sequence_id })),
  messages: coverage === 'preserve' ? [] : [
    { message_id: 'successor:key', conversation_id: 'successor', message_type: 'user', content: { text: 'opening' } },
    { message_id: 'successor:chat', conversation_id: 'successor', message_type: 'user', content: { text: 'submitted' } },
  ],
});
for (const coverage of ['preserve', 'complete', 'tail']) {
  test(`reconnect ${coverage} accepts exact contiguous interval and empty interval`, () => {
    reconnectCoverage(reconnectSnapshot(coverage));
    reconnectCoverage({ ...reconnectSnapshot(coverage), last_sequence_id: 5, pending_events: [] });
  });
  for (const [name, mutate] of Object.entries({
    interior_gap: d => d.pending_events.splice(1, 1), missing_first: d => d.pending_events.shift(),
    missing_tip: d => d.pending_events.pop(), duplicate: d => d.pending_events.splice(1, 0, { sequence_id: 6 }),
    reversed: d => d.pending_events.reverse(), beyond_tip: d => d.pending_events.push({ sequence_id: 9 }),
    anchor_missing: d => { delete d.pending_anchor_sequence_id; }, anchor_negative: d => { d.pending_anchor_sequence_id = -1; },
    anchor_fractional: d => { d.pending_anchor_sequence_id = 5.5; }, anchor_string: d => { d.pending_anchor_sequence_id = '5'; },
    anchor_past_tip: d => { d.pending_anchor_sequence_id = 9; },
    anchor_unsafe: d => { d.pending_anchor_sequence_id = Number.MAX_SAFE_INTEGER + 1; },
    empty_replay_gap: d => { d.pending_events = []; },
    truncated: d => { d.pending_truncated = true; },
  })) test(`reconnect ${coverage} rejects ${name}`, () => {
    const value = reconnectSnapshot(coverage); mutate(value); assert.throws(() => reconnectCoverage(value));
  });
}
for (const coverage of ['complete', 'tail']) {
  for (const [name, mutate] of Object.entries({
    missing: d => d.messages.pop(), duplicate: d => d.messages.push(structuredClone(d.messages[1])),
    wrong_owner: d => { d.messages[1].conversation_id = 'foreign'; }, wrong_type: d => { d.messages[1].message_type = 'agent'; },
    wrong_content: d => { d.messages[1].content.text += 'corrupt'; },
  })) test(`reconnect ${coverage} rejects ${name} acknowledged chat`, () => {
    const value = reconnectSnapshot(coverage); mutate(value); assert.throws(() => reconnectCoverage(value));
  });
  test(`reconnect ${coverage} replay is anchored by snapshot, not request cursor`, () => {
    reconnectCoverage({ ...reconnectSnapshot(coverage), pending_anchor_sequence_id: 4,
      pending_events: [5, 6, 7, 8].map(sequence_id => ({ sequence_id })) });
    reconnectCoverage({ ...reconnectSnapshot(coverage), pending_anchor_sequence_id: 6,
      pending_events: [7, 8].map(sequence_id => ({ sequence_id })) });
  });
}
test('preserve cannot claim an anchor newer than retained cursor', () => {
  assert.throws(() => reconnectCoverage({ ...reconnectSnapshot('preserve'), pending_anchor_sequence_id: 6 }));
});
