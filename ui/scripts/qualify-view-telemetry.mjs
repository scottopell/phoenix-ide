import { captureDiagnostic, samplingDecision, initialFailure, requestFact } from './view-telemetry-diagnostics.mjs';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';

export function captureReport(body, forbidden = []) {
  if (body.length > 8192 || Buffer.byteLength(body) > 8192 || forbidden.some(secret => body.includes(secret))) throw new Error('unsafe telemetry report rejected before retention');
  let report;
  try { report = JSON.parse(body); } catch { throw new Error('invalid telemetry report JSON'); }
  if (forbidden.some(secret => JSON.stringify(report).includes(secret))) throw new Error('unsafe telemetry report rejected before retention');
  const fields = new Set(['open_id', 'reporter_id', 'product_conversation_id', 'segment_id', 'route', 'trigger', 'standalone', 'platform',
    'frontend_source', 'frontend_source_dirty', 'cache', 'message_count_bucket', 'chapter_count_bucket', 'response_size_bucket', 'reconnect_count', 'visible',
    'marks', 'outcome', 'total_ms', 'timeline_saturated', 'background_ms', 'background_interval_count', 'background_open', 'observer_inspection_count',
    'observer_scan_limited', 'anchor_result', 'breadcrumb_ready_minus_meaningful_ms', 'presentation_evidence', 'correlations', 'dropped_correlations',
    'counters_saturated', 'sample_rate', 'admitted_count', 'sampled_count', 'transport_dropped_count', 'transport_failed_count']);
  if (!report || typeof report !== 'object' || Array.isArray(report) || Object.keys(report).some(key => !fields.has(key))) throw new Error('unknown telemetry report field');
  for (const [key, value] of Object.entries(report)) {
    if (key === 'marks') {
      const marks = ['shell', 'snapshot_received', 'hydration_ready', 'meaningful_transcript', 'chapter_data_ready', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled'];
      if (!value || Object.entries(value).some(([mark, time]) => !marks.includes(mark) || (time !== null && !Number.isFinite(time)))) throw new Error('invalid telemetry marks');
    } else if (key === 'correlations') {
      if (!Array.isArray(value) || value.length > 16 || value.some(entry => !entry || Object.keys(entry).some(field => !['kind', 'id'].includes(field)) || !['request', 'sse', 'trace'].includes(entry.kind) || typeof entry.id !== 'string')) throw new Error('invalid telemetry correlations');
    } else if (value !== null && !['string', 'number', 'boolean'].includes(typeof value)) throw new Error('invalid telemetry scalar');
  }
  return report;
}

export function summarizeTrials(trials, minimum = 8) {
  const strata = {};
  for (const route of ['ordinary', 'global']) {
    const measured = trials.filter(t => t.route === route && !t.warmup && !t.metric_reset && !Object.hasOwn(t, 'failure'));
    const sampled = measured.filter(t => t.sampled);
    const unsampled = measured.filter(t => !t.sampled);
    strata[route] = { sampled: sampled.length, unsampled: unsampled.length,
      status: sampled.length >= minimum && unsampled.length >= minimum ? 'raw_cohorts_ready_for_review' : 'inconclusive_sampling_budget' };
  }
  return { strata, overhead_clearance: false, methodology: 'same-build natural-sampling navigation-scoped raw CDP counters; no uninstrumented-build comparison or compositor proof' };
}

export function candidateDestination(route, baseURL) {
  const base = new URL(baseURL);
  const destination = new URL(route, base);
  assert.equal(base.hostname, '127.0.0.1');
  assert.equal(destination.origin, base.origin);
  assert.equal(destination.protocol, 'http:');
  assert.ok(!destination.username && !destination.password);
  return destination.href;
}

export function assertRequestCorrelation(requests, report) {
  assert.ok(requests.length > 0);
  const correlated = new Set(report.correlations.filter(c => c.kind === 'request').map(c => c.id));
  assert.ok(requests.every(id => correlated.has(id)), 'finite requests must belong to this reported journey');
}

async function collectorTransport(operation, message) {
  try { return await operation(); } catch { throw new Error(message); }
}

async function withCollectorResponse(acquire, inspect) {
  const response = await acquire();
  let primary;
  try { return await inspect(response); }
  catch (error) { primary = error; throw error; }
  finally {
    try { await collectorTransport(() => response.dispose(), 'collector response disposal failed'); }
    catch (cleanup) { if (!primary) throw cleanup; primary.cleanup_failure = cleanup.message; }
  }
}

export async function runCollectorNegativeChecks(context, baseURL, validReport) {
  const endpoint = candidateDestination('/api/telemetry/conversation-view', baseURL);
  const cases = [
    ['private_unknown_field', 422, { ...validReport, transcript_text: 'synthetic-private-negative' }],
    ['invalid_journey_id', 422, { ...validReport, open_id: 'not-a-uuid' }],
    ['invalid_outcome', 422, { ...validReport, outcome: 'invented' }],
    ['excess_correlations', 400, { ...validReport, correlations: Array.from({ length: 17 }, () => ({ kind: 'request', id: validReport.open_id })) }],
    ['invalid_duration', 400, { ...validReport, total_ms: 300001 }],
  ];
  const results = [];
  for (const [name, expectedStatus, report] of cases) {
    await withCollectorResponse(
      () => collectorTransport(() => context.request.post(endpoint, { data: report, timeout: 3000 }), 'collector negative POST transport failed'),
      response => {
        assert.equal(response.status(), expectedStatus, `collector must reject ${name}`);
        results.push({ name, status: response.status() });
      });
  }
  await withCollectorResponse(
    () => collectorTransport(() => context.request.post(endpoint, { data: 'x'.repeat(8193), headers: { 'Content-Type': 'application/json' }, timeout: 3000 }), 'collector oversized POST transport failed'),
    response => {
      assert.equal(response.status(), 413, 'collector body bound');
      results.push({ name: 'oversized_body', status: response.status() });
    });
  return results;
}

export async function requireCollectorAuth(context, baseURL, authenticated) {
  const endpoint = candidateDestination('/api/auth/status', baseURL);
  return withCollectorResponse(
    () => collectorTransport(() => context.request.get(endpoint, { timeout: 3000 }), 'collector authentication status GET transport failed'),
    async response => {
      assert.equal(response.status(), 200, 'collector authentication preflight status required');
      let status;
      try { status = await response.json(); } catch { throw new Error('invalid collector authentication status JSON'); }
      assert.ok(status?.auth_required === true, 'disposable collector fixture must require authentication');
      assert.ok(status?.authenticated === authenticated, 'collector context authentication prerequisite');
      return { auth_required: true, authenticated };
  });
}

export async function runUnauthenticatedCollectorCheck(browser, baseURL, validReport) {
  const isolated = await collectorTransport(() => browser.newContext({ storageState: { cookies: [], origins: [] } }), 'collector isolated context creation failed');
  let primary;
  try {
    await requireCollectorAuth(isolated, baseURL, false);
    const endpoint = candidateDestination('/api/telemetry/conversation-view', baseURL);
    return await withCollectorResponse(
      () => collectorTransport(() => isolated.request.post(endpoint, { data: validReport, timeout: 3000 }), 'collector unauthenticated POST transport failed'),
      response => {
        assert.equal(response.status(), 401, 'synthetic server must enforce collector authentication');
        return { name: 'unauthenticated_report', status: response.status() };
      });
  } catch (error) { primary = error; throw error; }
  finally {
    try { await collectorTransport(() => isolated.close(), 'collector isolated context cleanup failed'); }
    catch (cleanup) { if (!primary) throw cleanup; primary.context_cleanup_failure = cleanup.message; }
  }
}

export function journeyExpectation(report, identity) {
  assert.equal(report.frontend_source, identity.frontendSource);
  assert.equal(report.frontend_source_dirty, false);
  assert.equal(report.presentation_evidence, 'commit_frame_estimate');
  const expected = {
    'server.backend_source': identity.appSource, 'server.instance_id': identity.serverInstanceId,
    'browser.frontend_source': identity.frontendSource, 'browser.outcome': report.outcome,
    'browser.route': report.route, 'browser.trigger': report.trigger,
    ...(Object.hasOwn(report, 'product_conversation_id') && report.product_conversation_id !== null ? { 'product_conversation.id': report.product_conversation_id } : {}),
    ...(Object.hasOwn(report, 'cache') ? { 'browser.cache': report.cache } : {}),
    'browser.presentation_evidence': report.presentation_evidence, 'browser.total_ms': report.total_ms,
  };
  for (const mark of ['meaningful_transcript', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled']) expected[`browser.${mark}_ms`] = report.marks[mark];
  expected['browser.breadcrumb_ready_minus_meaningful_ms'] = report.breadcrumb_ready_minus_meaningful_ms;
  return { open_id: report.open_id, correlations: report.correlations, expected };
}

export function appendTerminalInventory(result, terminal, identity, forbidden = []) {
  const added = [];
  const ids = new Set(result.journeys.map(journey => journey.open_id));
  for (const trial of terminal.trials) for (const report of trial.reports) {
    captureReport(JSON.stringify(report), forbidden);
    assert.ok(!ids.has(report.open_id), 'journey identity must be unique across contexts');
    ids.add(report.open_id);
    added.push(journeyExpectation(report, identity));
  }
  assert.ok(result.journeys.length + added.length <= 64, 'final journey inventory bound');
  result.journeys.push(...added);
  result.terminal = terminal;
  result.capabilities.pagehide_incomplete = terminal.status;
}

export async function runPendingPagehideCheck({ browser, storageState, baseURL, route, outputDir, forbidden = [], attempts = 8, budgetMs = 40000, onTrial = () => {} }) {
  assert.ok(Number.isInteger(attempts) && attempts > 0 && attempts <= 8);
  assert.ok(Number.isInteger(budgetMs) && budgetMs > 0 && budgetMs <= 40000);
  const caseDeadline = Date.now() + budgetMs;
  const destination = candidateDestination(route, baseURL);
  const trials = [];
  for (let attempt = 0; attempt < attempts && Date.now() < caseDeadline; attempt++) {
    let owned;
    let canceled = false;
    let primaryError;
    const trial = { attempt, sampled: false, reports: [], statuses: [], event: 'actual_cross_document_pagehide', transport_facts: [], producer_events: [] };
    const postOrdinals = new Map();
    let transportOverflow = false;
    let phase = 'opening';
    let signalAdmissionRequest;
    let diagnosticFailure;
    const signalIfAttributed = () => {
      if (trial.requestId && trial.producer_events.some(event => event.kind === 'admission')) signalAdmissionRequest?.();
    };
    const transportFact = fact => {
      if (trial.transport_facts.length >= 32) { transportOverflow = true; return; }
      trial.transport_facts.push({ ...fact, observed_phase: phase });
    };
    try {
      owned = await boundedObservation(browser.newContext({ storageState, viewport: { width: 1280, height: 900 } }).then(async created => {
        if (canceled) {
          await boundedObservation(created.close(), 5000, 'late pagehide context cleanup');
          throw new Error('pagehide context arrived after cancellation');
        }
        return created;
      }), 5000, 'pagehide context');
      const operation = (async () => {
        await owned.exposeBinding('__phoenixPagehideDiagnostic', (_source, event) => {
          if (trial.producer_events.length >= 64) { diagnosticFailure = 'pagehide producer event bound'; return; }
          try { trial.producer_events.push({ ...captureDiagnostic(event), observed_phase: phase }); }
          catch { diagnosticFailure = 'invalid pagehide producer diagnostic'; }
          signalIfAttributed();
        });
        assert.ok(!canceled, 'pagehide diagnostic binding completed after cancellation');
        await owned.addInitScript(() => {
          const pending = [];
          let count = 0;
          let failed = false;
          window.addEventListener('phoenix:view-diagnostic', event => {
            if (count++ > 64) return;
            pending.push(window.__phoenixPagehideDiagnostic(event.detail).catch(() => { failed = true; }));
          });
          window.__phoenixPagehideDiagnosticDrain = async () => {
            await Promise.all(pending);
            if (failed) throw new Error('pagehide producer diagnostic delivery failed');
          };
        });
        assert.ok(!canceled, 'pagehide diagnostic initialization completed after cancellation');
        const postOrdinal = request => {
          const url = new URL(request.url());
          if (url.origin !== new URL(baseURL).origin || url.pathname !== '/api/telemetry/conversation-view' || request.method() !== 'POST') return null;
          if (!postOrdinals.has(request)) {
            if (postOrdinals.size >= 2) { transportOverflow = true; return null; }
            postOrdinals.set(request, postOrdinals.size);
          }
          return postOrdinals.get(request);
        };
        owned.on?.('response', response => {
          const ordinal = postOrdinal(response.request());
          if (ordinal !== null) transportFact({ kind: 'context_response', ordinal, status: response.status() });
        });
        for (const event of ['requestfinished', 'requestfailed']) owned.on?.(event, request => {
          const ordinal = postOrdinal(request);
          if (ordinal !== null) transportFact({ kind: `context_${event}`, ordinal });
        });
        owned.on?.('close', () => transportFact({ kind: 'context_closed' }));
        const candidate = await owned.newPage();
        const cdp = await owned.newCDPSession(candidate);
        await cdp.send('Network.enable');
        await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: 200, downloadThroughput: 1000000, uploadThroughput: 1000000 });
        candidate.on('close', () => transportFact({ kind: 'page_closed' }));
        candidate.on('crash', () => transportFact({ kind: 'page_crashed' }));
        const cdpPosts = new Map();
        cdp.on?.('Network.requestWillBeSent', event => {
          const url = new URL(event.request.url);
          if (url.origin !== new URL(baseURL).origin || url.pathname !== '/api/telemetry/conversation-view' || event.request.method !== 'POST') return;
          if (cdpPosts.size >= 2) { transportOverflow = true; return; }
          cdpPosts.set(event.requestId, cdpPosts.size);
          transportFact({ kind: 'cdp_post', cdp_ordinal: cdpPosts.get(event.requestId) });
        });
        cdp.on?.('Network.responseReceived', event => {
          if (cdpPosts.has(event.requestId)) transportFact({ kind: 'cdp_response', cdp_ordinal: cdpPosts.get(event.requestId), status: event.response.status });
        });
        cdp.on?.('Network.loadingFailed', event => {
          if (cdpPosts.has(event.requestId)) transportFact({ kind: 'cdp_failed', cdp_ordinal: cdpPosts.get(event.requestId), canceled: event.canceled === true });
        });
        cdp.on?.('Network.loadingFinished', event => {
          if (cdpPosts.has(event.requestId)) transportFact({ kind: 'cdp_finished', cdp_ordinal: cdpPosts.get(event.requestId) });
        });
        candidate.on('requestfailed', request => {
          const ordinal = postOrdinal(request);
          if (ordinal !== null) transportFact({ kind: 'page_requestfailed', ordinal });
        });
        const requestSeen = new Promise(resolveRequest => { signalAdmissionRequest = resolveRequest; });
        candidate.on('request', request => {
          const url = new URL(request.url());
          if (url.origin === new URL(baseURL).origin && url.searchParams.has('open_id')
            && (url.pathname.startsWith('/api/product-conversations/') || url.pathname.startsWith('/api/global/coordinator/history/'))) {
            trial.requestId = url.searchParams.get('open_id');
            signalIfAttributed();
          }
          if (url.origin === new URL(baseURL).origin && url.pathname === '/api/telemetry/conversation-view' && request.method() === 'POST') {
            const ordinal = postOrdinal(request);
            if (ordinal !== null) transportFact({ kind: 'page_request', ordinal, report_index: trial.reports.length });
            if (trial.reports.length >= 2) { trial.failure ??= 'pagehide report count bound'; return; }
            const body = request.postData() ?? '';
            if (Buffer.byteLength(body) > 8192) trial.failure = 'report byte bound';
            else {
              try { trial.reports.push(captureReport(body, forbidden)); } catch { trial.failure = 'invalid or unsafe report JSON'; }
            }
          }
        });
        candidate.on('response', response => {
          if (new URL(response.url()).origin === new URL(baseURL).origin && new URL(response.url()).pathname === '/api/telemetry/conversation-view' && trial.statuses.length < 2) trial.statuses.push(response.status());
        });
        const navigation = candidate.goto(destination, { timeout: 10000, waitUntil: 'domcontentloaded' });
        const readiness = candidate.waitForFunction(visibleReadiness, undefined, { timeout: 10000 }).then(() => 'ready', () => 'readiness_unavailable');
        const winner = await Promise.race([requestSeen.then(() => 'request'), readiness]);
        await navigation;
        await candidate.evaluate(() => window.__phoenixPagehideDiagnosticDrain());
        assert.ok(!diagnosticFailure, diagnosticFailure);
        const admission = samplingDecision(trial.producer_events, route.startsWith('/global/') ? 'global' : 'ordinary');
        trial.sampled = admission.sampled;
        trial.admitted_open_id = admission.open_id;
        assert.notEqual(winner, 'readiness_unavailable', 'case readiness unavailable before sampled request');
        if (winner === 'request' && admission.sampled) {
          assert.ok(trial.producer_events.some(event => event.kind === 'request' && event.open_id === admission.open_id && event.request_id === trial.requestId), 'pagehide finite request must belong to admitted producer');
          phase = 'pagehide_navigation';
          await candidate.goto('about:blank');
          phase = 'response_observation';
          await readiness.catch(() => {});
          const deadline = Date.now() + 4000;
          while (!trial.statuses.length && Date.now() < deadline) await new Promise(resolveWait => setTimeout(resolveWait, 20));
          assert.ok(!trial.failure, trial.failure);
          assert.equal(trial.reports.length, 1, 'sampled pagehide must report once');
          assert.ok(trial.statuses.length <= 1 && trial.statuses.every(status => status === 204), 'observed pagehide response must be single204; delivery still requires collector/export');
          const report = trial.reports[0];
          assert.equal(report.open_id, admission.open_id, 'pagehide report belongs to producer admission');
          assert.equal(report.outcome, 'incomplete', 'pagehide outcome is incomplete, not synthetic cancellation');
          assertRequestCorrelation([trial.requestId], report);
          assert.equal(report.marks.meaningful_transcript, null, 'incomplete case must precede presentation');
          trial.status = 'provisional_incomplete_collector_export_pending';
        } else {
          if (!admission.sampled) assert.equal(trial.reports.length, 0, 'unsampled admission cannot emit report');
          trial.status = admission.sampled ? 'sampled_already_ready' : 'unsampled';
        }
      })();
      await boundedObservation(operation, Math.max(1, caseDeadline - Date.now()), 'absolute pagehide stage');
    } catch (error) {
      primaryError = error;
      trial.failure ??= error.message;
    } finally {
      canceled = true;
      phase = 'context_closing';
      try {
        if (owned) await boundedObservation(owned.close(), 5000, 'pagehide context final drain');
        trial.context_close_returned = Boolean(owned);
        trial.transport_overflow = transportOverflow;
        assert.equal(transportOverflow, false, 'pagehide transport fact bound');
        assert.ok(!diagnosticFailure, diagnosticFailure);
        samplingDecision(trial.producer_events, route.startsWith('/global/') ? 'global' : 'ordinary');
        if (trial.status === 'unsampled') {
          assert.equal(trial.reports.length, 0, 'unsampled pagehide cannot emit report');
          assert.deepEqual(trial.statuses, []);
        }
        assert.ok(!trial.failure, 'pagehide late capture failure');
        if (trial.status === 'provisional_incomplete_collector_export_pending') {
          assert.equal(trial.reports.length, 1, 'late pagehide duplicate');
          assert.ok(trial.statuses.length <= 1 && trial.statuses.every(status => status === 204), 'late pagehide response must be single204; delivery still requires collector/export');
        }
      } catch (error) {
        trial.cleanup_failure = error.message;
        primaryError ??= error;
        trial.failure ??= error.message;
      }
      trials.push(trial);
      const stage = { status: trial.status ?? 'failed', scope: 'pagehide_incomplete_only_not_canceled_resume_or_background', trials };
      try { onTrial(trial, stage); }
      catch (error) {
        trial.callback_failure = error.message;
        primaryError ??= error;
        trial.failure ??= error.message;
      }
      if (outputDir) {
        try {
          mkdirSync(outputDir, { recursive: true });
          writeFileSync(resolve(outputDir, 'telemetry-pagehide-trials.json'), JSON.stringify(stage, null, 2));
        } catch {
          trial.persistence_failure = 'pagehide trial receipt persistence failed';
          primaryError ??= new Error(trial.persistence_failure);
          trial.failure ??= trial.persistence_failure;
        }
      }
    }
    if (primaryError) throw primaryError;
    if (trial.status === 'provisional_incomplete_collector_export_pending') return { status: 'provisional_incomplete_collector_export_pending', trials };
  }
  return { status: 'inconclusive_sampling_budget', trials };
}

export async function runVisibilityRoundTrip(page, context) {
  const witness = { scope: 'actual_tab_visibility_only_not_telemetry_background_readiness_or_physical_suspend', before: null, background: null, foreground: null };
  const other = await boundedObservation(context.newPage(), 5000, 'owned tab creation');
  try {
    await boundedObservation(page.bringToFront(), 5000, 'foreground tab');
    witness.before = await observePage(page, () => document.visibilityState);
    await boundedObservation(other.bringToFront(), 5000, 'background tab');
    witness.background = await observePage(page, () => document.visibilityState);
    await boundedObservation(page.bringToFront(), 5000, 'foreground tab');
    witness.foreground = await observePage(page, () => document.visibilityState);
    assert.equal(witness.before, 'visible');
    assert.equal(witness.foreground, 'visible');
    witness.status = witness.background === 'hidden' ? 'actual_tab_background_observed' : 'unsupported_no_actual_hidden_visibility';
    return witness;
  } finally {
    try { await boundedObservation(page.bringToFront(), 5000, 'foreground tab'); } finally { await boundedObservation(other.close(), 5000, 'owned tab cleanup'); }
  }
}

async function boundedObservation(promise, timeout, label) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} deadline`)), timeout);
    })]);
  } finally { clearTimeout(timer); }
}

async function observePage(page, expression) {
  return boundedObservation(page.evaluate(expression), 5000, 'page observation');
}

export function observePendingFiniteRequest(page, request, baseURL, expectedIdentity) {
  assert.ok(expectedIdentity && Object.hasOwn(expectedIdentity, 'productConversationId'));
  const requestURL = new URL(candidateDestination(request.url(), baseURL));
  const requestId = requestURL.searchParams.get('open_id');
  assert.ok(requestId && request.method() === 'GET');
  assert.ok(requestURL.pathname.startsWith('/api/product-conversations/') || requestURL.pathname.startsWith('/api/global/coordinator/history/'));
  assert.equal(request.timing().responseEnd, -1, 'capture must be armed before finite completion');
  const reports = [];
  const posts = new Map();
  let navigationFailure = null;
  let captureFailure = null;
  let observedPosts = 0;
  const onNavigation = frame => {
    if (frame !== page.mainFrame()) return;
    try { candidateDestination(frame.url(), baseURL); }
    catch (error) { navigationFailure ??= error; }
  };
  let finished = false;
  let finish;
  let accepted;
  const completion = new Promise(resolveCompletion => { finish = resolveCompletion; });
  const terminal = new Promise(resolveTerminal => { accepted = resolveTerminal; });
  const onFinished = observed => {
    if (observed === request) { finished = true; finish({ ok: true }); }
  };
  const onFailed = observed => {
    if (observed === request) { finished = true; finish({ ok: false }); }
  };
  const onRequest = observed => {
    const url = new URL(observed.url());
    if (url.origin !== new URL(baseURL).origin || url.pathname !== '/api/telemetry/conversation-view' || observed.method() !== 'POST') return;
    if (observedPosts >= 16 || reports.length >= 2) { captureFailure ??= new Error('terminal report count bound'); accepted({ error: captureFailure }); return; }
    observedPosts++;
    const body = observed.postData() ?? '';
    let report;
    try {
      assert.ok(body.length <= 8192 && Buffer.byteLength(body) <= 8192);
      report = captureReport(body, expectedIdentity.forbidden ?? []);
      if (!report.correlations?.some(correlation => correlation.kind === 'request' && correlation.id === requestId)) return;
      assertRequestCorrelation([requestId], report);
      assert.equal(report.product_conversation_id, expectedIdentity.productConversationId, 'terminal report must belong to the fixture view');
    } catch (error) { captureFailure ??= error; accepted({ error }); return; }
    reports.push(report);
    if (posts.size === 0) posts.set(observed, report);
  };
  const onResponse = response => {
    const report = posts.get(response.request());
    if (report) accepted({ report, status: response.status() });
  };
  page.on('framenavigated', onNavigation);
  page.on('requestfinished', onFinished);
  page.on('requestfailed', onFailed);
  page.on('request', onRequest);
  page.on('response', onResponse);
  return {
    requestId, reports,
    assertPending() {
      if (navigationFailure) throw navigationFailure;
      assert.ok(!finished && request.timing().responseEnd === -1, 'finite request must still be pending');
      assert.equal(reports.length, 0, 'journey must not already be terminal');
    },
    async waitFinished() {
      assert.equal((await boundedObservation(completion, 10000, 'finite response completion')).ok, true, 'finite request failed');
    },
    async waitReport() {
      const result = await boundedObservation(terminal, 5000, 'terminal collector response');
      if (navigationFailure) throw navigationFailure;
      if (captureFailure) throw captureFailure;
      if (result.error) throw result.error;
      assert.equal(reports.length, 1, 'one terminal report required');
      assert.equal(result.status, 204, 'actual collector acceptance required');
      return result.report;
    },
    assertFinal() {
      if (navigationFailure) throw navigationFailure;
      if (captureFailure) throw captureFailure;
      assert.equal(reports.length, 1, 'late duplicate report must fail case drain');
    },
    close() {
      page.off('framenavigated', onNavigation);
      page.off('requestfinished', onFinished);
      page.off('requestfailed', onFailed);
      page.off('request', onRequest);
      page.off('response', onResponse);
    },
  };
}

export async function runPendingBackgroundRoundTrip({ page, context, capture }) {
  assert.equal(page.context(), context);
  capture.assertPending();
  const startingURL = page.url();
  const origin = await observePage(page, () => performance.timeOrigin);
  const witness = { scope: 'pending_finite_response_actual_tab_visibility_not_physical_suspend', background: null, foregroundAt: null };
  const other = await boundedObservation(context.newPage(), 5000, 'owned tab creation');
  try {
    await boundedObservation(other.bringToFront(), 5000, 'background tab');
    witness.background = await observePage(page, () => document.visibilityState);
    if (witness.background !== 'hidden') {
      witness.status = 'unsupported_no_actual_hidden_visibility';
      return witness;
    }
    capture.assertPending();
    await capture.waitFinished();
    assert.equal(await observePage(page, () => document.visibilityState), 'hidden', 'response must finish while actually hidden');
    assert.equal(capture.reports.length, 0, 'pending hidden view must not become ready');
    witness.foregroundAt = await observePage(page, () => performance.now());
    await boundedObservation(page.bringToFront(), 5000, 'foreground tab');
    await page.waitForFunction(visibleReadiness, undefined, { timeout: 10000 });
    witness.status = 'actual_hidden_response_then_visible_readiness';
    witness.report = await capture.waitReport();
    assertRequestCorrelation([capture.requestId], witness.report);
    assertBackgroundReport(witness, witness.report);
    assert.equal(page.url(), startingURL, 'background case must retain its fixture view');
    assert.equal(await observePage(page, () => performance.timeOrigin), origin);
    return witness;
  } finally {
    try { await boundedObservation(page.bringToFront(), 5000, 'foreground tab'); } finally { await boundedObservation(other.close(), 5000, 'owned tab cleanup'); }
  }
}

export function assertBackgroundReport(witness, report) {
  assert.equal(witness.status, 'actual_hidden_response_then_visible_readiness');
  assert.equal(report.outcome, 'ready');
  assert.equal(report.visible, true);
  assert.ok(Number.isFinite(witness.foregroundAt));
  for (const mark of ['meaningful_transcript', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled']) {
    assert.ok(Number.isFinite(report.marks[mark]), `missing ${mark}`);
  }
  assert.ok(report.background_ms > 0, 'actual hidden interval must be retained');
}

export async function runPendingSPACancellation({ page, baseURL, selector, capture }) {
  capture.assertPending();
  candidateDestination(page.url(), baseURL);
  const target = page.locator(selector);
  assert.equal(await target.count(), 1, 'exactly one mounted SPA anchor required');
  assert.equal(await target.isVisible(), true, 'SPA anchor must be CSS visible');
  const href = await target.getAttribute('href');
  assert.ok(href, 'cancellation must use a real navigable anchor');
  const destination = candidateDestination(href, baseURL);
  assert.notEqual(destination, page.url(), 'route replacement must change destination');
  const origin = await observePage(page, () => performance.timeOrigin);
  capture.assertPending();
  await target.click({ timeout: 10000 });
  await page.waitForURL(destination, { timeout: 10000 });
  assert.equal(await observePage(page, () => performance.timeOrigin), origin, 'cross-document pagehide is not SPA cancellation');
  assert.equal(candidateDestination(page.url(), baseURL), destination);
  const report = await capture.waitReport();
  assert.equal(candidateDestination(page.url(), baseURL), destination);
  assert.equal(await observePage(page, () => performance.timeOrigin), origin);
  assert.equal(report.outcome, 'canceled', 'ready/incomplete may not be relabeled canceled');
  assertRequestCorrelation([capture.requestId], report);
  return { scope: 'actual_anchor_same_document_route_replacement', status: 'observed_spa_canceled', report };
}

export async function runBFCacheRoundTrip(page, baseURL) {
  const startingURL = candidateDestination(page.url(), baseURL);
  try {
    const before = await boundedObservation(page.evaluate(() => {
      window.__phoenixQualificationPageshow = null;
      window.addEventListener('pageshow', event => {
        window.__phoenixQualificationPageshow = { persisted: event.persisted, timeOrigin: performance.timeOrigin };
      }, { once: true });
      return performance.timeOrigin;
    }), 5000, 'BFCache witness setup');
    await page.goto('about:blank', { timeout: 10000 });
    await page.goBack({ timeout: 10000, waitUntil: 'domcontentloaded' });
    candidateDestination(page.url(), baseURL);
    const observed = await boundedObservation(page.evaluate(() => window.__phoenixQualificationPageshow ?? null), 5000, 'BFCache witness read');
    const restored = observed?.persisted === true && observed.timeOrigin === before;
    if (restored) await page.waitForFunction(visibleReadiness, undefined, { timeout: 10000 });
    return { scope: 'actual_pageshow_persisted_and_same_document_not_standalone_or_physical_suspend',
      status: restored ? 'actual_bfcache_restore_observed' : 'unsupported_no_actual_bfcache_restore', observed };
  } catch (error) {
    try { await page.goto(startingURL, { timeout: 10000, waitUntil: 'domcontentloaded' }); }
    catch (cleanupError) { throw new AggregateError([error, cleanupError], 'BFCache case and bounded restoration failed'); }
    throw error;
  }
}

export function appendLifecycleInventory(result, stage, identity, forbidden = []) {
  const added = [];
  const ids = new Set(result.journeys.map(journey => journey.open_id));
  for (const trial of stage.trials) for (const report of trial.reports) {
    captureReport(JSON.stringify(report), forbidden);
    assert.ok(!ids.has(report.open_id), 'unique lifecycle journey required');
    ids.add(report.open_id);
    added.push(journeyExpectation(report, identity));
  }
  assert.ok(result.journeys.length + added.length <= 64, 'final journey inventory bound');
  result.journeys.push(...added);
  result.lifecycle = stage;
  for (const trial of stage.trials) result.capabilities[trial.kind] = trial.status;
}

export async function runLifecycleQualificationStage({ browser, storageState, baseURL, fixture, outputDir, forbidden = [], budgetMs = 90000, onTrial = () => {} }) {
  assert.ok(fixture && Object.hasOwn(fixture, 'productConversationId'), 'authoritative lifecycle fixture product identity required');
  const destination = candidateDestination(fixture.route, baseURL);
  assert.ok(typeof fixture.spaSelector === 'string' && fixture.spaSelector.length > 0, 'existing SPA anchor selector required');
  assert.ok(Number.isInteger(budgetMs) && budgetMs > 0 && budgetMs <= 90000);
  const deadline = Date.now() + budgetMs;
  const stage = { scope: 'real_browser_actions_when_invoked_not_candidate_acceptance_or_overhead', trials: [] };
  for (const kind of ['pending_background', 'spa_canceled', 'bfcache_resume']) {
    for (let attempt = 0; attempt < 2; attempt++) {
      let owned;
      const trial = { kind, attempt, reports: [], statuses: [], status: 'started' };
      let capture;
      let page;
      const posts = new Map();
      let ledgerFailure = null;
      let captureArmed = false;
      let canceled = false;
      let finiteSeen;
      const pending = new Promise(resolvePending => { finiteSeen = resolvePending; });
      try {
        assert.ok(Date.now() < deadline, 'lifecycle stage admission deadline');
        const operation = (async () => {
          owned = await boundedObservation(browser.newContext({ storageState, viewport: { width: 1280, height: 900 } }).then(async created => {
            if (canceled) {
              await boundedObservation(created.close(), 5000, 'late lifecycle context cleanup');
              throw new Error('lifecycle context arrived after cancellation');
            }
            return created;
          }), 5000, 'lifecycle context');
          page = await boundedObservation(owned.newPage(), 5000, 'lifecycle page');
          assert.ok(!canceled, 'lifecycle page admission canceled');
          const cdp = await owned.newCDPSession(page);
          await cdp.send('Network.enable');
          await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: 400, downloadThroughput: 1000000, uploadThroughput: 1000000 });
          page.on('request', request => {
            const url = new URL(request.url());
            if (url.origin !== new URL(baseURL).origin) return;
            if (url.pathname === '/api/telemetry/conversation-view' && request.method() === 'POST') {
              if (trial.reports.length >= 16) { ledgerFailure ??= new Error('lifecycle report count bound'); return; }
              const body = request.postData() ?? '';
              try {
                assert.ok(body.length <= 8192 && Buffer.byteLength(body) <= 8192);
                const report = captureReport(body, forbidden);
                trial.reports.push(report);
                posts.set(request, report);
              } catch (error) { ledgerFailure ??= error; }
            } else if (!captureArmed && url.searchParams.has('open_id') && request.method() === 'GET'
              && (url.pathname.startsWith('/api/product-conversations/') || url.pathname.startsWith('/api/global/coordinator/history/'))) {
              try {
                captureArmed = true;
                capture = observePendingFiniteRequest(page, request, baseURL, { ...fixture, forbidden });
                finiteSeen('pending');
              } catch (error) { ledgerFailure ??= error; finiteSeen('invalid_pending'); }
            }
          });
          page.on('response', response => {
            if (posts.has(response.request())) trial.statuses.push({ open_id: posts.get(response.request()).open_id, status: response.status() });
          });
          const navigation = page.goto(destination, { timeout: 10000, waitUntil: 'domcontentloaded' });
          const ready = page.waitForFunction(visibleReadiness, undefined, { timeout: 10000 }).then(() => 'ready', () => 'unavailable');
          const winner = await Promise.race([pending, ready]);
          await navigation;
          if (ledgerFailure) throw ledgerFailure;
          if (winner === 'ready') {
            trial.status = 'inconclusive_readiness_preceded_finite_request_sampling_unknown';
          } else {
            assert.equal(winner, 'pending', 'actual pending admission required');
            if (kind === 'pending_background') {
              trial.witness = await runPendingBackgroundRoundTrip({ page, context: owned, capture });
              trial.status = trial.witness.status;
            } else if (kind === 'spa_canceled') {
              trial.witness = await runPendingSPACancellation({ page, baseURL, selector: fixture.spaSelector, capture });
              trial.status = trial.witness.status;
              await page.waitForFunction(visibleReadiness, undefined, { timeout: 10000 });
            } else {
              assert.equal(await ready, 'ready');
              const initial = await capture.waitReport();
              assert.equal(initial.outcome, 'ready');
              capture.assertFinal();
              capture.close();
              capture = null;
              const preceding = trial.reports.length;
              trial.witness = await runBFCacheRoundTrip(page, baseURL);
              trial.status = trial.witness.status;
              if (trial.status === 'actual_bfcache_restore_observed') {
                const until = Math.min(deadline, Date.now() + 5000);
                while (trial.reports.length === preceding && Date.now() < until) await new Promise(resolveWait => setTimeout(resolveWait, 20));
                const resumed = trial.reports.slice(preceding);
                if (!resumed.length) trial.status = 'actual_bfcache_restore_no_resume_report_sampling_unknown';
                else {
                  assert.equal(resumed.length, 1);
                  assert.notEqual(resumed[0].open_id, initial.open_id, 'fresh resumed journey identity required');
                  assert.equal(resumed[0].trigger, 'back_forward');
                  assert.equal(resumed[0].outcome, 'ready');
                  assert.equal(resumed[0].visible, true);
                  assert.equal(resumed[0].product_conversation_id, fixture.productConversationId);
                  assert.equal(resumed[0].route, initial.route);
                  assert.equal(resumed[0].cache, 'retained');
                  assert.equal(page.url(), destination);
                  for (const mark of ['meaningful_transcript', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled']) assert.ok(Number.isFinite(resumed[0].marks[mark]));
                  trial.status = 'observed_bfcache_fresh_ready_report';
                }
              }
            }
          }
          // Report-response drain is tied to every observed POST, including sampled SPA successor admissions.
          const until = Math.min(deadline, Date.now() + 5000);
          while (trial.statuses.length < trial.reports.length && Date.now() < until) await new Promise(resolveWait => setTimeout(resolveWait, 20));
          assert.equal(trial.statuses.length, trial.reports.length, 'every observed report must have collector response');
          assert.ok(trial.statuses.every(response => response.status === 204));
          if (ledgerFailure) throw ledgerFailure;
          if (capture && !trial.status.startsWith('unsupported')) capture.assertFinal();
        })();
        await boundedObservation(operation, Math.max(1, deadline - Date.now()), 'absolute lifecycle stage');
      } catch (error) {
        trial.failure = error.message;
        throw error;
      } finally {
        canceled = true;
        try {
          if (owned) await boundedObservation(owned.close(), 5000, 'lifecycle context cleanup');
          if (ledgerFailure) throw ledgerFailure;
          if (!trial.failure) {
            assert.equal(trial.statuses.length, trial.reports.length, 'final report-response drain');
            assert.equal(new Set(trial.reports.map(report => report.open_id)).size, trial.reports.length);
            if (trial.status === 'observed_bfcache_fresh_ready_report') assert.equal(trial.reports.length, 2);
          }
          if (!trial.failure && capture && !trial.status.startsWith('unsupported')) capture.assertFinal();
        } catch (error) { trial.failure ??= error.message; throw error; }
        finally {
          capture?.close();
          stage.trials.push(trial);
          try { onTrial(trial, stage); }
          catch (error) { trial.failure ??= error.message; throw error; }
          finally { if (outputDir) writeFileSync(resolve(outputDir, 'telemetry-lifecycle-trials.json'), JSON.stringify(stage, null, 2)); }
        }
      }
      if (!trial.status.startsWith('inconclusive')) break;
    }
  }
  return stage;
}

function visibleReadiness() {
  const visible = element => {
    if (!element || document.visibilityState !== 'visible') return false;
    for (let node = element; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (node.hidden || node.getAttribute('aria-hidden') === 'true' || style.display === 'none' || style.visibility !== 'visible') return false;
    }
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0 && rect.bottom > 0 && rect.top < innerHeight;
  };
  const messages = document.querySelector('#messages');
  const nav = document.querySelector('#conversation-nav-trail');
  return visible(messages) && [...messages.querySelectorAll('[data-render-unit-key] > *, .empty-state')].some(visible)
    && visible(nav) && [...nav.querySelectorAll('.conversation-nav-item[role="button"]:not([aria-disabled="true"]):not([disabled])')].some(visible);
}

export async function runTelemetryChecks({ page, context, baseURL, serviceName, runId, outputDir, fixtures }) {
  assert.equal(new URL(baseURL).hostname, '127.0.0.1');
  assert.match(runId, /^[A-Za-z0-9_-]{1,80}$/);
  assert.ok(serviceName.endsWith(runId));
  assert.ok(fixtures.identity?.serverInstanceId);
  assert.match(fixtures.identity?.appSource ?? '', /^[a-f0-9]{40}$/);
  assert.match(fixtures.identity?.frontendSource ?? '', /^[a-f0-9]{40}$/);
  assert.equal(page.context(), context);
  const aggregates = ['ordinary', 'global'].map(kind => {
    const aggregate = fixtures.aggregates.find(a => a.kind === kind);
    assert.ok(aggregate?.route);
    candidateDestination(aggregate.route, baseURL);
    return aggregate;
  });
  assert.ok(fixtures.lifecycle && Object.hasOwn(fixtures.lifecycle, 'productConversationId'), 'authoritative lifecycle fixture required before contexts');
  candidateDestination(fixtures.lifecycle.route, baseURL);
  assert.ok(fixtures.lifecycle.spaSelector, 'existing SPA navigation anchor required before contexts');
  const result = { runId, serviceName, identity: fixtures.identity, trials: [], journeys: [], forbidden: fixtures.forbidden ?? [],
    capabilities: { physical_mobile: 'not_tested', standalone_resume: 'not_exercised', bfcache_resume: 'not_exercised', background_terminal: 'not_exercised' } };
  mkdirSync(outputDir, { recursive: true });
  const receiptPath = resolve(outputDir, 'telemetry-browser-result.json');
  const outerDeadline = Date.now() + 285_000;
  const storageState = await boundedObservation(context.storageState(), 5000, 'qualification storage state');
  let negativeTemplate = null;
  const deadline = Math.min(outerDeadline, Date.now() + 150_000);
  const ownedContexts = new Set();
  const browser = { newContext: async options => {
    assert.ok(Date.now() < outerDeadline, 'outer context admission deadline');
    const owned = await context.browser().newContext(options);
    ownedContexts.add(owned);
    owned.on?.('close', () => ownedContexts.delete(owned));
    if (Date.now() >= outerDeadline) {
      await boundedObservation(owned.close(), 5000, 'late context cleanup');
      throw new Error('outer context creation deadline');
    }
    return owned;
  } };
  let execution;
  try {
    execution = (async () => {
      result.auth_preflight = await requireCollectorAuth(context, baseURL, true);
      for (let index = 0; index < 48; index++) {
        assert.ok(Date.now() < deadline, 'telemetry context budget deadline');
        const aggregate = aggregates[index % 2];
        const owned = await browser.newContext({ storageState, viewport: { width: 1280, height: 900 } });
        const trial = { index, route: aggregate.kind, warmup: index < 2, sampled: false, posts: 0, payloadBytes: 0, correlatedRequests: 0 };
        const requests = [];
        const reports = [];
        const statuses = [];
        const captureErrors = [];
        const diagnosticEvents = [];
        const requestFacts = [];
        const requestOwners = new Map();
        let primaryError;
        let diagnosticPhase = 'open';
        try {
          await owned.exposeBinding('__phoenixViewDiagnostic', (_source, event) => {
            if (diagnosticEvents.length >= 64) { if (!captureErrors.length) captureErrors.push('diagnostic event bound'); return; }
            try { diagnosticEvents.push({ ...captureDiagnostic(event), observed_phase: diagnosticPhase }); }
            catch { if (!captureErrors.length) captureErrors.push('invalid diagnostic event'); }
          });
          await owned.addInitScript(() => {
            const pending = [];
            let count = 0;
            let deliveryFailed = false;
            window.addEventListener('phoenix:view-diagnostic', event => {
              if (count++ > 64) return;
              pending.push(window.__phoenixViewDiagnostic(event.detail).catch(() => { deliveryFailed = true; }));
            });
            window.__phoenixViewDiagnosticDrain = async () => {
              await Promise.all(pending);
              if (deliveryFailed) throw new Error('producer diagnostic delivery failed');
            };
          });
          const candidate = await owned.newPage();
          const cdp = await owned.newCDPSession(candidate);
          await cdp.send('Performance.enable');
          await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 });
          candidate.on('request', request => {
            const fact = requestFact(request, baseURL);
            if (fact) {
              if (requestFacts.length >= 32) { if (!captureErrors.length) captureErrors.push('request fact bound'); }
              else { requestFacts.push({ ordinal: requestFacts.length, observed_phase: diagnosticPhase, ...fact }); requestOwners.set(request, requestFacts.at(-1)); }
            }
            const url = new URL(request.url());
            if (url.origin === new URL(baseURL).origin && url.pathname === '/api/telemetry/conversation-view' && request.method() === 'POST') {
              if (trial.posts >= 2) { if (!captureErrors.length) captureErrors.push('report count bound'); return; }
              trial.posts++;
              const body = request.postData() ?? '';
              trial.payloadBytes += Buffer.byteLength(body);
              if (Buffer.byteLength(body) > 8192) { if (!captureErrors.length) captureErrors.push('report byte bound'); }
              else {
                try {
                  const report = captureReport(body, fixtures.forbidden ?? []);
                  reports.push(report);
                  const reportRequest = requestOwners.get(request);
                  if (reportRequest) reportRequest.report_index = reports.length - 1;
                } catch { if (!captureErrors.length) captureErrors.push('invalid or unsafe report JSON'); }
              }
            } else if (url.origin === new URL(baseURL).origin && (url.pathname.startsWith('/api/product-conversations/') || url.pathname.startsWith('/api/global/coordinator/history/')) && url.searchParams.has('open_id')) { if (requests.length < 32) requests.push(url.searchParams.get('open_id')); else if (!captureErrors.length) captureErrors.push('correlated request bound'); }
          });
          candidate.on('requestfailed', request => { const fact = requestOwners.get(request); if (fact) fact.request_failed = true; });
          candidate.on('response', response => {
            const fact = requestOwners.get(response.request()); if (fact) fact.response_status = response.status();
            if (new URL(response.url()).origin === new URL(baseURL).origin && new URL(response.url()).pathname === '/api/telemetry/conversation-view' && statuses.length < 2) statuses.push(response.status());
          });
          const before = await cdp.send('Performance.getMetrics');
          const navigationStart = performance.now();
          await candidate.goto(candidateDestination(aggregate.route, baseURL), { timeout: 10_000 });
          await candidate.waitForFunction(visibleReadiness, undefined, { timeout: 10_000 });
          trial.navigation_to_visible_readiness_ms = performance.now() - navigationStart;
          trial.readiness_clock = 'driver_monotonic_includes_automation_latency_not_paint';
          assert.ok(Number.isFinite(trial.navigation_to_visible_readiness_ms) && trial.navigation_to_visible_readiness_ms >= 0);
          await candidate.evaluate(() => new Promise(resolveFrame => requestAnimationFrame(() => requestAnimationFrame(resolveFrame))));
          const after = await cdp.send('Performance.getMetrics');
          trial.rawMetrics = { before: before.metrics, after: after.metrics };
          trial.metric_reset = after.metrics.some(m => ['TaskDuration', 'ScriptDuration', 'LayoutDuration', 'RecalcStyleDuration'].includes(m.name) && m.value < (before.metrics.find(b => b.name === m.name)?.value ?? 0));
          assert.deepEqual(captureErrors, []);
          trial.metrics = Object.fromEntries(after.metrics.filter(m => ['TaskDuration', 'ScriptDuration', 'LayoutDuration', 'RecalcStyleDuration', 'JSHeapUsedSize'].includes(m.name)).map(m => [m.name, m.value]));
          trial.metric_method = 'navigation-scoped post-readiness raw counters; no aboutblank subtraction';
          await candidate.evaluate(() => window.__phoenixViewDiagnosticDrain());
          assert.deepEqual(captureErrors, []);
          const admission = samplingDecision(diagnosticEvents, aggregate.kind);
          trial.sampled = admission.sampled;
          trial.admitted_open_id = admission.open_id;
          trial.correlatedRequests = requests.length;
          if (trial.sampled) {
            const reportDeadline = Date.now() + 5000;
            while ((!reports.length || !statuses.length) && Date.now() < reportDeadline) await new Promise(resolveWait => setTimeout(resolveWait, 20));
            assert.equal(reports.length, 1, 'one sampled initial journey report required');
            assert.deepEqual(statuses, [204], 'actual collector acceptance required');
            const report = reports[0];
            assert.equal(report.open_id, trial.admitted_open_id, 'report must belong to producer admission');
            negativeTemplate ??= report;
            assertRequestCorrelation(requests, report);
            assert.equal(report.outcome, 'ready');
            assert.equal(report.presentation_evidence, 'commit_frame_estimate');
            assert.equal(report.route, aggregate.kind);
            assert.equal(report.frontend_source, fixtures.identity.frontendSource);
            assert.equal(report.frontend_source_dirty, false);
            assert.equal(report.sample_rate, 0.25);
            assert.equal(report.visible, true);
            assert.ok(report.open_id && report.marks.meaningful_transcript !== null && report.marks.breadcrumb_enabled !== null && report.marks.anchor_settled !== null);
            assert.ok((fixtures.forbidden ?? []).every(secret => !JSON.stringify(report).includes(secret)), 'report sentinel privacy');
            assert.ok(report.correlations.length <= 16);
            assert.equal(report.breadcrumb_ready_minus_meaningful_ms, Math.max(report.marks.breadcrumb_presented, report.marks.breadcrumb_enabled) - report.marks.meaningful_transcript);
            result.journeys.push(journeyExpectation(report, fixtures.identity));
            assert.ok(Number.isInteger(report.observer_inspection_count) && report.observer_inspection_count >= 0);
            assert.equal(typeof report.observer_scan_limited, 'boolean');
            trial.observer = { inspection_count: report.observer_inspection_count, scan_limited: report.observer_scan_limited,
              background_ms: report.background_ms, background_interval_count: report.background_interval_count,
              sample_rate: report.sample_rate, admitted_count: report.admitted_count, sampled_count: report.sampled_count,
              transport_dropped_count: report.transport_dropped_count, transport_failed_count: report.transport_failed_count };
          }
          if (!trial.sampled) {
            assert.equal(trial.posts, 0, 'unsampled cohort must not contain report POST');
            assert.deepEqual(reports, []);
          }
          if (!result.visibilityWitness && trial.sampled) {
            result.visibilityWitness = await runVisibilityRoundTrip(candidate, owned);
            result.capabilities.tab_visibility = result.visibilityWitness.status;
          }
          assert.ok(trial.posts <= 1, 'no per-frame or repeated POST');
        } catch (error) { primaryError = error; trial.failure = error.message; throw error; }
        finally {
          try {
            diagnosticPhase = 'closing';
            await boundedObservation(owned.close(), 5000, 'initial context final drain');
            trial.context_closed = true;
            trial.close_time_producer_delivery = 'not_proven';
            samplingDecision(diagnosticEvents, trial.route);
            assert.deepEqual(captureErrors, [], 'late initial capture failure');
            assert.equal(trial.posts, trial.sampled ? 1 : 0, 'late initial duplicate or unexpected POST');
            assert.equal(reports.length, trial.sampled ? 1 : 0);
            assert.deepEqual(statuses, trial.sampled ? [204] : []);
          } catch (error) { throw initialFailure(trial, primaryError, error); }
          finally { trial.reports = reports; trial.response_statuses = statuses; trial.request_facts = requestFacts; trial.producer_events = diagnosticEvents; trial.capture_errors = captureErrors; result.trials.push(trial); }
        }
        if (Object.values(summarizeTrials(result.trials).strata).every(s => s.status === 'raw_cohorts_ready_for_review')) break;
      }
      assert.ok(negativeTemplate, 'at least one naturally sampled report required for collector negatives');
      result.collectorNegatives = await runCollectorNegativeChecks(context, baseURL, negativeTemplate);
      result.collectorNegatives.push(await runUnauthenticatedCollectorCheck(browser, baseURL, negativeTemplate));
      await runPendingPagehideCheck({ browser, storageState, baseURL,
        route: aggregates[0].route, outputDir, forbidden: fixtures.forbidden ?? [], attempts: 2, budgetMs: 20000,
        onTrial: (trial, stage) => {
          result.terminal = stage;
          appendTerminalInventory(result, { ...stage, trials: [trial] }, fixtures.identity, fixtures.forbidden ?? []);
          result.terminal = stage;
        } });
      await runLifecycleQualificationStage({ browser, storageState, baseURL,
        fixture: fixtures.lifecycle, outputDir, forbidden: fixtures.forbidden ?? [], onTrial: (trial, stage) => {
          result.lifecycle = stage;
          appendLifecycleInventory(result, { trials: [trial] }, fixtures.identity, fixtures.forbidden ?? []);
          result.lifecycle = stage;
        } });
      result.comparison = summarizeTrials(result.trials);
      result.acceptance = 'partial_initial_terminal_and_lifecycle_export_raw_overhead_only';
      return result;
    })();
    return await boundedObservation(execution, Math.max(1, outerDeadline - Date.now()), 'absolute browser qualification');
  } catch (error) {
    result.failure = error.message;
    if (error.cleanup_failure) result.cleanup_failure = error.cleanup_failure;
    if (error.context_cleanup_failure) result.context_cleanup_failure = error.context_cleanup_failure;
    throw error;
  } finally {
    const failedBeforeCleanup = Boolean(result.failure);
    let cleanupFailed = false;
    try {
      await boundedObservation(Promise.all([...ownedContexts].map(owned => owned.close())), 5000, 'outer context cleanup');
      if (result.failure && execution) await boundedObservation(execution.catch(() => {}), 5000, 'outer operation drain');
    } catch { result.cleanup_failure = 'outer cleanup or operation drain failed'; result.failure ??= result.cleanup_failure; cleanupFailed = true; }
    writeFileSync(receiptPath, JSON.stringify(result, null, 2));
    if (cleanupFailed && !failedBeforeCleanup) throw new Error('outer cleanup or operation drain failed');
  }
}
