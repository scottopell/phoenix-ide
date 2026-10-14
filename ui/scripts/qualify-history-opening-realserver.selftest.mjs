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
