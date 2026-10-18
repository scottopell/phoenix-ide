import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import { captureReport, runPendingPagehideCheck } from './qualify-view-telemetry.mjs';
import { prepareJointFailureDiagnostic } from './history-joint-telemetry.mjs';

export const APP_PIN = 'f16afb227d9336a863c0b0345960ea9a6d62b7c7';
export const CASES = Object.freeze(['normal204', 'deferred204', 'never204', 'unsampled']);
export const SOURCE_PINS = Object.freeze({
  'ui/src/hooks/conversationViewLifecycle.ts': 'b4d91d42e7c354d6a57312bc171544f0b0a46856388ba4a9aeee74da61efd12f',
  'ui/src/hooks/conversationViewTelemetry.ts': '0502c3b1c98216e0a1610933791739a503ab652bc0e32efc0afdebe35d2c4b30',
  'ui/src/hooks/conversationViewJourney.ts': '1a9367da51cc8c18d8fe06226c24eabbbb82a0d540eeec3f8d1d848402562a75',
  'ui/src/hooks/conversationViewDiagnostics.ts': 'b43b5576ab14a8b3bc06273fa0b9f0e05e930d3bf3dee705cae9c64a5cdcd39d',
  'ui/src/utils/uuid.ts': '647e6721f03a25c9fd8a0d87f718892f498370a702e09e5a7b2f8e3a9ef39b35',
});
export const MODULE_PINS = Object.freeze({
  'ui/scripts/qualify-view-telemetry.mjs': '1c702c78bb06f74f3dae4c8f97d1932fc308959596f3602375bc65d7d99fb9e1',
  'ui/scripts/view-telemetry-diagnostics.mjs': 'e06232a39200d88e0a98aedb3d9e6183537f97a6e9498f27fe57a7e2a9a4696b',
  'tests/e2e/view_telemetry_sink.py': 'cdb1744c69eee7db301c990e53c8e636eee00ff3d5550d9097ba0cb96b0bd695',
  'tests/e2e/assert_view_telemetry.py': '18bb8465f046d7473b95a49157df5824019ae62e068d34fa98d24c1af656ecdb',
});
export const sha256 = value => createHash('sha256').update(value).digest('hex');
const root = fileURLToPath(new URL('../../', import.meta.url));
const ownBasename = 'reporter-pagehide-probe.mjs';
const ownHash = sha256(readFileSync(new URL(import.meta.url)));
const phases = ['qualifier', 'collector', 'context_cleanup', 'browser_cleanup', 'collector_cleanup'];
const causes = ['unknown', 'ERR_ASSERTION', 'collector_contract', 'observation_timeout'];
export class ProbeError extends Error {
  constructor(code) { super('reporter pagehide probe failure'); assert.ok(causes.slice(2).includes(code)); this.code = code; }
}
export function pinnedHashes(pins) {
  for (const [path, hash] of Object.entries(pins)) assert.equal(sha256(readFileSync(resolve(root, path))), hash);
  return { ...pins };
}
const modulePath = resolve(root, 'ui/scripts/qualify-view-telemetry.mjs');
const diagnosePinned = prepareJointFailureDiagnostic({ modulePath, dependencies: Object.fromEntries(
  Object.entries(MODULE_PINS).map(([path, hash]) => [resolve(root, path), hash])) });
export function diagnostic(error, phase) {
  assert.ok(phases.includes(phase));
  const pinned = diagnosePinned(error, 'module_checks');
  return { phase, cause: error instanceof ProbeError ? error.code : pinned.error_code,
    location: error instanceof ProbeError ? { module_basename: ownBasename, module_sha256: ownHash, site: `stage2-${error.code}` }
      : pinned.resolution === 'verified_assertion_site' ? { module_basename: pinned.module_basename, module_sha256: pinned.module_sha256, site: pinned.assertion_site_id } : null };
}
function gate() {
  let release;
  const promise = new Promise(resolve => { release = resolve; });
  return { promise, release };
}
async function bounded(promise, ms = 5000) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new ProbeError('observation_timeout')), ms); })]); }
  finally { clearTimeout(timer); }
}

