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
  const deadline = Date.now() + 240_000;
  try {
    for (let index = 0; index < 64; index++) {
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
          result.journeys.push({ open_id: report.open_id, correlations: report.correlations,
            expected: { 'server.backend_source': fixtures.identity.appSource, 'server.instance_id': fixtures.identity.serverInstanceId,
              'browser.frontend_source': fixtures.identity.frontendSource, 'browser.outcome': report.outcome,
              'browser.route': aggregate.kind, 'browser.trigger': report.trigger,
              'browser.presentation_evidence': report.presentation_evidence,
              'browser.total_ms': report.total_ms,
              'browser.meaningful_transcript_ms': report.marks.meaningful_transcript,
              'browser.breadcrumb_presented_ms': report.marks.breadcrumb_presented,
              'browser.breadcrumb_enabled_ms': report.marks.breadcrumb_enabled,
              'browser.anchor_settled_ms': report.marks.anchor_settled,
              'browser.breadcrumb_ready_minus_meaningful_ms': report.breadcrumb_ready_minus_meaningful_ms } });
        }
        if (!trial.sampled) {
          assert.equal(trial.posts, 0, 'unsampled cohort must not contain report POST');
          assert.deepEqual(reports, []);
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
    result.comparison = summarizeTrials(result.trials);
    result.acceptance = 'partial_initial_open_retrieval_and_raw_overhead_only';
    return result;
  } catch (error) {
    result.failure = error.message;
    throw error;
  } finally {
    writeFileSync(receiptPath, JSON.stringify(result, null, 2));
  }
}
