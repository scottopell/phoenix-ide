import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, readFileSync, symlinkSync, realpathSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { EventEmitter } from 'node:events';
import { APP_PIN, TELEMETRY_MODULE_SHA256, TELEMETRY_DIAGNOSTICS_SHA256, TELEMETRY_DIAGNOSTICS_RELATIVE_MODULE, UNIT_ONLY,
  prepareJointTelemetry, validateJointFixtures, validateTelemetryResult, runJointTelemetryStage, authenticateOwnedContext,
  installObservedHistoryTelemetry, installHistoryRequestOwnership, installHistoryPaginationDiagnostic, appendObservedHistory } from './history-joint-telemetry.mjs';

// All positive fake browser/module/fixture values in this file are UNIT ONLY, not runtime proof.
function input() {
  return { runId: 'unit-run', serviceName: 'phoenix-view-qualification-unit-run', fixtures: {
    identity: { appSource: APP_PIN, frontendSource: APP_PIN, serverInstanceId: '12345678-1234-4234-8234-123456789abc' },
    aggregates: [
      { kind: 'ordinary', product_conversation_id: 'ordinary-product', route: '/c/ordinary-product', endpoint: '/api/product-conversations/ordinary-product', members: ['ordinary-root', 'ordinary-latest'], latest_transcript_row_id: 'ordinary-latest' },
      { kind: 'global', product_conversation_id: 'global-product', route: '/global/global-latest', endpoint: '/api/global/coordinator/history/global-root', members: ['global-root', 'global-latest'], latest_transcript_row_id: 'global-latest' },
    ], lifecycle: { route: '/c/ordinary-product', productConversationId: 'ordinary-product', spaControl: { kind: 'sidebar_home' } }, forbidden: [],
  } };
}
function stock() {
  const report = { open_id: 'unit-ready', outcome: 'ready', anchor_result: 'tail_pinned', marks: {
    meaningful_transcript: 1, breadcrumb_presented: 2, breadcrumb_enabled: 3, anchor_settled: 4 } };
  return { acceptance: 'partial_initial_terminal_and_lifecycle_export_raw_overhead_only', comparison: { overhead_clearance: false },
    trials: [{ reports: [report] }], journeys: [{ open_id: report.open_id, expected: {
      'browser.outcome': report.outcome, ...Object.fromEntries(Object.entries(report.marks).map(([k, v]) => [`browser.${k}_ms`, v])) } }] };
}
function unitFiles() {
  const root = realpathSync(mkdtempSync(resolve(tmpdir(), 'joint-unit-')));
  mkdirSync(resolve(root, 'ui/scripts'), { recursive: true });
  const module = resolve(root, 'ui/scripts/qualify-view-telemetry.mjs'), fixture = resolve(root, 'fixture.json');
  const diagnostics = resolve(root, TELEMETRY_DIAGNOSTICS_RELATIVE_MODULE);
  writeFileSync(module, 'UNIT_ONLY_not_the_donor_module'); writeFileSync(diagnostics, 'UNIT_ONLY_not_the_diagnostics_helper'); writeFileSync(fixture, JSON.stringify(input()));
  const env = { HISTORY_TELEMETRY_JOINT: '1', HISTORY_TELEMETRY_FIXTURE: fixture,
    HISTORY_TELEMETRY_MODULE: module, HISTORY_TELEMETRY_MODULE_SHA256: TELEMETRY_MODULE_SHA256 };
  const UNIT = { label: UNIT_ONLY, digest: path => path === diagnostics ? TELEMETRY_DIAGNOSTICS_SHA256 : TELEMETRY_MODULE_SHA256 };
  return { root, module, diagnostics, fixture, env, UNIT, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

test('owned browser login mints session before use, disposes responses, and never returns password (UNIT ONLY)', async () => {
  const f = unitFiles(); try {
    const path = resolve(f.root, 'qualification-password'), password = '12345678-1234-4234-8234-123456789abc'; writeFileSync(path, password, { mode: 0o600 });
    const calls = [], disposed = [];
    const context = { request: {
      post: async (url, options) => { calls.push(['login', url, options.data.password]); return { status: () => 200, dispose: async () => disposed.push('login') }; },
      get: async url => { calls.push(['status', url]); return { status: () => 200, json: async () => ({ auth_required: true, authenticated: true }), dispose: async () => disposed.push('status') }; },
    }, cookies: async () => [{ name: 'phoenix-auth', value: 'opaque_session' }] };
    const env = { HISTORY_AUTH_HOME: f.root, HISTORY_AUTH_CREDENTIAL_FILE: path };
    const cookie = await authenticateOwnedContext(context, 'http://127.0.0.1:12345', env);
    assert.equal(cookie, 'phoenix-auth=opaque_session'); assert.ok(!cookie.includes(password));
    assert.deepEqual(disposed, ['login', 'status']); assert.equal(calls[0][2], password);
    assert.deepEqual(calls.map(c => c[0]), ['login', 'status']);
    chmodSync(path, 0o644);
    await assert.rejects(authenticateOwnedContext(context, 'http://127.0.0.1:12345', env), /credential file ownership/);
    chmodSync(path, 0o600); rmSync(path); symlinkSync(f.fixture, path);
    await assert.rejects(authenticateOwnedContext(context, 'http://127.0.0.1:12345', env), /credential file ownership/);
    assert.equal(calls.length, 2);
  } finally { f.cleanup(); }
});

test('browser login rejects auth-disabled/wrong booleans/private JSON and sanitizes login transport (UNIT ONLY)', async () => {
  const f = unitFiles(); try {
    const path = resolve(f.root, 'qualification-password'), password = '22345678-1234-4234-8234-123456789abc'; writeFileSync(path, password, { mode: 0o600 });
    const env = { HISTORY_AUTH_HOME: f.root, HISTORY_AUTH_CREDENTIAL_FILE: path };
    for (const value of [{ auth_required: false, authenticated: true }, { auth_required: 1, authenticated: true }, { auth_required: true, authenticated: 1 }, 'invalid']) {
      let disposed = 0;
      const context = { request: {
        post: async () => ({ status: () => 200, dispose: async () => disposed++ }),
        get: async () => ({ status: () => 200, json: async () => { if (value === 'invalid') throw new Error(password); return value; }, dispose: async () => disposed++ }),
      } };
      await assert.rejects(authenticateOwnedContext(context, 'http://127.0.0.1:12345', env), error => !String(error).includes(password));
      assert.equal(disposed, 2);
    }
    await assert.rejects(authenticateOwnedContext({ request: { post: async () => { throw new Error(password); } } }, 'http://127.0.0.1:12345', env), error => !String(error).includes(password));
    const privateSession = 'SYNTHETIC_SESSION_SENTINEL';
    await assert.rejects(authenticateOwnedContext({ request: {
      post: async () => ({ status: () => 200, dispose: async () => {} }),
      get: async () => { throw new Error(`apiRequestContext.get: timeout\nCall log:\n - cookie: phoenix-auth=${privateSession}`); },
    } }, 'http://127.0.0.1:12345', env), error => error.message === 'owned browser authentication status transport failed' && !String(error.stack).includes(privateSession));
    await assert.rejects(authenticateOwnedContext({ request: {
      post: async () => ({ status: () => 200, dispose: async () => {} }),
      get: async () => ({ status: () => 401, dispose: async () => { throw new Error(privateSession); } }),
    } }, 'http://127.0.0.1:12345', env), error => /owned browser authentication status/.test(error.message)
      && error.owned_auth_cleanup_failure === 'owned authentication response disposal failed'
      && !String(error.stack).includes(privateSession));
  } finally { f.cleanup(); }
});

test('disabled stage does not admit imports or contexts (UNIT ONLY)', async () => {
  assert.equal(prepareJointTelemetry({ env: {} }), null);
  assert.equal(await runJointTelemetryStage({ config: null, browser: null }), null);
});
test('UNIT ONLY complete fixture and preparation carry no runtime credit', () => {
  assert.equal(validateJointFixtures(input()).runId, 'unit-run');
  const f = unitFiles(); try { assert.equal(prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT }).unitOnly, true); } finally { f.cleanup(); }
});
for (const key of ['HISTORY_TELEMETRY_JOINT', 'HISTORY_TELEMETRY_FIXTURE', 'HISTORY_TELEMETRY_MODULE', 'HISTORY_TELEMETRY_MODULE_SHA256'])
  test(`partial opt-in rejects missing ${key} (UNIT ONLY)`, () => {
    const f = unitFiles(); try { delete f.env[key]; assert.throws(() => prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT })); } finally { f.cleanup(); }
  });
