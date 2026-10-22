// OFFLINE UNIT ONLY: in-memory harness proposal, never an admitted runtime module.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runLifecycleQualificationStage as baseline } from './qualify-view-telemetry.mjs';
import { captureDiagnostic, samplingDecision } from './view-telemetry-diagnostics.mjs';

const sourceURL = new URL('./qualify-view-telemetry.mjs', import.meta.url);
const source = readFileSync(sourceURL, 'utf8');
const sha256 = text => createHash('sha256').update(text).digest('hex');
assert.equal(sha256(source), '1c702c78bb06f74f3dae4c8f97d1932fc308959596f3602375bc65d7d99fb9e1');
const patch = readFileSync(new URL('../../docs/qa/stock-f16-lifecycle-close-drain.proposed.patch', import.meta.url), 'utf8');
assert.equal((patch.match(/^@@ /gm) ?? []).length, 1);
const hunk = patch.slice(patch.indexOf('\n@@ ')).split('\n').slice(2).filter(Boolean);
assert.ok(hunk.every(line => line.startsWith(' ') || line.startsWith('+')));
const before = hunk.filter(line => line.startsWith(' ')).map(line => line.slice(1)).join('\n');
const after = hunk.map(line => line.slice(1)).join('\n');
assert.equal(source.split(before).length, 2, 'proposal must apply to one exact original hunk');
const proposedSource = source.replace(before, after);
const diagnosticURL = new URL('./view-telemetry-diagnostics.mjs', import.meta.url).href;
const proposed = await import(`data:text/javascript;base64,${Buffer.from(proposedSource.replace("'./view-telemetry-diagnostics.mjs'", JSON.stringify(diagnosticURL))).toString('base64')}`);

const baseURL = 'http://127.0.0.1:8123';
const fixture = { route: '/c/product', productConversationId: 'product', spaControl: { kind: 'sidebar_home' } };
const flush = async () => { for (let turn = 0; turn < 80; turn++) await Promise.resolve(); };

// Deliberately fake Playwright, not a Phoenix producer or collector. Unlike the
// older fixture, POST and response are separate events on separate task turns.
function readinessFirstBrowser(mode = 'next_task_204') {
  const actions = [], trials = [];
  const primary = new Error('UNIT primary navigation failure');
  let contexts = 0;
  const browser = { newContext: async () => {
    const ordinal = ++contexts;
    const id = `40000000-0000-4000-8000-${String(ordinal).padStart(12, '0')}`;
    const requestId = `20000000-0000-4000-8000-${String(ordinal).padStart(12, '0')}`;
    const listeners = new Map();
    const emit = (event, value) => { for (const fn of [...(listeners.get(event) ?? [])]) fn(value); };
    let binding, contextClosed, url = 'about:blank';
    const request = { url: () => `${baseURL}/api/product-conversations/product/history?open_id=${requestId}`, method: () => 'GET', timing: () => ({ responseEnd: -1 }) };
    const report = () => ({ frontend_source: 'b'.repeat(40), frontend_source_dirty: false, presentation_evidence: 'commit_frame_estimate',
      open_id: mode === 'wrong_owner' ? '90000000-0000-4000-8000-000000000001' : id,
      product_conversation_id: 'product', route: 'ordinary', trigger: 'navigation', outcome: 'incomplete', cache: 'network',
      visible: false, background_ms: 0, total_ms: 5, marks: {}, correlations: [{ kind: 'request', id: requestId }] });
    const post = () => {
      const req = { url: () => `${baseURL}/api/telemetry/conversation-view`, method: () => 'POST', postData: () => JSON.stringify(report()) };
      actions.push([ordinal, 'POST']); emit('request', req);
      const respond = () => {
        actions.push([ordinal, 'response']);
        emit('response', { request: () => mode === 'wrong_response_request' ? { ...req } : req, status: () => mode === 'non204' ? 400 : 204 });
      };
      if (mode === 'sync_204') respond();
      else if (mode !== 'never_204') setTimeout(respond, 0);
    };
    const page = {
      on: (event, fn) => { if (!listeners.has(event)) listeners.set(event, new Set()); listeners.get(event).add(fn); },
      off: (event, fn) => listeners.get(event)?.delete(fn), url: () => url, mainFrame: () => ({}),
      evaluate: async () => undefined,
      goto: target => new Promise((resolve, reject) => {
        url = target;
        // The ready promise settles before this explicit fake task admits the
        // producer/finite GET; navigation still waits for diagnostic delivery.
        setTimeout(() => {
          if (mode === 'dual_failure') { reject(primary); return; }
          actions.push([ordinal, 'admission']);
          binding({}, { version: 1, kind: 'admission', route: 'ordinary', sampled: mode !== 'unsampled', open_id: mode === 'unsampled' ? null : id });
          if (mode !== 'unsampled') binding({}, { version: 1, kind: 'request', open_id: id, request_id: requestId });
          emit('request', request); resolve();
        }, 0);
      }),
      waitForFunction: async () => { actions.push([ordinal, 'ready']); },
    };
    const owned = {
      exposeBinding: async (_name, fn) => { binding = fn; }, addInitScript: async () => {}, newPage: async () => page,
      newCDPSession: async () => ({ send: async () => {} }), on: (event, fn) => { if (event === 'close') contextClosed = fn; },
      close: async () => {
        actions.push([ordinal, 'close']);
        if (mode === 'dual_failure') throw new Error('UNIT secondary close failure');
        if (!['no_post', 'unsampled'].includes(mode)) { post(); if (mode === 'duplicate') post(); }
        contextClosed?.();
      },
    };
    return owned;
  } };
  return { browser, actions, trials, primary, count: () => contexts,
    onTrial: trial => trials.push(structuredClone(trial)) };
}

