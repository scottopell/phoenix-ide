import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { lstatSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';

export const APP_PIN = '7d17bd70b9c539b3fca6deb133bb2475f9a96e04';
export const APP_BASE_PIN = '4894426888d175d4fd65095647bede65bb09718e';
export const TELEMETRY_MODULE_SHA256 = '1c702c78bb06f74f3dae4c8f97d1932fc308959596f3602375bc65d7d99fb9e1';
export const TELEMETRY_DIAGNOSTICS_SHA256 = 'e06232a39200d88e0a98aedb3d9e6183537f97a6e9498f27fe57a7e2a9a4696b';
export const TELEMETRY_DIAGNOSTICS_RELATIVE_MODULE = 'ui/scripts/view-telemetry-diagnostics.mjs';
export const TELEMETRY_RELATIVE_MODULE = 'ui/scripts/qualify-view-telemetry.mjs';
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
    spaControl: { kind: 'sidebar_home' } }, 'lifecycle must use the actual ordinary fixture and real mounted sidebar home control');
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
  const diagnosticsPath = resolve(root, TELEMETRY_DIAGNOSTICS_RELATIVE_MODULE);
  assert.equal(realpath(diagnosticsPath), diagnosticsPath, 'diagnostics helper realpath must remain within APP_ROOT');
  assert.ok(lstat(diagnosticsPath).isFile() && !lstat(diagnosticsPath).isSymbolicLink(), 'diagnostics helper must be a nonsymlink file');
  assert.equal(digest(diagnosticsPath), TELEMETRY_DIAGNOSTICS_SHA256, 'diagnostics helper actual hash mismatch');
  const dependencies = Object.freeze({ [modulePath]: TELEMETRY_MODULE_SHA256, [diagnosticsPath]: TELEMETRY_DIAGNOSTICS_SHA256 });
  const fixturePath = resolve(nonempty(env.HISTORY_TELEMETRY_FIXTURE, 'telemetry fixture required'));
  assert.ok(lstat(fixturePath).isFile() && !lstat(fixturePath).isSymbolicLink(), 'telemetry fixture must be a nonsymlink file');
  const input = validateJointFixtures(JSON.parse(readFileSync(fixturePath, 'utf8')));
  return Object.freeze({ modulePath, moduleHash: TELEMETRY_MODULE_SHA256, dependencies, fixturePath, input, unitOnly: Boolean(unit) });
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

export async function installHistoryRequestOwnership(context, config, UNIT) {
  assert.equal(config.unitOnly, UNIT !== undefined, 'UNIT history diagnostics cannot become runtime proof');
  if (UNIT) assert.equal(UNIT.label, UNIT_LABEL);
  const clock = UNIT?.clock ?? { setTimeout, clearTimeout };
  let captureDiagnostic;
  try {
    if (UNIT) ({ captureDiagnostic } = UNIT.module);
    else {
      const helper = resolve(dirname(config.modulePath), 'view-telemetry-diagnostics.mjs');
      assert.equal(sha256(readFileSync(helper)), TELEMETRY_DIAGNOSTICS_SHA256);
      ({ captureDiagnostic } = await import(pathToFileURL(helper).href));
    }
  } catch { throw new Error('history diagnostic parser setup failed'); }
  const pairs = [], streams = [], producerEvents = [], admissions = new Set(), subscribers = new Set();
  const signal = () => { for (const notify of subscribers) notify(); };
  let events = 0, failure = null, canceled = false, timer, observedPage, phase = 'binding';
  const binding = '__phoenixHistoryDiagnostic';
  const operation = (async () => {
    await context.exposeBinding(binding, (source, raw) => {
      if (canceled || failure || !observedPage || source.page !== observedPage || source.frame !== observedPage.mainFrame()) return;
      try {
        assert.ok(++events <= 2048, 'history producer diagnostic event bound');
        const event = captureDiagnostic(raw);
        producerEvents.push(event);
        if (event.kind === 'admission' && event.sampled) admissions.add(event.open_id);
        if (event.kind === 'request' || event.kind === 'sse') {
          assert.ok(admissions.has(event.open_id), 'history correlation owner must be admitted');
          assert.ok(pairs.length + streams.length < 512, 'history correlation owner bound');
          if (event.kind === 'request') pairs.push({ journey_id: event.open_id, request_id: event.request_id });
          else streams.push({ journey_id: event.open_id, stream_id: event.stream_id });
        }
      } catch { failure = 'invalid_or_overflow_history_request_ownership'; }
      signal();
    });
    assert.ok(!canceled, 'history diagnostics installation canceled');
    phase = 'init_script';
    await context.addInitScript(({ binding }) => {
      window.addEventListener('phoenix:view-diagnostic', event => {
        void Promise.resolve(window[binding](event.detail)).catch(() => {});
      });
    }, { binding });
    assert.ok(!canceled, 'history diagnostics installation canceled');
  })();
  try {
    await Promise.race([operation, new Promise((_, reject) => {
      timer = clock.setTimeout(() => { canceled = true; reject(new Error('history diagnostic installation deadline')); }, 10000);
    })]);
  } catch {
    canceled = true;
    throw new Error(`history diagnostic ${phase} setup failed_or_deadline`);
  } finally { clock.clearTimeout(timer); }
  return {
    bindPage(page) { assert.ok(!observedPage, 'history observed page must bind once'); observedPage = page; },
    snapshot: () => structuredClone({ pairs, streams, failure }),
    diagnostics: () => structuredClone(producerEvents),
    subscribe(notify) { subscribers.add(notify); return () => subscribers.delete(notify); },
  };
}

