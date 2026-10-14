import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { EventEmitter } from 'node:events';
import { acceptedManualOpening, tailSamplesReady, olderRequest, failedOlderRequest, retryMatches, offlineOlderWitness } from './qualify-history-realserver.mjs';
import { loopback, sourceLink, anchorMatches, occurrences, belongsToAggregate, APPEND_PROBE, CONTINUATION_PROBE, probeText, boundedDiagnosticJson, COMPOSER_SELECTOR, pinRejection } from './qualify-history-realserver.mjs';

test('only literal HTTP loopback ports are accepted', () => {
  assert.equal(loopback('http://127.0.0.1:54321'), 'http://127.0.0.1:54321');
  for (const value of ['https://127.0.0.1:1', 'http://localhost:1', 'http://example.com:1', 'http://127.0.0.1', 'http://x:y@127.0.0.1:1', 'http://127.0.0.1:1/path']) {
    assert.throws(() => loopback(value));
  }
});
test('exact source pins and tool/message escaping retain unrelated query', () => {
  const url = new URL(sourceLink({ route: '/global/leaf' }, 'root', 'message/id', 'tool:id'), 'http://127.0.0.1:1');
  assert.equal(url.searchParams.get('source_transcript'), 'root');
  assert.equal(url.searchParams.get('source_tool'), 'tool:id');
  assert.equal(url.searchParams.get('qualification'), 'retained');
  assert.equal(url.hash, '#message-message%2Fid');
});
test('physical anchor requires same stable key and normative two-pixel agreement', () => {
  const before = { key: 'same', offset: -3 };
  assert.equal(anchorMatches(before, { key: 'same', offset: -1 }), true);
  assert.equal(anchorMatches(before, { key: 'same', offset: -.99 }), false);
  assert.equal(anchorMatches(before, { key: 'different', offset: -3 }), false);
  assert.equal(anchorMatches(before, null), false);
  assert.equal(anchorMatches(before, { key: 'same', offset: NaN }), false);
});
test('pagination occurrences distinguish sequence restarts across real members', () => {
  const snapshot = { segments: [0, 1, 2].map(n => ({ segment_ordinal: n, messages: [{ sequence_id: 1, message_id: `row${n}` }] })) };
  assert.deepEqual(occurrences(snapshot), [[0, 1, 'row0'], [1, 1, 'row1'], [2, 1, 'row2']]);
});
test('Global responses can change pinned member without changing aggregate identity', () => {
  const a = { kind: 'global', members: ['root', 'child'] };
  const response = { url: 'http://127.0.0.1:1/api/global/coordinator/history/child?before=real', data: { canonical_root: { transcript_row_id: 'root' } } };
  assert.equal(belongsToAggregate(a, response), true);
  assert.equal(belongsToAggregate({ ...a, members: ['foreign'] }, response), false);
  assert.equal(belongsToAggregate({ kind: 'ordinary', endpoint: '/api/product-conversations/pc' }, { ...response, url: 'http://127.0.0.1:1/api/product-conversations/pc?before=real' }), true);
});

test('rendered readiness probes survive frozen 40-character prompt clipping', () => {
  for (const probe of [APPEND_PROBE, CONTINUATION_PROBE]) {
    assert.ok(probeText(probe).slice(0, 40).includes(probe));
  }
});

test('diagnostic bounds are bytes and retain valid truncation metadata', () => {
  const bounded = boundedDiagnosticJson({ html: '🌏'.repeat(5000) }, 1024);
  assert.ok(bounded.length <= 1024);
  assert.equal(JSON.parse(bounded.toString()).truncated, true);
  assert.deepEqual(JSON.parse(boundedDiagnosticJson({ ok: true }).toString()), { ok: true });
});

test('composer targets the actual owning InputArea textarea identity', () => {
  const input = readFileSync(new URL('../src/components/InputArea.tsx', import.meta.url), 'utf8');
  assert.equal(COMPOSER_SELECTOR, 'textarea#message-input');
  assert.match(input, /<textarea[\s\S]*?id="message-input"/);
});

test('negative pin expectations belong to ordinary and Global admission gates', () => {
  for (const [kind, file] of [['ordinary', '../src/App.tsx'], ['global', '../src/pages/CoordinatorPage.tsx']]) {
    const owner = readFileSync(new URL(file, import.meta.url), 'utf8');
    for (const duplicate of [false, true]) {
      const expected = pinRejection(kind, duplicate);
      assert.ok(owner.includes(expected.text));
      assert.equal(expected.selector, kind === 'global' ? '.coordinator-error' : '[role="alert"]');
    }
  }
  assert.notEqual(pinRejection('ordinary', false).text, pinRejection('ordinary', true).text);
});