for (const [name, mutate] of Object.entries({
  mode: f => { f.env.HISTORY_TELEMETRY_JOINT = 'true'; },
  foreign_module: f => { f.env.HISTORY_TELEMETRY_MODULE = resolve(f.root, 'foreign.mjs'); },
  expected_hash: f => { f.env.HISTORY_TELEMETRY_MODULE_SHA256 = '0'.repeat(64); },
  actual_hash: f => { f.UNIT.digest = () => '0'.repeat(64); },
  symbolic_module: f => { rmSync(f.module); symlinkSync(f.fixture, f.module); },
  missing_diagnostics: f => { rmSync(f.diagnostics); },
  symbolic_diagnostics: f => { rmSync(f.diagnostics); symlinkSync(f.fixture, f.diagnostics); },
  changed_diagnostics: f => { f.UNIT.digest = path => path === f.diagnostics ? '0'.repeat(64) : TELEMETRY_MODULE_SHA256; },
  app_pin: f => { const i = input(); i.fixtures.identity.appSource = '0'.repeat(40); writeFileSync(f.fixture, JSON.stringify(i)); },
})) test(`pre-import guard rejects ${name} (UNIT ONLY)`, () => {
  const f = unitFiles(); try { mutate(f); assert.throws(() => prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT })); } finally { f.cleanup(); }
});
for (const [name, mutate] of Object.entries({
  global_product_equals_root: i => { i.fixtures.aggregates[1].product_conversation_id = 'global-root'; },
  global_foreign_route: i => { i.fixtures.aggregates[1].route = '/global/foreign'; },
  foreign_origin: i => { i.fixtures.aggregates[0].route = 'http://evil.test/c/ordinary-product'; },
  lifecycle_foreign_product: i => { i.fixtures.lifecycle.productConversationId = 'foreign'; },
  lifecycle_generic_link: i => { i.fixtures.lifecycle.spaControl = { kind: 'anchor', selector: 'a' }; },
  repeated_members: i => { i.fixtures.aggregates[1].members[0] = 'global-latest'; },
})) test(`actual-fixture shape rejects ${name} (UNIT ONLY)`, () => { const i = input(); mutate(i); assert.throws(() => validateJointFixtures(i)); });
for (const [name, mutate] of Object.entries({
  bad_anchor: r => { r.trials[0].reports[0].anchor_result = 'target_missing'; },
  absent_anchor: r => { r.trials[0].reports[0].anchor_result = null; },
  missing_mark: r => { delete r.trials[0].reports[0].marks.anchor_settled; },
  stale_export_mark: r => { r.journeys[0].expected['browser.anchor_settled_ms'] = 99; },
  invented_clearance: r => { r.comparison.overhead_clearance = true; },
  invented_acceptance: r => { r.acceptance = 'qualified'; },
  missing_export: r => { r.journeys = []; },
})) test(`current owning-report contract rejects ${name} (UNIT ONLY)`, () => { const r = stock(); mutate(r); assert.throws(() => validateTelemetryResult(r)); });
test('UNIT ONLY stage owns a fresh context and closes it without promoting partial scope', async () => {
  const f = unitFiles(); const calls = []; try {
    const config = prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT });
    const context = { newPage: async () => ({ unit: true }), close: async () => calls.push('close') };
    const result = await runJointTelemetryStage({ config, browser: { newContext: async () => { calls.push('fresh'); return context; } },
      baseURL: 'http://127.0.0.1:12345', outputDir: f.root, harnessPath: import.meta.filename,
      historyPhysical: { count: 1, visible: true, persistedMessageID: 'unit-exact' }, historyGlobalOwnedGetCount: 0,
      UNIT: { label: UNIT_ONLY, importModule: async () => ({ runTelemetryChecks: async () => stock() }) } });
    assert.deepEqual(calls, ['fresh', 'close']); assert.equal(result.receipt.classification.overhead_clearance, false);
    assert.match(result.receipt.scope.telemetry, /not telemetry lifecycle physical-target proof/);
  } finally { f.cleanup(); }
});
for (const phase of ['runtime_import', 'export_admission', 'context_acquisition'])
  test(`UNIT ONLY ${phase} failure retains before/after manifest without claiming checks executed`, async () => {
    const f = unitFiles(); try {
      const config = prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT });
      const primary = new Error('admission failure'); let checks = 0;
      const load = async () => {
        if (phase === 'runtime_import') throw primary;
        if (phase === 'export_admission') return {};
        return { runTelemetryChecks: async () => { checks++; } };
      };
      const browser = { newContext: async () => { throw primary; } };
      await assert.rejects(runJointTelemetryStage({ config, browser,
        baseURL: 'http://127.0.0.1:12345', outputDir: f.root, harnessPath: import.meta.filename,
        historyPhysical: { count: 1, visible: true, persistedMessageID: 'unit-exact' },
        UNIT: { label: UNIT_ONLY, importModule: load } }),
      error => phase === 'export_admission' ? /export required/.test(error.message) : error === primary);
      const manifest = JSON.parse(readFileSync(resolve(f.root, 'joint-telemetry-dependency-manifest.json'), 'utf8'));
      assert.equal(manifest.failure_phase, phase); assert.equal(manifest.context_acquired, false);
      assert.deepEqual(manifest.before, manifest.after); assert.equal(checks, 0);
      assert.equal(manifest.before[f.diagnostics], TELEMETRY_DIAGNOSTICS_SHA256);
    } finally { f.cleanup(); }
  });
