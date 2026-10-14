/** INSTRUMENTED actual-router witness. No app imports, public controls or fabricated HTTP/SSE. */
import assert from 'node:assert/strict';
import { appendFileSync, readFileSync, writeFileSync, renameSync, statSync, lstatSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve, dirname } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createConnection } from 'node:net';
import { randomUUID } from 'node:crypto';
import { loopback, sourceLink, anchorMatches, COMPOSER_SELECTOR, belongsToAggregate } from './qualify-history-realserver.mjs';

export function persistedMessageSelector(id) {
  assert.ok(typeof id === 'string' && id.length > 0, 'exact persisted message identity required');
  return `#messages [id=${JSON.stringify(`message-${id}`)}], #messages [data-message-id=${JSON.stringify(id)}]`;
}

const EDGE_FIELDS = ['product_conversation_id', 'predecessor_transcript_row_id', 'successor_transcript_row_id', 'continuation_message_id'];
export function sameEdge(a, b) {
  return !!a && !!b && EDGE_FIELDS.every(key => typeof a[key] === 'string' && a[key].length > 0 && a[key] === b[key]);
}
export function openingMatches(value, evidence, status) {
  return value?.status === status && sameEdge(value.edge, evidence)
    && value.opening_authority === 'user_authorized_instruction'
    && (status === 'pending' ? value.client_turn_key === evidence.client_turn_key
      : value.accepted_message_id === `${evidence.successor_transcript_row_id}:${evidence.client_turn_key}`
        && value.presentation === 'separate_message');
}
export function faultTarget(method, url, state, kind) {
  const parsed = new URL(url, 'http://127.0.0.1:1');
  const path = parsed.pathname;
  if (method !== 'GET' || parsed.protocol !== 'http:' || parsed.hostname !== '127.0.0.1' || parsed.username || parsed.password) return false;
  if (kind === 'stream') return /^\/api\/conversations\/[^/]+\/stream$/.test(path) && path === state.streamPath;
  // The pinned app has no /history-roster route: its roster refresh is this bounded aggregate GET.
  return state.failRoster === true && path === state.aggregatePath
    && path !== '/api/product-conversations/events'
    && (/^\/api\/product-conversations\/[^/]+$/.test(path)
      || /^\/api\/global\/coordinator\/history\/[^/]+$/.test(path));
}
export function ownedFile(path) {
  const parent = statSync(dirname(path));
  assert.ok(parent.isDirectory() && (parent.mode & 0o077) === 0 && parent.uid === process.getuid(), 'control root must be owner-only');
  assert.ok(lstatSync(path).isFile() && !lstatSync(path).isSymbolicLink(), 'existing non-symlink owned control file required');
  return path;
}
export function updateFault(path, values) {
  ownedFile(path);
  const next = { ...JSON.parse(readFileSync(path, 'utf8')), ...values };
  const staging = path + '.next';
  writeFileSync(staging, JSON.stringify(next), { mode: 0o600 });
  renameSync(staging, path);
}
export function previewTransport(base, file, log) {
  loopback(base); ownedFile(file);
  const read = () => JSON.parse(readFileSync(file, 'utf8'));
  return { '/api': { target: base, xfwd: true,
    bypass(req, res) {
      if (faultTarget(req.method, req.url, read(), 'roster')) {
        appendFileSync(log, JSON.stringify({ action: 'destroy-roster', method: req.method, url: req.url }) + '\n');
        res.destroy(); req.destroy();
        return false;
      }
    },
    configure(proxy) {
      const streams = new Set();
      let previous = read().drop;
      const timer = setInterval(() => {
        const state = read();
        if (state.drop === previous) return;
        previous = state.drop;
        let count = 0;
        for (const entry of streams) {
          if (!faultTarget(entry.method, entry.url, state, 'stream')) continue;
          count++; entry.upstream.destroy(); entry.downstream.destroy(); streams.delete(entry);
        }
        appendFileSync(log, JSON.stringify({ action: 'drop-streams', token: previous, count, path: state.streamPath }) + '\n');
      }, 25);
      timer.unref();
      proxy.on('proxyRes', (upstream, req, downstream) => {
        if (!faultTarget(req.method, req.url, read(), 'stream')) return;
        const entry = { method: req.method, url: req.url, upstream, downstream };
        streams.add(entry);
        downstream.once('close', () => streams.delete(entry));
      });
      proxy.once('close', () => { clearInterval(timer); for (const s of streams) { s.upstream.destroy(); s.downstream.destroy(); } });
    },
  }, '/preview': { target: base } };
}
export function ipc(path, command) {
  return new Promise((resolveReply, reject) => {
    const bytes = JSON.stringify(command) + '\n';
    assert.ok(Buffer.byteLength(bytes) <= 1024);
    const socket = createConnection(path);
    let raw = '', done = false;
    const finish = (error, value) => {
      if (done) return; done = true; socket.destroy();
      if (error) reject(error); else resolveReply(value);
    };
    socket.setTimeout(2000, () => finish(new Error('owned IPC deadline')));
    socket.once('connect', () => socket.write(bytes));
    socket.on('error', error => finish(error));
    socket.on('end', () => finish(new Error('IPC closed without receipt')));
    socket.on('data', chunk => {
      raw += chunk;
      if (Buffer.byteLength(raw) > 16384) return finish(new Error('IPC receipt bound'));
      if (!raw.includes('\n')) return;
      try { const reply = JSON.parse(raw.split('\n')[0]); assert.equal(reply.ok, true, reply.error); finish(null, reply.result); }
      catch (error) { finish(error); }
    });
  });
}
export async function releaseOnce(state, send) {
  if (!state.armed || state.releaseAttempted) return;
  state.releaseAttempted = true;
  assert.equal((await send({ command: 'release', predecessor: state.predecessor, client_turn_key: state.key })).state, 'released');
}

