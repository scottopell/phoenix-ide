import { test, vi } from 'vitest';
import assert from 'node:assert/strict';
import { runInNewContext } from 'node:vm';
import { probeDocument, probeMatrix, runProbeCell } from './probe-view-keepalive.mjs';

test('synthetic producer initiates exact keepalive synchronously, bounded deadline and failure facts without private fields', async () => {
  const id = '00000000-0000-4000-8000-000000000001';
  const html = probeDocument(id); const script = html.match(/<script>([\s\S]*)<\/script>/)[1];
  const facts = []; const order = []; let fetchCall; let hide; let timeout; let cleared = 0;
  runInNewContext(script, { window: { probeFact: async event => { facts.push(event); order.push(event.kind); }, addEventListener: (_name, fn) => { hide = fn; } },
    performance: { now: () => 1 }, AbortController, Promise, Error,
    setTimeout: (fn, ms) => { assert.equal(ms, 3000); timeout = fn; return 1; }, clearTimeout: () => { cleared++; },
    fetch: (url, init) => { order.push('fetch'); fetchCall = { url, init }; return Promise.resolve({ status: 204 }); } });
  hide();
  assert.deepEqual(order, ['pagehide', 'dispatch', 'fetch', 'handler_return']);
  assert.equal(fetchCall.url, `/collect/${id}`);
  assert.equal(fetchCall.init.keepalive, true);
  assert.equal(fetchCall.init.signal.aborted, false);
  await Promise.resolve(); await Promise.resolve();
  assert.equal(cleared, 1); assert.equal(typeof timeout, 'function');
  assert.ok(facts.every(f => Object.keys(f).sort().join(',') === 'at,id,kind'));
  assert.throws(() => probeDocument('</script>PRIVATE'));
});

function fakeBrowser(mode) {
  const commands = []; const destinations = []; let closed = 0; let contextOptions; let binding;
  const callbacks = {}; const cdpEvents = {}; const pageEvents = {};
  const page = { on: (name, fn) => { pageEvents[name] = fn; }, waitForFunction: async () => {}, goto: async url => {
    destinations.push(url);
    if (mode === 'primary_cleanup') throw new Error('Cookie: PRIVATE');
    if (url.includes('/source/')) {
      const id = url.split('/').at(-1);
      binding({}, { id, kind: 'dispatch', at: 1, secret: 'PRIVATE' });
      const request = { method: () => 'POST', url: () => url.replace('/source/', '/collect/') };
      callbacks.requestfailed?.(request); pageEvents.requestfailed?.(request);
      cdpEvents['Network.requestWillBeSent']?.({ requestId: 'PRIVATE', request: { method: 'POST', url: request.url() } });
      cdpEvents['Network.loadingFailed']?.({ requestId: 'PRIVATE', canceled: true, errorText: 'PRIVATE' });
      if (mode === 'overflow') for (let n = 0; n < 80; n++) callbacks.requestfailed?.(request);
    }
  } };
  const context = { on: (name, fn) => { callbacks[name] = fn; }, exposeBinding: async (_name, fn) => { binding = fn; }, newPage: async () => page,
    newCDPSession: async () => ({ on: (name, fn) => { cdpEvents[name] = fn; }, send: async (name, options) => { commands.push({ name, options }); } }),
    close: async () => { closed++; if (mode === 'primary_cleanup') throw new Error('Cookie: PRIVATE'); callbacks.close?.(); } };
  return { browser: { newContext: async options => { contextOptions = options; return context; } }, commands, destinations, closed: () => closed, options: () => contextOptions };
}

test('four explicit cells have fresh empty contexts, exact emulation and departure with no native execution', async () => {
  assert.equal(probeMatrix.length, 4);
  const ids = new Set();
  for (const cell of probeMatrix) {
    const fake = fakeBrowser(); const ledger = new Map();
    const trial = await runProbeCell(fake.browser, 'http://127.0.0.1:1234', cell, ledger, { wait: async ms => { assert.equal(ms, 3500); } });
    ids.add(trial.id); assert.equal(ledger.size, 0); assert.equal(fake.closed(), 1);
    assert.deepEqual(fake.options(), { storageState: { cookies: [], origins: [] }, serviceWorkers: 'block' });
    assert.equal(fake.commands[1].options.latency, cell.latency);
    assert.equal(fake.destinations[1], cell.departure === 'blank' ? 'about:blank' : 'http://127.0.0.1:1234/destination');
    assert.equal(trial.failure, null); assert.equal(trial.observer_delivery_complete, false);
    assert.equal(JSON.stringify(trial).includes('PRIVATE'), false);
    assert.ok(trial.facts.some(f => f.kind === 'context_requestfailed'));
    assert.ok(trial.facts.some(f => f.kind === 'cdp_failed' && f.canceled));
  }
  assert.equal(ids.size, 4);
});