test('UNIT ONLY donor primary survives context cleanup failure and retains dependency manifests', async () => {
  const f = unitFiles(); try {
    const config = prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT });
    const primary = new Error('donor primary');
    const context = { newPage: async () => ({}), close: async () => { throw new Error('context cleanup'); } };
    await assert.rejects(runJointTelemetryStage({ config, browser: { newContext: async () => context },
      baseURL: 'http://127.0.0.1:12345', outputDir: f.root, harnessPath: import.meta.filename,
      historyPhysical: { count: 1, visible: true, persistedMessageID: 'unit-exact' },
      UNIT: { label: UNIT_ONLY, importModule: async () => ({ runTelemetryChecks: async () => { throw primary; } }) } }),
    error => error === primary && error.joint_cleanup_failures[0] === 'context cleanup');
    const manifest = JSON.parse(readFileSync(resolve(f.root, 'joint-telemetry-dependency-manifest.json'), 'utf8'));
    assert.equal(manifest.primary_failure, 'donor primary');
    assert.deepEqual(manifest.cleanup_failures, ['context cleanup']);
    assert.deepEqual(manifest.before, manifest.after);
    assert.equal(manifest.before[f.diagnostics], TELEMETRY_DIAGNOSTICS_SHA256);
  } finally { f.cleanup(); }
});
test('UNIT ONLY module failure propagates and still closes fresh context', async () => {
  const f = unitFiles(); let closed = false; try {
    const config = prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT });
    await assert.rejects(runJointTelemetryStage({ config, browser: { newContext: async () => ({ newPage: async () => ({}), close: async () => { closed = true; } }) },
      outputDir: f.root, historyPhysical: { count: 1, visible: true, persistedMessageID: 'unit-exact' },
      UNIT: { label: UNIT_ONLY, importModule: async () => ({ runTelemetryChecks: async () => { throw Error('UNIT module failure'); } }) } }), /UNIT module failure/);
    assert.equal(closed, true);
  } finally { f.cleanup(); }
});
test('source stage remains after sealed physical history and before server stop; original history deadline unchanged', () => {
  const source = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const stage = source.indexOf('const jointStage = await runJointTelemetryStage');
  const sealed = source.lastIndexOf('await recorder.sealAndDrain(); recorder.assertQualified();', stage);
  assert.ok(sealed > 0 && sealed < stage);
  assert.ok(source.indexOf("command({ command: 'stop' })", stage) > stage);
  assert.match(source, /let hardStop = setTimeout\(wallFailure, 170000\)/);
  assert.ok(source.indexOf('480000 - (performance.now() - deadlineStarted)') > sealed);
  assert.match(source.slice(stage), /harnessPath: fileURLToPath\(import.meta.url\)/);
});

