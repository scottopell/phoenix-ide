import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, readFileSync, symlinkSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { APP_PIN, TELEMETRY_MODULE_SHA256, TELEMETRY_SPA_SELECTOR, UNIT_ONLY,
  prepareJointTelemetry, validateJointFixtures, validateTelemetryResult, runJointTelemetryStage } from './history-joint-telemetry.mjs';

// All positive fake browser/module/fixture values in this file are UNIT ONLY, not runtime proof.
function input() {
  return { runId: 'unit-run', serviceName: 'phoenix-view-qualification-unit-run', fixtures: {
    identity: { appSource: APP_PIN, frontendSource: APP_PIN, serverInstanceId: '12345678-1234-4234-8234-123456789abc' },
    aggregates: [
      { kind: 'ordinary', product_conversation_id: 'ordinary-product', route: '/c/ordinary-product', endpoint: '/api/product-conversations/ordinary-product', members: ['ordinary-root', 'ordinary-latest'], latest_transcript_row_id: 'ordinary-latest' },
      { kind: 'global', product_conversation_id: 'global-product', route: '/global/global-latest', endpoint: '/api/global/coordinator/history/global-root', members: ['global-root', 'global-latest'], latest_transcript_row_id: 'global-latest' },
    ], lifecycle: { route: '/c/ordinary-product', productConversationId: 'ordinary-product', spaSelector: TELEMETRY_SPA_SELECTOR }, forbidden: [],
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
  writeFileSync(module, 'UNIT_ONLY_not_the_donor_module'); writeFileSync(fixture, JSON.stringify(input()));
  const env = { HISTORY_TELEMETRY_JOINT: '1', HISTORY_TELEMETRY_FIXTURE: fixture,
    HISTORY_TELEMETRY_MODULE: module, HISTORY_TELEMETRY_MODULE_SHA256: TELEMETRY_MODULE_SHA256 };
  const UNIT = { label: UNIT_ONLY, digest: () => TELEMETRY_MODULE_SHA256 };
  return { root, module, fixture, env, UNIT, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

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
  app_pin: f => { const i = input(); i.fixtures.identity.appSource = '0'.repeat(40); writeFileSync(f.fixture, JSON.stringify(i)); },
})) test(`pre-import guard rejects ${name} (UNIT ONLY)`, () => {
  const f = unitFiles(); try { mutate(f); assert.throws(() => prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT })); } finally { f.cleanup(); }
});
for (const [name, mutate] of Object.entries({
  global_product_equals_root: i => { i.fixtures.aggregates[1].product_conversation_id = 'global-root'; },
  global_foreign_route: i => { i.fixtures.aggregates[1].route = '/global/foreign'; },
  foreign_origin: i => { i.fixtures.aggregates[0].route = 'http://evil.test/c/ordinary-product'; },
  lifecycle_foreign_product: i => { i.fixtures.lifecycle.productConversationId = 'foreign'; },
  lifecycle_generic_link: i => { i.fixtures.lifecycle.spaSelector = 'a'; },
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
    assert.deepEqual(calls, ['fresh', 'close']); assert.equal(result.classification.overhead_clearance, false);
    assert.match(result.scope.telemetry, /not telemetry lifecycle physical-target proof/);
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
  const stage = source.indexOf('result.joint_telemetry = await runJointTelemetryStage');
  const sealed = source.lastIndexOf('await recorder.sealAndDrain(); recorder.assertQualified();', stage);
  assert.ok(sealed > 0 && sealed < stage);
  assert.ok(source.indexOf("command({ command: 'stop' })", stage) > stage);
  assert.match(source, /let hardStop = setTimeout\(wallFailure, 170000\)/);
  assert.ok(source.indexOf('480000 - (performance.now() - deadlineStarted)') > sealed);
  assert.match(source.slice(stage), /harnessPath: fileURLToPath\(import.meta.url\)/);
});