export function installHistoryPaginationDiagnostic(page, baseURL, endpoint) {
  const origin = new URL(baseURL).origin, requests = new Map(), correlations = new Set();
  let errors = 0, reportStarted = false;
  page.on('request', request => {
    try {
      const url = new URL(request.url());
      if (url.origin !== origin) return;
      if (url.pathname === '/api/telemetry/conversation-view' && request.method() === 'POST') reportStarted = true;
      if (url.pathname !== endpoint || request.method() !== 'GET' || !url.searchParams.has('before')) return;
      assert.ok(requests.size < 128);
      const record = { has_open_id: url.searchParams.getAll('open_id').length === 1 && Boolean(url.searchParams.get('open_id')), before_report: !reportStarted, status: null, finished: false, failed: false };
      requests.set(request, { record, id: url.searchParams.get('open_id') });
    } catch { errors = Math.min(128, errors + 1); }
  });
  page.on('response', response => {
    try { const observed = requests.get(response.request()); if (observed) observed.record.status = response.status(); }
    catch { errors = Math.min(128, errors + 1); }
  });
  page.on('requestfinished', request => { const observed = requests.get(request); if (observed) observed.record.finished = true; });
  page.on('requestfailed', request => { const observed = requests.get(request); if (observed) observed.record.failed = true; });
  return { snapshot(reports = []) {
    correlations.clear();
    for (const report of reports) for (const c of report.correlations ?? []) if (c.kind === 'request') correlations.add(c.id);
    return { errors, requests: [...requests.values()].map(({ record, id }) => ({ ...record, in_report: Boolean(id) && correlations.has(id) })) };
  } };
}

export function installObservedHistoryTelemetry(page, baseURL, ownership) {
  ownership?.bindPage(page);
  const origin = new URL(baseURL).origin, requests = [], reports = [], statuses = [], errors = [];
  const postReports = new Map(), requestRecords = new Map();
  page.on('request', request => {
    try {
      const url = new URL(request.url()); if (url.origin !== origin) return;
      if (url.pathname === '/api/telemetry/conversation-view' && request.method() === 'POST') {
        const raw = request.postData() ?? '';
        assert.ok(Buffer.byteLength(raw) <= 8192 && reports.length < 7, 'history report capture bound');
        const report = JSON.parse(raw); assert.equal(typeof report.open_id, 'string');
        postReports.set(request, report); reports.push(report);
      } else if (url.pathname.startsWith('/api/') && (url.searchParams.has('open_id') || url.searchParams.has('request_id'))) {
        assert.ok(requests.length < 128, 'history correlated request capture bound');
        const keys = ['open_id', 'request_id'].filter(key => url.searchParams.has(key));
        assert.equal(keys.length, 1, 'one network correlation purpose per request');
        const key = keys[0];
        assert.equal(url.searchParams.getAll(key).length, 1);
        assert.ok(url.searchParams.get(key));
        const kind = /\/api\/conversations\/[^/]+\/stream$/.test(url.pathname) ? 'sse' : 'request';
        if (kind === 'sse') assert.equal(key, 'open_id');
        const record = { method: request.method(), pathname: url.pathname, has_before: url.searchParams.has('before'), open_id: url.searchParams.get(key),
          kind, trace_id: null, status: null, finished: false, failed: false };
        requests.push(record); requestRecords.set(request, record);
      }
    } catch { if (errors.length === 0) errors.push('invalid_or_overflow_history_network_observation'); }
  });
  page.on('response', response => {
    try {
      const request = response.request();
      if (postReports.has(request)) statuses.push({ open_id: postReports.get(request).open_id, status: response.status() });
      const record = requestRecords.get(request);
      if (record) {
        record.status = response.status();
        const trace = response.headers().traceparent?.match(/^00-([0-9a-f]{32})-([0-9a-f]{16})-[0-9a-f]{2}$/i);
        if (trace && !/^0{32}$/.test(trace[1]) && !/^0{16}$/.test(trace[2])) record.trace_id = trace[1].toLowerCase();
      }
    } catch { if (errors.length === 0) errors.push('invalid_or_overflow_history_network_observation'); }
  });
  page.on('requestfinished', request => { const record = requestRecords.get(request); if (record) record.finished = true; });
  page.on('requestfailed', request => { const record = requestRecords.get(request); if (record) record.failed = true; });
  return { snapshot() {
    const owned = ownership?.snapshot();
    return structuredClone({ scope: 'independently_captured_original_history_page_not_receiver_derived',
      reports, requestRecords: requests, statuses, errors, requestOwners: owned?.pairs ?? [],
      streamOwners: owned?.streams ?? [], ownershipFailure: owned?.failure ?? null });
  } };
}