// Owned synthetic collector, not Phoenix auth/schema/export/DB acceptance.
export async function startCollector(name, bundle) {
  assert.ok(CASES.includes(name));
  const received = gate(), pagehide = gate(), sockets = new Set();
  const facts = { body: null, bodyReceived: false, server204Attempted: false, serverFinished: false, trigger: false, error: null };
  let postCount = 0, finiteCount = 0, triggerCount = 0;
  const invalid = () => { facts.error ??= new ProbeError('collector_contract'); };
  const server = createServer((req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    if (req.method === 'GET' && url.pathname === `/probe/${name}`) {
      res.setHeader('Content-Type', 'text/html'); res.end('<!doctype html><body><script src="/probe/bundle.js"></script>'); return;
    }
    if (req.method === 'GET' && url.pathname === '/probe/bundle.js') {
      res.setHeader('Content-Type', 'text/javascript'); res.end(bundle); return;
    }
    if (req.method === 'GET' && url.pathname === '/api/product-conversations/11111111-1111-4111-8111-111111111111') {
      if (++finiteCount !== 1 || !/^[a-f0-9-]{36}$/.test(url.searchParams.get('open_id') ?? '')) invalid();
      // Leave the real finite request unfinished; no response data/hydration is fabricated.
      return;
    }
    if (req.method === 'POST' && url.pathname === '/probe/pagehide') {
      if (++triggerCount !== 1) invalid();
      facts.trigger = true; pagehide.release(); res.writeHead(204); res.end(); return;
    }
    if (req.method === 'POST' && url.pathname === '/api/telemetry/conversation-view') {
      if (++postCount !== 1 || name === 'unsampled') invalid();
      let body = '', bytes = 0;
      req.on('data', chunk => { bytes += chunk.length; if (bytes > 8192) { invalid(); req.destroy(); } else body += chunk; });
      req.on('error', invalid);
      req.on('end', () => {
        facts.bodyReceived = true;
        try { facts.body = captureReport(body); } catch { invalid(); }
        received.release();
        res.on('finish', () => { facts.serverFinished = true; });
        const reply = () => {
          facts.server204Attempted = true;
          res.writeHead(204); res.end();
        };
        if (name === 'normal204') reply();
        if (name === 'deferred204') void Promise.all([received.promise, pagehide.promise]).then(reply);
      });
      return;
    }
    res.writeHead(404); res.end();
  });
  server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
  await new Promise((resolveListen, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolveListen); });
  return { origin: `http://127.0.0.1:${server.address().port}`, facts, received: received.promise,
    async stop() { for (const socket of sockets) socket.destroy(); await bounded(new Promise((done, reject) => server.close(error => error ? reject(error) : done()))); } };
}

