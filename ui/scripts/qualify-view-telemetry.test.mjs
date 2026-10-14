import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, vi } from 'vitest';
import assert from 'node:assert/strict';
import { summarizeTrials, runTelemetryChecks, candidateDestination, assertRequestCorrelation, runCollectorNegativeChecks, runUnauthenticatedCollectorCheck, runPendingPagehideCheck, appendTerminalInventory, runVisibilityRoundTrip, runPendingBackgroundRoundTrip, assertBackgroundReport, runBFCacheRoundTrip, runPendingSPACancellation, observePendingFiniteRequest, runLifecycleQualificationStage, appendLifecycleInventory, captureReport } from './qualify-view-telemetry.mjs';

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

function lifecycleBrowserFixture(mode = 'normal') {
  const actions = [];
  let contexts = 0;
  const browser = { newContext: async () => {
    const kind = ['pending_background', 'spa_canceled', 'bfcache_resume'][contexts++];
    const listeners = new Map();
    const emit = (event, value) => { for (const fn of [...(listeners.get(event) ?? [])]) fn(value); };
    let url = 'about:blank';
    let state = 'visible';
    let end = mode === 'precompleted' ? 1 : -1;
    let initialReady = false;
    const readyWaiters = [];
    const requestId = `request-${kind}`;
    const request = { url: () => `http://127.0.0.1:8123/api/product-conversations/product/history?open_id=${requestId}`, method: () => 'GET', timing: () => ({ responseEnd: end }) };
    const identity = { frontend_source: 'b'.repeat(40), frontend_source_dirty: false, presentation_evidence: 'commit_frame_estimate' };
    const report = (suffix, outcome = 'ready', trigger = 'navigation') => ({ ...identity, cache: trigger === 'back_forward' ? 'retained' : 'network', open_id: `${kind}-${suffix}`, product_conversation_id: 'product', route: 'ordinary', trigger,
      outcome, visible: true, background_ms: 1, total_ms: 5, marks: { meaningful_transcript: 1, breadcrumb_presented: 2, breadcrumb_enabled: 2, anchor_settled: 3 }, breadcrumb_ready_minus_meaningful_ms: 1, correlations: [{ kind: 'request', id: requestId }] });
    const post = value => {
      if (mode === 'private' && kind === 'pending_background') value = { ...value, reporter_id: 'PRIVATE_CANARY' };
      if (mode === 'wrong_resume' && value.trigger === 'back_forward') value = { ...value, product_conversation_id: 'other' };
      const req = { url: () => 'http://127.0.0.1:8123/api/telemetry/conversation-view', method: () => 'POST', postData: () => JSON.stringify(value) };
      emit('request', req);
      emit('response', { request: () => req, status: () => 204 });
    };
    const makeReady = () => { initialReady = true; for (const resolve of readyWaiters.splice(0)) resolve(); };
    const page = {
      on: (event, fn) => { if (!listeners.has(event)) listeners.set(event, new Set()); listeners.get(event).add(fn); },
      off: (event, fn) => listeners.get(event)?.delete(fn), context: () => owned, url: () => url,
      mainFrame: () => ({ url: () => url }),
      evaluate: async fn => {
        const expression = fn.toString();
        if (expression.includes('__phoenixQualificationPageshow = null')) return 10;
        if (expression.includes('__phoenixQualificationPageshow ??')) return { persisted: true, timeOrigin: 10 };
        if (expression.includes('performance.timeOrigin')) return 10;
        if (expression.includes('performance.now')) return 100;
        return state;
      },
      goto: async target => {
        actions.push([kind, 'goto', target]); url = target;
        if (target !== 'about:blank') queueMicrotask(() => {
          emit('request', request);
          if (kind === 'bfcache_resume') setTimeout(() => { end = 1; emit('requestfinished', request); post(report('initial')); makeReady(); }, 0);
          if (mode === 'preterminal') post(report('early'));
        });
      },
      waitForFunction: async () => { if (!initialReady) await new Promise(resolve => readyWaiters.push(resolve)); },
      bringToFront: async () => {
        state = 'visible'; actions.push([kind, 'foreground']);
        if (kind === 'pending_background' && end === 1 && !initialReady) { post(report('initial')); makeReady(); }
      },
      locator: () => ({ getAttribute: async () => '/c/successor', click: async () => {
        actions.push([kind, 'click']); url = 'http://127.0.0.1:8123/c/successor';
        post(report('initial', 'canceled'));
        post({ ...report('successor'), correlations: [{ kind: 'request', id: 'successor-request' }] });
        makeReady();
      } }), waitForURL: async target => assert.equal(url, target),
      goBack: async () => { actions.push([kind, 'back']); url = 'http://127.0.0.1:8123/c/product'; post(report('resume', 'ready', 'back_forward')); },
    };
    let pages = 0;
    let contextClosed;
    const owned = { on: (event, fn) => { if (event === 'close') contextClosed = fn; }, newPage: async () => ++pages === 1 ? page : {
      bringToFront: async () => { state = 'hidden'; actions.push([kind, 'hidden']); setTimeout(() => { end = 1; emit('requestfinished', request); }, 0); },
      close: async () => actions.push([kind, 'other_closed']),
    }, newCDPSession: async () => ({ send: async () => {} }), close: async () => {
      actions.push([kind, 'closed']);
      if (mode === 'late_duplicate' && kind === 'pending_background') post(report('duplicate'));
      contextClosed?.();
    } };
    return owned;
  } };
  return { browser, actions, count: () => contexts };
}

