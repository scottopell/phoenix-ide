import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import { CASES, EVENTS, ERRORS, signal, deadline, recorder, payloadOK, observeRequests,
  classify, releaseResponse, runCase, runProbe, validateResult, functionalPass, safeJSON } from './context-close-probe.mjs';

const runId = '11111111-1111-4111-8111-111111111111';
const caseId = '22222222-2222-4222-8222-222222222222';
const provenance = { node: 'v26.8.2', playwright: '1.61.0', lockSha256: 'a'.repeat(64),
  browserRevision: '1234', sourceCommit: 'b'.repeat(40) };
const packet = cases => ({ version: 1, runId, provenance: { ...provenance }, cases,
  suiteError: null, cleanupErrors: [] });

// Only fake objects. No HTTP listener, Playwright import, browser, GET or network.
function rig(mode = 'normal') {
  let record, collector, request, closeCalls = 0, stopCalls = 0;
  const received = signal(), terminal = signal(), browserEntered = signal(), browserBarrier = signal();
  const context = new EventEmitter(), page = new EventEmitter();
  context.close = () => {
    closeCalls += 1;
    if (mode === 'cleanup_failure') return Promise.reject(new Error('PRIVATE cleanup stack/URL'));
    if (mode === 'close_hang') return new Promise(() => {});
    if (mode === 'instant_close') return Promise.resolve();
    return new Promise(resolve => setImmediate(() => {
      if (request && !record.result.facts.responseJoined) {
        if (!['no_failure_event', 'never_deferred_failure'].includes(mode)) context.emit('requestfailed', request);
        if (mode !== 'response_after') {
          record.result.facts.serverClosed = true; record.event('server_response_close'); terminal.resolve();
        }
      }
      resolve();
    }));
  };
  Object.assign(page, {
    goto: async () => {},
    evaluate: async (_fn, payload) => {
      assert.match(_fn.toString(), /keepalive: true/);
      assert.deepEqual(Object.keys(payload).sort(), ['caseId', 'runId', 'version']);
      if (mode === 'no_request') return;
      request = { method: () => 'POST', url: () => `${collector.origin}/collect`, postData: () => JSON.stringify(payload) };
      if (mode === 'cleanup_failure') request.url = () => 'PRIVATE incorrect URL';
      context.emit('request', request);
      if (mode === 'duplicate') context.emit('request', request);
      record.result.facts.collectorOwned = true; record.event('collector_received'); received.resolve();
    },
  });
  context.newPage = async () => page;
  const emitResponse = (target, wrong = false) => target.emit('response', {
    request: () => wrong ? { ...request } : request, status: () => mode === 'non204' ? 500 : 204 });
  let browserCloses = 0;
  const browser = { newContext: async () => context, close: async () => {
    browserCloses++;
    browserEntered.resolve();
    if (mode === 'gated_browser_close') {
      await browserBarrier.promise; emitResponse(context); emitResponse(page);
    }
    if (mode === 'browser_close_hang') await new Promise(() => {});
    if (mode === 'browser_close_failure' || mode === 'cleanup_failure') throw new Error('PRIVATE browser cleanup');
    if (mode === 'deferred_both' || mode === 'deferred_context') emitResponse(context);
    if (mode === 'deferred_both' || mode === 'deferred_page') emitResponse(page);
    if (mode === 'never_deferred_page') emitResponse(page);
    if (mode === 'never_deferred_failure') context.emit('requestfailed', request);
  } };
  return {
    createBrowser: async () => browser, context, page, emitResponse, browserEntered, browserBarrier,
    collectorFactory: async args => {
      record = args.record;
      collector = { origin: 'http://synthetic.invalid', received: received.promise, terminal: terminal.promise,
        release() {
          releaseResponse({ destroyed: record.result.facts.serverClosed, socket: { writable: true },
            end: callback => callback(record.result.facts.serverClosed ? new Error('PRIVATE callback') : undefined),
          }, record);
          if (record.result.facts.writeAvailable) {
            record.result.facts.serverFinished = true; record.event('server_response_finish');
            record.result.facts.serverClosed = true; record.event('server_response_close'); terminal.resolve();
            if (mode !== 'page_only' && mode !== 'deferred_context' && mode !== 'deferred_both' && mode !== 'gated_browser_close') emitResponse(context, mode === 'wrong_response');
            if (mode !== 'context_only' && mode !== 'deferred_page' && mode !== 'deferred_both' && mode !== 'gated_browser_close') emitResponse(page, mode === 'wrong_page_response');
          }
        },
        disposeHeld() {
          record.result.facts.forcedSocketDisposal = true;
          if (!record.result.facts.serverClosed) {
            record.result.facts.serverClosed = true; record.event('server_response_close'); terminal.resolve();
          }
        },
        stop: async () => {
          stopCalls += 1;
          if (mode === 'cleanup_failure') throw new Error('PRIVATE server cleanup');
          if (mode === 'response_on_cleanup') context.emit('response', { request: () => request, status: () => 204 });
        },
      };
      return collector;
    },
    counts: () => ({ closeCalls, stopCalls, browserCloses }),
  };
}
const runFake = (caseName, mode, extra = {}) => runCase({ ...rig(mode), caseName, runId, caseId, ...extra });