test('primary and cleanup are independently sanitized, overflow fails closed at64 records', async () => {
  for (const mode of ['primary_cleanup', 'overflow']) {
    const fake = fakeBrowser(mode);
    const result = await runProbeCell(fake.browser, 'http://127.0.0.1:1234', probeMatrix[0], new Map(), { wait: async () => {} });
    assert.equal(JSON.stringify(result).includes('PRIVATE'), false);
    assert.ok(result.failure); assert.ok(result.facts.length <= 64);
    if (mode === 'primary_cleanup') assert.equal(result.cleanup_failure, 'probe context cleanup failed');
    else assert.equal(result.overflow, true);
  }
});

test('no off-host or unbounded matrix inputs admitted', async () => {
  for (const base of ['http://remote.invalid', 'http://secret@127.0.0.1:1234', 'http://127.0.0.1:1234/?secret=1']) {
    await assert.rejects(runProbeCell({}, base, probeMatrix[0], new Map()));
  }
  await assert.rejects(runProbeCell({}, 'http://127.0.0.1:1234', { latency: 900, departure: 'blank' }, new Map()));
});

test('stalled binding times out, preserves bounded failure and cannot admit a page after late resolution', async () => {
  vi.useFakeTimers();
  let release; let pages = 0; let closed = 0;
  const stalled = new Promise(resolve => { release = resolve; });
  const context = { on: () => {}, exposeBinding: () => stalled,
    newPage: async () => { pages++; throw new Error('late page'); }, close: async () => { closed++; } };
  try {
    const operation = runProbeCell({ newContext: async () => context }, 'http://127.0.0.1:1234', probeMatrix[0], new Map());
    await vi.advanceTimersByTimeAsync(10000);
    const result = await operation;
    assert.equal(result.failure, 'probe cell operation failed'); assert.equal(closed, 1); assert.equal(pages, 0);
    release(); await vi.advanceTimersByTimeAsync(0);
    assert.equal(pages, 0); assert.equal(closed, 1);
  } finally { vi.useRealTimers(); }
});

test.each(['enable', 'destination'])('late %s completion after deadline cannot start another operation', async mode => {
  vi.useFakeTimers();
  let release; let emulations = 0; let waits = 0; let closed = 0; let gotos = 0;
  const stalled = new Promise(resolve => { release = resolve; });
  const context = { on: () => {}, exposeBinding: async () => {}, close: async () => { closed++; },
    newPage: async () => ({ on: () => {}, waitForFunction: async () => {}, goto: async () => { if (++gotos === 2 && mode === 'destination') await stalled; } }),
    newCDPSession: async () => ({ on: () => {}, send: async name => {
      if (name === 'Network.enable' && mode === 'enable') await stalled;
      if (name === 'Network.emulateNetworkConditions') emulations++;
    } }) };
  try {
    const operation = runProbeCell({ newContext: async () => context }, 'http://127.0.0.1:1234', probeMatrix[0], new Map(), { wait: async () => { waits++; } });
    await vi.advanceTimersByTimeAsync(10000);
    const result = await operation;
    assert.equal(result.failure, 'probe cell operation failed'); assert.equal(closed, 1);
    release(); await vi.advanceTimersByTimeAsync(0);
    assert.equal(emulations, mode === 'enable' ? 0 : 1); assert.equal(waits, 0);
  } finally { vi.useRealTimers(); }
});

test.each(['stalled', 'rejected'])('late context cleanup %s is bounded and cannot rewrite pending receipt as clean', async mode => {
  vi.useFakeTimers();
  let created; let closes = 0; let pages = 0;
  const creation = new Promise(resolve => { created = resolve; });
  try {
    const operation = runProbeCell({ newContext: () => creation }, 'http://127.0.0.1:1234', probeMatrix[0], new Map());
    await vi.advanceTimersByTimeAsync(10000);
    const result = await operation;
    assert.equal(result.failure, 'probe cell operation failed');
    assert.equal(result.context_cleanup, 'pending_or_unknown');
    const snapshot = JSON.stringify(result);
    created({ close: async () => { closes++; if (mode === 'rejected') throw new Error('PRIVATE'); await new Promise(() => {}); }, newPage: async () => { pages++; } });
    await vi.advanceTimersByTimeAsync(2000);
    assert.equal(closes, 1); assert.equal(pages, 0);
    assert.equal(JSON.stringify(result), snapshot);
    assert.equal(vi.getTimerCount(), 0);
  } finally { vi.useRealTimers(); }
});
