import http from 'node:http';
import { randomUUID } from 'node:crypto';
import { performance } from 'node:perf_hooks';

export const CASES = ['before', 'during', 'after', 'never'];
export const EVENTS = ['request_emit', 'collector_received', 'close_begin', 'response_release',
  'server_response_finish', 'server_response_close', 'client_response', 'request_failed', 'close_resolve'];
export const ERRORS = ['deadline', 'incorrect_owner', 'multiple_post', 'invalid_payload',
  'event_overflow', 'invalid_event', 'unexpected_response', 'missing_barrier', 'operation_failed'];
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const matches = (pattern, value) => typeof value === 'string' && pattern.test(value);
export const signal = () => {
  let resolve;
  const promise = new Promise(r => { resolve = r; });
  return { promise, resolve };
};
export class ProbeError extends Error {
  constructor(code) { super('probe failure'); this.code = ERRORS.includes(code) ? code : 'operation_failed'; }
}
export function deadline(work, ms, timers = globalThis) {
  let timer;
  return Promise.race([Promise.resolve(work), new Promise((_, reject) => {
    timer = timers.setTimeout(() => reject(new ProbeError('deadline')), ms);
  })]).finally(() => timers.clearTimeout(timer));
}
export function payloadOK(value, runId, caseId) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).sort().join(',') === 'caseId,runId,version'
    && value.version === 1 && value.runId === runId && value.caseId === caseId
    && matches(UUID, runId) && matches(UUID, caseId);
}
export function recorder(caseName, runId, caseId, now = () => performance.now()) {
  if (!CASES.includes(caseName) || !matches(UUID, runId) || !matches(UUID, caseId)) throw new ProbeError('invalid_payload');
  const start = now();
  const result = { case: caseName, runId, caseId, primaryError: null, cleanupErrors: [], events: [],
    facts: { postCount: 0, requestOwned: false, collectorOwned: false, responseJoined: false,
      failureJoined: false, clientStatus: null, releaseWhileClosePending: false,
      writeAvailable: null, serverFinished: false, serverClosed: false, forcedSocketDisposal: false },
    outcome: 'not_observed' };
  const fail = code => { result.primaryError ??= ERRORS.includes(code) ? code : 'operation_failed'; };
  const event = kind => {
    if (!EVENTS.includes(kind)) { fail('invalid_event'); return; }
    if (result.events.length === 24) { fail('event_overflow'); return; }
    const atMs = now() - start;
    if (!Number.isFinite(atMs) || atMs < (result.events.at(-1)?.atMs ?? 0) || atMs > 70000) {
      fail('invalid_event'); return;
    }
    result.events.push({ kind, atMs });
  };
  return { result, fail, event };
}

// Internal addresses and request objects never cross the result boundary.
export async function loopbackCollector({ runId, caseId, record }) {
  const received = signal(), terminal = signal();
  const sockets = new Set();
  let response, postCount = 0, stopped = false;
  const server = http.createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/') {
      res.setHeader('Content-Type', 'text/html');
      res.end('<!doctype html><title>Synthetic close probe</title>');
      return;
    }
    if (req.method === 'GET' && req.url === '/favicon.ico') { res.writeHead(204).end(); return; }
    if (req.method !== 'POST' || req.url !== '/collect') {
      record.fail('incorrect_owner'); res.writeHead(404).end(); return;
    }
    postCount += 1;
    if (postCount !== 1) { record.fail('multiple_post'); res.writeHead(409).end(); return; }
    response = res;
    res.on('finish', () => {
      record.result.facts.serverFinished = true; record.event('server_response_finish'); terminal.resolve();
    });
    res.on('close', () => {
      record.result.facts.serverClosed = true; record.event('server_response_close'); terminal.resolve();
    });
    let body = '', bytes = 0;
    req.on('error', () => { record.fail('operation_failed'); received.resolve(); });
    req.on('data', chunk => {
      bytes += chunk.length;
      if (bytes > 256) { record.fail('invalid_payload'); req.destroy(); received.resolve(); }
      else body += chunk.toString('utf8');
    });
    req.on('end', () => {
      let valid = false;
      try { valid = payloadOK(JSON.parse(body), runId, caseId); } catch { /* fixed error below */ }
      if (!valid) record.fail('invalid_payload');
      record.result.facts.collectorOwned = valid;
      record.event('collector_received'); received.resolve();
    });
  });
  server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
  await new Promise((resolve, reject) => {
    server.once('error', reject); server.listen(0, '127.0.0.1', resolve);
  });
  server.on('error', () => record.fail('operation_failed'));
  const origin = `http://127.0.0.1:${server.address().port}`;
  return { origin, received: received.promise, terminal: terminal.promise,
    release() {
      if (!response) throw new ProbeError('missing_barrier');
      record.event('response_release');
      record.result.facts.writeAvailable = !response.destroyed && !response.writableEnded && !!response.socket?.writable;
      if (record.result.facts.writeAvailable) response.writeHead(204).end();
    },
    disposeHeld() {
      record.result.facts.forcedSocketDisposal = true;
      response?.destroy();
      for (const socket of sockets) socket.destroy();
    },
    async stop() {
      if (stopped) return;
      stopped = true;
      const closed = new Promise(resolve => server.close(resolve));
      for (const socket of sockets) socket.destroy();
      await closed;
    },
  };
}