test('offline setup needs four fresh, stable, positive-overflow native-bottom tail frames', () => {
  const samples = [1, 2, 3, 4].map(frame => ({ frame, tailId: 'tail', tailKey: 'message:tail',
    scrollTop: 10196, scrollHeight: 10936, clientHeight: 740,
    viewportTop: 80, viewportLeft: 10, viewportWidth: 1000, viewportHeight: 740,
    tailTop: 700, tailBottom: 810 }));
  assert.equal(tailSamplesReady(samples, 'tail'), true);
  assert.equal(tailSamplesReady(samples.slice(1), 'tail'), false);
  assert.equal(tailSamplesReady(samples, 'other'), false);
  assert.equal(tailSamplesReady([null, ...samples.slice(1)], 'tail'), false);
  for (const change of [
    { tailId: 'wrong' }, { tailKey: '' }, { tailKey: 'replacement' }, { frame: 3 }, { frame: 2 },
    { frame: NaN }, { scrollTop: NaN }, { scrollTop: 10193 }, { scrollHeight: 10937 },
    { clientHeight: 741 }, { viewportTop: 81 }, { viewportLeft: 11 },
    { viewportWidth: 999 }, { viewportHeight: 739 }, { tailTop: 701 }, { tailBottom: 811 },
  ]) {
    assert.equal(tailSamplesReady(samples.map((s, i) => i === 3 ? { ...s, ...change } : s), 'tail'), false, JSON.stringify(change));
  }
  for (const change of [
    { scrollTop: 0, scrollHeight: 740 }, { scrollTop: -1 }, { clientHeight: 0 },
    { viewportWidth: 0 }, { viewportHeight: 0 }, { tailTop: 820, tailBottom: 840 },
    { tailTop: 50, tailBottom: 80 }, { tailTop: 810, tailBottom: 810 },
    { scrollTop: 10000 },
  ]) {
    assert.equal(tailSamplesReady(samples.map(s => ({ ...s, ...change })), 'tail'), false, JSON.stringify(change));
  }
});

const globalAggregate = { kind: 'global', members: ['root', 'child'] };
const ordinaryAggregate = { kind: 'ordinary', members: ['ordinary-root'], endpoint: '/api/product-conversations/pc' };
const historyUrl = member => `http://127.0.0.1:1/api/global/coordinator/history/${member}?before=cursor%2B1`;
const disconnected = url => ({ url, method: 'GET', failure: { errorText: 'net::ERR_INTERNET_DISCONNECTED' } });

test('failed older witness requires a GET cursor on this aggregate including its new latest member', () => {
  for (const member of ['root', 'child', 'new-latest']) {
    assert.deepEqual(failedOlderRequest(globalAggregate, disconnected(historyUrl(member)), 'new-latest'), {
      endpoint: `/api/global/coordinator/history/${member}`, cursor: 'cursor+1',
    });
  }
  assert.ok(failedOlderRequest(ordinaryAggregate, disconnected('http://127.0.0.1:1/api/product-conversations/pc?before=old'), 'new'));
  for (const url of [
    historyUrl('foreign'), historyUrl('new-latest') + '&before=another', historyUrl('child').split('?')[0],
    'http://127.0.0.1:1/api/global/coordinator/history/child?before=',
    'http://127.0.0.1:1/api/global/coordinator/history/child/extra?before=old',
    'http://127.0.0.1:1/api/coordinator/watches?before=old',
    'http://127.0.0.1:1/api/product-conversations/other?before=old',
  ]) assert.equal(failedOlderRequest(globalAggregate, disconnected(url), 'new-latest'), null);
  assert.equal(failedOlderRequest(ordinaryAggregate, disconnected(historyUrl('child')), 'new'), null);
  assert.equal(failedOlderRequest(globalAggregate, { ...disconnected(historyUrl('child')), method: 'POST' }, 'new-latest'), null);
  assert.equal(failedOlderRequest(globalAggregate, { ...disconnected(historyUrl('child')), failure: { errorText: 'net::ERR_ABORTED' } }, 'new-latest'), null);
});

test('successful Retry must recover the same failed endpoint and cursor with correct canonical/latest identity', () => {
  const url = historyUrl('new-latest');
  const failed = olderRequest(globalAggregate, url, 'new-latest');
  const response = { url, status: 200, data: { canonical_root: { transcript_row_id: 'root' }, latest_transcript_row_id: 'new-latest' } };
  assert.equal(retryMatches(globalAggregate, response, failed, 'new-latest'), true);
  for (const change of [
    { url: historyUrl('child') }, { url: url.replace('cursor%2B1', 'different') },
    { url: url.split('?')[0] }, { status: 500 },
    { data: { ...response.data, canonical_root: { transcript_row_id: 'foreign' } } },
    { data: { ...response.data, latest_transcript_row_id: 'child' } },
  ]) assert.equal(retryMatches(globalAggregate, { ...response, ...change }, failed, 'new-latest'), false);
  const ordinary = { url: 'http://127.0.0.1:1/api/product-conversations/pc?before=cursor', status: 200,
    data: { latest_transcript_row_id: 'new' } };
  assert.equal(retryMatches(ordinaryAggregate, ordinary, olderRequest(ordinaryAggregate, ordinary.url, 'new'), 'new'), true);
  assert.equal(retryMatches(ordinaryAggregate, { ...ordinary, url: ordinary.url.replace('/pc?', '/foreign?') },
    olderRequest(ordinaryAggregate, ordinary.url, 'new'), 'new'), false);
});

