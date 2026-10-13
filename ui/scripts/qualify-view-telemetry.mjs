import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';

export function summarizeTrials(trials, minimum = 8) {
  const strata = {};
  for (const route of ['ordinary', 'global']) {
    const measured = trials.filter(t => t.route === route && !t.warmup && !t.metric_reset);
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
    const response = await context.request.post(endpoint, { data: report, timeout: 3000 });
    assert.equal(response.status(), expectedStatus, `collector must reject ${name}`);
    results.push({ name, status: response.status() });
    await response.dispose();
  }
  const oversized = await context.request.post(endpoint, { data: 'x'.repeat(8193), headers: { 'Content-Type': 'application/json' }, timeout: 3000 });
  assert.equal(oversized.status(), 413, 'collector body bound');
  results.push({ name: 'oversized_body', status: oversized.status() });
  await oversized.dispose();
  return results;
}

export async function runUnauthenticatedCollectorCheck(browser, baseURL, validReport) {
  const isolated = await browser.newContext({ storageState: { cookies: [], origins: [] } });
  try {
    const response = await isolated.request.post(candidateDestination('/api/telemetry/conversation-view', baseURL), { data: validReport, timeout: 3000 });
    try {
      assert.equal(response.status(), 401, 'synthetic server must enforce collector authentication');
      return { name: 'unauthenticated_report', status: response.status() };
    } finally { await response.dispose(); }
  } finally { await isolated.close(); }
}

export function journeyExpectation(report, identity) {
  assert.equal(report.frontend_source, identity.frontendSource);
  assert.equal(report.frontend_source_dirty, false);
  assert.equal(report.presentation_evidence, 'commit_frame_estimate');
  const expected = {
    'server.backend_source': identity.appSource, 'server.instance_id': identity.serverInstanceId,
    'browser.frontend_source': identity.frontendSource, 'browser.outcome': report.outcome,
    'browser.route': report.route, 'browser.trigger': report.trigger,
    'browser.presentation_evidence': report.presentation_evidence, 'browser.total_ms': report.total_ms,
  };
  for (const mark of ['meaningful_transcript', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled']) expected[`browser.${mark}_ms`] = report.marks[mark];
  expected['browser.breadcrumb_ready_minus_meaningful_ms'] = report.breadcrumb_ready_minus_meaningful_ms;
  return { open_id: report.open_id, correlations: report.correlations, expected };
}

export function appendTerminalInventory(result, terminal, identity, forbidden = []) {
  for (const trial of terminal.trials) for (const report of trial.reports) {
    assert.ok(forbidden.every(secret => !JSON.stringify(report).includes(secret)));
    assert.ok(!result.journeys.some(j => j.open_id === report.open_id), 'journey identity must be unique across contexts');
    result.journeys.push(journeyExpectation(report, identity));
  }
  result.terminal = terminal;
  result.capabilities.pagehide_incomplete = terminal.status;
}

export async function runPendingPagehideCheck({ browser, storageState, baseURL, route, outputDir, attempts = 8, budgetMs = 40000 }) {
  assert.ok(Number.isInteger(attempts) && attempts > 0 && attempts <= 8);
  assert.ok(Number.isInteger(budgetMs) && budgetMs > 0 && budgetMs <= 40000);
  const caseDeadline = Date.now() + budgetMs;
  const destination = candidateDestination(route, baseURL);
  const trials = [];
  for (let attempt = 0; attempt < attempts && Date.now() < caseDeadline; attempt++) {
    const owned = await browser.newContext({ storageState, viewport: { width: 1280, height: 900 } });
    const trial = { attempt, sampled: false, reports: [], statuses: [], event: 'actual_cross_document_pagehide' };
    try {
      const candidate = await owned.newPage();
      const cdp = await owned.newCDPSession(candidate);
      await cdp.send('Network.enable');
      await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: 200, downloadThroughput: 1000000, uploadThroughput: 1000000 });
      let signal;
      const requestSeen = new Promise(resolveRequest => { signal = resolveRequest; });
      candidate.on('request', request => {
        const url = new URL(request.url());
        if (url.origin === new URL(baseURL).origin && url.searchParams.has('open_id')
          && (url.pathname.startsWith('/api/product-conversations/') || url.pathname.startsWith('/api/global/coordinator/history/'))) {
          trial.sampled = true;
          trial.requestId = url.searchParams.get('open_id');
          signal();
        }
        if (url.pathname === '/api/telemetry/conversation-view' && request.method() === 'POST') {
          const body = request.postData() ?? '';
          if (Buffer.byteLength(body) > 8192) trial.failure = 'report byte bound';
          else {
            try { trial.reports.push(JSON.parse(body)); } catch { trial.failure = 'invalid report JSON'; }
          }
        }
      });
      candidate.on('response', response => {
        if (new URL(response.url()).pathname === '/api/telemetry/conversation-view') trial.statuses.push(response.status());
      });
      const navigation = candidate.goto(destination, { timeout: 10000, waitUntil: 'domcontentloaded' });
      const readiness = candidate.waitForFunction(visibleReadiness, undefined, { timeout: 10000 }).then(() => 'ready', () => 'readiness_unavailable');
      const winner = await Promise.race([requestSeen.then(() => 'request'), readiness]);
      await navigation;
      assert.notEqual(winner, 'readiness_unavailable', 'case readiness unavailable before sampled request');
      if (winner === 'request') {
        await candidate.goto('about:blank');
        await readiness.catch(() => {});
        const deadline = Date.now() + 4000;
        while (!trial.statuses.length && Date.now() < deadline) await new Promise(resolveWait => setTimeout(resolveWait, 20));
        assert.ok(!trial.failure, trial.failure);
        assert.equal(trial.reports.length, 1, 'sampled pagehide must report once');
        assert.deepEqual(trial.statuses, [204]);
        const report = trial.reports[0];
        assert.equal(report.outcome, 'incomplete', 'pagehide outcome is incomplete, not synthetic cancellation');
        assertRequestCorrelation([trial.requestId], report);
        assert.equal(report.marks.meaningful_transcript, null, 'incomplete case must precede presentation');
        trial.status = 'observed_incomplete';
      } else trial.status = 'unsampled_or_already_ready';
    } catch (error) {
      trial.failure = error.message;
      throw error;
    } finally {
      trials.push(trial);
      await owned.close();
      if (outputDir) {
        mkdirSync(outputDir, { recursive: true });
        writeFileSync(resolve(outputDir, 'telemetry-pagehide-trials.json'), JSON.stringify({ scope: 'pagehide_incomplete_only_not_canceled_resume_or_background', trials }, null, 2));
      }
    }
    if (trial.status === 'observed_incomplete') return { status: 'observed_incomplete', trials };
  }
  return { status: 'inconclusive_sampling_budget', trials };
}