function historyInventory() {
  const report = { ...structuredClone(stock().trials[0].reports[0]), open_id: 'unit-history', correlations: [{ kind: 'request', id: 'unit-history' }] };
  return { scope: 'UNIT_ONLY_original_history', errors: [], ownershipFailure: null,
    requestOwners: [{ journey_id: report.open_id, request_id: report.open_id }],
    streamOwners: [],
    reports: [report], statuses: [{ open_id: report.open_id, status: 204 }],
    requestRecords: [{ kind: 'request', open_id: report.open_id, trace_id: null }] };
}
const unitModule = {
  captureReport: raw => JSON.parse(raw),
  assertRequestCorrelation: (ids, report) => { for (const id of ids) assert.ok(report.correlations.some(c => c.kind === 'request' && c.id === id)); },
  journeyExpectation: report => ({ open_id: report.open_id, expected: { 'browser.outcome': report.outcome,
    ...Object.fromEntries(Object.entries(report.marks).map(([k, v]) => [`browser.${k}_ms`, v])) } }),
};
test('UNIT ONLY independently captured original history views join exact stock inventory without receiver output', () => {
  const result = appendObservedHistory(stock(), historyInventory(), input().fixtures.identity, unitModule);
  assert.equal(result.journeys.length, 2); assert.equal(result.additional_history.reports, 1);
  assert.deepEqual(result.trials.at(-1).reports.map(r => r.open_id), ['unit-history']);
});
test('UNIT ONLY journey UUID differs from independently observed finite request UUID and trace stays attached to that request', () => {
  const inventory = historyInventory();
  inventory.reports[0].correlations = [{ kind: 'request', id: 'distinct-request-id' },
    { kind: 'trace', id: 'a'.repeat(32) }];
  inventory.requestRecords = [{ kind: 'request', open_id: 'distinct-request-id', trace_id: 'a'.repeat(32) }];
  inventory.requestOwners = [{ journey_id: 'unit-history', request_id: 'distinct-request-id' }];
  const result = appendObservedHistory(stock(), inventory, input().fixtures.identity, unitModule);
  assert.equal(result.journeys.at(-1).open_id, 'unit-history');
  assert.equal(result.trials.at(-1).requestRecords[0].open_id, 'distinct-request-id');
  const foreign = structuredClone(inventory);
  foreign.requestRecords = [{ kind: 'request', open_id: 'unit-history', trace_id: 'a'.repeat(32) }];
  assert.throws(() => appendObservedHistory(stock(), foreign, input().fixtures.identity, unitModule),
    /independently observed finite request/);
  const borrowed = structuredClone(inventory);
  borrowed.requestRecords[0].trace_id = null;
  borrowed.requestRecords.push({ kind: 'request', open_id: 'unreported-request', trace_id: 'a'.repeat(32) });
  assert.throws(() => appendObservedHistory(stock(), borrowed, input().fixtures.identity, unitModule),
    /independent request\/response provenance/);
});

