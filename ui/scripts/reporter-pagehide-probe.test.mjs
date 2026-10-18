import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { CASES, APP_PIN, SOURCE_PINS, MODULE_PINS, pinnedHashes, sha256, ProbeError, diagnostic, projectTrial, functionalPass, safeJSON } from './reporter-pagehide-probe.mjs';

// Compiler-free UNIT ONLY controls. No HTTP listener, browser, build, DB or network.
const report = { open_id: 'UNIT_ONLY', outcome: 'incomplete', marks: { meaningful_transcript: null } };
const trial = { status: 'provisional_incomplete_collector_export_pending', sampled: true,
  reports: [report], statuses: [], context_close_returned: true, transport_facts: [] };
const facts = { body: report, bodyReceived: true, server204Attempted: false, serverFinished: false, trigger: true, error: null };
function packet(cases = []) {
  return { version: 1, scope: 'synthetic_actual_reporter_lifecycle_not_Phoenix_joint_acceptance', provenance: {
    node: process.version, playwright: '1.61.0', chromiumRevision: '1234', chromiumVersion: '145.0.0.0',
    sourceCommit: 'a'.repeat(40), appPin: APP_PIN, lockSha256: 'b'.repeat(64), bundleSha256: 'c'.repeat(64),
    sourceHashes: { ...SOURCE_PINS }, moduleHashes: { ...MODULE_PINS },
  }, cases };
}
function unitCases() {
  return CASES.map(name => name === 'unsampled'
    ? projectTrial(name, { ...trial, sampled: false, reports: [], status: 'unsampled' }, { ...facts, body: null, bodyReceived: false, trigger: false }, 'inconclusive_sampling_budget')
    : projectTrial(name, trial, { ...facts, server204Attempted: name !== 'never204', serverFinished: name !== 'never204' }, 'provisional_incomplete_collector_export_pending'));
}
test('pins match the five actual f16 sources and all four immutable telemetry modules', () => {
  assert.deepEqual(pinnedHashes(SOURCE_PINS), SOURCE_PINS);
  assert.deepEqual(pinnedHashes(MODULE_PINS), MODULE_PINS);
});
test('never204 may be qualifier-provisional with zero statuses, but cannot confirm delivery', () => {
  const c = projectTrial('never204', trial, facts, 'provisional_incomplete_collector_export_pending');
  assert.equal(c.qualifierStatus, trial.status); assert.equal(c.pageStatusCount, 0);
  assert.equal(c.bodyReceived, true); assert.equal(c.reportCaptured, true);
  assert.equal(c.deliveryConfirmed, false); assert.equal(c.client204, false);
  assert.equal(functionalPass(packet(unitCases())), true);
});
test('report capture, collector body, server 204 and page/context client 204 remain independent', () => {
  assert.equal(projectTrial('normal204', trial, { ...facts, body: null, bodyReceived: false }).bodyReceived, false);
  assert.equal(projectTrial('normal204', { ...trial, reports: [] }, facts).reportCaptured, false);
  assert.equal(projectTrial('normal204', trial, { ...facts, body: { ...report, open_id: 'foreign' } }).bodyMatches, false);
  const server = { ...facts, server204Attempted: true, serverFinished: true };
  assert.equal(projectTrial('normal204', trial, server).deliveryConfirmed, false);
  const page = projectTrial('normal204', { ...trial, statuses: [204] }, server);
  assert.equal(page.page204, true); assert.equal(page.context204, false); assert.equal(page.deliveryConfirmed, true);
  const context = projectTrial('normal204', { ...trial, transport_facts: [{ kind: 'context_response', status: 204, ordinal: 0 }] }, server);
  assert.equal(context.page204, false); assert.equal(context.context204, true); assert.equal(context.deliveryConfirmed, true);
  assert.equal(projectTrial('normal204', { ...trial, transport_facts: [{ kind: 'context_response', status: 204, ordinal: 1 }] }, server).client204, false);
});
for (const [name, mutate] of Object.entries({
  missing_case: r => r.cases.pop(), duplicate_case: r => { r.cases[3] = structuredClone(r.cases[0]); },
  missing_body: r => { r.cases[0].bodyMatches = false; }, primary: r => { r.cases[0].primary = diagnostic(new Error('PRIVATE'), 'qualifier'); },
  cleanup: r => { r.cases[0].cleanup.push(diagnostic(new Error('PRIVATE'), 'browser_cleanup')); },
  ambiguous: r => { r.cases[0].ambiguousQualifierFailure = diagnostic(new Error('PRIVATE'), 'qualifier'); },
  never_response: r => { r.cases[2].client204 = true; }, unsampled_report: r => { r.cases[3].reportCaptured = true; },
})) test(`functional oracle rejects ${name} (UNIT ONLY)`, () => {
  const r = packet(unitCases()); mutate(r); assert.equal(functionalPass(r), false);
});
test('diagnostics freeze fixed cause/location without messages, stacks or unknown first callers', () => {
  const error = new ProbeError('collector_contract'), d = diagnostic(error, 'collector');
  error.message = 'PRIVATE'; error.stack = 'PRIVATE'; error.code = 'PRIVATE';
  assert.equal(d.cause, 'collector_contract'); assert.equal(d.location.site, 'stage2-collector_contract');
  assert.doesNotMatch(JSON.stringify(d), /PRIVATE/);
  const unknown = diagnostic(new Error('https://private.invalid/SECRET'), 'qualifier');
  assert.deepEqual(unknown, { phase: 'qualifier', cause: 'unknown', location: null });
  const r = packet(unitCases()); r.cases[0].primary = d; assert.doesNotMatch(safeJSON(r), /PRIVATE/);
});
for (const [name, mutate] of Object.entries({
  raw_report: r => { r.cases[0].report = report; }, raw_message: r => { r.cases[0].primary = { message: 'PRIVATE' }; },
  unsafe_source: r => { r.provenance.sourceCommit = 'https://private'; },
  unexpected_pin: r => { r.provenance.moduleHashes['private/path'] = 'd'.repeat(64); },
  unsafe_status: r => { r.cases[0].qualifierStatus = 'PRIVATE'; },
  raw_cleanup: r => { r.cases[0].cleanup.push('PRIVATE'); },
})) test(`safe JSON rejects ${name}`, () => { const r = packet(unitCases()); mutate(r); assert.throws(() => safeJSON(r)); });
test('safe JSON keeps only bounded boolean facts, pins and approved source/version values', () => {
  assert.deepEqual(JSON.parse(safeJSON(packet(unitCases()))), packet(unitCases()));
});
test('CLI and driver import are inert without resolving Vite/Playwright or listening', async () => {
  const handles = process._getActiveHandles().length;
  const cli = await import('./reporter-pagehide-probe.cli.mjs');
  assert.equal(typeof cli.main, 'function'); assert.equal(process._getActiveHandles().length, handles);
});
test('actual imports and real request begin precede fetch; no factory/global overrides', () => {
  const entry = readFileSync(new URL('../qualification/reporter-pagehide.entry.ts', import.meta.url), 'utf8');
  assert.match(entry, /import \{ ViewJourneyReporter \} from '\.\.\/src\/hooks\/conversationViewTelemetry'/);
  assert.match(entry, /import \{ ViewJourneyLifecycle, defaultViewIdentity \} from '\.\.\/src\/hooks\/conversationViewLifecycle'/);
  assert.ok(entry.indexOf('lifecycle.requestStarted(requestId)') < entry.indexOf('void fetch('));
  assert.match(entry, /new ViewJourneyReporter\(\{\s+random:/);
  assert.match(entry, /send: \(input, init\) => fetch\(input, init\)/);
  assert.doesNotMatch(entry, /sampleRate:|Math\.random\s*=|createViewJourneyReporter|finish\('ready'\)/);
  assert.match(entry, /addEventListener\('pagehide'/);
  const driver = readFileSync(new URL('./reporter-pagehide-probe.mjs', import.meta.url), 'utf8');
  assert.match(driver, /attempts: 1,/); assert.doesNotMatch(driver, /runJointTelemetryStage|validateTelemetryResult|finalizeHistory|runTelemetryChecks/);
  assert.match(driver, /Promise\.all\(\[received\.promise, pagehide\.promise\]\)/);
  assert.ok(driver.indexOf('primary ??= collectorPrimary') < driver.indexOf('await bounded(browser.close())'));
});
const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
const job = workflow.split('  reporter-pagehide-functional-probe:\n')[1].split('\n  qualify-history-realserver:')[0];
test('Stage 2 requires same-repo owned PR and both independent opt-in labels', () => {
  assert.equal(job.split('\n').find(line => line.startsWith('    if:')),
    "    if: ${{ github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository && github.head_ref == 'prepare/exact-f16-stock-admission' && contains(github.event.pull_request.labels.*.name, 'offline-regression-only') && contains(github.event.pull_request.labels.*.name, 'reporter-pagehide-probe') }}");
  assert.match(job, /pnpm install --frozen-lockfile --ignore-scripts/);
  assert.match(job, /pnpm exec playwright install --with-deps chromium/);
  assert.doesNotMatch(job, /context-close-probe\.cli|cargo|dev\.py|history_opening_browser|rerun|retry/);
  assert.match(job, /120s node ui\/scripts\/reporter-pagehide-probe.cli.mjs/);
  assert.match(job, /reporter-pagehide-evidence\/result.json/);
});
test('hosted bundle uses pinned Vite, actual exact module inventory and bundle hash, without app build', () => {
  const cli = readFileSync(new URL('./reporter-pagehide-probe.cli.mjs', import.meta.url), 'utf8');
  assert.match(cli, /await import\('vite'\)/); assert.match(cli, /Object.keys\(chunk.modules\).sort\(\)/);
  assert.match(cli, /bundleSha256: sha256\(chunk.code\)/);
  const config = readFileSync(new URL('../vite.reporter-pagehide.config.ts', import.meta.url), 'utf8');
  assert.match(config, /write: false/); assert.match(config, /formats: \['iife'\]/);
  assert.doesNotMatch(config, /vite\.config|controlled-functional|viewReporterBuild/);
  assert.equal(sha256(readFileSync(new URL('../src/hooks/conversationViewTelemetry.ts', import.meta.url))), SOURCE_PINS['ui/src/hooks/conversationViewTelemetry.ts']);
});