for (const name of CASES) test(`fake choreography ${name}; not Playwright evidence`, async () => {
  const value = await runFake(name);
  assert.equal(value.primaryError, null);
  assert.deepEqual(value.cleanupErrors, []);
  assert.equal(value.facts.postCount, 1);
  assert.equal(value.facts.requestOwned, true);
  assert.equal(value.facts.collectorOwned, true);
  assert.equal(value.events.filter(e => e.kind === 'request_emit').length, 1);
  if (name === 'before') assert.equal(value.facts.responseJoined, true);
  if (name === 'during') assert.equal(value.facts.releaseWhileClosePending, true);
  if (name === 'after') assert.equal(value.facts.writeAvailable, false);
  if (name === 'never') {
    assert.equal(value.outcome, 'request_failed_observed');
    assert.equal(value.facts.forcedSocketDisposal, true);
    assert.equal(value.events.some(e => e.kind === 'response_release'), false);
  }
});

test('fake AFTER demonstrates reducer can retain a response after close resolves', async () => {
  const value = await runFake('after', 'response_after');
  assert.equal(value.primaryError, null);
  assert.equal(value.outcome, 'response_after_close');
  assert.equal(value.facts.responseJoined, true);
});

test('final reduction includes late callbacks delivered during cleanup', async () => {
  const value = await runFake('after', 'response_on_cleanup');
  assert.equal(value.primaryError, null);
  assert.equal(value.outcome, 'response_after_close');
});

for (const [mode, contextJoined, pageJoined] of [
  ['deferred_both', true, true], ['deferred_context', true, true], ['deferred_page', true, true],
  ['context_only', true, false], ['page_only', false, true],
]) test(`independent response listeners retained through browser close: ${mode}`, async () => {
  const fake = rig(mode);
  const value = await runCase({ ...fake, caseName: 'during', runId, caseId });
  assert.equal(value.primaryError, null);
  assert.equal(value.facts.responseJoined, contextJoined);
  assert.equal(value.facts.pageResponseJoined, pageJoined);
  const kinds = value.events.map(e => e.kind);
  assert.equal(kinds.at(-1), 'browser_close_resolve');
  for (const event of mode === 'deferred_both' ? ['client_response', 'page_response']
    : mode === 'deferred_context' ? ['client_response'] : mode === 'deferred_page' ? ['page_response'] : []) {
    assert.ok(kinds.indexOf(event) > kinds.indexOf('close_resolve'));
  }
  assert.equal(fake.context.listenerCount('response'), 0);
  assert.equal(fake.page.listenerCount('response'), 0);
  const before = structuredClone(value);
  fake.emitResponse(fake.context); fake.emitResponse(fake.page);
  assert.deepEqual(value, before); // Post-barrier emissions cannot mutate the finalized result.
});