test('wired lifecycle stage actually runs background, SPA and native BFCache paths and inventories every report', async () => {
  const fixture = lifecycleBrowserFixture();
  const result = { journeys: [], capabilities: {} };
  const identity = { appSource: 'a'.repeat(40), frontendSource: 'b'.repeat(40), serverInstanceId: 'instance' };
  const stage = await runLifecycleQualificationStage({ browser: fixture.browser, storageState: {}, baseURL: 'http://127.0.0.1:8123',
    fixture: { route: '/c/product', productConversationId: 'product', spaSelector: 'a[href="/c/successor"]' },
    onTrial: trial => appendLifecycleInventory(result, { trials: [trial] }, identity) });
  assert.equal(fixture.count(), 3);
  assert.deepEqual(stage.trials.map(trial => trial.status), ['actual_hidden_response_then_visible_readiness', 'observed_spa_canceled', 'observed_bfcache_fresh_ready_report']);
  assert.equal(result.journeys.length, 5, 'initial, canceled, SPA successor, BFCache initial and resumed reports all retained');
  assert.equal(new Set(result.journeys.map(journey => journey.open_id)).size, 5);
  for (const [kind, action] of [['pending_background', 'hidden'], ['spa_canceled', 'click'], ['bfcache_resume', 'back']]) assert.ok(fixture.actions.some(observed => observed[0] === kind && observed[1] === action));
  assert.equal(fixture.actions.filter(action => action[1] === 'closed').length, 3);
});
test('wired lifecycle stage rejects precompleted, preterminal and late duplicate cases with preserved trial inventory', async () => {
  for (const mode of ['precompleted', 'preterminal', 'late_duplicate']) {
    const fixture = lifecycleBrowserFixture(mode);
    const trials = [];
    await assert.rejects(runLifecycleQualificationStage({ browser: fixture.browser, storageState: {}, baseURL: 'http://127.0.0.1:8123',
      fixture: { route: '/c/product', productConversationId: 'product', spaSelector: 'a' }, onTrial: trial => trials.push(trial) }));
    assert.equal(trials.length, 1);
    assert.ok(trials[0].failure);
    assert.equal(fixture.actions.filter(action => action[1] === 'closed').length, 1);
    if (mode !== 'precompleted') assert.ok(trials[0].reports.length > 0, 'failure reports are preserved');
  }
});
test.each(['normal', 'precompleted', 'preterminal', 'late_duplicate', 'initial_late_duplicate', 'initial_late_private', 'pagehide_late_duplicate', 'pagehide_late_private', 'initial_timeout'])('main driver executes lifecycle stage with complete success/failure inventory: %s', async mode => {
  if (mode === 'initial_timeout') vi.useFakeTimers();
  const outputDir = mkdtempSync(join(tmpdir(), 'cvt-main-contract-'));
  const lifecycle = lifecycleBrowserFixture(mode);
  let contexts = 0;
  let authContexts = 0;
  const baseURL = 'http://127.0.0.1:8123';
  const identity = { appSource: 'a'.repeat(40), frontendSource: 'b'.repeat(40), serverInstanceId: 'instance' };
  const browser = { newContext: async options => {
    if (options.storageState.cookies?.length === 0 && !options.viewport) {
      authContexts++;
      return { request: { post: async () => ({ status: () => 401, dispose: async () => {} }) }, close: async () => {} };
    }
    const index = contexts++;
    if (index >= 49) return lifecycle.browser.newContext(options);
    const pagehide = index === 48;
    const callbacks = {};
    let readiness;
    const requestId = `initial-request-${index}`;
    const report = { open_id: `initial-journey-${index}`, frontend_source: identity.frontendSource, frontend_source_dirty: false,
      presentation_evidence: 'commit_frame_estimate', sample_rate: 0.25, visible: true, route: index % 2 ? 'global' : 'ordinary', trigger: 'navigation',
      observer_inspection_count: 2, observer_scan_limited: false, background_ms: 0, background_interval_count: 0,
      admitted_count: 1, sampled_count: 1, transport_dropped_count: 0, transport_failed_count: 0,
      outcome: pagehide ? 'incomplete' : 'ready', total_ms: 5, correlations: [{ kind: 'request', id: requestId }],
      marks: pagehide ? { meaningful_transcript: null, breadcrumb_presented: null, breadcrumb_enabled: null, anchor_settled: null }
        : { meaningful_transcript: 1, breadcrumb_presented: 2, breadcrumb_enabled: 2, anchor_settled: 3 }, breadcrumb_ready_minus_meaningful_ms: pagehide ? null : 1 };
    const post = () => {
      callbacks.request?.({ url: () => `${baseURL}/api/telemetry/conversation-view`, method: () => 'POST', postData: () => JSON.stringify(report) });
      callbacks.response?.({ url: () => `${baseURL}/api/telemetry/conversation-view`, status: () => 204 });
    };
    const candidate = { context: () => owned, on: (event, fn) => { callbacks[event] = fn; }, bringToFront: async () => {}, evaluate: async () => 'visible',
      goto: async url => {
        if (url === 'about:blank') { post(); readiness?.(); }
        else {
          callbacks.request?.({ url: () => `${baseURL}/api/product-conversations/product/history?open_id=${requestId}`, method: () => 'GET' });
          if (!pagehide) post();
        }
      }, waitForFunction: async () => { if (pagehide || (index === 0 && mode === 'initial_timeout')) await new Promise(resolve => { readiness = resolve; }); } };
    let pages = 0;
    let contextClosed;
    const owned = { on: (event, fn) => { if (event === 'close') contextClosed = fn; }, newPage: async () => ++pages === 1 ? candidate : { bringToFront: async () => {}, close: async () => {} },
      newCDPSession: async () => ({ send: async method => method === 'Performance.getMetrics' ? { metrics: [{ name: 'ScriptDuration', value: 1 }] } : {} }), close: async () => {
        if (index === 0 && mode === 'initial_late_duplicate') post();
        if (index === 0 && mode === 'initial_timeout') readiness?.();
        if (pagehide && mode === 'pagehide_late_duplicate') post();
        if (pagehide && mode === 'pagehide_late_private') callbacks.request?.({ url: () => `${baseURL}/api/telemetry/conversation-view`, method: () => 'POST', postData: () => JSON.stringify({ ...report, reporter_id: 'PRIVATE_CANARY' }) });
        if (index === 0 && mode === 'initial_late_private') callbacks.request?.({ url: () => `${baseURL}/api/telemetry/conversation-view`, method: () => 'POST', postData: () => JSON.stringify({ ...report, reporter_id: 'PRIVATE_CANARY' }) });
        contextClosed?.();
      } };
    return owned;
  } };
  const statuses = [422, 422, 422, 400, 400, 413];
  const context = { browser: () => browser, storageState: async () => ({ cookies: [{ name: 'synthetic' }], origins: [] }),
    request: { post: async () => { const status = statuses.shift(); return { status: () => status, dispose: async () => {} }; } } };
  try {
    const execution = runTelemetryChecks({ page: { context: () => context }, context, baseURL, serviceName: 'synthetic-main', runId: 'main', outputDir,
      fixtures: { identity, aggregates: [{ kind: 'ordinary', route: '/c/product' }, { kind: 'global', route: '/global/coordinator' }],
        lifecycle: { route: '/c/product', productConversationId: 'product', spaSelector: 'a[href="/c/successor"]' }, forbidden: ['PRIVATE_CANARY'] } });
    if (mode !== 'normal') {
      const rejection = assert.rejects(execution);
      if (mode === 'initial_timeout') await vi.advanceTimersByTimeAsync(285000);
      await rejection;
      const receipt = JSON.parse(readFileSync(join(outputDir, 'telemetry-browser-result.json'), 'utf8'));
      assert.ok(receipt.failure);
      if (mode === 'initial_timeout') {
        assert.match(receipt.failure, /absolute browser qualification deadline/);
        assert.equal(receipt.trials.length, 1);
        assert.equal(contexts, 1, 'no contexts admitted after outer deadline');
        return;
      }
      if (mode.startsWith('initial_late')) {
        assert.equal(receipt.trials.length, 1);
        assert.ok(receipt.trials[0].failure);
        const { forbidden, ...observedReceipt } = receipt;
        assert.deepEqual(forbidden, ['PRIVATE_CANARY']);
        assert.equal(JSON.stringify(observedReceipt).includes('PRIVATE_CANARY'), false);
        return;
      }
      if (mode.startsWith('pagehide_late')) {
        assert.equal(receipt.terminal.trials.length, 1);
        assert.ok(receipt.terminal.trials[0].failure);
        const { forbidden, ...observedReceipt } = receipt;
        assert.deepEqual(forbidden, ['PRIVATE_CANARY']);
        assert.equal(JSON.stringify(observedReceipt).includes('PRIVATE_CANARY'), false);
        return;
      }
      assert.equal(receipt.lifecycle.trials.length, 1);
      assert.ok(receipt.lifecycle.trials[0].failure);
      assert.equal(receipt.journeys.length, 49 + receipt.lifecycle.trials[0].reports.length);
      assert.equal(lifecycle.actions.filter(action => action[1] === 'closed').length, 1);
      return;
    }
    const result = await execution;
    assert.equal(result.lifecycle.trials.length, 3);
    assert.equal(result.journeys.length, 54, '48 initial + pagehide + five lifecycle reports');
    assert.equal(contexts, 52);
    assert.equal(authContexts, 1);
    assert.equal(result.comparison.overhead_clearance, false);
    assert.ok(result.trials.every(trial => Number.isFinite(trial.navigation_to_visible_readiness_ms)));
    assert.equal(result.trials[0].readiness_clock, 'driver_monotonic_includes_automation_latency_not_paint');
    assert.equal(result.trials[0].observer.inspection_count, 2);
    assert.equal(result.trials[0].observer.scan_limited, false);
    assert.equal(result.capabilities.spa_canceled, 'observed_spa_canceled');
    assert.equal(result.capabilities.bfcache_resume, 'observed_bfcache_fresh_ready_report');
    assert.equal(result.capabilities.pending_background, 'actual_hidden_response_then_visible_readiness');
    assert.equal(readFileSync(join(outputDir, 'telemetry-browser-result.json'), 'utf8').includes('bfcache_resume-resume'), true);
    assert.equal(new Set(result.journeys.map(journey => journey.open_id)).size, 54);
  } finally { rmSync(outputDir, { recursive: true, force: true }); if (mode === 'initial_timeout') vi.useRealTimers(); }
});
test('wired lifecycle receipts reject private payload before writing and record context/callback failures', async () => {
  for (const mode of ['private', 'context_failure', 'callback_failure', 'wrong_resume']) {
    const outputDir = mkdtempSync(join(tmpdir(), 'cvt-stage-failure-'));
    const fixture = lifecycleBrowserFixture(mode);
    if (mode === 'context_failure') fixture.browser.newContext = async () => { throw new Error('context unavailable'); };
    try {
      await assert.rejects(runLifecycleQualificationStage({ browser: fixture.browser, storageState: {}, baseURL: 'http://127.0.0.1:8123', outputDir,
        forbidden: ['PRIVATE_CANARY'], fixture: { route: '/c/product', productConversationId: 'product', spaSelector: 'a' },
        onTrial: () => { if (mode === 'callback_failure') throw new Error('inventory callback failed'); } }));
      const text = readFileSync(join(outputDir, 'telemetry-lifecycle-trials.json'), 'utf8');
      assert.equal(text.includes('PRIVATE_CANARY'), false);
      const receipt = JSON.parse(text);
      assert.ok(receipt.trials.at(-1).failure);
      assert.equal(receipt.trials.length, mode === 'wrong_resume' ? 3 : 1);
    } finally { rmSync(outputDir, { recursive: true, force: true }); }
  }
});
test('report retention is strict and inventory append is atomic at duplicate/volume bounds', () => {
  assert.throws(() => captureReport('{"message":"untracked"}'));
  assert.throws(() => captureReport('{"reporter_id":"PRIVATE_CANARY"}', ['PRIVATE_CANARY']));
  assert.throws(() => captureReport('{"reporter_id":"\\u0050RIVATE_CANARY"}', ['PRIVATE_CANARY']));
  const identity = { appSource: 'a'.repeat(40), frontendSource: 'b'.repeat(40), serverInstanceId: 'instance' };
  const report = { open_id: 'new', frontend_source: identity.frontendSource, frontend_source_dirty: false, outcome: 'ready', marks: {} };
  const result = { journeys: Array.from({ length: 64 }, (_, index) => ({ open_id: `existing-${index}` })), capabilities: {} };
  assert.throws(() => appendLifecycleInventory(result, { trials: [{ kind: 'spa_canceled', reports: [report] }] }, identity));
  assert.equal(result.journeys.length, 64);
  const duplicate = { journeys: [], capabilities: {} };
  assert.throws(() => appendLifecycleInventory(duplicate, { trials: [{ kind: 'spa_canceled', reports: [report, report] }] }, identity));
  assert.deepEqual(duplicate.journeys, []);
});
test('absolute lifecycle deadline preserves receipt and closes a context arriving after cancellation', async () => {
  vi.useFakeTimers();
  let create;
  let closed = 0;
  const trials = [];
  const browser = { newContext: () => new Promise(resolve => { create = resolve; }) };
  try {
    const execution = runLifecycleQualificationStage({ browser, storageState: {}, baseURL: 'http://127.0.0.1:8123', budgetMs: 100,
      fixture: { route: '/c/product', productConversationId: 'product', spaSelector: 'a' }, onTrial: trial => trials.push(trial) });
    const rejection = assert.rejects(execution, /absolute lifecycle stage deadline/);
    await vi.advanceTimersByTimeAsync(100);
    await rejection;
    assert.equal(trials.length, 1);
    assert.match(trials[0].failure, /deadline/);
    create({ close: async () => { closed++; }, newPage: async () => { throw new Error('must not create late page'); } });
    await vi.advanceTimersByTimeAsync(0);
    assert.equal(closed, 1);
  } finally { vi.useRealTimers(); }
});
test('raw cohorts exclude failed observations including empty failure flag, warmup and reset without erasing raw records', () => {
  const trials = Array.from({ length: 16 }, (_, index) => ({ route: 'ordinary', sampled: index < 8 }));
  trials[0].failure = '';
  trials[8].metric_reset = true;
  trials[9].warmup = true;
  const summary = summarizeTrials(trials);
  assert.equal(summary.strata.ordinary.sampled, 7);
  assert.equal(summary.strata.ordinary.unsampled, 6);
  assert.equal(summary.strata.ordinary.status, 'inconclusive_sampling_budget');
  assert.equal(summary.overhead_clearance, false);
  assert.equal(trials.length, 16);
});
test('pagehide deadline records failure and closes context arriving after creation guard', async () => {
  vi.useFakeTimers();
  let create;
  let closed = 0;
  const trials = [];
  try {
    const execution = runPendingPagehideCheck({ browser: { newContext: () => new Promise(resolve => { create = resolve; }) }, storageState: {},
      baseURL: 'http://127.0.0.1:8123', route: '/c/product', onTrial: trial => trials.push(trial) });
    const rejection = assert.rejects(execution);
    await vi.advanceTimersByTimeAsync(5000);
    await rejection;
    assert.match(trials[0].failure, /pagehide context deadline/);
    create({ close: async () => { closed++; } });
    await vi.advanceTimersByTimeAsync(0);
    assert.equal(closed, 1);
  } finally { vi.useRealTimers(); }
});