export function appendObservedHistory(result, inventory, identity, module, fixtures) {
  assert.deepEqual(inventory.errors, [], 'history report observer errors');
  assert.equal(inventory.ownershipFailure, null, 'history producer ownership observation must succeed');
  assert.ok(Array.isArray(inventory.requestOwners) && Array.isArray(inventory.streamOwners)
    && inventory.requestOwners.length + inventory.streamOwners.length <= 512,
    'bounded independent history correlation ownership required');
  assert.equal(inventory.statuses.length, inventory.reports.length, 'history report response drain');
  assert.ok(inventory.reports.length <= 7 && inventory.requestRecords.length <= 128);
  const seen = new Set(result.journeys.map(journey => journey.open_id));
  const added = [];
  for (const report of inventory.reports) {
    module.captureReport(JSON.stringify(report), result.forbidden ?? []);
    assert.ok(!seen.has(report.open_id), 'history/module report identity must be unique'); seen.add(report.open_id);
    const statuses = inventory.statuses.filter(status => status.open_id === report.open_id);
    assert.equal(statuses.length, 1, 'every captured history report has one response');
    assert.equal(statuses[0].status, 204, 'actual history collector acceptance');
    const reportedRequests = new Set(report.correlations.filter(c => c.kind === 'request').map(c => c.id));
    const ownedRequests = new Set(inventory.requestOwners.filter(pair => pair.journey_id === report.open_id).map(pair => pair.request_id));
    const ownedStreams = new Set(inventory.streamOwners.filter(pair => pair.journey_id === report.open_id).map(pair => pair.stream_id));
    assert.ok(report.correlations.filter(c => c.kind === 'sse').every(c => ownedStreams.has(c.id)),
      'history SSE must belong to independently observed journey');
    assert.ok([...reportedRequests].every(id => ownedRequests.has(id)), 'history report request must belong to independently observed journey');
    const fixture = fixtures.aggregates.find(f => f.kind === report.route
      && (report.product_conversation_id === null || f.product_conversation_id === report.product_conversation_id));
    assert.ok(fixture, 'history report requires independently selected fixture');
    const detailPaths = new Set([fixture.endpoint, ...(fixture.kind === 'global'
      ? fixture.members.map(member => `/api/global/coordinator/history/${encodeURIComponent(member)}`) : [])]);
    const finite = inventory.requestRecords.filter(record => record.kind === 'request' && record.method === 'GET'
      && detailPaths.has(record.pathname) && reportedRequests.has(record.open_id) && ownedRequests.has(record.open_id));
    assert.ok(finite.length > 0, 'history report needs independently observed finite request');
    assert.ok([...reportedRequests].every(id => finite.some(record => record.open_id === id)), 'every history request correlation requires fixture detail GET');
    if (report.outcome === 'ready') assert.ok(finite.every(record => record.status >= 200 && record.status < 300
      && record.finished === true && record.failed === false), 'ready history detail GET requires successful completed response');
    module.assertRequestCorrelation(finite.map(record => record.open_id), report);
    for (const correlation of report.correlations) {
      const observed = inventory.requestRecords.some(record => correlation.kind === 'trace'
        ? record.kind === 'request' && reportedRequests.has(record.open_id) && ownedRequests.has(record.open_id)
          && record.trace_id === correlation.id
        : record.kind === correlation.kind && record.open_id === correlation.id);
      assert.ok(observed, 'history report correlation must have independent request/response provenance');
    }
    added.push(module.journeyExpectation(report, identity));
  }
  assert.ok(result.journeys.length + added.length <= 64, 'complete history/module inventory bound');
  result.journeys.push(...added);
  result.trials.push({ ...inventory, status: 'observed_history_inventory', sampled: inventory.reports.length > 0 });
  result.additional_history = { reports: inventory.reports.length, scope: inventory.scope };
  validateTelemetryResult(result);
  return result;
}