async function execute(t, runStage, mode, budgetMs = 90000) {
  t.mock.timers.enable({ apis: ['Date', 'setTimeout'], now: 0 });
  const fake = readinessFirstBrowser(mode);
  let settled = false, outcome;
  const operation = runStage({ browser: fake.browser, baseURL, storageState: {}, fixture, budgetMs, onTrial: fake.onTrial })
    .then(stage => { settled = true; outcome = { stage }; }, error => { settled = true; outcome = { error }; });
  await flush();
  // Bounded fake time only: no wall-clock sleep, HTTP, browser or output files.
  for (let task = 0; task < 600 && !settled; task++) {
    t.mock.timers.tick(20); await flush();
  }
  assert.ok(settled, 'must finish inside 12000 fake ms / 600 explicit tasks');
  await operation;
  return { ...fake, ...outcome, elapsed: Date.now() };
}

test('baseline same fixture passes when POST and 204 are synchronous (old fixture blind spot)', async t => {
  const run = await execute(t, baseline, 'sync_204');
  assert.ifError(run.error); assert.equal(run.trials.length, 6);
  assert.ok(run.trials.every(trial => trial.sampled && trial.reports.length === 1 && trial.statuses.length === 1));
});

test('RED exact f16: first POST at close + next-task 204 fails immediate final drain', async t => {
  const run = await execute(t, baseline, 'next_task_204');
  assert.equal(run.error?.code, 'ERR_ASSERTION');
  assert.match(run.error.message, /final report-response drain/);
  assert.match(run.error.stack, /qualify-view-telemetry\.mjs:784:20/);
  assert.equal(run.count(), 1);
  assert.equal(run.trials[0].sampled, true);
  assert.equal(run.trials[0].status, 'inconclusive_readiness_preceded_finite_request_sampling_unknown');
  assert.equal(run.trials[0].reports.length, 1);
  assert.equal(run.trials[0].statuses.length, 0, 'snapshot at failure, not a later mutated trial');
  assert.equal(run.trials[0].cleanup_failure, run.trials[0].failure);
  assert.deepEqual(run.actions.slice(0, 4).map(action => action[1]), ['ready', 'admission', 'close', 'POST']);
  t.mock.timers.tick(0); await flush();
  assert.equal(run.actions.at(-1)[1], 'response', '204 actually arrives on the next fake task, after failure');
});

