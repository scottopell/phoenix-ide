import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { EventEmitter } from 'node:events';
import { CASES, APP_PIN, SOURCE_PINS, MODULE_PINS, pinnedHashes, sha256, ProbeError, diagnostic, projectTrial, functionalPass, safeJSON, emptyResult, emptyTransport, observeTransport, acquireOwned, listenCollector, runSuite } from './reporter-pagehide-probe.mjs';

// Compiler-free UNIT ONLY controls. No HTTP listener, browser, build, DB or network.
const report = { open_id: 'UNIT_ONLY', outcome: 'incomplete', marks: { meaningful_transcript: null } };
const trial = { status: 'provisional_incomplete_collector_export_pending', sampled: true,
  reports: [report], statuses: [], context_close_returned: true, transport_facts: [] };
const facts = { body: report, bodyReceived: true, server204Attempted: false, serverFinished: false, trigger: true, error: null };
function packet(cases = []) {
  return { ...emptyResult(), status: 'complete', provenance: {
    node: process.version, playwright: '1.61.0', chromiumRevision: '1234', chromiumVersion: '145.0.0.0',
    sourceCommit: 'a'.repeat(40), appPin: APP_PIN, lockSha256: 'b'.repeat(64), bundleSha256: 'c'.repeat(64),
    sourceHashes: { ...SOURCE_PINS }, moduleHashes: { ...MODULE_PINS },
  }, cases };
}
function unitCases() {
  return CASES.map(name => name === 'unsampled'
    ? projectTrial(name, { ...trial, sampled: false, reports: [], status: 'unsampled' }, { ...facts, body: null, bodyReceived: false, trigger: false }, 'inconclusive_sampling_budget')
    : projectTrial(name, { ...trial, statuses: name === 'never204' ? [] : [204] }, { ...facts, server204Attempted: name !== 'never204', serverFinished: name !== 'never204' }, 'provisional_incomplete_collector_export_pending'));
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
for (const name of ['normal204', 'deferred204']) test(`${name} without any client 204 cannot qualify despite server finish`, () => {
  const r = packet(unitCases());
  r.cases[CASES.indexOf(name)] = projectTrial(name, trial,
    { ...facts, server204Attempted: true, serverFinished: true }, trial.status);
  assert.equal(r.cases[CASES.indexOf(name)].deliveryConfirmed, false);
  assert.equal(functionalPass(r), false);
});
test('all server finishes without client receipt is not a functional pass', () => {
  const r = packet(unitCases());
  for (const c of r.cases) { c.serverFinished = true; c.client204 = false; c.deliveryConfirmed = false; }
  assert.equal(functionalPass(r), false);
});
test('three sampled pagehide controls and one ADMISSION_ONLY negative; unsampled is not pagehide coverage', () => {
  const r = packet(unitCases());
  assert.equal(r.cases.filter(c => c.controlScope === 'SAMPLED_PAGEHIDE' && c.pagehideTrigger).length, 3);
  assert.equal(r.cases[3].controlScope, 'ADMISSION_ONLY');
  assert.equal(r.cases[3].pagehideTrigger, false);
  assert.equal(functionalPass(r), true);
  r.cases[0].pagehideTrigger = false;
  assert.equal(functionalPass(r), false);
});
test('exact Request object owns all four terminal boolean inventories, not same URL or other traffic', () => {
  const context = new EventEmitter(), page = new EventEmitter(), observed = emptyTransport();
  observeTransport(context, 'http://unit.invalid', observed); context.emit('page', page);
  const request = () => ({ method: () => 'POST', url: () => 'http://unit.invalid/api/telemetry/conversation-view' });
  const owner = request(), foreign = request();
  context.emit('request', owner); page.emit('request', owner);
  for (const emitter of [context, page]) for (const kind of ['requestfailed', 'requestfinished']) emitter.emit(kind, foreign);
  assert.deepEqual(observed, emptyTransport());
  page.emit('requestfailed', owner); context.emit('requestfinished', owner);
  assert.deepEqual(observed, { ...emptyTransport(), pageRequestFailed: true, contextRequestFinished: true });
  page.emit('requestfinished', owner); context.emit('requestfailed', owner);
  assert.ok(Object.values(observed).every(Boolean));
  const c = projectTrial('normal204', { ...trial, statuses: [204] },
    { ...facts, server204Attempted: true, serverFinished: true }, trial.status, observed);
  assert.equal(c.deliveryConfirmed, true); // requestfailed does not imply non-delivery.
  assert.equal(functionalPass(packet([c, ...unitCases().slice(1)])), true);
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
  assert.ok(driver.indexOf('primary ??= collectorPrimary') < driver.indexOf('await cleanupResources(cleanup)'));
});
function suiteControl(overrides = {}) {
  const owned = { launched: 0, closed: 0, collected: 0, stopped: 0, receipts: [] };
  const options = {
    prepare: async () => ({ provenance: { ...packet().provenance, chromiumVersion: null }, bundle: 'UNIT_ONLY' }),
    launch: async () => { owned.launched++; return { version: () => '145.0.0.0', close: async () => { owned.closed++; } }; },
    collect: async () => { owned.collected++; return { facts: { error: null }, stop: async () => { owned.stopped++; } }; },
    execute: async ({ name }) => unitCases().find(c => c.name === name),
    checkPins: () => {}, publish: async json => { owned.receipts.push(JSON.parse(json)); },
    setupMs: 1000, ...overrides,
  };
  return { owned, options };
}
for (const failure of ['launch', 'collector', 'pins']) test(`later ${failure} failure persists prior completed cases and closes acquired resources`, async () => {
  const { owned, options } = suiteControl();
  const original = { ...options };
  if (failure === 'launch') options.launch = async value => {
    if (owned.launched === 1) throw Error('PRIVATE_LAUNCH');
    return original.launch(value);
  };
  if (failure === 'collector') options.collect = async (...args) => {
    if (owned.collected === 1) throw Error('PRIVATE_LISTEN');
    return original.collect(...args);
  };
  if (failure === 'pins') options.checkPins = () => { throw Error('PRIVATE_PIN'); };
  const result = await runSuite(options);
  assert.equal(result.status, 'partial'); assert.equal(functionalPass(result), false);
  assert.equal(result.cases.length, failure === 'pins' ? 4 : 1);
  assert.equal(owned.closed, owned.launched); assert.equal(owned.stopped, owned.collected);
  assert.equal(owned.receipts.length, 1); assert.deepEqual(owned.receipts[0], result);
  assert.equal(result.primary.phase, { launch: 'browser_launch', collector: 'collector_listen', pins: 'suite_pins' }[failure]);
  assert.doesNotMatch(safeJSON(result), /PRIVATE/);
});
test('browser acquisition failure does not acquire a collector and publishes a fixed partial receipt', async () => {
  const { owned, options } = suiteControl({ launch: async () => { throw Error('PRIVATE'); } });
  const result = await runSuite(options);
  assert.equal(owned.collected, 0); assert.equal(owned.closed, 0); assert.equal(result.cases.length, 0);
  assert.equal(result.primary.cause, 'setup_failure'); assert.equal(functionalPass(result), false);
  assert.equal(owned.receipts.length, 1);
});
test('collector acquisition failure after browser acquisition closes the browser even when cleanup fails', async () => {
  let closed = 0;
  const { owned, options } = suiteControl({
    launch: async () => ({ version: () => '145.0.0.0', close: async () => { closed++; throw Error('PRIVATE_CLEANUP'); } }),
    collect: async () => { throw new ProbeError('collector_contract'); },
  });
  const result = await runSuite(options);
  assert.equal(closed, 1); assert.equal(result.primary.phase, 'collector_listen');
  assert.equal(result.primary.cause, 'collector_contract');
  assert.equal(result.cleanup[0].phase, 'browser_cleanup'); assert.equal(owned.receipts.length, 1);
});
test('cleanup-only failure does not manufacture suite primary or qualify', async () => {
  const { options } = suiteControl({ launch: async () => ({ version: () => '145.0.0.0', close: async () => { throw Error('PRIVATE'); } }) });
  const result = await runSuite(options);
  assert.equal(result.primary, null); assert.equal(result.status, 'complete');
  assert.equal(result.cases[0].primary, null); assert.equal(result.cases[0].cleanup[0].phase, 'browser_cleanup');
  assert.equal(functionalPass(result), false);
});
test('first primary projection is frozen before cleanup mutates the same Error object', async () => {
  const primary = new ProbeError('collector_contract');
  const { options } = suiteControl({
    launch: async () => ({ version: () => '145.0.0.0', close: async () => { primary.code = 'PRIVATE'; throw primary; } }),
    execute: async () => { throw primary; },
  });
  const result = await runSuite(options);
  assert.equal(result.primary.cause, 'collector_contract'); assert.equal(result.primary.phase, 'suite');
  assert.equal(result.primary.location.site, 'stage2-collector_contract');
  assert.equal(result.cleanup[0].cause, 'unknown'); assert.doesNotMatch(safeJSON(result), /PRIVATE/);
});
test('CLI finally publishes a schema-safe partial fallback for invalid or missing source provenance', async () => {
  const { main } = await import('./reporter-pagehide-probe.cli.mjs');
  for (const sourceCommit of [undefined, 'PRIVATE_SOURCE']) {
    const { owned, options } = suiteControl({ prepare: async () => ({ provenance: { ...packet().provenance, sourceCommit }, bundle: 'UNIT_ONLY' }) });
    assert.equal(await main(options), 1);
    const result = owned.receipts[0];
    assert.equal(result.provenance, null); assert.equal(result.status, 'partial'); assert.equal(result.cases.length, 0);
    assert.equal(result.primary.phase, 'setup_provenance'); assert.equal(owned.launched, 0);
    assert.doesNotMatch(safeJSON(result), /PRIVATE/);
  }
});
test('CLI finally publishes fixed setup-phase fallback on early import/pin or bundle failure', async () => {
  const { main } = await import('./reporter-pagehide-probe.cli.mjs');
  for (const phase of ['setup_provenance', 'setup_bundle']) {
    const { owned, options } = suiteControl({ prepare: async setPhase => { setPhase(phase); throw Error('PRIVATE'); } });
    assert.equal(await main(options), 1);
    assert.equal(owned.receipts[0].provenance, null); assert.equal(owned.receipts[0].primary.phase, phase);
    assert.equal(owned.receipts[0].primary.cause, 'setup_failure');
    assert.equal(owned.launched, 0); assert.doesNotMatch(JSON.stringify(owned.receipts), /PRIVATE/);
  }
});
test('setup deadline publishes failure without launching any resource', async () => {
  const { owned, options } = suiteControl({ prepare: () => new Promise(() => {}), setupMs: 5 });
  const result = await runSuite(options);
  assert.equal(result.primary.cause, 'observation_timeout'); assert.equal(owned.launched, 0);
  assert.equal(result.provenance, null); assert.equal(owned.receipts.length, 1);
});
const turn = () => new Promise(resolve => setImmediate(resolve));
test('launch arriving after its deadline is still closed exactly once and never qualifies', async () => {
  let arrive, closed = 0;
  const { owned, options } = suiteControl({ launch: () => new Promise(resolve => { arrive = resolve; }), setupMs: 5 });
  const result = await runSuite(options);
  assert.equal(result.primary.phase, 'browser_launch'); assert.equal(result.primary.cause, 'observation_timeout');
  assert.equal(owned.collected, 0); assert.equal(functionalPass(result), false);
  arrive({ close: async () => { closed++; } }); await turn();
  assert.equal(closed, 1); assert.equal(owned.receipts.length, 1);
});
test('late owned acquisition cleanup failure is observed, without raw error leakage or unhandled rejection', async () => {
  let arrive, cleanupError;
  const pending = acquireOwned(() => new Promise(resolve => { arrive = resolve; }),
    async () => { throw Error('PRIVATE'); }, 5, error => { cleanupError = diagnostic(error, 'browser_cleanup'); });
  await assert.rejects(pending, error => error.code === 'observation_timeout');
  arrive({}); await turn();
  assert.deepEqual(cleanupError, { phase: 'browser_cleanup', cause: 'unknown', location: null });
});
test('late fake collector listen callback is closed after deadline; no actual listener exists', async () => {
  const server = new EventEmitter(); let listening, closed = 0, destroyed = 0;
  server.listen = (_port, _host, done) => { listening = done; };
  server.close = done => { closed++; done(); };
  await assert.rejects(listenCollector(server, new Set([{ destroy: () => { destroyed++; } }]), 5),
    error => error.code === 'observation_timeout');
  assert.equal(closed, 1); listening(); await turn();
  assert.equal(closed, 2); assert.equal(destroyed, 2);
});
test('fake collector listen rejection preserves original Error identity through close failure', async () => {
  const server = new EventEmitter(), primary = Error('PRIVATE_LISTEN'); let closed = 0;
  server.listen = () => { queueMicrotask(() => server.emit('error', primary)); };
  server.close = done => { closed++; done(Error('PRIVATE_CLEANUP')); };
  await assert.rejects(listenCollector(server, new Set(), 100), error => error === primary);
  assert.equal(closed, 1);
});
test('late collector acquisition is stopped while its already-owned browser closes on timeout', async () => {
  let arrive, stopped = 0;
  const { owned, options } = suiteControl({ collect: () => new Promise(resolve => { arrive = resolve; }), setupMs: 5 });
  const result = await runSuite(options);
  assert.equal(result.primary.phase, 'collector_listen'); assert.equal(owned.closed, 1);
  arrive({ stop: async () => { stopped++; } }); await turn(); assert.equal(stopped, 1);
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