async function inspectOwnedAuthResponse(response, inspect) {
  let primary = null;
  try { return await inspect(response); }
  catch (error) { primary = error; throw error; }
  finally {
    try { await response.dispose(); }
    catch {
      if (!primary) throw new Error('owned authentication response disposal failed');
      primary.owned_auth_cleanup_failure = 'owned authentication response disposal failed';
    }
  }
}

export async function authenticateOwnedContext(context, baseURL, env = process.env) {
  const home = resolve(nonempty(env.HISTORY_AUTH_HOME, 'owned authentication HOME required'));
  assert.equal(realpathSync(home), home, 'authentication HOME must be canonical');
  const directory = lstatSync(home);
  assert.ok(directory.isDirectory() && (directory.mode & 0o777) === 0o700 && directory.uid === process.getuid(), 'authentication HOME ownership');
  const path = resolve(nonempty(env.HISTORY_AUTH_CREDENTIAL_FILE, 'owned authentication credential required'));
  assert.equal(path, resolve(home, 'qualification-password'), 'credential must belong to actual server HOME');
  const info = lstatSync(path);
  assert.ok(info.isFile() && !info.isSymbolicLink() && (info.mode & 0o777) === 0o600 && info.uid === process.getuid() && info.size === 36, 'credential file ownership');
  const password = readFileSync(path, 'utf8');
  assert.ok(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(password), 'synthetic credential format');
  const origin = new URL(baseURL);
  assert.ok(origin.protocol === 'http:' && origin.hostname === '127.0.0.1' && !origin.username && !origin.password && origin.pathname === '/' && !origin.search && !origin.hash, 'owned login requires bare loopback origin');
  const endpoint = new URL('/api/auth/login', origin).href;
  let response;
  try { response = await context.request.post(endpoint, { data: { password }, timeout: 3000 }); }
  catch { throw new Error('owned browser session login transport failed'); }
  await inspectOwnedAuthResponse(response, value => assert.equal(value.status(), 200, 'owned browser session login status'));
  let status;
  try { status = await context.request.get(new URL('/api/auth/status', baseURL).href, { timeout: 3000 }); }
  catch { throw new Error('owned browser authentication status transport failed'); }
  await inspectOwnedAuthResponse(status, async response => {
    assert.equal(response.status(), 200, 'owned browser authentication status');
    let value; try { value = await response.json(); } catch { throw new Error('owned browser invalid authentication status'); }
    assert.ok(value?.auth_required === true && value?.authenticated === true, 'owned browser authenticated session required');
  });
  const cookies = (await context.cookies(baseURL)).filter(cookie => cookie.name === 'phoenix-auth');
  assert.equal(cookies.length, 1, 'one owned session cookie required');
  assert.ok(typeof cookies[0].value === 'string' && /^[A-Za-z0-9_-]+$/.test(cookies[0].value), 'owned session cookie format');
  return `phoenix-auth=${cookies[0].value}`;
}