export function observeRequests(context, collector, record, runId, caseId) {
  const observed = signal(), client = signal();
  let owner;
  const onRequest = request => {
    if (request.method() !== 'POST') return;
    record.result.facts.postCount = Math.min(2, record.result.facts.postCount + 1);
    if (record.result.facts.postCount !== 1) { record.fail('multiple_post'); return; }
    owner = request;
    let valid = false;
    try { valid = payloadOK(JSON.parse(request.postData()), runId, caseId); } catch { /* fixed error below */ }
    const owned = valid && request.url() === `${collector.origin}/collect`;
    record.result.facts.requestOwned = owned;
    if (!owned) record.fail('incorrect_owner');
    record.event('request_emit'); observed.resolve();
  };
  const onResponse = response => {
    const request = response.request();
    if (request.method() !== 'POST') return;
    if (request !== owner) { record.fail('incorrect_owner'); return; }
    if (record.result.facts.responseJoined) { record.fail('unexpected_response'); return; }
    record.result.facts.responseJoined = true;
    const status = response.status();
    record.result.facts.clientStatus = status === 204 ? 204 : 'other';
    if (status !== 204) record.fail('unexpected_response');
    record.event('client_response'); client.resolve();
  };
  const onFailed = request => {
    if (request.method() !== 'POST') return;
    if (request !== owner) { record.fail('incorrect_owner'); return; }
    if (record.result.facts.failureJoined) { record.fail('invalid_event'); return; }
    record.result.facts.failureJoined = true;
    record.event('request_failed');
  };
  context.on('request', onRequest); context.on('response', onResponse); context.on('requestfailed', onFailed);
  return { observed: observed.promise, client: client.promise, detach() {
    context.off('request', onRequest); context.off('response', onResponse); context.off('requestfailed', onFailed);
  } };
}

export function classify(record) {
  const { result } = record, { facts, events } = result;
  const index = kind => events.findIndex(e => e.kind === kind);
  const has = kind => index(kind) >= 0;
  if (facts.postCount !== 1 || !facts.requestOwned || !facts.collectorOwned
      || !has('request_emit') || !has('collector_received') || !has('close_resolve')
      || index('close_begin') <= Math.max(index('request_emit'), index('collector_received'))
      || index('close_resolve') <= index('close_begin')) record.fail('missing_barrier');
  if (result.case === 'before' && (!facts.responseJoined || facts.clientStatus !== 204
      || index('response_release') >= index('client_response')
      || index('client_response') >= index('close_begin'))) record.fail('missing_barrier');
  if (result.case === 'during' && (!facts.releaseWhileClosePending
      || index('response_release') <= index('close_begin')
      || index('response_release') >= index('close_resolve'))) record.fail('missing_barrier');
  if (result.case === 'after' && index('response_release') <= index('close_resolve')) record.fail('missing_barrier');
  if (result.case === 'never' && (has('response_release') || facts.responseJoined || facts.serverFinished
      || !facts.forcedSocketDisposal)) record.fail('unexpected_response');
  if (facts.responseJoined !== has('client_response') || facts.failureJoined !== has('request_failed')
      || facts.serverFinished !== has('server_response_finish') || facts.serverClosed !== has('server_response_close')
      || (facts.responseJoined && (facts.clientStatus !== 204 || !has('response_release')
        || index('client_response') <= index('response_release')))) record.fail('unexpected_response');
  result.outcome = facts.responseJoined
    ? (index('client_response') > index('close_resolve') ? 'response_after_close' : 'response_before_close_resolve')
    : (facts.failureJoined ? 'request_cancelled' : 'no_response_observed');
  return result;
}

