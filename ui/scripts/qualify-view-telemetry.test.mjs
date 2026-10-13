import { test } from 'vitest';
import assert from 'node:assert/strict';
import { summarizeTrials, runTelemetryChecks, candidateDestination, assertRequestCorrelation, runCollectorNegativeChecks, runUnauthenticatedCollectorCheck, runPendingPagehideCheck, appendTerminalInventory, runVisibilityRoundTrip, runPendingBackgroundRoundTrip, assertBackgroundReport, runBFCacheRoundTrip, runPendingSPACancellation, observePendingFiniteRequest } from './qualify-view-telemetry.mjs';

test('retains insufficient natural sampling as inconclusive, never overhead clearance', () => {
  const result = summarizeTrials([{ route: 'ordinary', sampled: true }]);
  assert.equal(result.strata.ordinary.status, 'inconclusive_sampling_budget');
  assert.equal(result.overhead_clearance, false);
});
test('warmup and reset metrics do not masquerade as measured cohorts', () => {
  const trials = [];
  for (const route of ['ordinary', 'global']) for (const sampled of [true, false]) {
    trials.push(...Array.from({ length: 8 }, () => ({ route, sampled })));
    trials.push({ route, sampled, warmup: true }, { route, sampled, metric_reset: true });
  }
  const result = summarizeTrials(trials);
  assert.equal(result.strata.global.sampled, 8);
  assert.equal(result.strata.ordinary.unsampled, 8);
  assert.equal(result.strata.global.status, 'raw_cohorts_ready_for_review');
  assert.equal(result.overhead_clearance, false);
});
test('rejects nonloopback target before browser actions', async () => {
  await assert.rejects(runTelemetryChecks({ baseURL: 'https://production.example', runId: 'test' }));
});
test('fixture absolute destination cannot bypass loopback origin', () => {
  assert.equal(candidateDestination('/c/synthetic', 'http://127.0.0.1:8123'), 'http://127.0.0.1:8123/c/synthetic');
  for (const route of ['https://production.example/c/x', '//production.example/x', 'http://127.0.0.1:9999/x']) assert.throws(() => candidateDestination(route, 'http://127.0.0.1:8123'));
});
test('finite request identity must be present in journey correlation, not equal journey id', () => {
  const report = { open_id: 'journey', correlations: [{ kind: 'request', id: 'request' }] };
  assertRequestCorrelation(['request'], report);
  assert.throws(() => assertRequestCorrelation(['unrelated'], report));
  assert.throws(() => assertRequestCorrelation([], report));
});
test('collector negatives distinguish schema validation from bounded semantic rejection', async () => {
  const statuses = [422, 422, 422, 400, 400, 413];
  const requests = [];
  let disposed = 0;
  const context = { request: { post: async (url, options) => {
    requests.push({ url, options });
    const status = statuses.shift();
    return { status: () => status, dispose: async () => { disposed++; } };
  } } };
  const result = await runCollectorNegativeChecks(context, 'http://127.0.0.1:8123', { open_id: 'journey' });
  assert.equal(result.length, 6);
  assert.equal(disposed, 6);
  assert.equal(requests[5].options.data.length, 8193);
  assert.ok(requests.every(request => request.options.timeout === 3000));
});
test('collector negative fail-open is a failed qualification', async () => {
  const context = { request: { post: async () => ({ status: () => 204, dispose: async () => {} }) } };
  await assert.rejects(runCollectorNegativeChecks(context, 'http://127.0.0.1:8123', {}));
});
test('unauthenticated collector check uses empty state and cleans up even on fail-open', async () => {
  for (const status of [401, 204]) {
    let disposed = false;
    let closed = false;
    const browser = { newContext: async options => {
      assert.deepEqual(options.storageState, { cookies: [], origins: [] });
      return { request: { post: async () => ({ status: () => status, dispose: async () => { disposed = true; } }) }, close: async () => { closed = true; } };
    } };
    if (status === 401) assert.equal((await runUnauthenticatedCollectorCheck(browser, 'http://127.0.0.1:8123', {})).status, 401);
    else await assert.rejects(runUnauthenticatedCollectorCheck(browser, 'http://127.0.0.1:8123', {}));
    assert.ok(disposed && closed);
  }
});
test('pending pagehide case requires actual incomplete report, not relabeled ready, and closes context', async () => {
  for (const outcome of ['incomplete', 'ready']) {
    const listeners = {};
    let finishReadiness;
    let closed = false;
    const report = { open_id: 'journey', outcome, marks: { meaningful_transcript: null }, correlations: [{ kind: 'request', id: 'request' }] };
    const page = {
      on: (event, callback) => { listeners[event] = callback; },
      waitForFunction: () => new Promise(resolve => { finishReadiness = resolve; }),
      goto: async url => {
        if (url === 'about:blank') {
          listeners.request({ url: () => 'http://127.0.0.1:8123/api/telemetry/conversation-view', method: () => 'POST', postData: () => JSON.stringify(report) });
          listeners.response({ url: () => 'http://127.0.0.1:8123/api/telemetry/conversation-view', status: () => 204 });
          finishReadiness();
        } else queueMicrotask(() => listeners.request({ url: () => 'http://127.0.0.1:8123/api/product-conversations/product/history?open_id=request', method: () => 'GET' }));
      },
    };
    const browser = { newContext: async () => ({ newPage: async () => page, newCDPSession: async () => ({ send: async () => {} }), close: async () => { closed = true; } }) };
    const args = { browser, storageState: { cookies: [], origins: [] }, baseURL: 'http://127.0.0.1:8123', route: '/c/product', attempts: 1 };
    if (outcome === 'incomplete') assert.equal((await runPendingPagehideCheck(args)).status, 'observed_incomplete');
    else await assert.rejects(runPendingPagehideCheck(args));
    assert.ok(closed);
  }
});
test('terminal inventory preserves incomplete/null marks and forbids duplicate or private reports', () => {
  const identity = { appSource: 'a'.repeat(40), frontendSource: 'b'.repeat(40), serverInstanceId: 'instance' };
  const report = { open_id: 'terminal', frontend_source: identity.frontendSource, frontend_source_dirty: false,
    presentation_evidence: 'commit_frame_estimate', outcome: 'incomplete', route: 'ordinary', trigger: 'navigation',
    total_ms: 2, marks: { meaningful_transcript: null, breadcrumb_presented: null, breadcrumb_enabled: null, anchor_settled: null },
    breadcrumb_ready_minus_meaningful_ms: null, correlations: [{ kind: 'request', id: 'request' }] };
  const result = { journeys: [], capabilities: {} };
  const terminal = { status: 'observed_incomplete', trials: [{ reports: [report] }] };
  appendTerminalInventory(result, terminal, identity);
  assert.equal(result.journeys[0].expected['browser.outcome'], 'incomplete');
  assert.equal(result.journeys[0].expected['browser.meaningful_transcript_ms'], null);
  assert.equal(result.capabilities.pagehide_incomplete, 'observed_incomplete');
  assert.throws(() => appendTerminalInventory(result, terminal, identity));
  assert.throws(() => appendTerminalInventory({ journeys: [], capabilities: {} }, terminal, identity, ['request']));
});
test('actual visibility witness never labels unsupported headless tab as physical suspension', async () => {
  for (const supportsHidden of [true, false]) {
    let state = 'visible';
    let closed = false;
    const page = { bringToFront: async () => { state = 'visible'; }, evaluate: async () => state };
    const context = { newPage: async () => ({ bringToFront: async () => { state = supportsHidden ? 'hidden' : 'visible'; }, close: async () => { closed = true; } }) };
    const witness = await runVisibilityRoundTrip(page, context);
    assert.equal(witness.status, supportsHidden ? 'actual_tab_background_observed' : 'unsupported_no_actual_hidden_visibility');
    assert.match(witness.scope, /not_telemetry_background_readiness_or_physical_suspend/);
    assert.equal(state, 'visible');
    assert.ok(closed);
  }
});