test('finalization waits for actual owned browser.close resolution, not an event-loop turn', async () => {
  const fake = rig('gated_browser_close');
  let finalized = false;
  const pending = runCase({ ...fake, caseName: 'during', runId, caseId }).then(value => {
    finalized = true; return value;
  });
  await fake.browserEntered.promise;
  assert.equal(finalized, false);
  assert.equal(fake.context.listenerCount('response'), 1);
  assert.equal(fake.page.listenerCount('response'), 1);
  fake.browserBarrier.resolve();
  const value = await pending;
  assert.equal(value.primaryError, null);
  assert.equal(value.facts.responseJoined, true);
  assert.equal(value.facts.pageResponseJoined, true);
  assert.equal(value.events.at(-1).kind, 'browser_close_resolve');
});

for (const mode of ['context_only', 'page_only']) test(`BEFORE requires both listeners: ${mode}`, async () => {
  const value = await runFake('before', mode, { caseMs: 5, cleanupMs: 5 });
  assert.equal(value.primaryError, 'deadline');
  assert.equal(value.events.some(e => e.kind === 'close_begin'), false);
});

test('NEVER retains a failure delivered only during browser.close, then finalizes', async () => {
  const value = await runFake('never', 'never_deferred_failure');
  assert.equal(value.primaryError, null);
  assert.equal(value.outcome, 'request_failed_observed');
  assert.ok(value.events.findIndex(e => e.kind === 'request_failed') > value.events.findIndex(e => e.kind === 'close_resolve'));
  assert.equal(value.events.at(-1).kind, 'browser_close_resolve');
});

test('NEVER rejects a page response delivered during browser.close even without a context response', async () => {
  const value = await runFake('never', 'never_deferred_page');
  assert.equal(value.primaryError, 'unexpected_response');
  assert.equal(value.facts.responseJoined, false);
  assert.equal(value.facts.pageResponseJoined, true);
  assert.equal(value.events.at(-1).kind, 'browser_close_resolve');
});

for (const mode of ['browser_close_failure', 'browser_close_hang']) test(`browser barrier failure cannot qualify absence: ${mode}`, async () => {
  const value = await runFake('never', mode, { cleanupMs: 5 });
  assert.equal(value.facts.browserClosed, false);
  assert.deepEqual(value.cleanupErrors, ['browser_cleanup']);
  assert.equal(value.primaryError, 'missing_barrier');
  assert.doesNotMatch(safeJSON(packet([value])), /PRIVATE/);
});

test('end is attempted even when unwritable; callback, finish and close are distinct', () => {
  const record = recorder('after', runId, caseId);
  let callback, attempts = 0;
  const response = { destroyed: true, writableEnded: false, socket: { writable: false },
    end: done => { attempts++; callback = done; } };
  releaseResponse(response, record);
  assert.equal(attempts, 1);
  assert.equal(response.statusCode, 204);
  assert.equal(record.result.facts.writeAvailable, false);
  assert.equal(record.result.facts.responseEndAttempted, true);
  assert.equal(record.result.facts.responseEndCallback, false);
  assert.equal(record.result.facts.responseEndCallbackError, null);
  callback(new Error('PRIVATE callback detail'));
  assert.equal(record.result.facts.responseEndCallback, true);
  assert.equal(record.result.facts.responseEndCallbackError, true);
  assert.equal(record.result.facts.serverFinished, false);
  assert.equal(record.result.facts.serverClosed, false);
  assert.deepEqual(record.result.events.map(e => e.kind), ['response_release', 'response_end_attempt', 'response_end_callback']);
  assert.doesNotMatch(safeJSON(packet([record.result])), /PRIVATE/);
});

test('DURING fails if the close promise already resolved at release continuation', async () => {
  const value = await runFake('during', 'instant_close');
  assert.equal(value.facts.releaseWhileClosePending, false);
  assert.equal(value.primaryError, 'missing_barrier');
});