test('offline orchestration registers first, wheels once over measured extent, requires request AND alert, always restores online', async () => {
  for (const mode of ['success', 'no-request', 'aborted', 'no-alert', 'wheel-error']) {
    const calls = [];
    const page = new EventEmitter();
    page.locator = selector => ({ hover: async () => { assert.equal(selector, '#messages'); calls.push('hover'); } });
    const context = { setOffline: async value => {
      assert.equal(page.listenerCount('requestfailed'), 1, 'witness registered before going offline and retained through cleanup');
      calls.push(`offline:${value}`);
    } };
    page.mouse = { wheel: async (x, y) => {
      assert.equal(x, 0); assert.equal(y, -10936 - 740); calls.push('wheel');
      if (mode === 'wheel-error') throw new Error('wheel error');
      if (mode !== 'no-request') {
        const failure = mode === 'aborted' ? 'net::ERR_ABORTED' : 'net::ERR_INTERNET_DISCONNECTED';
        page.emit('requestfailed', { url: () => historyUrl('new-latest'), method: () => 'GET', failure: () => ({ errorText: failure }) });
      }
    } };
    const alert = { isVisible: async () => mode !== 'no-alert' };
    const poll = async fn => { calls.push('poll'); assert.ok(await fn(), 'both witnesses required'); };
    const action = offlineOlderWitness(page, context, globalAggregate, 'new-latest', { scrollHeight: 10936, clientHeight: 740 }, alert, poll);
    if (mode === 'success') assert.equal((await action).cursor, 'cursor+1');
    else await assert.rejects(action);
    assert.deepEqual(calls, ['offline:true', 'hover', 'wheel', ...(mode === 'wheel-error' ? [] : ['poll']), 'offline:false']);
    assert.equal(page.listenerCount('requestfailed'), 0);
  }
});

test('manual opening readiness requires accepted DTO, exact edge, separate presentation and persisted identity', () => {
  const snapshot = { product_conversation_id: 'pc', latest_transcript_row_id: 'new', writable_transcript_row_id: 'new',
    latest_opening: { status: 'accepted', accepted_message_id: 'opening', opening_authority: 'user_authorized_instruction',
      presentation: 'separate_message', edge: { product_conversation_id: 'pc', predecessor_transcript_row_id: 'old',
        successor_transcript_row_id: 'new', continuation_message_id: 'boundary' } },
    boundaries: [{ message_id: 'boundary', predecessor_transcript_row_id: 'old', successor_transcript_row_id: 'new',
      represented_successor_opening_message_id: null }],
    segments: [{ transcript_row_id: 'new', messages: [{ message_id: 'opening', content: 'same as boundary' }] }] };
  assert.equal(acceptedManualOpening(snapshot, 'old', 'new'), true);
  for (const change of [
    { product_conversation_id: undefined },
    { latest_opening: undefined, latest_opening_pending: false },
    { latest_opening: { status: 'none' } },
    { latest_opening: { ...snapshot.latest_opening, status: 'pending' } },
    { writable_transcript_row_id: null }, { latest_transcript_row_id: 'old' },
    { segments: [] }, { boundaries: [] },
    { boundaries: [{ ...snapshot.boundaries[0], represented_successor_opening_message_id: 'opening' }] },
    ...[{ presentation: 'boundary' }, { opening_authority: 'generated_predecessor_context' },
      { accepted_message_id: 'boundary' }, { accepted_message_id: 'foreign' }].map(change =>
      ({ latest_opening: { ...snapshot.latest_opening, ...change } })),
    ...[{ product_conversation_id: 'foreign' }, { predecessor_transcript_row_id: 'foreign' },
      { successor_transcript_row_id: 'foreign' }, { continuation_message_id: 'foreign' }].map(change =>
      ({ latest_opening: { ...snapshot.latest_opening, edge: { ...snapshot.latest_opening.edge, ...change } } })),
  ]) assert.equal(acceptedManualOpening({ ...snapshot, ...change }, 'old', 'new'), false, JSON.stringify(change));
});