// Injection is for compiler-free choreography tests, not evidence of browser ordering.
export async function runCase({ browser, caseName, runId, caseId = randomUUID(),
  collectorFactory = loopbackCollector, caseMs = 10000, cleanupMs = 1000 }) {
  const record = recorder(caseName, runId, caseId);
  let collector, context, observer, closePromise, cancelled = false;
  const guard = () => { if (cancelled) throw new ProbeError('deadline'); };
  const startClose = () => {
    record.event('close_begin');
    let pending = true;
    closePromise = Promise.resolve(context.close()).then(() => {
      pending = false; record.event('close_resolve');
    });
    // Immediately attach a rejection handler even when release precedes awaiting close.
    closePromise.catch(() => {});
    return () => pending;
  };
  const work = (async () => {
    collector = await collectorFactory({ runId, caseId, record });
    if (cancelled) { await deadline(collector.stop(), cleanupMs); guard(); }
    context = await browser.newContext();
    if (cancelled) { await deadline(context.close(), cleanupMs); guard(); }
    observer = observeRequests(context, collector, record, runId, caseId);
    const page = await context.newPage(); guard();
    await page.goto(collector.origin); guard();
    await page.evaluate(payload => {
      // Do not await fetch: its response is deliberately held by the collector.
      void fetch('/collect', { method: 'POST', body: JSON.stringify(payload) }).catch(() => {});
    }, { version: 1, runId, caseId }); guard();
    await Promise.all([collector.received, observer.observed]); guard();
    if (record.result.primaryError) throw new ProbeError(record.result.primaryError);
    if (caseName === 'before') {
      collector.release(); await observer.client; guard(); startClose(); await closePromise;
    } else if (caseName === 'during') {
      const pending = startClose();
      await Promise.resolve(); guard(); // explicit continuation, not a timed delay
      record.result.facts.releaseWhileClosePending = pending();
      collector.release(); await closePromise;
    } else {
      startClose(); await closePromise; guard();
      if (caseName === 'after') collector.release();
      else collector.disposeHeld();
    }
    await collector.terminal; guard();
    // Drain one Node continuation before removing our event listeners.
    await new Promise(resolve => setImmediate(resolve)); guard();
  })();
  try { await deadline(work, caseMs); }
  catch (error) { record.fail(error instanceof ProbeError ? error.code : 'operation_failed'); }
  finally {
    cancelled = true;
    const clean = async (kind, operation) => {
      try { await deadline(operation(), cleanupMs); }
      catch { if (!record.result.cleanupErrors.includes(kind)) record.result.cleanupErrors.push(kind); }
    };
    await clean('context_cleanup', async () => { if (context) await (closePromise ?? context.close()); });
    await clean('server_cleanup', async () => {
      if (collector) await collector.stop();
      await new Promise(resolve => setImmediate(resolve));
    });
    observer?.detach();
  }
  return structuredClone(classify(record));
}