test('NEVER missing response is a classified observation, not by itself failure', async () => {
  const value = await runFake('never', 'no_failure_event');
  assert.equal(value.primaryError, null);
  assert.equal(value.outcome, 'no_response_observed');
});

for (const [mode, expected] of [['wrong_response', 'incorrect_owner'], ['wrong_page_response', 'incorrect_owner'], ['duplicate', 'multiple_post'], ['non204', 'unexpected_response']]) {
  test(`fake functional rejection: ${mode}`, async () => {
    const value = await runFake('during', mode);
    assert.equal(value.primaryError, expected);
    assert.equal(functionalPass(packet([value])), false);
  });
}

test('primary ownership error remains separate from all three cleanup failures; no raw error retained', async () => {
  const value = await runFake('never', 'cleanup_failure');
  assert.equal(value.primaryError, 'incorrect_owner');
  assert.deepEqual(value.cleanupErrors, ['context_cleanup', 'server_cleanup', 'browser_cleanup']);
  assert.doesNotMatch(safeJSON(packet([value])), /PRIVATE|URL|stack/);
});

test('case hang times out and still attempts context and server cleanup', async () => {
  const fake = rig('no_request');
  const value = await runCase({ ...fake, caseName: 'never', runId, caseId, caseMs: 5, cleanupMs: 5 });
  assert.equal(value.primaryError, 'deadline');
  assert.deepEqual(fake.counts(), { closeCalls: 1, stopCalls: 1, browserCloses: 1 });
});

test('held close hang and cleanup hang are bounded and classified separately', async () => {
  const value = await runFake('never', 'close_hang', { caseMs: 5, cleanupMs: 5 });
  assert.equal(value.primaryError, 'deadline');
  assert.deepEqual(value.cleanupErrors, ['context_cleanup']);
});

test('suite deadline has fixed error and independent browser cleanup', async () => {
  let closed = 0;
  const value = await runProbe({ createBrowser: async () => ({ close: async () => { closed++; throw new Error('PRIVATE'); } }),
    provenance, runId, suiteMs: 5, caseRunner: async ({ createBrowser }) => {
      await createBrowser(); return new Promise(() => {});
    } });
  assert.equal(value.suiteError, 'deadline');
  assert.equal(closed, 1);
  assert.deepEqual(value.cleanupErrors, ['browser_cleanup']);
  assert.doesNotMatch(safeJSON(value), /PRIVATE/);
});

test('suite stops after failed case cleanup rather than starting another context', async () => {
  let cases = 0, closed = 0;
  const fake = rig('cleanup_failure');
  const value = await runProbe({ createBrowser: async () => {
    const browser = await fake.createBrowser();
    return { newContext: () => browser.newContext(), close: async () => { closed++; await browser.close(); } };
  }, provenance, runId,
    caseRunner: args => { cases++; return runCase({ ...fake, ...args }); } });
  assert.equal(cases, 1);
  assert.equal(closed, 1);
  assert.equal(value.cases.length, 1);
  assert.equal(functionalPass(value), false);
});

test('deadline helper clears fake timers on success and rejects on expiry', async () => {
  let fire, cleared = 0;
  const timers = { setTimeout: fn => { fire = fn; return 7; }, clearTimeout: id => { assert.equal(id, 7); cleared++; } };
  assert.equal(await deadline(Promise.resolve(42), 10, timers), 42);
  const expired = deadline(new Promise(() => {}), 10, timers);
  fire(); await assert.rejects(expired, e => e.code === 'deadline');
  assert.equal(cleared, 2);
});

test('request identity join rejects a lookalike object, never reads failure text', () => {
  const context = new EventEmitter(), r = recorder('never', runId, caseId);
  const request = { method: () => 'POST', url: () => 'http://synthetic.invalid/collect',
    postData: () => JSON.stringify({ version: 1, runId, caseId }), failure: () => { throw new Error('must not read'); } };
  const observer = observeRequests(context, { origin: 'http://synthetic.invalid' }, r, runId, caseId);
  context.emit('request', request);
  context.emit('requestfailed', request);
  context.emit('response', { request: () => ({ ...request }), status: () => 204 });
  assert.equal(r.result.facts.failureJoined, true);
  assert.equal(r.result.facts.responseJoined, false);
  assert.equal(r.result.primaryError, 'incorrect_owner');
  observer.detach(); assert.equal(context.listenerCount('request'), 0);
});

