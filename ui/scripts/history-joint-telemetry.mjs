import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { lstatSync, readFileSync, realpathSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export const APP_PIN = 'c825370814d22f0e03faf72d18f37df97bb59329';
export const APP_BASE_PIN = '84fbebb0781067cbe843f61c53093dcc0f0d64f2';
export const TELEMETRY_MODULE_SHA256 = 'd0a52f49d583ae55ede9320d6f2ef1bd581fdd2f17dff9f6f989e9a6f75adb83';
export const TELEMETRY_RELATIVE_MODULE = 'ui/scripts/qualify-view-telemetry.mjs';
export const TELEMETRY_SPA_SELECTOR = 'a.statebar-slug[href="/"][title="Back to conversations"]:visible';
const ENV_KEYS = ['HISTORY_TELEMETRY_JOINT', 'HISTORY_TELEMETRY_FIXTURE', 'HISTORY_TELEMETRY_MODULE', 'HISTORY_TELEMETRY_MODULE_SHA256'];
const ANCHOR_RESULTS = new Set(['applied', 'tail_pinned', 'empty', 'target_missing', 'superseded']);
const READY_ANCHORS = new Set(['applied', 'tail_pinned', 'empty']);
const UNIT_LABEL = 'UNIT_ONLY_NOT_FIXTURE_PROOF';

const nonempty = (value, label) => { assert.ok(typeof value === 'string' && value.length > 0, label); return value; };
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const exactRoute = value => {
  nonempty(value, 'fixture route required');
  const parsed = new URL(value, 'http://127.0.0.1');
  assert.equal(parsed.origin, 'http://127.0.0.1');
  assert.equal(parsed.pathname, value, 'fixture route must be an unadorned absolute path');
  return value;
};

export function validateJointFixtures(input) {
  assert.ok(input && typeof input === 'object' && !Array.isArray(input), 'telemetry input object required');
  assert.match(input.runId ?? '', /^[A-Za-z0-9_-]{1,80}$/);
  assert.ok(typeof input.serviceName === 'string' && input.serviceName.endsWith(input.runId), 'run-scoped telemetry service required');
  const fixtures = input.fixtures;
  assert.ok(fixtures && typeof fixtures === 'object' && !Array.isArray(fixtures));
  assert.deepEqual(Object.keys(fixtures.identity ?? {}).sort(), ['appSource', 'frontendSource', 'serverInstanceId']);
  assert.equal(fixtures.identity.appSource, APP_PIN); assert.equal(fixtures.identity.frontendSource, APP_PIN);
  assert.match(fixtures.identity.serverInstanceId, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i);
  assert.deepEqual(fixtures.forbidden, [], 'joint fixture has no synthetic privacy sentinels');
  assert.ok(Array.isArray(fixtures.aggregates) && fixtures.aggregates.length === 2, 'exact ordinary and Global fixtures required');
  const byKind = new Map();
  for (const aggregate of fixtures.aggregates) {
    assert.ok(aggregate && ['ordinary', 'global'].includes(aggregate.kind) && !byKind.has(aggregate.kind));
    nonempty(aggregate.product_conversation_id, 'product identity required');
    exactRoute(aggregate.route); exactRoute(aggregate.endpoint);
    assert.ok(Array.isArray(aggregate.members) && aggregate.members.length >= 2);
    assert.ok(aggregate.members.every(member => typeof member === 'string' && member.length > 0));
    assert.equal(new Set(aggregate.members).size, aggregate.members.length, 'aggregate member identities must be unique');
    assert.equal(aggregate.latest_transcript_row_id, aggregate.members.at(-1), 'latest identity must be actual final member');
    byKind.set(aggregate.kind, aggregate);
  }
  assert.deepEqual([...byKind.keys()].sort(), ['global', 'ordinary']);
  const ordinary = byKind.get('ordinary'), global = byKind.get('global');
  assert.equal(ordinary.endpoint, `/api/product-conversations/${ordinary.product_conversation_id}`);
  assert.equal(ordinary.route, `/c/${ordinary.product_conversation_id}`);
  assert.equal(global.endpoint, `/api/global/coordinator/history/${global.members[0]}`);
  assert.equal(global.route, `/global/${global.latest_transcript_row_id}`);
  assert.equal(new Set([global.product_conversation_id, global.members[0], global.latest_transcript_row_id]).size, 3,
    'Global product/root/latest identities must be distinct');
  assert.equal(new Set(fixtures.aggregates.flatMap(a => [a.product_conversation_id, ...a.members])).size,
    fixtures.aggregates.reduce((total, a) => total + a.members.length + 1, 0), 'ordinary and Global generated identities must be disjoint');
  assert.deepEqual(fixtures.lifecycle, { route: ordinary.route, productConversationId: ordinary.product_conversation_id,
    spaSelector: TELEMETRY_SPA_SELECTOR }, 'lifecycle must use the actual ordinary fixture and real SPA home anchor');
  JSON.stringify(input, (_, value) => { if (typeof value === 'number') assert.ok(Number.isFinite(value)); return value; });
  return input;
}

export function prepareJointTelemetry({ env, appRoot, UNIT } = {}) {
  const present = ENV_KEYS.filter(key => env?.[key] !== undefined);
  if (present.length === 0) return null;
  assert.equal(env.HISTORY_TELEMETRY_JOINT, '1', 'joint telemetry mode must be exactly 1');
  assert.deepEqual(present, ENV_KEYS, 'joint telemetry environment must be complete');
  const unit = UNIT === undefined ? null : UNIT;
  if (unit) assert.equal(unit.label, UNIT_LABEL);
  const root = resolve(nonempty(appRoot, 'APP_ROOT required'));
  const modulePath = resolve(nonempty(env.HISTORY_TELEMETRY_MODULE, 'telemetry module required'));
  const expectedModule = resolve(root, TELEMETRY_RELATIVE_MODULE);
  assert.equal(modulePath, expectedModule, 'telemetry module must be the installed APP_ROOT module');
  assert.equal(env.HISTORY_TELEMETRY_MODULE_SHA256, TELEMETRY_MODULE_SHA256, 'telemetry module expected hash mismatch');
  const realpath = unit?.realpath ?? realpathSync;
  const lstat = unit?.lstat ?? lstatSync;
  assert.equal(realpath(root), root, 'APP_ROOT must be a real path');
  assert.equal(realpath(modulePath), expectedModule, 'telemetry module realpath must remain within APP_ROOT');
  assert.ok(lstat(modulePath).isFile() && !lstat(modulePath).isSymbolicLink(), 'telemetry module must be a nonsymlink file');
  const digest = unit?.digest ?? (path => sha256(readFileSync(path)));
  assert.equal(digest(modulePath), TELEMETRY_MODULE_SHA256, 'telemetry module actual hash mismatch');
  const fixturePath = resolve(nonempty(env.HISTORY_TELEMETRY_FIXTURE, 'telemetry fixture required'));
  assert.ok(lstat(fixturePath).isFile() && !lstat(fixturePath).isSymbolicLink(), 'telemetry fixture must be a nonsymlink file');
  const input = validateJointFixtures(JSON.parse(readFileSync(fixturePath, 'utf8')));
  return Object.freeze({ modulePath, moduleHash: TELEMETRY_MODULE_SHA256, fixturePath, input, unitOnly: Boolean(unit) });
}

function reportInventory(result) {
  const groups = [result.trials, result.terminal?.trials, result.lifecycle?.trials].filter(Array.isArray);
  return groups.flatMap(trials => trials.flatMap(trial => Array.isArray(trial.reports) ? trial.reports : []));
}

export function validateTelemetryResult(result) {
  assert.equal(result.acceptance, 'partial_initial_terminal_and_lifecycle_export_raw_overhead_only');
  assert.equal(result.comparison?.overhead_clearance, false, 'stock module must not promote overhead clearance');
  const reports = reportInventory(result), reportById = new Map();
  for (const report of reports) {
    assert.ok(typeof report.open_id === 'string' && !reportById.has(report.open_id), 'fresh unique report identity required');
    assert.ok(report.anchor_result === null || ANCHOR_RESULTS.has(report.anchor_result), 'unsupported app anchor result');
    if (report.outcome === 'ready') {
      assert.ok(READY_ANCHORS.has(report.anchor_result), 'ready report requires an app-successful anchor result');
      for (const mark of ['meaningful_transcript', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled'])
        assert.ok(Number.isFinite(report.marks?.[mark]), `ready report missing current ${mark}`);
    }
    reportById.set(report.open_id, report);
  }
  assert.ok([...reportById.values()].some(report => report.outcome === 'ready'), 'accepted ready browser report required');
  assert.equal(result.journeys?.length, reports.length, 'every current report must have one exported journey');
  for (const journey of result.journeys) {
    const report = reportById.get(journey.open_id); assert.ok(report, 'exported journey lacks current report');
    assert.equal(journey.expected?.['browser.outcome'], report.outcome);
    for (const mark of ['meaningful_transcript', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled'])
      assert.equal(journey.expected?.[`browser.${mark}_ms`], report.marks[mark], `exported ${mark} differs from current report`);
  }
  return [...reportById.values()].filter(report => report.outcome === 'ready').map(report => ({
    open_id: report.open_id, anchor_result: report.anchor_result,
    current_marks: Object.fromEntries(['meaningful_transcript', 'breadcrumb_presented', 'breadcrumb_enabled', 'anchor_settled'].map(mark => [mark, report.marks[mark]])),
  }));
}

export async function runJointTelemetryStage({ config, browser, baseURL, outputDir, historyPhysical, harnessPath, historyGlobalOwnedGetCount, UNIT }) {
  if (config === null) return null;
  assert.equal(config.unitOnly, UNIT !== undefined, 'UNIT dependency injection cannot become fixture proof');
  if (UNIT) assert.equal(UNIT.label, UNIT_LABEL);
  assert.ok(historyPhysical?.count === 1 && historyPhysical.visible === true && historyPhysical.persistedMessageID,
    'independent sealed history physical target receipt required');
  const load = UNIT?.importModule ?? (url => import(url));
  const module = await load(pathToFileURL(config.modulePath).href);
  assert.equal(typeof module.runTelemetryChecks, 'function', 'installed telemetry module export required');
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  try {
    const page = await context.newPage();
    const result = await module.runTelemetryChecks({ page, context, baseURL,
      serviceName: config.input.serviceName, runId: config.input.runId, outputDir: resolve(outputDir, 'telemetry'), fixtures: config.input.fixtures });
    const readyReportOracle = validateTelemetryResult(result);
    return { status: 'passed_assertions', imported_source: config.modulePath, imported_sha256: config.moduleHash,
      app_pin: APP_PIN, app_base_pin: APP_BASE_PIN, harness_path: resolve(harnessPath),
      generated_fixtures: config.input, history_physical: historyPhysical,
      scope: { history_physical: 'sealed original history page exact persisted target',
        telemetry: 'stock generic report plus installed owning-probe source contract; not telemetry lifecycle physical-target proof' },
      classification: { stock_acceptance: result.acceptance, overhead_clearance: false,
        history_global_cache_zero_get_is_history_evidence_not_telemetry_cold_start: historyGlobalOwnedGetCount === 0 },
      ready_report_oracle: readyReportOracle };
  } finally { await context.close(); }
}

export const UNIT_ONLY = UNIT_LABEL;