for (const [name, mutate] of Object.entries({
  missing_independent_owner: i => { i.requestOwners = []; },
  ownership_capture_failure: i => { i.ownershipFailure = 'invalid_or_overflow_history_request_ownership'; },
  missing_capture_status: i => { i.statuses = []; },
  unexpected_export_without_browser_report: i => { i.reports = []; },
  missing_request: i => { i.requestRecords = []; },
  foreign_request_correlation: i => { i.reports[0].correlations[0].id = 'foreign'; },
  unobserved_trace_correlation: i => { i.reports[0].correlations.push({ kind: 'trace', id: 'f'.repeat(32) }); },
  duplicate_original_report: i => { i.reports.push(structuredClone(i.reports[0])); i.statuses.push({ ...i.statuses[0] }); },
  observer_failed: i => { i.errors.push('UNIT capture failure'); },
  duplicate_stock_identity: i => { i.reports[0].open_id = 'unit-ready'; },
  rejected_collector: i => { i.statuses[0].status = 400; },
})) test(`original history inventory rejects ${name} (UNIT ONLY)`, () => {
  const i = historyInventory(); mutate(i); assert.throws(() => appendObservedHistory(stock(), i, input().fixtures.identity, unitModule));
});
test('UNIT pagination diagnostic distinguishes actual uncorrelated GETs without retaining private values', () => {
  const page = new EventEmitter(), base = 'http://127.0.0.1:12345';
  const diagnostic = installHistoryPaginationDiagnostic(page, base, '/api/private-product');
  const emit = (path, method = 'GET') => {
    const request = { method: () => method, url: () => base + path };
    page.emit('request', request);
    page.emit('response', { request: () => request, status: () => 200 });
  };
  emit('/api/private-product?before=SECRET-cursor');
  emit('/api/other?before=SECRET-cursor');
  emit('/api/private-product?open_id=SECRET-initial');
  emit('/api/telemetry/conversation-view', 'POST');
  emit('/api/private-product?before=SECRET-cursor&open_id=SECRET-request');
  const result = diagnostic.snapshot([{ correlations: [{ kind: 'request', id: 'SECRET-request' }] }]);
  assert.deepEqual(result, { errors: 0, requests: [
    { has_open_id: false, before_report: true, status: 200, in_report: false },
    { has_open_id: true, before_report: false, status: 200, in_report: true },
  ] });
  assert.doesNotMatch(JSON.stringify(result), /SECRET|private-product|cursor|http/);
  for (let n = 0; n < 130; n++) emit('/api/private-product?before=SECRET');
  assert.equal(diagnostic.snapshot().requests.length, 128);
  assert.ok(diagnostic.snapshot().errors > 0);
});