test('pending background driver waits on finite completion while hidden and restores on failure', async () => {
  for (const mode of ['hidden', 'unsupported', 'early_report', 'response_failure']) {
    let state = 'visible';
    let closed = false;
    let readiness = 0;
    const reports = [];
    const context = { newPage: async () => ({ bringToFront: async () => { state = mode === 'unsupported' ? 'visible' : 'hidden'; }, close: async () => { closed = true; } }) };
    const page = { url: () => 'http://127.0.0.1:8123/c/product', context: () => context, bringToFront: async () => { state = 'visible'; },
      evaluate: async fn => fn.toString().includes('performance.now') ? 100 : state,
      waitForFunction: async () => { assert.equal(state, 'visible'); readiness++; } };
    const capture = { requestId: 'request', reports, assertPending: () => assert.equal(reports.length, 0),
      waitFinished: async () => {
        assert.equal(state, 'hidden');
        if (mode === 'response_failure') throw new Error('actual response failed');
        if (mode === 'early_report') reports.push({ outcome: 'ready' });
      },
      waitReport: async () => ({ outcome: 'ready', visible: true, background_ms: 1,
        correlations: [{ kind: 'request', id: 'request' }],
        marks: { meaningful_transcript: 1, breadcrumb_presented: 1, breadcrumb_enabled: 1, anchor_settled: 1 } }),
    };
    const args = { page, context, capture };
    if (['early_report', 'response_failure'].includes(mode)) await assert.rejects(runPendingBackgroundRoundTrip(args));
    else {
      const witness = await runPendingBackgroundRoundTrip(args);
      assert.equal(witness.status, mode === 'hidden' ? 'actual_hidden_response_then_visible_readiness' : 'unsupported_no_actual_hidden_visibility');
    }
    assert.equal(readiness, mode === 'hidden' ? 1 : 0);
    assert.equal(state, 'visible');
    assert.ok(closed);
  }
});
test('background report cannot turn unsupported, incomplete, or absent marks into readiness', () => {
  const witness = { status: 'actual_hidden_response_then_visible_readiness', foregroundAt: 100 };
  const report = { outcome: 'ready', visible: true, background_ms: 5,
    marks: { meaningful_transcript: 2, breadcrumb_presented: 3, breadcrumb_enabled: 3, anchor_settled: 4 } };
  assertBackgroundReport(witness, report);
  assert.throws(() => assertBackgroundReport({ ...witness, status: 'unsupported_no_actual_hidden_visibility' }, report));
  assert.throws(() => assertBackgroundReport(witness, { ...report, outcome: 'incomplete' }));
  assert.throws(() => assertBackgroundReport(witness, { ...report, marks: { ...report.marks, anchor_settled: null } }));
});
test('BFCache witness requires actual persisted pageshow with unchanged document origin', async () => {
  for (const observed of [null, { persisted: false, timeOrigin: 10 }, { persisted: true, timeOrigin: 11 }, { persisted: true, timeOrigin: 10 }]) {
    let evaluations = 0;
    let readiness = 0;
    const navigation = [];
    const page = { url: () => 'http://127.0.0.1:8123/c/product', evaluate: async () => ++evaluations === 1 ? 10 : observed,
      goto: async url => { navigation.push(url); }, goBack: async () => { navigation.push('back'); },
      waitForFunction: async () => { readiness++; } };
    const result = await runBFCacheRoundTrip(page, 'http://127.0.0.1:8123');
    const restored = observed?.persisted === true && observed.timeOrigin === 10;
    assert.equal(result.status, restored ? 'actual_bfcache_restore_observed' : 'unsupported_no_actual_bfcache_restore');
    assert.deepEqual(navigation, ['about:blank', 'back']);
    assert.equal(readiness, restored ? 1 : 0);
    assert.match(result.scope, /not_standalone_or_physical_suspend/);
  }
});
test('SPA driver requires real same-document replacement and correlated canceled outcome', async () => {
  for (const mode of ['canceled', 'ready', 'incomplete', 'cross_document', 'wrong_request', 'off_host', 'rejected_report']) {
    let url = 'http://127.0.0.1:8123/c/old';
    let clicked = false;
    let evaluations = 0;
    const page = { url: () => url, evaluate: async () => ++evaluations === 1 || mode !== 'cross_document' ? 10 : 11,
      locator: () => ({ getAttribute: async () => mode === 'off_host' ? 'https://production.example/c/new' : '/c/new',
        click: async () => { clicked = true; url = 'http://127.0.0.1:8123/c/new'; } }),
      waitForURL: async target => { assert.equal(url, target); } };
    const report = { outcome: ['ready', 'incomplete'].includes(mode) ? mode : 'canceled',
      correlations: [{ kind: 'request', id: mode === 'wrong_request' ? 'unrelated' : 'request' }] };
    const capture = { requestId: 'request', assertPending: () => assert.equal(clicked, false), waitReport: async () => {
      assert.ok(clicked);
      assert.notEqual(mode, 'rejected_report');
      return report;
    } };
    const args = { page, baseURL: 'http://127.0.0.1:8123', selector: 'a', capture };
    if (mode === 'canceled') assert.equal((await runPendingSPACancellation(args)).status, 'observed_spa_canceled');
    else await assert.rejects(runPendingSPACancellation(args));
    assert.equal(clicked, mode !== 'off_host');
  }
});
test('actual request capture rejects precompleted requests, preterminal reports, and rejected collector response', async () => {
  for (const mode of ['pending', 'precompleted', 'preterminal', 'rejected', 'failed']) {
    const callbacks = {};
    let end = mode === 'precompleted' ? 1 : -1;
    const request = { url: () => 'http://127.0.0.1:8123/api/product-conversations/product/history?open_id=request',
      method: () => 'GET', timing: () => ({ responseEnd: end }) };
    const page = { on: (event, callback) => { callbacks[event] = callback; }, off: event => { delete callbacks[event]; } };
    if (mode === 'precompleted') { assert.throws(() => observePendingFiniteRequest(page, request, 'http://127.0.0.1:8123', { productConversationId: 'product' })); continue; }
    const capture = observePendingFiniteRequest(page, request, 'http://127.0.0.1:8123', { productConversationId: 'product' });
    const report = { outcome: 'canceled', product_conversation_id: 'product', correlations: [{ kind: 'request', id: 'request' }] };
    const post = { url: () => 'http://127.0.0.1:8123/api/telemetry/conversation-view', method: () => 'POST', postData: () => JSON.stringify(report) };
    capture.assertPending();
    if (mode === 'preterminal') { callbacks.request(post); assert.throws(() => capture.assertPending()); }
    else {
      end = 1;
      callbacks[mode === 'failed' ? 'requestfailed' : 'requestfinished'](request);
      assert.throws(() => capture.assertPending());
      if (mode === 'failed') await assert.rejects(capture.waitFinished());
      else {
        await capture.waitFinished();
        callbacks.request(post);
        callbacks.response({ request: () => post, status: () => mode === 'rejected' ? 400 : 204 });
        if (mode === 'rejected') await assert.rejects(capture.waitReport());
        else assert.deepEqual(await capture.waitReport(), report);
      }
    }
    capture.close();
    assert.deepEqual(callbacks, {});
  }
});
test('BFCache failure restores starting candidate without inventing restore evidence', async () => {
  const visited = [];
  const page = { url: () => 'http://127.0.0.1:8123/c/product', evaluate: async () => 10,
    goto: async url => { visited.push(url); }, goBack: async () => { throw new Error('back failed'); } };
  await assert.rejects(runBFCacheRoundTrip(page, 'http://127.0.0.1:8123'), /back failed/);
  assert.deepEqual(visited, ['about:blank', 'http://127.0.0.1:8123/c/product']);
});
test('capture retains late duplicate and transient off-host failure through final drain', async () => {
  for (const mode of ['duplicate', 'off_host', 'wrong_view']) {
    const callbacks = {};
    const frame = { url: () => 'https://production.example/c/other' };
    const page = { mainFrame: () => frame, on: (event, callback) => { callbacks[event] = callback; }, off: event => { delete callbacks[event]; } };
    const request = { url: () => 'http://127.0.0.1:8123/api/product-conversations/product/history?open_id=request', method: () => 'GET', timing: () => ({ responseEnd: -1 }) };
    const capture = observePendingFiniteRequest(page, request, 'http://127.0.0.1:8123', { productConversationId: 'product' });
    const post = { url: () => 'http://127.0.0.1:8123/api/telemetry/conversation-view', method: () => 'POST', postData: () => JSON.stringify({ product_conversation_id: mode === 'wrong_view' ? 'other' : 'product', correlations: [{ kind: 'request', id: 'request' }] }) };
    callbacks.request(post);
    if (mode === 'wrong_view') await assert.rejects(capture.waitReport());
    else {
      callbacks.response({ request: () => post, status: () => 204 });
      await capture.waitReport();
      if (mode === 'duplicate') callbacks.request(post);
      else callbacks.framenavigated(frame);
    }
    assert.throws(() => capture.assertFinal());
    capture.close();
    assert.deepEqual(callbacks, {});
  }
});