export function projectTrial(name, trial, facts, qualifierStatus = 'failed') {
  const reports = trial?.reports ?? [], statuses = trial?.statuses ?? [];
  let bodyMatches = false;
  if (facts.body && reports.length === 1) {
    try { assert.deepEqual(facts.body, reports[0]); bodyMatches = true; } catch { /* Separate capture and collector facts. */ }
  }
  const page204 = statuses.length === 1 && statuses[0] === 204;
  const context204 = (trial?.transport_facts ?? []).some(fact => fact.kind === 'context_response' && fact.status === 204 && fact.ordinal === 0);
  const client204 = page204 || context204;
  return { name, qualifierStatus, trialStatus: trial?.status ?? 'failed', sampled: trial?.sampled ?? null,
    reportCaptured: reports.length === 1, bodyReceived: facts.bodyReceived, bodyMatches,
    server204Attempted: facts.server204Attempted, serverFinished: facts.serverFinished,
    client204, page204, context204, pageStatusCount: statuses.length, pagehideTrigger: facts.trigger,
    deliveryConfirmed: bodyMatches && facts.server204Attempted && facts.serverFinished && client204,
    contextClosed: trial?.context_close_returned === true, primary: null, ambiguousQualifierFailure: null, cleanup: [] };
}
export async function runCase({ name, browser, collector }) {
  let trial, stage, primary = null, ambiguousQualifierFailure = null;
  try {
    stage = await runPendingPagehideCheck({ browser, storageState: {}, baseURL: collector.origin, route: `/probe/${name}`,
      attempts: 1, budgetMs: 20000, onTrial: value => { trial = value; } });
  } catch (error) {
    // Frozen trial strings cannot distinguish identical primary/cleanup messages.

    if (!trial?.cleanup_failure || trial.failure !== trial.cleanup_failure) primary = diagnostic(error, 'qualifier');
    else ambiguousQualifierFailure = diagnostic(error, 'qualifier');
  }
  if (name !== 'unsampled' && !primary && !trial?.cleanup_failure) {
    try { await bounded(collector.received); } catch (error) { primary = diagnostic(error, 'collector'); }
  }
  const collectorPrimary = collector.facts.error;
  primary ??= collectorPrimary ? diagnostic(collectorPrimary, 'collector') : null;
  const cleanup = [];
  if (trial?.cleanup_failure) cleanup.push(diagnostic(null, 'context_cleanup'));
  try { await bounded(browser.close()); } catch (error) { cleanup.push(diagnostic(error, 'browser_cleanup')); }
  try { await collector.stop(); } catch (error) { cleanup.push(diagnostic(error, 'collector_cleanup')); }
  const result = projectTrial(name, trial, collector.facts, stage?.status ?? 'failed');
  result.primary = primary;
  result.ambiguousQualifierFailure = ambiguousQualifierFailure;
  if (!collectorPrimary && collector.facts.error) cleanup.push(diagnostic(collector.facts.error, 'collector_cleanup'));
  result.cleanup = cleanup;
  return result;
}
export function functionalPass(result) {
  return result.cases.length === 4 && CASES.every(name => result.cases.filter(c => c.name === name).length === 1)
    && result.cases.every(c => !c.primary && !c.ambiguousQualifierFailure && !c.cleanup.length && c.contextClosed && (c.name === 'unsampled'
      ? c.sampled === false && c.trialStatus === 'unsampled' && c.qualifierStatus === 'inconclusive_sampling_budget' && !c.reportCaptured && !c.bodyReceived && !c.client204
      : c.sampled === true && c.qualifierStatus === 'provisional_incomplete_collector_export_pending' && c.trialStatus === c.qualifierStatus
        && c.reportCaptured && c.bodyMatches && c.pagehideTrigger && (c.name === 'never204'
          ? !c.deliveryConfirmed && !c.server204Attempted && !c.serverFinished && !c.client204
          : c.server204Attempted && c.serverFinished)));
}
function keys(value, expected) { assert.deepEqual(Object.keys(value).sort(), expected.split(' ').sort()); }
function hash(value) { assert.match(value, /^[a-f0-9]{64}$/); }
export function safeJSON(result) {
  keys(result, 'version scope provenance cases'); assert.equal(result.version, 1);
  assert.equal(result.scope, 'synthetic_actual_reporter_lifecycle_not_Phoenix_joint_acceptance');
  const p = result.provenance;
  keys(p, 'node playwright chromiumRevision chromiumVersion sourceCommit appPin lockSha256 bundleSha256 sourceHashes moduleHashes');
  assert.match(p.node, /^v26\.\d+\.\d+$/); assert.equal(p.playwright, '1.61.0');
  assert.match(p.chromiumRevision, /^\d{1,8}$/);
  if (p.chromiumVersion === null) assert.equal(result.cases.length, 0);
  else assert.match(p.chromiumVersion, /^\d+(\.\d+){1,3}$/);
  assert.match(p.sourceCommit, /^[a-f0-9]{40}$/); assert.equal(p.appPin, APP_PIN);
  hash(p.lockSha256); hash(p.bundleSha256);
  for (const [value, pins] of [[p.sourceHashes, SOURCE_PINS], [p.moduleHashes, MODULE_PINS]]) {
    assert.deepEqual(value, pins);
  }
  assert.ok(Array.isArray(result.cases) && result.cases.length <= 4);
  for (const c of result.cases) {
    keys(c, 'name qualifierStatus trialStatus sampled reportCaptured bodyReceived bodyMatches server204Attempted serverFinished client204 page204 context204 pageStatusCount pagehideTrigger deliveryConfirmed contextClosed primary ambiguousQualifierFailure cleanup');
    assert.ok(CASES.includes(c.name));
    assert.ok(['failed', 'inconclusive_sampling_budget', 'provisional_incomplete_collector_export_pending'].includes(c.qualifierStatus));
    assert.ok(['failed', 'unsampled', 'sampled_already_ready', 'provisional_incomplete_collector_export_pending'].includes(c.trialStatus));
    assert.ok(c.sampled === null || typeof c.sampled === 'boolean');
    for (const key of ['reportCaptured', 'bodyReceived', 'bodyMatches', 'server204Attempted', 'serverFinished', 'client204', 'page204', 'context204', 'pagehideTrigger', 'deliveryConfirmed', 'contextClosed']) assert.equal(typeof c[key], 'boolean');
    assert.ok(Number.isInteger(c.pageStatusCount) && c.pageStatusCount >= 0 && c.pageStatusCount <= 2);
    assert.ok(Array.isArray(c.cleanup) && c.cleanup.length <= 4);
    for (const d of [c.primary, c.ambiguousQualifierFailure, ...c.cleanup].filter(d => d !== null)) {
      keys(d, 'phase cause location'); assert.ok(phases.includes(d.phase)); assert.ok(causes.includes(d.cause));
      if (d.location !== null) {
        keys(d.location, 'module_basename module_sha256 site');
        const l = d.location;
        if (l.module_basename === ownBasename) {
          assert.equal(l.module_sha256, ownHash); assert.ok(causes.slice(2).some(code => l.site === `stage2-${code}`));
        } else {
          const path = `ui/scripts/${l.module_basename}`;
          assert.ok(Object.hasOwn(MODULE_PINS, path)); assert.equal(l.module_sha256, MODULE_PINS[path]);
          assert.match(l.site, /^f16-(qualify-view-telemetry|view-telemetry-diagnostics)-L\d{1,4}-C\d{1,4}$/);
        }
      }
    }
  }
  return JSON.stringify(result, null, 2) + '\n';
}
