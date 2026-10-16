import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { observePhysicalHistoricalRow, holdPostFaultBaseline } from './history-postfault-baseline.mjs';
import { installHistoryRequestOwnership, installObservedHistoryTelemetry, authenticateOwnedContext } from './history-joint-telemetry.mjs';

export const CONTROLLED_CLASS = 'controlled_functional';
export const CONTROLLED_VIEWPORT = Object.freeze({ width: 1360, height: 900 });

export async function settleControlledHistoricalTarget({ read, nextFrame, check }) {
  let physical;
  while (!physical) {
    check();
    physical = await read(null);
    if (!physical) { await nextFrame(); check(); }
  }
  const baseline = await holdPostFaultBaseline(physical, async expected => {
    check(); return read(expected);
  }, async () => { check(); await nextFrame(); check(); });
  check();
  return { ...baseline, observation_scope: 'controlled original-page strict historical target held four animation frames' };
}

const rows = ['history', 'pagehide', 'cancellation'];

export function assertControlledInventory(result) {
  assert.equal(result.evidence_class, CONTROLLED_CLASS);
  assert.equal(result.overhead_clearance, false);
  assert.equal(result.full_acceptance, false);
  assert.equal(result.resume.status, 'unsupported');
  assert.equal(result.trials.length, 6);
  const seen = new Set();
  for (const route of ['ordinary', 'global']) for (const kind of rows) {
    const entries = result.trials.filter(t => t.route === route && t.kind === kind);
    assert.equal(entries.length, 1, 'controlled scenario must occur exactly once');
    const trial = entries[0];
    assert.equal(trial.status, 'passed_assertions');
    assert.equal(trial.reports.length, 1, 'one actual report per scenario');
    assert.equal(trial.journeys.length, 1);
    const report = trial.reports[0], journey = trial.journeys[0];
    assert.equal(report.open_id, journey.open_id);
    assert.equal(report.route, route, 'report route must match the actual scenario');
    assert.equal(trial.admission.route, route);
    assert.ok(!seen.has(report.open_id), 'controlled report UUID must be unique'); seen.add(report.open_id);
    assert.equal(report.sample_rate, 1, 'controlled selection must be truthful');
    assert.equal(report.outcome, kind === 'history' ? 'ready' : kind === 'pagehide' ? 'incomplete' : 'canceled');
    assert.equal(trial.admission.sampled, true);
    assert.equal(trial.admission.open_id, report.open_id);
    if (kind !== 'history') assert.ok(trial.primaryRequestOpenIds.length > 0, 'pending transport must be independently observed');
    if (kind !== 'pagehide') assert.deepEqual(trial.statuses, [204], 'non-departing collector response must be observed');
    else assert.equal(trial.delivery_proof, 'provisional_until_real_collector_and_export_join');
    assert.deepEqual(trial.errors, []);
  }
  assert.equal(result.journeys.length, 6);
  assert.deepEqual(new Set(result.journeys.map(j => j.open_id)), seen);
  return result;
}

export function assertOwnedReport(report, inventory) {
  assert.equal(inventory.ownershipFailure, null);
  assert.deepEqual(inventory.errors, []);
  const ownedRequests = new Set(inventory.requestOwners.filter(p => p.journey_id === report.open_id).map(p => p.request_id));
  const ownedStreams = new Set(inventory.streamOwners.filter(p => p.journey_id === report.open_id).map(p => p.stream_id));
  const finite = inventory.requestRecords.filter(r => r.kind === 'request' && r.method === 'GET' && ownedRequests.has(r.open_id));
  assert.ok(finite.length > 0, 'same-journey finite request observation required');
  for (const correlation of report.correlations) {
    if (correlation.kind === 'request') assert.ok(finite.some(r => r.open_id === correlation.id), 'request must have producer AND network ownership');
    else if (correlation.kind === 'sse') assert.ok(ownedStreams.has(correlation.id) && inventory.requestRecords.some(r => r.kind === 'sse' && r.open_id === correlation.id), 'SSE must have producer AND network ownership');
    else if (correlation.kind === 'trace') assert.ok(finite.some(r => r.trace_id === correlation.id && report.correlations.some(c => c.kind === 'request' && c.id === r.open_id)), 'trace must attach to owned reported finite request');
    else assert.fail('unknown controlled correlation kind');
  }
  assert.ok(report.correlations.some(c => c.kind === 'request' && finite.some(r => r.open_id === c.id)));
}

