import { test } from 'vitest';
import assert from 'node:assert/strict';
import { summarizeTrials, runTelemetryChecks, candidateDestination, assertRequestCorrelation, runCollectorNegativeChecks, runUnauthenticatedCollectorCheck, runPendingPagehideCheck, appendTerminalInventory, runVisibilityRoundTrip } from './qualify-view-telemetry.mjs';

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
