import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';
import { performance } from 'node:perf_hooks';
import { pathToFileURL } from 'node:url';

export const probeMatrix = Object.freeze([0, 200].flatMap(latency => ['blank', 'same_origin'].map(departure => ({ latency, departure }))));

export function probeDocument(id) {
  assert.match(id, /^[a-f0-9-]{36}$/);
  return `<!doctype html><title>synthetic keepalive probe</title><script>
  const id = ${JSON.stringify(id)};
  let count = 0;
  const note = kind => { if (count++ < 12) void window.probeFact({ kind, id, at: performance.now() }).catch(() => {}); };
  window.addEventListener('pagehide', () => {
    note('pagehide');
    const controller = new AbortController();
    controller.signal.addEventListener('abort', () => note('signal_aborted'));
    const timer = setTimeout(() => { note('timeout_abort'); controller.abort(); }, 3000);
    let submission;
    try {
      note('dispatch');
      submission = fetch('/collect/' + id, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}', keepalive: true, signal: controller.signal });
    } catch { submission = Promise.reject(new Error('synthetic submission failed')); }
    void submission.then(() => note('resolved'), () => note('rejected')).finally(() => clearTimeout(timer));
    note('handler_return');
  }, { once: true });
  window.probeReady = true;
  </script>`;
}

function bound(promise, ms, label) {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(label)), ms); })]).finally(() => clearTimeout(timer));
}

export async function runProbeCell(browser, baseURL, cell, ledger, { wait = ms => new Promise(resolve => setTimeout(resolve, ms)), limit = bound } = {}) {
  const origin = new URL(baseURL);
  assert.ok(origin.protocol === 'http:' && origin.hostname === '127.0.0.1' && !origin.username && !origin.password && origin.pathname === '/' && !origin.search && !origin.hash);
  assert.ok(probeMatrix.some(value => value.latency === cell.latency && value.departure === cell.departure));
  const id = randomUUID();
  const start = performance.now();
  const trial = { id, ...cell, facts: [], failure: null, cleanup_failure: null, context_cleanup: 'pending_or_unknown', overflow: false, observer_delivery_complete: false };
  let recording = true;
  const record = (kind, extra = {}) => {
    if (!recording) return;
    if (trial.facts.length >= 64) { trial.overflow = true; return; }
    trial.facts.push({ ordinal: trial.facts.length, kind, ms: Math.round((performance.now() - start) * 1000) / 1000, ...extra });
  };
  ledger.set(id, record);
  let context;
  let canceled = false;
  const post = request => request.method() === 'POST' && request.url() === `${baseURL}/collect/${id}`;
  try {
    await limit((async () => {
      context = await browser.newContext({ storageState: { cookies: [], origins: [] }, serviceWorkers: 'block' });
      if (canceled) {
        try { await limit(context.close(), 2000, 'late probe context close deadline'); }
        catch { /* The returned trial retains pending_or_unknown; no false clean claim. */ }
        return;
      }
      for (const event of ['request', 'requestfinished', 'requestfailed']) context.on(event, request => { if (post(request)) record(`context_${event}`); });
      context.on('response', response => { if (post(response.request())) record('context_response', { status: response.status() }); });
      context.on('close', () => record('context_closed'));
      await context.exposeBinding('probeFact', (_source, event) => {
        const kinds = ['pagehide', 'dispatch', 'handler_return', 'timeout_abort', 'signal_aborted', 'resolved', 'rejected'];
        if (event?.id !== id || !kinds.includes(event.kind) || !Number.isFinite(event.at) || event.at < 0 || event.at > 60000) { record('invalid_producer_fact'); return; }
        record(`producer_${event.kind}`, { producer_ms: event.at });
      });
      if (canceled) return;
      const page = await context.newPage();
      if (canceled) return;
      const cdp = await context.newCDPSession(page);
      if (canceled) return;
      await cdp.send('Network.enable');
      if (canceled) return;
      await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: cell.latency, downloadThroughput: 1000000, uploadThroughput: 1000000 });
      if (canceled) return;
      for (const event of ['request', 'requestfinished', 'requestfailed']) page.on(event, request => { if (post(request)) record(`page_${event}`); });
      page.on('response', response => { if (post(response.request())) record('page_response', { status: response.status() }); });
      page.on('close', () => record('page_closed'));
      const requests = new Set();
      cdp.on('Network.requestWillBeSent', event => { if (event.request.url === `${baseURL}/collect/${id}` && event.request.method === 'POST') { if (requests.size >= 2) { trial.overflow = true; return; } requests.add(event.requestId); record('cdp_request'); } });
      cdp.on('Network.loadingFailed', event => { if (requests.has(event.requestId)) record('cdp_failed', { canceled: event.canceled === true }); });
      cdp.on('Network.loadingFinished', event => { if (requests.has(event.requestId)) record('cdp_finished'); });
      cdp.on('Network.responseReceived', event => { if (requests.has(event.requestId)) record('cdp_response', { status: event.response.status }); });
      await page.goto(`${baseURL}/source/${id}`, { timeout: 5000, waitUntil: 'load' });
      if (canceled) return;
      await page.waitForFunction(() => window.probeReady === true, undefined, { timeout: 1000 });
      if (canceled) return;
      record('navigation_start');
      await page.goto(cell.departure === 'blank' ? 'about:blank' : `${baseURL}/destination`, { timeout: 5000, waitUntil: 'load' });
      if (canceled) return;
      record('navigation_return');
      // Observe beyond the fixed producer abort deadline without replacing any delivery assertion.
      await wait(3500);
      record('observation_end');
    })(), 10000, 'probe cell deadline');
  } catch { trial.failure = 'probe cell operation failed'; }
  finally {
    canceled = true;
    if (context) {
      record('context_close_start');
      try {
        await limit(context.close(), 2000, 'probe context close deadline');
        trial.context_cleanup = 'close_returned';
      } catch {
        trial.context_cleanup = 'failed_or_unknown';
        trial.cleanup_failure = 'probe context cleanup failed';
      }
      record('context_close_return');
    }
    ledger.delete(id);
    recording = false;
  }
  if (trial.overflow) trial.failure ??= 'probe fact bound exceeded';
  return trial;
}