function controlledObserver(page, network, module, identity, fixture, ownership) {
  const pending = new Set(), reportTransports = new Set(), changed = new Set();
  let failure = null;
  const signal = () => { for (const notify of changed) notify(); };
  const request = req => {
    try {
      const url = new URL(req.url());
      if (url.pathname === '/api/telemetry/conversation-view' && req.method() === 'POST') reportTransports.add(req);
      if (url.pathname === fixture.endpoint && req.method() === 'GET' && url.searchParams.has('open_id')) pending.add(req);
    } catch { failure = 'invalid_controlled_request'; }
    signal();
  };
  const finished = req => { pending.delete(req); reportTransports.delete(req); signal(); };
  const response = () => signal();
  page.on('request', request); page.on('requestfinished', finished); page.on('requestfailed', finished); page.on('response', response);
  const getReports = () => network.snapshot().reports.map(raw => {
    const report = module.captureReport(JSON.stringify(raw), []);
    assert.equal(report.product_conversation_id, fixture.product_conversation_id);
    assert.equal(report.frontend_source, identity.frontendSource);
    return report;
  });
  return {
    getReports,
    failureCounts: () => ({ reports: Math.min(8, network.snapshot().reports.length),
      responses: network.snapshot().statuses.map(s => s.status).slice(0, 8),
      admissions: Math.min(8, ownership.diagnostics().filter(e => e.kind === 'admission').length),
      requests: Math.min(512, network.snapshot().requestRecords.length),
      pending: Math.min(512, pending.size), errors: Math.min(32, network.snapshot().errors.length) }),
    reportTransportSettled: () => reportTransports.size === 0,
    getStatuses: () => network.snapshot().statuses.map(s => s.status),
    getErrors: () => [...network.snapshot().errors, ...(network.snapshot().ownershipFailure ? [network.snapshot().ownershipFailure] : []), ...(failure ? [failure] : [])],
    getAdmissionCount: () => ownership.diagnostics().filter(event => event.kind === 'admission').length,
    getPendingPrimaryRequests: () => [...pending].map(req => ({ open_id: new URL(req.url()).searchParams.get('open_id') })),
    wait(predicate, timeout) {
      return new Promise((resolve, reject) => {
        let timer;
        const clean = () => { changed.delete(test); clearTimeout(timer); };
        const test = () => { try { if (predicate()) { clean(); resolve(); } } catch { clean(); reject(new Error('controlled observation invalid')); } };
        changed.add(test); timer = setTimeout(() => { clean(); reject(new Error('controlled observation deadline')); }, timeout); test();
      });
    },
    dispose() { page.off('request', request); page.off('requestfinished', finished); page.off('requestfailed', finished); page.off('response', response); },
  };
}

function deadlineClock(limitMs, UNIT) {
  const now = UNIT?.now ?? (() => performance.now()), start = now();
  return { remaining() { const left = limitMs - (now() - start); assert.ok(left > 0, 'controlled absolute deadline exhausted'); return left; } };
}

async function stage(operation, clock, ceiling, UNIT, abort) {
  const timers = UNIT?.timers ?? { setTimeout, clearTimeout };
  let timer;
  try {
    return await Promise.race([operation(), new Promise((_, reject) => {
      timer = timers.setTimeout(() => { abort?.(); reject(new Error('controlled stage deadline exhausted')); }, Math.min(clock.remaining(), ceiling));
    })]);
  } finally { timers.clearTimeout(timer); }
}