test('UNIT ONLY late report capture retains raw body/response and independent header trace provenance', () => {
  const page = new EventEmitter(), capture = installObservedHistoryTelemetry(page, 'http://127.0.0.1:12345', {
    bindPage: () => {},
    snapshot: () => ({ pairs: historyInventory().requestOwners, failure: null }),
  });
  const request = { method: () => 'GET', url: () => 'http://127.0.0.1:12345/api/product-conversations/unit?open_id=unit-history&before=SECRET-cursor&unknown=SECRET' };
  page.emit('request', request);
  page.emit('response', { request: () => request, status: () => 200, headers: () => ({ traceparent: '00-' + 'a'.repeat(32) + '-' + 'b'.repeat(16) + '-01' }) });
  assert.equal(capture.snapshot().reports.length, 0);
  const report = historyInventory().reports[0], post = { method: () => 'POST', url: () => 'http://127.0.0.1:12345/api/telemetry/conversation-view', postData: () => JSON.stringify(report) };
  page.emit('request', post); page.emit('response', { request: () => post, status: () => 204, headers: () => ({}) });
  const retained = capture.snapshot(); assert.equal(retained.requestRecords[0].trace_id, 'a'.repeat(32));
  assert.equal(retained.requestRecords[0].pathname, '/api/product-conversations/unit');
  assert.equal(retained.requestRecords[0].has_before, true);
  assert.equal(Object.hasOwn(retained.requestRecords[0], 'url'), false);
  assert.doesNotMatch(JSON.stringify(retained), /SECRET|127\.0\.0\.1/);
  assert.deepEqual(retained.reports, [report]); assert.equal(retained.statuses[0].status, 204);
  appendObservedHistory(stock(), retained, input().fixtures.identity, unitModule);
});
test('source observer precedes first history navigation and union follows browser close before server stop', () => {
  const source = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const observer = source.indexOf('historyTelemetry = installObservedHistoryTelemetry(page, ui, requestOwnership)');
  const navigation = source.indexOf('await recorder.navigateAfterRequiredBodies(');
  assert.ok(observer > 0 && navigation > observer);
  assert.ok(source.indexOf('() => page.goto(', navigation) > navigation);
  const closed = source.indexOf('await browser.close(); browser = undefined;');
  const union = source.indexOf('finalizeHistoryTelemetry(inventory)', closed);
  const stopped = source.indexOf("command({ command: 'stop' })", union);
  assert.ok(closed > 0 && closed < union && union < stopped);
});

test('UNIT ONLY history trace belongs to the same report request, never a different observed journey', () => {
  const own = historyInventory();
  own.requestRecords[0].trace_id = 'a'.repeat(32);
  own.reports[0].correlations.push({ kind: 'trace', id: 'a'.repeat(32) });
  own.requestRecords.push({ kind: 'request', open_id: 'unit-other-journey', trace_id: 'b'.repeat(32) });
  appendObservedHistory(stock(), own, input().fixtures.identity, unitModule);
  const borrowed = structuredClone(own);
  borrowed.reports[0].correlations.find(c => c.kind === 'trace').id = 'b'.repeat(32);
  assert.throws(() => appendObservedHistory(stock(), borrowed, input().fixtures.identity, unitModule), /independent request\/response provenance/);
  borrowed.reports[0].correlations.push({ kind: 'request', id: 'unit-other-journey' });
  borrowed.requestOwners.push({ journey_id: 'foreign-journey', request_id: 'unit-other-journey' });
  assert.throws(() => appendObservedHistory(stock(), borrowed, input().fixtures.identity, unitModule), /independently observed journey/);
  borrowed.requestOwners.push({ journey_id: 'unit-history', request_id: 'unit-other-journey' });
  appendObservedHistory(stock(), borrowed, input().fixtures.identity, unitModule);
});