export async function runNativeProbe(chromium) {
  const ledger = new Map();
  const sockets = new Set();
  const server = createServer((request, response) => {
    const match = /^\/(source|collect)\/([a-f0-9-]{36})$/.exec(request.url ?? '');
    if (match?.[1] === 'source' && request.method === 'GET') { response.setHeader('Content-Type', 'text/html'); response.end(probeDocument(match[2])); return; }
    if (match?.[1] === 'collect' && request.method === 'POST') {
      const record = ledger.get(match[2]);
      record?.('server_received');
      let bytes = 0;
      request.on('data', chunk => { bytes += chunk.length; if (bytes > 16) request.destroy(); });
      request.on('aborted', () => record?.('server_request_aborted'));
      request.socket.once('close', () => record?.('server_connection_closed', { response_finished: response.writableFinished, body_completed: request.complete }));
      request.on('error', () => record?.('server_request_error'));
      response.on('finish', () => record?.('server_response_finished', { status: 204 }));
      response.on('close', () => record?.('server_response_closed', { finished: response.writableFinished }));
      request.on('end', () => { record?.('server_body_completed', { bytes }); response.writeHead(204); response.end(); });
      return;
    }
    if (request.method === 'GET' && request.url === '/destination') { response.end('<!doctype html><title>synthetic destination</title>'); return; }
    response.writeHead(404); response.end();
  });
  server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
  let browser;
  let stopped = false;
  const result = { schema_version: 1, scope: 'synthetic_direct_keepalive_isolation_not_app_qualification', trials: [], failure: null, cleanup_failure: null };
  try {
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
    const baseURL = `http://127.0.0.1:${server.address().port}`;
    browser = await bound(chromium.launch({ headless: true }).then(async created => {
      if (stopped) { await bound(created.close(), 3000, 'late probe browser cleanup'); throw new Error('probe launch canceled'); }
      return created;
    }), 10000, 'probe browser launch deadline');
    const version = browser.version();
    result.browser_version = /^[0-9.]{1,80}$/.test(version) ? version : 'unavailable';
    for (const cell of probeMatrix) result.trials.push(await runProbeCell(browser, baseURL, cell, ledger));
  } catch { result.failure = 'native probe operation failed'; }
  finally {
    stopped = true;
    if (browser) try { await bound(browser.close(), 3000, 'probe browser close deadline'); } catch { result.cleanup_failure = 'probe browser cleanup failed'; }
    for (const socket of sockets) socket.destroy();
    await new Promise(resolve => server.close(resolve));
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  assert.deepEqual(process.argv.slice(2), ['--run-native'], 'explicit native probe execution required');
  const { chromium } = await import('playwright');
  const result = await runNativeProbe(chromium);
  process.stdout.write(JSON.stringify(result, null, 2) + '\n');
  if (result.failure || result.cleanup_failure || result.trials.some(trial => trial.failure || trial.cleanup_failure)) process.exitCode = 1;
}