test('payload shape and UUIDs are strict, extra content is not accepted', () => {
  const value = { version: 1, runId, caseId };
  assert.equal(payloadOK(value, runId, caseId), true);
  for (const bad of [null, [], { ...value, url: 'PRIVATE' }, { ...value, runId: 'PRIVATE' }, { ...value, version: 2 }]) {
    assert.equal(payloadOK(bad, runId, caseId), false);
  }
});

test('event reducer enforces monotonic bounds, finite enums, overflow and first error', () => {
  let time = 10;
  const r = recorder('never', runId, caseId, () => time);
  time = 11; r.event('request_emit'); time = 10; r.event('collector_received');
  r.fail('PRIVATE');
  assert.equal(r.result.primaryError, 'invalid_event');
  assert.equal(r.result.events.length, 1);
  const full = recorder('never', runId, caseId, () => 0);
  for (let n = 0; n < 25; n++) full.event('request_emit');
  assert.equal(full.result.events.length, 24);
  assert.equal(full.result.primaryError, 'event_overflow');
  const invalid = recorder('never', runId, caseId);
  invalid.event('PRIVATE event'); assert.equal(invalid.result.primaryError, 'invalid_event');
});

test('classify rejects missing barriers, and collector receipt is not a client response', () => {
  const r = recorder('after', runId, caseId);
  r.result.facts.collectorOwned = true; r.event('collector_received');
  classify(r);
  assert.equal(r.result.primaryError, 'missing_barrier');
  assert.equal(r.result.facts.responseJoined, false);
  assert.equal(r.result.facts.clientStatus, null);
});

test('schema/redaction rejects unknown properties, raw strings, invalid UUIDs and unbounded vectors', async () => {
  const value = packet([await runFake('never')]);
  assert.equal(validateResult(value), true);
  const mutations = [v => { v.raw = 'PRIVATE'; }, v => { v.provenance.path = 'PRIVATE'; },
    v => { v.provenance.browserRevision = '/PRIVATE/path'; }, v => { v.provenance.playwright = 'other'; },
    v => { v.cases[0].caseId = 'PRIVATE'; }, v => { v.cases[0].facts.url = 'PRIVATE'; },
    v => { v.cases[0].primaryError = 'PRIVATE raw error'; }, v => { v.cases[0].events[0].message = 'PRIVATE'; },
    v => { v.cases[0].events = Array(25).fill({ kind: 'request_emit', atMs: 0 }); },
    v => { v.cases[0].facts.postCount = 3; }, v => { v.cases[0].events[0].atMs = NaN; },
    v => { v.cases[0].cleanupErrors = ['PRIVATE']; }, v => { v.cleanupErrors = ['PRIVATE']; },
    v => { v.provenance.sourceCommit = [provenance.sourceCommit]; },
    v => { v.provenance.browserRevision = 1234; }, v => { v.runId = [runId]; },
    v => { v.provenance.node = `v${'1'.repeat(100)}.1.1`; }];
  for (const mutate of mutations) {
    const bad = structuredClone(value); mutate(bad);
    assert.equal(validateResult(bad), false);
    assert.throws(() => safeJSON(bad), e => e.code === 'invalid_payload');
  }
});