async function run() {
  const app = resolve(process.env.APP_ROOT), output = resolve(process.env.HISTORY_OUTPUT);
  const ui = loopback(process.env.HISTORY_BASE_URL), api = loopback(process.env.HISTORY_API_URL);
  const a = JSON.parse(readFileSync(process.env.HISTORY_FIXTURE, 'utf8'));
  const fault = ownedFile(process.env.HISTORY_FAULT), control = process.env.HISTORY_CONTROL;
  const result = { status: 'failed', evidence_class: 'instrumented_runtime_not_shipping_proof', checks: [],
    uncovered: ['Generated authority cannot be selected by pinned manual POST; not qualified.', 'Shipping binary/providers/production/physical devices not exercised.'] };
  let records = 0;
  const record = (file, value) => { assert.ok(++records < 15000, 'receipt bound'); appendFileSync(resolve(output, file), JSON.stringify(value) + '\n'); };
  const checked = (name, data) => { result.checks.push(name); record('assertions.jsonl', { name, ...data }); };
  const gate = { armed: false, releaseAttempted: false, predecessor: a.members.at(-1), key: randomUUID() };
  const command = async body => { const reply = await ipc(control, body); record('ipc.jsonl', { command: body, reply }); return reply; };
  let browser, page, continuation;
  const errors = [], captureErrors = [], tasks = new Set(), snapshots = [], events = [], requests = [], failed = [];
  const hardStop = setTimeout(() => { writeFileSync(resolve(output, 'browser-result.json'), JSON.stringify({ ...result, error: 'wall deadline' })); process.exit(1); }, 170000);
  async function poll(fn, label, ms = 10000) {
    const end = performance.now() + ms;
    for (;;) { const value = await fn(); if (value) return value; assert.ok(performance.now() < end, label); await new Promise(r => setTimeout(r, 40)); }
  }
  async function http(method, path, body) {
    const response = await fetch(api + path, { method, headers: body ? { 'Content-Type': 'application/json' } : {},
      body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(10000) });
    const raw = await response.text(); record('driver-http.jsonl', { method, path, body, status: response.status, raw });
    assert.ok(response.ok, `${method} ${path}: ${response.status}`); return JSON.parse(raw);
  }
  async function anchor(key) {
    return page.evaluate(key => {
      const scroller = document.querySelector('#messages'); if (!scroller) return null;
      const viewport = scroller.getBoundingClientRect();
      const row = [...scroller.querySelectorAll('[data-virtual-key]')].find(el => {
        const box = el.getBoundingClientRect(); return (key ? el.dataset.virtualKey === key : box.top >= viewport.top)
          && box.height > 0 && box.bottom > viewport.top && box.top < viewport.bottom;
      });
      return row ? { key: row.dataset.virtualKey, offset: row.getBoundingClientRect().top - viewport.top } : null;
    }, key);
  }
  async function sameAnchor(before, label) {
    let frames = 0;
    const after = await poll(async () => { await page.evaluate(() => new Promise(r => requestAnimationFrame(r)));
      const value = await anchor(before.key); frames = anchorMatches(before, value) ? frames + 1 : 0; return frames >= 4 && value; }, label);
    checked(label, { before, after, tolerance_css_px: 2 });
  }
  const composerReadonly = async () => {
    const composer = page.locator(COMPOSER_SELECTOR);
    return await composer.count() === 0 || !await composer.isEditable();
  };
  try {
    const { chromium } = createRequire(resolve(app, 'ui/package.json'))('playwright');
    browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ viewport: { width: 1360, height: 900 } });
    page = await context.newPage(); page.setDefaultTimeout(10000); page.setDefaultNavigationTimeout(10000);
    page.on('pageerror', error => errors.push(String(error)));
    page.on('request', req => { const entry = { url: req.url(), method: req.method(), body: req.postData(), at: performance.now() };
      requests.push(entry); record('browser-requests.jsonl', entry); });
    page.on('requestfailed', req => { const entry = { url: req.url(), method: req.method(), failure: req.failure() };
      failed.push(entry); record('browser-failed.jsonl', entry); });
    page.on('response', response => {
      if (!new URL(response.url()).pathname.startsWith('/api/')) return;
      const task = (async () => {
        const entry = { url: response.url(), status: response.status(), type: response.headers()['content-type'], at: performance.now() };
        if (!entry.type?.includes('text/event-stream')) {
          const raw = await response.text(); assert.ok(Buffer.byteLength(raw) < 1000000, 'HTTP receipt bound'); entry.raw = raw;
          if (entry.type?.includes('application/json') && raw) {
            const data = JSON.parse(raw); if (data.segments && data.boundaries) snapshots.push({ ...entry, data });
          }
        }
        record('browser-responses.jsonl', entry);
      })();
      tasks.add(task); task.finally(() => tasks.delete(task)).catch(error => captureErrors.push(String(error)));
    });
    const cdp = await context.newCDPSession(page); await cdp.send('Network.enable');
    cdp.on('Network.eventSourceMessageReceived', event => { events.push(event); record('browser-sse.jsonl', event); });

    // Verify the already-accepted manual edge independently; identical payload is not suppression authority.
    const previous = await http('GET', a.endpoint + '?boundary_representation=original_messages');
    assert.equal(previous.latest_opening.status, 'accepted');
    assert.equal(previous.latest_opening.opening_authority, 'user_authorized_instruction');
    assert.equal(previous.latest_opening.presentation, 'separate_message');
    const oldEdge = previous.latest_opening.edge;
    const boundary = previous.boundaries.find(b => b.message_id === oldEdge.continuation_message_id);
    const openingId = previous.latest_opening.accepted_message_id;
    assert.ok(boundary); assert.equal(boundary.represented_successor_opening_message_id, null);
    const oldRow = await http('GET', `/api/conversations/${gate.predecessor}`);
    const openingMessage = oldRow.messages.find(m => m.message_id === openingId);
    assert.ok(openingMessage); assert.equal(openingMessage.message_type, 'user');
    const predecessor = await http('GET', `/api/conversations/${oldEdge.predecessor_transcript_row_id}`);
    const boundaryMessage = predecessor.messages.find(m => m.message_id === oldEdge.continuation_message_id);
    assert.ok(boundaryMessage); assert.equal(boundaryMessage.content.summary, openingMessage.content.text);
    await page.goto(ui + sourceLink(a, gate.predecessor, openingId), { waitUntil: 'domcontentloaded' });
    await page.locator(persistedMessageSelector(openingId)).first().waitFor({ state: 'visible' });
    checked('manual equal-text accepted opening remains separate', { edge: oldEdge, opening_id: openingId, boundary_id: boundary.message_id });

    assert.equal((await command({ command: 'arm', predecessor: gate.predecessor, client_turn_key: gate.key })).state, 'armed'); gate.armed = true;
    // POST acknowledgement may precede message persistence; only Entered certifies committed Pending.
    continuation = http('POST', `/api/conversations/${gate.predecessor}/continue`, { handoff: 'opening-probe [[perf:1]] [[ttft:0]]', message_id: gate.key });
    continuation.catch(() => {});
    const entered = await poll(async () => { const reply = await command({ command: 'entered' }); return reply.state === 'entered' && reply.evidence; }, 'committed Pending IPC', 10000);
    assert.equal(entered.predecessor_transcript_row_id, gate.predecessor); assert.equal(entered.client_turn_key, gate.key);
    assert.equal(entered.edge, 'committed_intent_before_first_message_persistence');
    const latest = entered.successor_transcript_row_id, acceptedId = `${latest}:${gate.key}`;
    const pending = await http('GET', a.endpoint + '?boundary_representation=original_messages');
    assert.equal(pending.latest_transcript_row_id, latest); assert.ok(openingMatches(pending.latest_opening, entered, 'pending'));
    const direct = await http('GET', `/api/conversations/${latest}`);
    assert.equal(direct.messages.some(m => [gate.key, acceptedId].includes(m.message_id)), false);
    // New independent actual subscription must get Init while the write is held.
    const stream = await fetch(api + `/api/conversations/${latest}/stream`, { signal: AbortSignal.timeout(5000) });
    assert.equal(stream.status, 200); assert.match(stream.headers.get('content-type'), /text\/event-stream/);
    const reader = stream.body.getReader(); let raw = '';
    try { while (!raw.includes('\n\n')) { const value = await reader.read(); assert.equal(value.done, false); raw += new TextDecoder().decode(value.value); assert.ok(raw.length < 1000000); }
      record('independent-sse-init.jsonl', { latest, raw });
      const frame = raw.split('\n\n').find(frame => frame.includes('event: init'));
      assert.ok(frame); const init = JSON.parse(frame.split('\n').filter(line => line.startsWith('data:')).map(line => line.slice(5).trim()).join('\n'));
      assert.ok(openingMatches(init.opening, entered, 'pending'));
    } finally { await reader.cancel(); }
    updateFault(fault, { streamPath: `/api/conversations/${latest}/stream` });
    const start = snapshots.length;
    const middle = a.occurrences.filter(k => k[0] === 1)[20][2];
    await page.goto(ui + sourceLink(a, a.members[1], middle), { waitUntil: 'domcontentloaded' });
    await poll(() => snapshots.slice(start).find(s => belongsToAggregate(a, s) && s.data.latest_transcript_row_id === latest && openingMatches(s.data.latest_opening, entered, 'pending')), 'browser Pending aggregate');
    await page.locator(persistedMessageSelector(middle)).first().waitFor({ state: 'visible' });
    await poll(composerReadonly, 'Pending composer readonly');
    assert.equal(await page.locator(persistedMessageSelector(acceptedId)).count(), 0);
    const before = await poll(() => anchor(), 'historical physical anchor');
    const pin = new URL(page.url()).searchParams.get('source_transcript'); assert.equal(pin, a.members[1]);
    assert.equal(requests.some(r => r.method === 'POST' && new URL(r.url).pathname.endsWith('/chat')), false);
    checked('committed Pending: browser/HTTP/new SSE init available, opening deferred and composer readonly', { entered, before });
    const actualRoster = snapshots.slice(start).find(s => belongsToAggregate(a, s));
    updateFault(fault, { aggregatePath: new URL(actualRoster.url).pathname, failRoster: true });
    const acceptedStart = events.length;
    await releaseOnce(gate, command);
    const continued = await continuation; assert.equal(continued.status, 'accepted'); assert.equal(continued.conversation_id, latest);
    const accepted = await poll(() => events.slice(acceptedStart).find(e => {
      const value = JSON.parse(e.data); return ['message', 'message_updated'].includes(e.eventName) && openingMatches(value.opening, entered, 'accepted');
    }), 'actual accepted opening SSE', 10000);
    const event = JSON.parse(accepted.data); assert.ok(accepted.eventId, 'actual accepted cursor');
    assert.equal(event.message?.message_id ?? event.message_id, acceptedId);
    await poll(() => failed.find(r => faultTarget(r.method, r.url, JSON.parse(readFileSync(fault)), 'roster')), 'actual failed owned roster GET');
    await page.locator('[role="alert"]').first().waitFor({ state: 'visible' });
    await poll(async () => await page.locator(COMPOSER_SELECTOR).isEditable(), 'Accepted composer enabled despite roster failure');
    await sameAnchor(before, 'acceptance retains physical anchor');
    assert.equal(new URL(page.url()).searchParams.get('source_transcript'), pin);
    const text = `composer-${randomUUID()} [[perf:1]] [[ttft:0]]`;
    const posted = page.waitForRequest(req => req.method() === 'POST' && new URL(req.url()).pathname.endsWith('/chat'));
    await page.locator(COMPOSER_SELECTOR).fill(text); await page.locator(COMPOSER_SELECTOR).press('Enter');
    const req = await posted; assert.equal(new URL(req.url()).pathname, `/api/conversations/${latest}/chat`);
    const body = req.postDataJSON(); assert.equal(body.text, text);
    const ack = await req.response(); assert.ok(ack?.ok()); const ackData = await ack.json(); assert.equal(ackData.queued, true); assert.notEqual(ackData.steering, true);
    await poll(async () => (await http('GET', `/api/conversations/${latest}`)).messages.some(m => m.message_id === `${latest}:${body.message_id}`), 'actual composer message persisted');
    checked('accepted SSE unblocks exact successor composer despite actual roster failure', { accepted, chat_message_id: body.message_id });
    const reconnectStart = events.length, requestStart = requests.length, url = page.url();
    updateFault(fault, { drop: 1 });
    await poll(() => requests.slice(requestStart).find(r => faultTarget(r.method, r.url, JSON.parse(readFileSync(fault)), 'stream')), 'same-page actual SSE reconnect request', 15000);
    const reconnect = await poll(() => events.slice(reconnectStart).find(e => e.eventName === 'init' && openingMatches(JSON.parse(e.data).opening, entered, 'accepted')), 'same-page accepted reconnect Init', 15000);
    const transport = readFileSync(resolve(output, 'preview-transport.jsonl'), 'utf8').trim().split('\n').map(line => JSON.parse(line));
    assert.ok(transport.some(t => t.action === 'drop-streams' && t.token === 1 && t.count > 0), 'owned preview actually destroyed live SSE responses');
    assert.equal(page.url(), url); assert.equal(new URL(page.url()).searchParams.get('source_transcript'), pin);
    await poll(async () => await page.locator(COMPOSER_SELECTOR).isEditable(), 'reconnected composer stays enabled');
    await sameAnchor(before, 'reconnect retains physical anchor');
    // Exact identity, not text heuristics: at most one occurrence in aggregate DOM/navigation.
    assert.ok(await page.locator(persistedMessageSelector(acceptedId)).count() <= 1);
    const finalRow = await http('GET', `/api/conversations/${latest}`);
    assert.equal(finalRow.messages.filter(m => m.message_id === acceptedId).length, 1);
    checked('same-page transport SSE drop/reconnect preserves accepted full-edge identity', { accepted_cursor: accepted.eventId, reconnect, requests: requests.slice(requestStart) });
    await page.screenshot({ path: resolve(output, 'accepted-reconnected.png') });
    // An explicit chapter selection may move the reader; verify its persisted ID after the click.
    const chapter = page.locator('#conversation-nav [role="button"]').filter({ hasText: 'opening-probe' });
    assert.equal(await chapter.count(), 1); await chapter.click();
    await page.waitForURL(value => value.hash === `#message-${encodeURIComponent(acceptedId)}`);
    const rendered = page.locator(persistedMessageSelector(acceptedId));
    await rendered.waitFor({ state: 'visible' }); assert.equal(await rendered.count(), 1);
    const physical = await rendered.evaluate(el => { const box = el.getBoundingClientRect(); const view = document.querySelector('#messages').getBoundingClientRect(); return box.height > 0 && box.bottom > view.top && box.top < view.bottom; });
    assert.equal(physical, true);
    checked('accepted manual opening renders once with exact persisted identity after real chapter click', { acceptedId });
    await Promise.all([...tasks]); assert.deepEqual(errors, []); assert.deepEqual(captureErrors, []);
    result.status = 'passed_assertions';
  } catch (error) {
    result.error = String(error.stack ?? error); process.exitCode = 1;
    if (page) await page.screenshot({ path: resolve(output, 'failure.png') }).catch(() => {});
  } finally {
    try { if (gate.armed && !gate.releaseAttempted) {
      const entered = await command({ command: 'entered' });
      if (entered.state === 'entered') await releaseOnce(gate, command);
    } } catch (error) { record('cleanup-errors.jsonl', { error: String(error) }); process.exitCode = 1; result.status = 'failed'; }
    try { await command({ command: 'stop' }); } catch (error) { record('cleanup-errors.jsonl', { error: String(error) }); process.exitCode = 1; result.status = 'failed'; }
    try { updateFault(fault, { failRoster: false }); if (browser) await browser.close(); }
    catch (error) { result.status = 'failed'; result.cleanup_error = String(error); process.exitCode = 1; }
    finally { clearTimeout(hardStop); writeFileSync(resolve(output, 'browser-result.json'), JSON.stringify(result, null, 2) + '\n'); }
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) await run();