export async function runControlledFunctional({ browser, baseURL, fixtures, config, module, historyTargets, deadlineMs = 195000 }, UNIT) {
  assert.equal(config.evidence_class, CONTROLLED_CLASS);
  assert.equal(config.unitOnly, UNIT !== undefined, 'UNIT controlled driver cannot become runtime proof');
  if (UNIT) assert.equal(UNIT.label, 'UNIT_ONLY_NOT_RUNTIME_PROOF');
  assert.ok(deadlineMs > 0 && deadlineMs <= 195000);
  assert.equal(fixtures.aggregates.length, 2);
  const clock = deadlineClock(deadlineMs, UNIT), trials = [], contexts = new Set();
  let canceled = false, primaryFailure;
  const createContext = async () => {
    assert.ok(!canceled); clock.remaining();
    const context = await browser.newContext({ viewport: { ...CONTROLLED_VIEWPORT } });
    contexts.add(context);
    if (canceled) { await context.close(); contexts.delete(context); throw new Error('controlled late context creation canceled'); }
    return context;
  };
  const close = async context => {
    await context.close(); contexts.delete(context);
  };
  const abort = () => { canceled = true; };
  const check = () => { assert.ok(!canceled, 'controlled operation canceled'); return clock.remaining(); };
  const bounded = (operation, ceiling) => stage(operation, clock, ceiling, UNIT, abort);
  try {
    for (const kind of rows) for (const fixture of fixtures.aggregates) {
      clock.remaining();
      assert.ok(!canceled);
      const route = fixture.kind, context = await bounded(createContext, 10000);
      let page, observer, scenarioFailure, collectedTrial, physicalDiagnostics = null, phase = 'authenticate';
      try {
        const trial = await bounded(async () => {
          await (UNIT?.authenticate ?? authenticateOwnedContext)(context, baseURL);
          assert.ok(!canceled); clock.remaining();
          phase = 'ownership_install';
          const ownership = await installHistoryRequestOwnership(context, config, UNIT?.ownership);
          assert.ok(!canceled); clock.remaining();
          phase = 'page_create';
          page = await context.newPage();
          if (canceled) { await page.close(); throw new Error('controlled late page creation canceled'); }
          const network = installObservedHistoryTelemetry(page, baseURL, ownership);
          observer = controlledObserver(page, network, module, fixtures.identity, fixture, ownership);
          const reports = [], statuses = [];
          // Observer helpers validate network bodies before they enter any retained receipt.
          const collect = async () => {
            const actual = observer.getReports();
            assert.equal(actual.length, 1, 'controlled scenario report count');
            reports.push(actual[0]); statuses.push(...observer.getStatuses());
            const admissions = ownership.diagnostics().filter(e => e.kind === 'admission');
            assert.equal(admissions.length, 1, 'one producer admission per controlled document');
            const admission = admissions[0];
            assert.equal(admission.route, route);
            assert.equal(reports[0].route, route, 'actual report route must match producer and scenario');
            assert.equal(admission.sampled, true);
            assert.equal(admission.open_id, reports[0].open_id);
            const journey = module.journeyExpectation(reports[0], fixtures.identity);
            return { route, kind, status: 'passed_assertions', admission, reports, statuses,
              journeys: [journey], errors: observer.getErrors(), ownership: network.snapshot(), primaryRequestOpenIds: network.snapshot().requestRecords.filter(r => r.kind === 'request').map(r => r.open_id) };
          };
          if (kind === 'history') {
            const target = historyTargets.find(t => t.kind === route);
            assert.ok(target && target.route && target.anchor && target.composerOwner);
            phase = 'history_navigate';
            await page.goto(baseURL + target.route, { waitUntil: 'domcontentloaded', timeout: Math.min(15000, clock.remaining()) });
            phase = 'history_report_wait';
            await observer.wait(() => observer.getReports().length === 1 && observer.getStatuses().length === 1, Math.min(12000, check()));
            phase = 'physical_history_settlement';
            const physical = await bounded(() => settleControlledHistoricalTarget({
              check,
              nextFrame: () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve))),
              read: async expected => {
                check();
                const probe = await page.evaluate(observePhysicalHistoricalRow, { known: [target.anchor], expected, diagnostics: true });
                check();
                const observed = probe.observation;
                physicalDiagnostics = { predicates: probe.predicates, observation_present: observed !== null,
                  fixture_identity_matches: observed ? observed.persistedMessageID === target.anchor.persistedMessageID && observed.sourceRow === target.anchor.sourceRow : null,
                  sourcepin_matches: observed ? observed.sourcepin === target.anchor.sourceRow : null };
                if (!observed) { phase = 'physical_history_present'; return null; }
                phase = 'physical_history_fixture_identity';
                assert.equal(observed.persistedMessageID, target.anchor.persistedMessageID);
                assert.equal(observed.sourceRow, target.anchor.sourceRow);
                phase = 'physical_history_sourcepin';
                assert.equal(observed.sourcepin, target.anchor.sourceRow);
                assert.equal(page.url(), new URL(target.route, baseURL).href, 'physical settlement retains original route');
                phase = 'physical_history_stability';
                return observed;
              },
            }), 5000);
            phase = 'composer_owner';
            assert.equal(await page.locator('textarea[placeholder="Message..."]').count(), 1, 'exactly one current composer');
            assert.equal(await page.locator('textarea[placeholder="Message..."]').isEditable(), true, 'current composer is writable');
            assert.equal(page.url(), new URL(target.route, baseURL).href, 'original retained route cannot be replaced');
            phase = 'report_collect';
            const trial = await collect();
            assertOwnedReport(trial.reports[0], network.snapshot());
            phase = 'additional_history_owner';
            const historicalRequests = network.snapshot().requestRecords.filter(r => r.kind === 'request'
              && new URL(r.url).pathname === fixture.endpoint && new URL(r.url).searchParams.has('before'));
            assert.ok(historicalRequests.length > 0, 'actual additional history GET required before historical ready');
            for (const request of historicalRequests) assert.ok(trial.reports[0].correlations.some(c => c.kind === 'request' && c.id === request.open_id), 'additional history request must remain in its original journey');
            trial.additional_history = { report_count: 1, request_count: historicalRequests.length, original_owner: trial.reports[0].open_id };
            phase = 'stream_owner';
            assert.ok(network.snapshot().requestRecords.some(r => r.kind === 'sse' && new URL(r.url).pathname === `/api/conversations/${target.composerOwner}/stream`), 'stream must belong to independently selected writable owner');
            trial.physical_history = { original_page: true, anchor: physical, composer_owner: target.composerOwner,
              viewport: { ...CONTROLLED_VIEWPORT }, stability_frames: physical.stability_frames.length };
            return trial;
          }
          phase = 'pending_navigation';
          const cdp = await context.newCDPSession(page);
          check();
          await cdp.send('Network.enable');
          await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: 400, downloadThroughput: -1, uploadThroughput: -1 });
          check();
          await page.goto(baseURL + fixture.route, { waitUntil: 'commit', timeout: Math.min(15000, clock.remaining()) });
          await observer.wait(() => observer.getPendingPrimaryRequests().length > 0, Math.min(10000, check()));
          const pending = observer.getPendingPrimaryRequests();
          const before = await page.evaluate(() => performance.timeOrigin);
          phase = kind === 'pagehide' ? 'pagehide_departure' : 'sidebar_cancellation';
          if (kind === 'pagehide') {
            assert.ok(observer.getPendingPrimaryRequests().length > 0, 'request remains pending at actual departure');
            check();
            await page.goto('about:blank', { waitUntil: 'commit', timeout: Math.min(10000, clock.remaining()) });
          } else {
            const anchor = page.locator(module.sidebarHomeSelector);
            await anchor.waitFor({ state: 'visible', timeout: Math.min(10000, check()) });
            assert.equal(await anchor.count(), 1, 'unique actual Sidebar home control');
            assert.equal(await anchor.isEnabled(), true);
            assert.equal(await anchor.evaluate(element => element.tagName), 'BUTTON');
            assert.ok(observer.getPendingPrimaryRequests().length > 0, 'request remains pending at actual navigation');
            await anchor.click({ timeout: Math.min(10000, check()) });
            await page.waitForURL(new URL('/', baseURL).href, { timeout: Math.min(10000, check()) });
            assert.equal(await page.evaluate(() => performance.timeOrigin), before, 'cancellation must be same-document');
          }
          phase = 'departure_report_wait';
          await observer.wait(() => observer.getReports().length === 1, Math.min(5000, check()));
          if (kind !== 'pagehide') await observer.wait(() => observer.getStatuses().length === 1, Math.min(5000, check()));
          check();
          phase = 'report_collect';
          const trial = await collect();
          assert.ok(pending.some(p => trial.reports[0].correlations.some(c => c.kind === 'request' && c.id === p.open_id)), 'report must own the actual pending request');
          assertOwnedReport(trial.reports[0], network.snapshot());
          if (kind === 'pagehide') trial.delivery_proof = 'provisional_until_real_collector_and_export_join';
          return trial;
        }, kind === 'history' ? 30000 : 20000);
        collectedTrial = trial;
        if (kind === 'pagehide') {
          phase = 'departure_transport_settle';
          await bounded(() => observer.wait(() => observer.reportTransportSettled(), Math.min(5000, check())), 5000);
          trial.transport_settled_before_context_close = true;
        }
        phase = 'context_close';
        // Keep observers until context closure so late/unexpected reports cannot disappear.
        await bounded(() => close(context), 5000);
        phase = 'final_inventory';
        assert.equal(observer.getReports().length, 1, 'late report cannot change controlled inventory');
        assert.equal(observer.getAdmissionCount(), 1, 'late or successor admission cannot disappear');
        assert.deepEqual(observer.getErrors(), []);
        trials.push(trial);
      } catch (error) {
        canceled = true;
        const failure = UNIT ? error : new Error(`controlled ${route}/${kind} failed: ${error instanceof assert.AssertionError ? 'assertion' : 'operation_or_deadline'}`);
        failure.controlledReceipt = { evidence_class: CONTROLLED_CLASS, status: 'failed', trials,
          failure_location: { scope: 'scenario', route, kind, phase,
            error_kind: error instanceof assert.AssertionError ? 'assertion' : 'operation',
            counts: observer?.failureCounts() ?? null, physical: physicalDiagnostics },
          failed_trial: collectedTrial ? { ...collectedTrial, reports: observer.getReports(), status: 'failed_assertions' } : null,
          overhead_clearance: false, full_acceptance: false, failure: 'controlled_scenario_failed' };
        scenarioFailure = failure;
        throw failure;
      } finally {
        observer?.dispose();
        if (contexts.has(context)) {
          try { await stage(() => close(context), { remaining: () => 5000 }, 5000, UNIT, abort); }
          catch {
            if (!scenarioFailure) throw new Error('controlled context cleanup failed');
            scenarioFailure.controlledReceipt.cleanup_failures = ['controlled_context_close_failed'];
          }
        }
      }
    }
    return assertControlledInventory({ evidence_class: CONTROLLED_CLASS, status: 'passed_assertions',
      acceptance: 'partial_controlled_functional_collector_export_only', comparison: { overhead_clearance: false, reason: 'controlled_admission_not_stock_overhead' },
      forbidden: [],
      overhead_clearance: false, full_acceptance: false, resume: { status: 'unsupported', reason: 'no_qualified_native_resume_lane' },
      runId: config.runId, serviceName: config.serviceName, identity: fixtures.identity,
      trials, journeys: trials.flatMap(t => t.journeys) });
  } catch (error) {
    primaryFailure = error;
    throw error;
  } finally {
    canceled = true;
    const cleanup = await Promise.allSettled([...contexts].map(context => stage(() => close(context), { remaining: () => 5000 }, 5000, UNIT, abort)));
    if (cleanup.some(outcome => outcome.status === 'rejected')) {
      if (!primaryFailure) throw new Error('controlled context cleanup failed');
      primaryFailure.controlledReceipt ??= { evidence_class: CONTROLLED_CLASS, status: 'failed', trials,
        overhead_clearance: false, full_acceptance: false, failure: 'controlled_driver_failed' };
      primaryFailure.controlledReceipt.cleanup_failures = ['controlled_context_close_failed'];
    }
  }
}