test('full fake suite passes only when all four distinct cases and valid barriers exist', async () => {
  let count = 0;
  let fake;
  const browsers = [];
  const value = await runProbe({ createBrowser: async () => { fake = rig(); browsers.push(fake); return fake.createBrowser(); }, provenance, runId,
    caseRunner: args => runCase({ ...args, collectorFactory: params => fake.collectorFactory(params),
      caseId: `22222222-2222-4222-8222-${String(++count).padStart(12, '0')}` }) });
  assert.equal(functionalPass(value), true);
  assert.equal(browsers.length, 4);
  assert.equal(new Set(browsers.map(b => b.context)).size, 4);
  assert.ok(browsers.every(b => b.counts().browserCloses === 1));
  const missing = structuredClone(value); missing.cases[0].events = [];
  assert.equal(functionalPass(missing), false);
  const duplicate = structuredClone(value); duplicate.cases[1].caseId = duplicate.cases[0].caseId;
  assert.equal(validateResult(duplicate), false);
});

test('library and CLI imports are inert (no Playwright resolution, autolaunch, output or server)', async () => {
  const before = process.getActiveResourcesInfo();
  const cli = await import('./context-close-probe.cli.mjs');
  assert.equal(typeof cli.main, 'function');
  assert.deepEqual(process.getActiveResourcesInfo(), before);
});

const workflow = readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8');
const job = workflow.split('  context-close-functional-probe:\n')[1].split('\n  qualify-history-realserver:')[0];
const stock = workflow.split('  qualify-history-realserver:\n')[1].split('\n  check-ui:')[0];
test('source guard: hosted probe requires exact owned same-repo PR + label, no opt-outs', () => {
  const gate = job.split('\n').find(line => line.startsWith('    if:'));
  assert.equal(gate, "    if: ${{ github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository && github.head_ref == 'prepare/exact-f16-stock-admission' && contains(github.event.pull_request.labels.*.name, 'offline-regression-only') }}");
  assert.match(job, /ref: \$\{\{ github.event.pull_request.head.sha \}\}/);
  assert.match(job, /persist-credentials: false/);
  assert.match(job, /uses: pnpm\/action-setup@v4\n        with:\n          package_json_file: ui\/package.json/);
  assert.match(job, /pnpm install --frozen-lockfile --ignore-scripts/);
  assert.match(job, /pnpm exec playwright install --with-deps chromium/);
  assert.match(job, /timeout --signal=TERM --kill-after=5s 65s node ui\/scripts\/context-close-probe.cli.mjs/);
  assert.match(job, /run: node --test ui\/scripts\/context-close-probe.test.mjs/);
  assert.doesNotMatch(job, /cargo|dev.py|history_opening|continue-on-error|workflow_dispatch|pull_request_target/);
  const paths = job.split('          path: |\n')[1].trim().split('\n').map(s => s.trim());
  assert.deepEqual(paths, ['context-close-evidence/result.json', 'docs/qa/context-close-probe.schema.json', 'docs/qa/context-close-probe.md']);
});

test('source guard: stock runtime negative-label predicate remains exact', () => {
  assert.equal(stock.split('\n').find(line => line.startsWith('    if:')),
    "    if: ${{ github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository && (github.head_ref == 'qualification/continuation-opening-codegen' || github.head_ref == 'integration/v013-history-current-main' || github.head_ref == 'prepare/exact-f16-stock-admission') && !contains(github.event.pull_request.labels.*.name, 'offline-regression-only') }}");
});

test('JSON schema and unexecuted template agree with the closed serializer', () => {
  const schema = JSON.parse(readFileSync(new URL('../../docs/qa/context-close-probe.schema.json', import.meta.url)));
  const template = JSON.parse(readFileSync(new URL('../../docs/qa/context-close-probe.template.json', import.meta.url)));
  assert.equal(validateResult(template), true);
  assert.equal(functionalPass(template), false);
  assert.equal(schema.additionalProperties, false);
  assert.deepEqual(schema.$defs.case.properties.case.enum, CASES);
  assert.deepEqual(schema.$defs.event.properties.kind.enum, EVENTS);
  assert.deepEqual(schema.$defs.error.enum, [null, ...ERRORS]);
  assert.equal(schema.$defs.case.properties.events.maxItems, 24);
  assert.deepEqual(Object.keys(schema.$defs.facts.properties).sort(), Object.keys(recorder('never', runId, caseId).result.facts).sort());
});