test('UNIT ONLY independently captured producer admission owns distinct transport UUID; malformed and overflow fail closed', async () => {
  const config = { unitOnly: true };
  const UNIT = ownershipUnit();
  const frame = {}, page = { mainFrame: () => frame }, source = { page, frame };
  const journey = '11111111-1111-4111-8111-111111111111', request = '22222222-2222-4222-8222-222222222222';
  let callback, installed = 0;
  const context = { exposeBinding: async (_name, cb) => { callback = cb; }, addInitScript: async () => { installed++; } };
  const owner = await installHistoryRequestOwnership(context, config, UNIT);
  assert.equal(installed, 1);
  owner.bindPage(page);
  callback(source, { version: 1, kind: 'admission', route: 'ordinary', open_id: journey, sampled: true });
  callback(source, { version: 1, kind: 'request', open_id: journey, request_id: request });
  assert.deepEqual(owner.snapshot(), { pairs: [{ journey_id: journey, request_id: request }], streams: [], failure: null });
  callback(source, { version: 1, kind: 'request', open_id: request, request_id: request });
  assert.equal(owner.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  const overflow = await installHistoryRequestOwnership(context, config, UNIT);
  overflow.bindPage(page);
  callback(source, { version: 1, kind: 'admission', route: 'ordinary', open_id: journey, sampled: true });
  for (let n = 0; n < 513; n++) callback(source, { version: 1, kind: 'request', open_id: journey, request_id: request });
  assert.equal(overflow.snapshot().pairs.length, 512);
  assert.equal(overflow.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  const invalid = await installHistoryRequestOwnership(context, config, UNIT);
  invalid.bindPage(page);
  callback(source, { version: 1, kind: 'request', open_id: journey, request_id: request, body: 'private' });
  assert.equal(invalid.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  assert.ok(!JSON.stringify(invalid.snapshot()).includes('private'));
});

test('history diagnostic installation is awaited before page creation/navigation and has a finite deadline', () => {
  const source = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const install = source.indexOf('await installHistoryRequestOwnership(context, jointTelemetry)');
  assert.ok(install > 0 && install < source.indexOf('page = await context.newPage()', install));
  const helper = readFileSync(new URL('./history-joint-telemetry.mjs', import.meta.url), 'utf8');
  assert.match(helper, /history diagnostic installation deadline/);
  assert.match(helper, /history diagnostics installation canceled/);
});

function ownershipUnit(clock) {
  return { label: UNIT_ONLY, clock, module: { captureDiagnostic: raw => {
    assert.equal(raw.version, 1);
    const keys = raw.kind === 'admission'
      ? ['version', 'kind', 'route', 'open_id', 'sampled']
      : ['version', 'kind', 'open_id', raw.kind === 'sse' ? 'stream_id' : 'request_id'];
    assert.deepEqual(Object.keys(raw).sort(), keys.sort());
    return structuredClone(raw);
  } } };
}
function ownershipClock() {
  let fire, cleared = 0;
  return { setTimeout(callback, ms) { assert.equal(ms, 10000); fire = callback; return 1; },
    clearTimeout(id) { assert.equal(id, 1); cleared++; }, expire() { fire(); }, get cleared() { return cleared; } };
}
function deferred() { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }

for (const phase of ['binding', 'init_script']) test(`UNIT ONLY ownership ${phase} deadline blocks late work and sanitizes errors`, async () => {
  const clock = ownershipClock(), pending = deferred(), reached = deferred(), calls = [];
  let callback;
  const context = {
    exposeBinding: async (_name, cb) => { callback = cb; calls.push('binding'); if (phase === 'binding') { reached.resolve(); await pending.promise; } },
    addInitScript: async () => { calls.push('init_script'); if (phase === 'init_script') { reached.resolve(); await pending.promise; } },
  };
  const operation = installHistoryRequestOwnership(context, { unitOnly: true }, ownershipUnit(clock));
  const rejected = assert.rejects(operation, new RegExp(`^Error: history diagnostic ${phase} setup failed_or_deadline$`));
  await reached.promise; clock.expire(); await rejected;
  assert.equal(clock.cleared, 1);
  pending.resolve(); await Promise.resolve(); await Promise.resolve();
  assert.deepEqual(calls, phase === 'binding' ? ['binding'] : ['binding', 'init_script']);
  callback({}, { private: 'UNIT_PRIVATE_SENTINEL' });
});
for (const phase of ['binding', 'init_script']) test(`UNIT ONLY ownership ${phase} rejection never exposes private setup error`, async () => {
  const clock = ownershipClock();
  const fail = async () => { throw new Error('UNIT_PRIVATE_SENTINEL'); };
  const context = { exposeBinding: phase === 'binding' ? fail : async () => {}, addInitScript: fail };
  await assert.rejects(installHistoryRequestOwnership(context, { unitOnly: true }, ownershipUnit(clock)), error => {
    assert.equal(error.message, `history diagnostic ${phase} setup failed_or_deadline`);
    assert.ok(!error.stack.includes('UNIT_PRIVATE_SENTINEL')); return true;
  });
  assert.equal(clock.cleared, 1);
});
test('UNIT ONLY original page/main frame scope cannot borrow foreign admissions or exhaust bounds', async () => {
  let callback;
  const frame = {}, page = { mainFrame: () => frame }, source = { page, frame };
  const owner = await installHistoryRequestOwnership({ exposeBinding: async (_, cb) => { callback = cb; }, addInitScript: async () => {} },
    { unitOnly: true }, ownershipUnit(ownershipClock()));
  const admission = { version: 1, kind: 'admission', route: 'ordinary', open_id: 'UNIT-journey', sampled: true };
  callback(source, admission);
  owner.bindPage(page);
  assert.throws(() => owner.bindPage(page), /bind once/);
  for (const foreign of [{ page: {}, frame }, { page, frame: {} }]) {
    for (let i = 0; i < 2050; i++) callback(foreign, admission);
  }
  assert.deepEqual(owner.snapshot(), { pairs: [], streams: [], failure: null });
  callback(source, { version: 1, kind: 'request', open_id: admission.open_id, request_id: 'UNIT-request' });
  assert.equal(owner.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  const clock = ownershipClock();
  const valid = await installHistoryRequestOwnership({ exposeBinding: async (_, cb) => { callback = cb; }, addInitScript: async () => {} },
    { unitOnly: true }, ownershipUnit(clock));
  valid.bindPage(page); callback(source, admission);
  callback(source, { version: 1, kind: 'request', open_id: admission.open_id, request_id: 'UNIT-request' });
  assert.deepEqual(valid.snapshot(), { pairs: [{ journey_id: admission.open_id, request_id: 'UNIT-request' }], streams: [], failure: null });
  assert.equal(clock.cleared, 1);
});
test('UNIT ONLY ownership injection cannot enter runtime config', async () => {
  await assert.rejects(installHistoryRequestOwnership({}, { unitOnly: false }, ownershipUnit()), /cannot become runtime proof/);
  await assert.rejects(installHistoryRequestOwnership({}, { unitOnly: true }), /cannot become runtime proof/);
  await assert.rejects(installHistoryRequestOwnership({}, { unitOnly: true }, { label: 'wrong' }));
});

test('UNIT ONLY SSE correlation needs independent same-journey producer edge AND original-page network observation', () => {
  const inventory = historyInventory();
  inventory.reports[0].correlations.push({ kind: 'sse', id: 'UNIT-stream' });
  inventory.requestRecords.push({ kind: 'sse', open_id: 'UNIT-stream', trace_id: null });
  assert.throws(() => appendObservedHistory(stock(), inventory, input().fixtures.identity, unitModule), /SSE must belong/);
  inventory.streamOwners.push({ journey_id: 'UNIT-foreign', stream_id: 'UNIT-stream' });
  assert.throws(() => appendObservedHistory(stock(), inventory, input().fixtures.identity, unitModule), /SSE must belong/);
  inventory.streamOwners.push({ journey_id: 'unit-history', stream_id: 'UNIT-stream' });
  appendObservedHistory(stock(), inventory, input().fixtures.identity, unitModule);
  inventory.requestRecords.pop();
  assert.throws(() => appendObservedHistory(stock(), inventory, input().fixtures.identity, unitModule), /independent request\/response provenance/);
});
test('UNIT ONLY request and SSE producer edges share the fixed ownership cap', async () => {
  let callback;
  const frame = {}, page = { mainFrame: () => frame }, source = { page, frame };
  const owner = await installHistoryRequestOwnership({ exposeBinding: async (_, cb) => { callback = cb; }, addInitScript: async () => {} },
    { unitOnly: true }, ownershipUnit(ownershipClock()));
  owner.bindPage(page);
  callback(source, { version: 1, kind: 'admission', route: 'ordinary', open_id: 'UNIT-journey', sampled: true });
  callback(source, { version: 1, kind: 'sse', open_id: 'UNIT-journey', stream_id: 'UNIT-stream' });
  assert.deepEqual(owner.snapshot().streams, [{ journey_id: 'UNIT-journey', stream_id: 'UNIT-stream' }]);
  for (let n = 0; n < 511; n++) callback(source, { version: 1, kind: 'request', open_id: 'UNIT-journey', request_id: 'UNIT-request' });
  assert.equal(owner.snapshot().failure, null);
  callback(source, { version: 1, kind: 'sse', open_id: 'UNIT-journey', stream_id: 'UNIT-overflow' });
  assert.equal(owner.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  assert.equal(owner.snapshot().streams.length + owner.snapshot().pairs.length, 512);
});

test('UNIT ONLY original-page network observer latches sanitized bounded errors', () => {
  const page = new EventEmitter(), observer = installObservedHistoryTelemetry(page, 'http://127.0.0.1:12345');
  const request = { url() { throw new Error('UNIT_PRIVATE_SENTINEL'); } };
  for (let n = 0; n < 2050; n++) page.emit('request', request);
  assert.deepEqual(observer.snapshot().errors, ['invalid_or_overflow_history_network_observation']);
  assert.ok(!JSON.stringify(observer.snapshot()).includes('UNIT_PRIVATE_SENTINEL'));
});
test('UNIT ONLY producer event bound is independent of ownership edge count', async () => {
  let callback;
  const frame = {}, page = { mainFrame: () => frame }, source = { page, frame };
  const owner = await installHistoryRequestOwnership({ exposeBinding: async (_, cb) => { callback = cb; }, addInitScript: async () => {} },
    { unitOnly: true }, ownershipUnit(ownershipClock()));
  owner.bindPage(page);
  const admission = { version: 1, kind: 'admission', route: 'ordinary', open_id: 'UNIT-journey', sampled: true };
  for (let n = 0; n < 2048; n++) callback(source, admission);
  assert.equal(owner.snapshot().failure, null);
  callback(source, admission);
  assert.equal(owner.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  assert.deepEqual(owner.snapshot().pairs, []);
});