export function validateResult(result) {
  const keys = (value, names) => value && Object.keys(value).sort().join(',') === names.split(',').sort().join(',');
  if (!keys(result, 'version,runId,provenance,cases,suiteError,cleanupErrors') || result.version !== 1
      || !matches(UUID, result.runId) || !Array.isArray(result.cases) || result.cases.length > 4
      || ![null, ...ERRORS].includes(result.suiteError)
      || !Array.isArray(result.cleanupErrors) || result.cleanupErrors.length > 1
      || result.cleanupErrors.some(e => e !== 'browser_cleanup')) return false;
  const p = result.provenance;
  if (!keys(p, 'node,playwright,lockSha256,browserRevision,sourceCommit')
      || !matches(/^v\d{1,3}\.\d{1,3}\.\d{1,3}$/, p.node) || p.playwright !== '1.61.0'
      || !matches(/^[a-f0-9]{64}$/, p.lockSha256) || !matches(/^\d{1,10}$/, p.browserRevision)
      || !matches(/^[a-f0-9]{40}$/, p.sourceCommit)) return false;
  const names = new Set(), ids = new Set();
  return result.cases.every(c => {
    if (!keys(c, 'case,runId,caseId,primaryError,cleanupErrors,events,facts,outcome')
        || !CASES.includes(c.case) || names.has(c.case) || ids.has(c.caseId) || !matches(UUID, c.caseId)
        || c.runId !== result.runId || ![null, ...ERRORS].includes(c.primaryError)
        || !Array.isArray(c.cleanupErrors) || c.cleanupErrors.length > 2
        || new Set(c.cleanupErrors).size !== c.cleanupErrors.length
        || c.cleanupErrors.some(e => !['context_cleanup', 'server_cleanup'].includes(e))
        || !['not_observed', 'response_after_close', 'response_before_close_resolve', 'request_cancelled', 'no_response_observed'].includes(c.outcome)
        || !Array.isArray(c.events) || c.events.length > 24) return false;
    names.add(c.case); ids.add(c.caseId);
    let previous = 0;
    if (!c.events.every(e => {
      const ok = keys(e, 'kind,atMs') && EVENTS.includes(e.kind) && Number.isFinite(e.atMs)
        && e.atMs >= previous && e.atMs <= 70000;
      previous = e.atMs; return ok;
    })) return false;
    const f = c.facts;
    const bools = ['requestOwned', 'collectorOwned', 'responseJoined', 'failureJoined',
      'releaseWhileClosePending', 'serverFinished', 'serverClosed', 'forcedSocketDisposal'];
    return keys(f, `postCount,clientStatus,writeAvailable,${bools.join(',')}`)
      && Number.isInteger(f.postCount) && f.postCount >= 0 && f.postCount <= 2
      && [null, 204, 'other'].includes(f.clientStatus) && [null, true, false].includes(f.writeAvailable)
      && bools.every(key => typeof f[key] === 'boolean');
  });
}

export function functionalPass(result) {
  return validateResult(result) && result.suiteError === null && result.cleanupErrors.length === 0
    && result.cases.length === 4 && result.cases.every(c => {
      const copy = structuredClone(c);
      const classified = classify({ result: copy, fail: code => { copy.primaryError ??= code; } });
      return c.primaryError === null && c.cleanupErrors.length === 0 && c.outcome !== 'not_observed'
        && classified.primaryError === null && classified.outcome === c.outcome;
    });
}

export async function runProbe({ browser, provenance, runId = randomUUID(), caseRunner = runCase,
  suiteMs = 60000, cleanupMs = 1000 }) {
  const result = { version: 1, runId, provenance, cases: [], suiteError: null, cleanupErrors: [] };
  let stopped = false;
  const work = (async () => {
    for (const caseName of CASES) {
      if (stopped) return;
      const value = await caseRunner({ browser, runId, caseName });
      if (stopped) return;
      result.cases.push(value);
      if (value.cleanupErrors.length) return;
    }
  })();
  try { await deadline(work, suiteMs); }
  catch (error) { result.suiteError = error instanceof ProbeError ? error.code : 'operation_failed'; }
  finally {
    stopped = true;
    try { await deadline(browser.close(), cleanupMs); }
    catch { result.cleanupErrors.push('browser_cleanup'); }
  }
  return result;
}

export function safeJSON(result) {
  if (!validateResult(result)) throw new ProbeError('invalid_payload');
  return JSON.stringify(result, null, 2) + '\n';
}