export async function runVisibilityRoundTrip(page, context) {
  const witness = { scope: 'actual_tab_visibility_only_not_telemetry_background_readiness_or_physical_suspend', before: null, background: null, foreground: null };
  const other = await context.newPage();
  try {
    await page.bringToFront();
    witness.before = await page.evaluate(() => document.visibilityState);
    await other.bringToFront();
    witness.background = await page.evaluate(() => document.visibilityState);
    await page.bringToFront();
    witness.foreground = await page.evaluate(() => document.visibilityState);
    assert.equal(witness.before, 'visible');
    assert.equal(witness.foreground, 'visible');
    witness.status = witness.background === 'hidden' ? 'actual_tab_background_observed' : 'unsupported_no_actual_hidden_visibility';
    return witness;
  } finally {
    try { await page.bringToFront(); } finally { await other.close(); }
  }
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
  const result = { runId, serviceName, identity: fixtures.identity, trials: [], journeys: [], forbidden: fixtures.forbidden ?? [],
    capabilities: { physical_mobile: 'not_tested', standalone_resume: 'not_exercised', bfcache_resume: 'not_exercised', background_terminal: 'not_exercised' } };
  mkdirSync(outputDir, { recursive: true });
  const receiptPath = resolve(outputDir, 'telemetry-browser-result.json');
  const storageState = await context.storageState();
  let negativeTemplate = null;
  const deadline = Date.now() + 180_000;
  try {
    for (let index = 0; index < 56; index++) {
      assert.ok(Date.now() < deadline, 'telemetry context budget deadline');
      const aggregate = aggregates[index % 2];
      const owned = await context.browser().newContext({ storageState, viewport: { width: 1280, height: 900 } });
      const trial = { index, route: aggregate.kind, warmup: index < 2, sampled: false, posts: 0, payloadBytes: 0, correlatedRequests: 0 };
      try {
        const candidate = await owned.newPage();
        const cdp = await owned.newCDPSession(candidate);
        await cdp.send('Performance.enable');
        await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 });
        const requests = [];
        const reports = [];
        const statuses = [];
        const captureErrors = [];
        candidate.on('request', request => {
          const url = new URL(request.url());
          if (url.pathname === '/api/telemetry/conversation-view' && request.method() === 'POST') {
            trial.posts++;
            const body = request.postData() ?? '';
            trial.payloadBytes += Buffer.byteLength(body);
            if (Buffer.byteLength(body) > 8192) captureErrors.push('report byte bound');
            else {
              try { reports.push(JSON.parse(body)); } catch { captureErrors.push('invalid report JSON'); }
            }
          } else if (url.origin === new URL(baseURL).origin && (url.pathname.startsWith('/api/product-conversations/') || url.pathname.startsWith('/api/global/coordinator/history/')) && url.searchParams.has('open_id')) requests.push(url.searchParams.get('open_id'));
        });
        candidate.on('response', response => {
          if (new URL(response.url()).pathname === '/api/telemetry/conversation-view') statuses.push(response.status());
        });
        const before = await cdp.send('Performance.getMetrics');
        await candidate.goto(candidateDestination(aggregate.route, baseURL), { timeout: 10_000 });
        await candidate.waitForFunction(visibleReadiness, undefined, { timeout: 10_000 });
        await candidate.evaluate(() => new Promise(resolveFrame => requestAnimationFrame(() => requestAnimationFrame(resolveFrame))));
        const after = await cdp.send('Performance.getMetrics');
        trial.rawMetrics = { before: before.metrics, after: after.metrics };
        trial.metric_reset = after.metrics.some(m => ['TaskDuration', 'ScriptDuration', 'LayoutDuration', 'RecalcStyleDuration'].includes(m.name) && m.value < (before.metrics.find(b => b.name === m.name)?.value ?? 0));
        assert.deepEqual(captureErrors, []);
        trial.metrics = Object.fromEntries(after.metrics.filter(m => ['TaskDuration', 'ScriptDuration', 'LayoutDuration', 'RecalcStyleDuration', 'JSHeapUsedSize'].includes(m.name)).map(m => [m.name, m.value]));
        trial.metric_method = 'navigation-scoped post-readiness raw counters; no aboutblank subtraction';
        trial.sampled = requests.length > 0;
        trial.correlatedRequests = requests.length;
        if (trial.sampled) {
          const reportDeadline = Date.now() + 5000;
          while ((!reports.length || !statuses.length) && Date.now() < reportDeadline) await new Promise(resolveWait => setTimeout(resolveWait, 20));
          assert.equal(reports.length, 1, 'one sampled initial journey report required');
          assert.deepEqual(statuses, [204], 'actual collector acceptance required');
          const report = reports[0];
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
      } finally {
        result.trials.push(trial);
        await owned.close();
      }
      if (Object.values(summarizeTrials(result.trials).strata).every(s => s.status === 'raw_cohorts_ready_for_review')) break;
    }
    assert.ok(negativeTemplate, 'at least one naturally sampled report required for collector negatives');
    result.collectorNegatives = await runCollectorNegativeChecks(context, baseURL, negativeTemplate);
    result.collectorNegatives.push(await runUnauthenticatedCollectorCheck(context.browser(), baseURL, negativeTemplate));
    const terminal = await runPendingPagehideCheck({ browser: context.browser(), storageState, baseURL,
      route: aggregates[0].route, outputDir, attempts: 8 });
    appendTerminalInventory(result, terminal, fixtures.identity, fixtures.forbidden ?? []);
    result.comparison = summarizeTrials(result.trials);
    result.acceptance = 'partial_initial_and_pagehide_incomplete_export_raw_overhead_only';
    return result;
  } catch (error) {
    result.failure = error.message;
    throw error;
  } finally {
    writeFileSync(receiptPath, JSON.stringify(result, null, 2));
  }
}