test('GREEN proposal only: same real lifecycle function drains post-close response without upgrading inconclusive', async t => {
  const run = await execute(t, proposed.runLifecycleQualificationStage, 'next_task_204');
  assert.ifError(run.error);
  assert.equal(run.count(), 6);
  assert.equal(run.stage.trials.length, 6);
  for (const trial of run.trials) {
    assert.equal(trial.sampled, true);
    assert.equal(trial.status, 'inconclusive_readiness_preceded_finite_request_sampling_unknown');
    assert.equal(trial.reports.length, 1); assert.equal(trial.statuses.length, 1);
    assert.equal(trial.statuses[0].status, 204);
    assert.equal(trial.statuses[0].open_id, trial.admitted_open_id);
    assert.ok(!trial.failure && !trial.cleanup_failure);
  }
  for (let ordinal = 1; ordinal <= 6; ordinal++) {
    assert.deepEqual(run.actions.filter(action => action[0] === ordinal).map(action => action[1]), ['ready', 'admission', 'close', 'POST', 'response']);
  }
});

for (const mode of ['never_204', 'duplicate', 'wrong_owner', 'wrong_response_request', 'non204']) {
  test(`proposal still rejects ${mode} with trial inventory`, async t => {
    const run = await execute(t, proposed.runLifecycleQualificationStage, mode);
    assert.equal(run.error?.code, 'ERR_ASSERTION');
    assert.equal(run.count(), 1); assert.equal(run.trials.length, 1);
    assert.equal(run.trials[0].sampled, true); assert.ok(run.trials[0].failure);
    assert.equal(run.trials[0].reports.length, mode === 'duplicate' ? 2 : 1);
    if (['never_204', 'wrong_response_request'].includes(mode)) {
      assert.match(run.error.message, /final report-response drain/);
      assert.equal(run.trials[0].statuses.length, 0);
      assert.ok(run.elapsed >= 5000 && run.elapsed <= 5100, 'bounded existing five-second drain');
    }
    if (mode === 'wrong_owner') assert.match(run.error.message, /terminal report must belong to admitted journey/);
  });
}

for (const [name, runStage] of [['baseline', baseline], ['proposal', proposed.runLifecycleQualificationStage]]) {
  for (const mode of ['no_post', 'unsampled']) test(`${name}: ${mode} stays inconclusive without reports`, async t => {
    const run = await execute(t, runStage, mode);
    assert.ifError(run.error); assert.equal(run.trials.length, 6);
    assert.ok(run.trials.every(trial => trial.sampled === (mode !== 'unsampled') && !trial.reports.length && !trial.statuses.length && trial.status.startsWith('inconclusive')));
  });
  test(`${name}: primary Error identity survives distinct cleanup failure`, async t => {
    const run = await execute(t, runStage, 'dual_failure');
    assert.equal(run.error, run.primary);
    assert.equal(run.trials[0].failure, 'UNIT primary navigation failure');
    assert.equal(run.trials[0].cleanup_failure, 'UNIT secondary close failure');
  });
}

test('proposal post-close drain respects the existing absolute stage deadline', async t => {
  const run = await execute(t, proposed.runLifecycleQualificationStage, 'never_204', 100);
  assert.match(run.error?.message ?? '', /final report-response drain/);
  assert.ok(run.elapsed <= 120);
});

test('proposal is exactly three added lines; trusted original is not edited', () => {
  assert.equal(hunk.filter(line => line.startsWith('+')).length, 3);
  assert.equal(readFileSync(sourceURL, 'utf8'), source);
  assert.equal(proposedSource.split('\n').length, source.split('\n').length + 3);
});

test('sampling selector cannot turn missing/ambiguous/foreign evidence into false or pick the first admission', () => {
  const unsampled = captureDiagnostic({ version: 1, kind: 'admission', route: 'ordinary', sampled: false, open_id: null });
  const sampled = captureDiagnostic({ version: 1, kind: 'admission', route: 'ordinary', sampled: true, open_id: '40000000-0000-4000-8000-000000000001' });
  assert.equal(samplingDecision([unsampled], 'ordinary'), unsampled);
  assert.equal(samplingDecision([sampled], 'ordinary'), sampled);
  assert.throws(() => samplingDecision([], 'ordinary'), /exactly one producer admission/);
  assert.throws(() => samplingDecision([unsampled, sampled], 'ordinary'), /exactly one producer admission/);
  assert.throws(() => samplingDecision([unsampled], 'global'));
  assert.throws(() => samplingDecision([unsampled, { version: 1, kind: 'request', open_id: sampled.open_id }], 'ordinary'), /producer evidence must belong/);
});