export async function runJointTelemetryStage({ config, browser, baseURL, outputDir, historyPhysical, harnessPath, historyGlobalOwnedGetCount, UNIT }) {
  if (config === null) return null;
  assert.equal(config.unitOnly, UNIT !== undefined, 'UNIT dependency injection cannot become fixture proof');
  if (UNIT) assert.equal(UNIT.label, UNIT_LABEL);
  assert.ok(historyPhysical?.count === 1 && historyPhysical.visible === true && historyPhysical.persistedMessageID,
    'independent sealed history physical target receipt required');
  const dependencyManifest = () => Object.fromEntries(Object.keys(config.dependencies).map(path => [path, sha256(readFileSync(path))]));
  const before = config.unitOnly ? config.dependencies : dependencyManifest();
  assert.deepEqual(before, config.dependencies, 'telemetry dependency changed before runtime import');
  const load = UNIT?.importModule ?? (url => import(url));
  let context = null, primaryError = null, phase = 'runtime_import';
  try {
    const module = await load(pathToFileURL(config.modulePath).href);
    phase = 'export_admission';
    assert.equal(typeof module.runTelemetryChecks, 'function', 'installed telemetry module export required');
    phase = 'context_acquisition';
    context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
    phase = 'session_authentication';
    if (config.unitOnly) { if (UNIT?.authenticateContext) await UNIT.authenticateContext(context, baseURL); }
    else await authenticateOwnedContext(context, baseURL);
    phase = 'page_acquisition';
    const page = await context.newPage();
    phase = 'module_checks';
    const result = await module.runTelemetryChecks({ page, context, baseURL,
      serviceName: config.input.serviceName, runId: config.input.runId, outputDir: resolve(outputDir, 'telemetry'), fixtures: config.input.fixtures });
    phase = 'result_validation';
    const after = config.unitOnly ? config.dependencies : dependencyManifest();
    assert.deepEqual(after, before, 'telemetry dependency changed during runtime stage');
    const readyReportOracle = validateTelemetryResult(result);
    const receipt = { status: 'passed_assertions', imported_source: config.modulePath, imported_sha256: config.moduleHash,
      dependency_manifest_before: before, dependency_manifest_after: after,
      evidence_class: config.unitOnly ? 'UNIT_ONLY_not_runtime' : 'joint_runtime_source_receipt_pending_final_drain',
      helper_source: fileURLToPath(import.meta.url), helper_sha256: sha256(readFileSync(fileURLToPath(import.meta.url))),
      app_pin: APP_PIN, app_base_pin: APP_BASE_PIN, harness_path: resolve(harnessPath),
      generated_fixtures: config.input, history_physical: historyPhysical,
      scope: { history_physical: 'sealed original history page exact persisted target',
        telemetry: 'stock generic report plus installed owning-probe source contract; not telemetry lifecycle physical-target proof' },
      classification: { stock_acceptance: result.acceptance, overhead_clearance: false,
        history_global_cache_zero_get_is_history_evidence_not_telemetry_cold_start: historyGlobalOwnedGetCount === 0 },
      ready_report_oracle: readyReportOracle };
    let finalized = false;
    return { receipt, finalizeHistory(inventory) {
      assert.equal(finalized, false, 'history inventory finalizes once after browser closure'); finalized = true;
      appendObservedHistory(result, inventory, config.input.fixtures.identity, module, config.input.fixtures);
      writeFileSync(resolve(outputDir, 'telemetry/telemetry-browser-result.json'), JSON.stringify(result, null, 2));
      receipt.additional_history = result.additional_history;
    } };
  } catch (error) { primaryError = error; throw error; }
  finally {
    const cleanupErrors = [];
    try { if (context) await context.close(); } catch (error) { cleanupErrors.push(error); }
    let after = null;
    try {
      after = config.unitOnly ? config.dependencies : dependencyManifest();
      assert.deepEqual(after, before, 'telemetry dependency changed before context drain completed');
    } catch (error) { cleanupErrors.push(error); }
    try {
      writeFileSync(resolve(outputDir, 'joint-telemetry-dependency-manifest.json'), JSON.stringify({
        evidence_class: config.unitOnly ? 'UNIT_ONLY_not_runtime' : 'runtime_dependency_bytes_not_acceptance',
        before, after, primary_failure: primaryError ? String(primaryError.message).slice(0, 1024) : null,
        failure_phase: primaryError ? phase : null, context_acquired: context !== null,
        authentication_response_cleanup_failure: primaryError?.owned_auth_cleanup_failure ?? null,
        cleanup_failures: cleanupErrors.map(error => String(error.message).slice(0, 1024)),
      }, null, 2));
    } catch (error) { cleanupErrors.push(error); }
    if (cleanupErrors.length) {
      if (primaryError) {
        primaryError.joint_cleanup_failures = cleanupErrors.map(error => String(error.message).slice(0, 1024));
        throw primaryError;
      }
      throw cleanupErrors[0];
    }
  }
}

export const UNIT_ONLY = UNIT_LABEL;
