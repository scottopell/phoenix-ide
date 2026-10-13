/** Real HTTP/SSE qualification driver. No routes, response stubs or app imports.
 * Invoked by tests/e2e/history_browser.py; exports below are lightweight selftests.
 */
import assert from 'node:assert/strict';
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { randomUUID } from 'node:crypto';

export function loopback(value) {
  const url = new URL(value);
  assert.equal(url.protocol, 'http:');
  assert.equal(url.hostname, '127.0.0.1');
  assert.ok(url.port && !url.username && !url.password);
  assert.equal(url.pathname, '/');
  assert.equal(url.search + url.hash, '');
  return url.origin;
}
export function sourceLink(aggregate, row, message, tool) {
  const query = new URLSearchParams({ source_transcript: row, qualification: 'retained' });
  if (tool) query.set('source_tool', tool);
  return `${aggregate.route}?${query}#message-${encodeURIComponent(message)}`;
}
export function anchorMatches(before, after) {
  return !!before && !!after && before.key === after.key && Math.abs(before.offset - after.offset) <= 2;
}
export function boundedDiagnosticJson(value, limit = 1_000_000) {
  assert.ok(limit >= 1024);
  const bytes = Buffer.from(JSON.stringify(value, null, 2));
  if (bytes.length <= limit) return bytes;
  return Buffer.from(JSON.stringify({ truncated: true, original_bytes: bytes.length,
    prefix_base64: bytes.subarray(0, Math.floor(limit / 2)).toString('base64') }));
}
export const APPEND_PROBE = 'append-probe';
export const CONTINUATION_PROBE = 'live-probe';
export function probeText(probe) { return `${probe} [[perf:1]] [[ttft:0]]`; }
export function occurrences(snapshot) {
  return snapshot.segments.flatMap(s => s.messages.map(m => [s.segment_ordinal, m.sequence_id, m.message_id]));
}

export function belongsToAggregate(aggregate, response) {
  const path = new URL(response.url).pathname;
  return aggregate.kind === 'global'
    ? path.startsWith('/api/global/coordinator/history/') && response.data.canonical_root?.transcript_row_id === aggregate.members[0]
    : path === aggregate.endpoint;
}

async function run() {
  const app = resolve(process.env.APP_ROOT);
  const output = resolve(process.env.HISTORY_OUTPUT);
  const ui = loopback(process.env.HISTORY_BASE_URL);
  const api = loopback(process.env.HISTORY_API_URL);
  const { aggregates } = JSON.parse(readFileSync(process.env.HISTORY_FIXTURE, 'utf8'));
  assert.equal(aggregates.length, 2);
  const require = createRequire(resolve(app, 'ui/package.json'));
  const { chromium } = require('playwright');
  const result = { status: 'failed', checks: [], uncovered: [
    'Deterministically delayed older-page versus newer navigation race is not exercised (no response interception).',
    'Physical devices, non-Chromium engines, real providers and production data are not qualified.',
  ] };
  const record = (file, data) => appendFileSync(resolve(output, file), JSON.stringify(data) + '\n');
  const checked = (name, data = {}) => { result.checks.push(name); record('assertions.jsonl', { name, ...data }); };
  let browser, context, page;
  let records = 0;
  const bounded = (file, data) => {
    assert.ok(++records <= 15000, 'receipt event budget exceeded');
    record(file, data);
  };
  const hardStop = setTimeout(() => {
    writeFileSync(resolve(output, 'browser-result.json'), JSON.stringify({ ...result, error: 'driver wall bound' }, null, 2));
    process.exit(1); // Python owns/kills the process group, including Chromium.
  }, 300_000);
  const pending = new Set();
  const snapshots = [];
  const pageErrors = [];
  const captureErrors = [];
  let sseEvents = 0;
  try {
    browser = await chromium.launch({ headless: true });
    context = await browser.newContext({ viewport: { width: 1360, height: 900 } });
    page = await context.newPage();
    page.setDefaultTimeout(20_000);
    page.setDefaultNavigationTimeout(20_000);
    page.on('pageerror', error => {
      pageErrors.push(String(error));
      bounded('page-errors.jsonl', { error: String(error) });
    });
    page.on('console', msg => bounded('console.jsonl', { type: msg.type(), text: msg.text() }));
    page.on('requestfailed', req => bounded('network-errors.jsonl', { url: req.url(), failure: req.failure() }));
    page.on('response', response => {
      if (!new URL(response.url()).pathname.startsWith('/api/')) return;
      const task = (async () => {
        const type = response.headers()['content-type'] ?? '';
        const entry = { url: response.url(), status: response.status(), type, at_ms: performance.now() };
        if (!type.includes('text/event-stream')) {
          try {
            const raw = await response.text();
            entry.body = raw.slice(0, 64000);
            entry.truncated = raw.length > 64000;
            const data = JSON.parse(raw);
            if (Array.isArray(data.segments) && Array.isArray(data.boundaries)) {
              snapshots.push({ url: response.url(), data, at_ms: entry.at_ms });
            }
          } catch (error) { entry.read_error = String(error); }
        }
        assert.ok(snapshots.length <= 300, 'aggregate request bound');
        bounded('browser-api.jsonl', entry);
      })();
      pending.add(task);
      task.finally(() => pending.delete(task)).catch(error => captureErrors.push(String(error)));
    });
    const cdp = await context.newCDPSession(page);
    await cdp.send('Network.enable');
    cdp.on('Network.eventSourceMessageReceived', event => {
      sseEvents++;
      bounded('browser-sse.jsonl', event);
    });

    async function poll(fn, label) {
      const end = performance.now() + 20_000;
      for (;;) {
        const value = await fn();
        if (value) return value;
        assert.ok(performance.now() < end, `bounded readiness: ${label}`);
        await new Promise(r => setTimeout(r, 50));
      }
    }
    async function request(method, path, body) {
      assert.ok(path.startsWith('/api/'), 'API path only');
      const response = await context.request.fetch(api + path, { method, data: body, timeout: 10_000 });
      const raw = await response.text();
      bounded('driver-api.jsonl', { method, path, body, status: response.status(), response: raw });
      assert.ok(response.ok(), `${method} ${path}: ${response.status()} ${raw.slice(0, 500)}`);
      return JSON.parse(raw);
    }
    const state = data => typeof data.conversation.state === 'string' ? data.conversation.state : data.conversation.state.type;
    async function settled(row, expected = 'idle', mid) {
      return poll(async () => {
        const data = await request('GET', `/api/conversations/${row}`);
        if (state(data) === 'error') throw new Error(JSON.stringify(data.conversation));
        if (state(data) !== expected) return false;
        if (mid) {
          const index = data.messages.findIndex(m => m.message_id === mid);
          if (index < 0 || (expected === 'idle' && !data.messages.slice(index + 1).some(m => m.message_type === 'agent'))) return false;
        }
        return data;
      }, `persisted ${row} ${expected}`);
    }
    async function send(row, text, expected = 'idle') {
      const mid = randomUUID();
      const accepted = await request('POST', `/api/conversations/${row}/chat`, { text, images: [], files: [], message_id: mid });
      assert.ok(accepted.queued === true && accepted.steering !== true, 'delivered admission, not steering');
      await settled(row, expected, `${row}:${mid}`);
      return mid;
    }
    async function visibleMessage(mid) {
      const locator = page.locator(`[id=${JSON.stringify(`message-${mid}`)}], [data-message-id=${JSON.stringify(mid)}]`).first();
      await locator.waitFor({ state: 'visible' });
      await poll(() => locator.evaluate(el => {
        const viewport = document.querySelector('#messages').getBoundingClientRect();
        const box = el.getBoundingClientRect();
        return box.bottom > viewport.top && box.top < viewport.bottom;
      }), `physical message ${mid}`);
      return locator;
    }
    async function anchor(key) {
      return page.evaluate(key => {
        const scroller = document.querySelector('#messages');
        if (!scroller) return null;
        const viewport = scroller.getBoundingClientRect();
        const rows = [...scroller.querySelectorAll('[data-virtual-key]')];
        const row = key ? rows.find(r => r.dataset.virtualKey === key) : rows.find(r => {
          const box = r.getBoundingClientRect();
          return box.top >= viewport.top && box.top < viewport.bottom && box.height > 0;
        });
        if (!row) return null;
        const box = row.getBoundingClientRect();
        if (box.bottom <= viewport.top || box.top >= viewport.bottom) return null;
        return { key: row.dataset.virtualKey, offset: box.top - viewport.top };
      }, key);
    }
    async function sameAnchor(before, label) {
      assert.ok(before, 'physical visible anchor captured');
      let consecutiveFrames = 0;
      const after = await poll(async () => {
        await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
        const value = await anchor(before.key);
        consecutiveFrames = anchorMatches(before, value) ? consecutiveFrames + 1 : 0;
        return consecutiveFrames >= 4 && value;
      }, label);
      checked(label, { before, after, tolerance_css_px: 2 });
    }
    async function goto(path, waitMessages = true) {
      await page.goto(ui + path, { waitUntil: 'domcontentloaded' });
      if (waitMessages) await page.locator('#messages').waitFor({ state: 'visible' });
    }
    async function snapshotSeen(a, predicate, since = 0) {
      return poll(() => snapshots.slice(since).find(s => belongsToAggregate(a, s) && predicate(s.data)), 'real browser aggregate response');
    }

    for (const a of aggregates) {
      assert.ok(a.persisted_rows > 201 && a.members.length >= 3 && a.page_count >= 3);
      const prefix = a.kind;
      const firstSnapshot = snapshots.length;
      const started = performance.now();
      await goto(a.route + '?qualification=retained');
      const tail = await snapshotSeen(a, s => s.has_older, firstSnapshot);
      await poll(() => anchor(), 'first meaningful visible transcript row');
      const meaningfulTranscriptMs = performance.now() - started;
      const boundary = page.locator('#conversation-nav [role="button"][aria-label="Load context boundary 1"], #conversation-nav [role="button"][aria-label="Context boundary 1"]').first();
      await boundary.waitFor({ state: 'visible' });
      await poll(() => boundary.isEnabled(), 'enabled boundary navigation');
      const enabledMs = performance.now() - started;
      checked(`${prefix}: enabled navigation readiness`, {
        navigation_ms: enabledMs, first_snapshot_ms: tail.at_ms - started,
        meaningful_transcript_observed_ms: meaningfulTranscriptMs,
        observation_note: 'upper-bound driver observation after navigation/snapshot, not first-paint instrumentation',
        elapsed_after_snapshot_ms: performance.now() - tail.at_ms,
        threshold: null, threshold_note: 'readiness observed, no invented performance pass threshold',
      });
      await boundary.click();
      const b = a.boundaries[0];
      await page.waitForURL(url => url.hash === `#message-${encodeURIComponent(b.message_id)}`);
      await visibleMessage(b.message_id);
      assert.equal(new URL(page.url()).searchParams.get('qualification'), 'retained');
      await snapshotSeen(a, s => occurrences(s).some(k => k[2] === b.message_id), firstSnapshot);
      checked(`${prefix}: original boundary navigation`, {
        loaded_in_initial_tail: occurrences(tail.data).some(k => k[2] === b.message_id),
      });
      const boundaryUrl = page.url();
      const second = page.locator('#conversation-nav [role="button"][aria-label="Context boundary 2"], #conversation-nav [role="button"][aria-label="Load context boundary 2"]').first();
      await second.click();
      await visibleMessage(a.boundaries[1].message_id);
      const secondUrl = page.url();
      await page.goBack({ waitUntil: 'domcontentloaded' });
      await page.waitForURL(boundaryUrl);
      await visibleMessage(b.message_id);
      await page.goForward({ waitUntil: 'domcontentloaded' });
      await page.waitForURL(secondUrl);
      await visibleMessage(a.boundaries[1].message_id);
      checked(`${prefix}: boundary browser back/forward`);

      const sourceStart = snapshots.length;
      await goto(sourceLink(a, a.members[0], a.old_message));
      await visibleMessage(a.old_message);
      await snapshotSeen(a, s => occurrences(s).some(k => k[2] === a.old_message), sourceStart);
      assert.equal(new URL(page.url()).searchParams.get('source_transcript'), a.members[0]);
      const acquired = snapshots.slice(sourceStart).filter(s => belongsToAggregate(a, s));
      assert.ok(acquired.some(s => new URL(s.url).searchParams.has('before')), 'deep link must cross real page cursor');
      checked(`${prefix}: paged old message with exact transcript pin`);
      const pinnedOldUrl = page.url();
      await page.locator('#conversation-nav [role="button"][aria-label="Context boundary 2"], #conversation-nav [role="button"][aria-label="Load context boundary 2"]').first().click();
      await visibleMessage(a.boundaries[1].message_id);
      assert.equal(new URL(page.url()).searchParams.get('source_transcript'), a.boundaries[1].predecessor_transcript_row_id);
      assert.equal(new URL(page.url()).searchParams.get('qualification'), 'retained');
      const pinnedBoundaryUrl = page.url();
      await page.goBack({ waitUntil: 'domcontentloaded' });
      await page.waitForURL(pinnedOldUrl);
      await visibleMessage(a.old_message);
      assert.equal(new URL(page.url()).searchParams.get('source_transcript'), a.members[0]);
      await page.goForward({ waitUntil: 'domcontentloaded' });
      await page.waitForURL(pinnedBoundaryUrl);
      await visibleMessage(a.boundaries[1].message_id);
      checked(`${prefix}: explicit source pins survive boundary browser back/forward`);
      await goto(sourceLink(a, a.members[0], a.tool.message_id));
      const thinkOwner = await visibleMessage(a.tool.message_id);
      // Pinned ThinkAside has no per-tool locator/reveal authority. Inspect the
      // real DOM, and report that capability gap rather than passing a generic
      // message jump as an exact-tool qualification.
      const exposesTool = await thinkOwner.evaluate((el, toolId) =>
        !!el.querySelector(`[data-tool-id="${CSS.escape(toolId)}"]`), a.tool.tool_id);
      if (!exposesTool) {
        await thinkOwner.locator('.think-aside').waitFor({ state: 'visible' });
        result.uncovered.push(`${prefix}: exact source_tool think selection not qualified: persisted ${a.tool.tool_id} renders as ThinkAside without a per-tool DOM target.`);
      } else {
        await goto(sourceLink(a, a.members[0], a.tool.message_id, a.tool.tool_id));
        await visibleMessage(a.tool.message_id);
        const detail = page.locator(`.message.agent[data-message-id=${JSON.stringify(a.tool.message_id)}] [data-tool-id=${JSON.stringify(a.tool.tool_id)}]`).first();
        await detail.waitFor({ state: 'visible' });
        await poll(() => detail.evaluate(el => {
          const box = el.getBoundingClientRect();
          const viewport = document.querySelector('#messages').getBoundingClientRect();
          return box.bottom > viewport.top && box.top < viewport.bottom;
        }), 'exact selected tool detail physically visible');
        assert.equal(new URL(page.url()).searchParams.get('source_tool'), a.tool.tool_id);
        checked(`${prefix}: exact historical think tool selection`);
      }

      // Select a middle historical prompt, away from both automatic paging edge and tail.
      const middle = a.occurrences.filter(k => k[0] === 1)[20][2];
      await goto(sourceLink(a, a.members[1], middle));
      await visibleMessage(middle);
      const before = await anchor();
      const liveStart = snapshots.length;
      let latest = a.members.at(-1);
      await send(latest, probeText(APPEND_PROBE));
      await poll(() => page.locator('#conversation-nav [role="button"]').evaluateAll((buttons, probe) =>
        buttons.some(button => button.textContent.includes(probe)), APPEND_PROBE), 'append reflected in browser navigation');
      // Read real HTTP tail as well as browser SSE-derived refresh.
      const appended = await request('GET', a.endpoint + '?boundary_representation=original_messages');
      assert.equal(appended.latest_transcript_row_id, latest);
      await sameAnchor(before, `${prefix}: anchor after actual append`);
      await send(latest, '[[scenario:context_window_exceeded]] [[ttft:0]]', 'context_exhausted');
      const continued = await request('POST', `/api/conversations/${latest}/continue`, {
        handoff: probeText(CONTINUATION_PROBE), message_id: randomUUID(),
      });
      assert.equal(continued.status, 'accepted');
      assert.notEqual(continued.conversation_id, latest);
      latest = continued.conversation_id;
      await settled(latest);
      await snapshotSeen(a, s => s.latest_transcript_row_id === latest, liveStart);
      await poll(() => page.locator('#conversation-nav [role="button"]').evaluateAll((buttons, probe) =>
        buttons.some(button => button.textContent.includes(probe)), CONTINUATION_PROBE), 'successor opening adopted in rendered navigation');
      await sameAnchor(before, `${prefix}: anchor after actual fourth continuation`);
      assert.equal(new URL(page.url()).searchParams.get('source_transcript'), a.members[1]);

      const composer = page.locator('textarea.input-textarea');
      await composer.waitFor({ state: 'visible' });
      await poll(() => composer.isEnabled(), 'newest composer enabled');
      const text = `[[perf:1]] [[ttft:0]] composer-${randomUUID()}`;
      await composer.fill(text);
      const posted = page.waitForRequest(req => req.method() === 'POST' && new URL(req.url()).pathname.endsWith('/chat'));
      await composer.press('Enter');
      const req = await posted;
      assert.equal(new URL(req.url()).pathname, `/api/conversations/${latest}/chat`);
      const body = req.postDataJSON();
      assert.equal(body.text, text);
      await settled(latest, 'idle', `${latest}:${body.message_id}`);
      checked(`${prefix}: historical reader composer sends to newest writable member`, { latest, message_id: body.message_id });
      await page.screenshot({ path: resolve(output, `${prefix}-historical-reader.png`) });

      const foreign = aggregates.find(other => other.kind !== a.kind).members[0];
      for (const pin of [`source_transcript=${foreign}`, `source_transcript=${a.members[0]}&source_transcript=${a.members[1]}`]) {
        await goto(a.route + '?' + pin + '&qualification=retained', false);
        if (a.kind === 'global') {
          await page.locator('.coordinator-page-status').filter({ hasText: 'Original source conversation unavailable' }).waitFor({ state: 'visible' });
        } else {
          await page.getByRole('alert').filter({ hasText: 'Original source transcript unavailable.' }).waitFor({ state: 'visible' });
        }
        assert.equal(new URL(page.url()).searchParams.get('qualification'), 'retained');
      }
      checked(`${prefix}: foreign and duplicate transcript pins rejected visibly`);

      // Real offline transport failure, not an API/EventSource stub. Latest route
      // starts with a bounded tail; top scrolling invokes the production loader.
      await goto((a.kind === 'global' ? `/global/${latest}` : a.route) + '?qualification=retained');
      await page.locator('#conversation-nav').waitFor({ state: 'visible' });
      const current = await settled(latest);
      await visibleMessage(current.messages.at(-1).message_id);
      await context.setOffline(true);
      try {
        await page.locator('#messages').evaluate(el => { el.scrollTop = 0; });
        await page.getByRole('alert').filter({ hasText: 'Could not load earlier history:' }).waitFor({ state: 'visible' });
      } finally { await context.setOffline(false); }
      const retryStart = snapshots.length;
      const retryAnchor = await anchor();
      await page.getByRole('button', { name: 'Retry', exact: true }).click();
      await snapshotSeen(a, () => true, retryStart);
      await poll(async () => (await page.getByRole('alert').filter({ hasText: 'Could not load earlier history:' }).count()) === 0, 'retry recovered');
      assert.ok(snapshots.slice(retryStart).some(s => belongsToAggregate(a, s) && new URL(s.url).searchParams.has('before')));
      await sameAnchor(retryAnchor, `${prefix}: physical anchor after real prefix Retry`);
      checked(`${prefix}: actual failed earlier transport then successful Retry`);
    }
    assert.ok(records > 0 && result.checks.length >= 20);
    await Promise.all([...pending]);
    assert.deepEqual(pageErrors, [], 'unexpected application JS errors');
    assert.deepEqual(captureErrors, [], 'API capture failures');
    assert.ok(sseEvents > 0, 'real browser EventSource traffic must be captured');
    checked('real browser SSE evidence captured', { event_count: sseEvents });
    result.status = 'passed_assertions';
  } catch (error) {
    result.error = String(error.stack ?? error);
    if (page) {
      await page.screenshot({ path: resolve(output, 'failure.png') }).catch(() => {});
      const diagnostic = await page.evaluate(() => {
        const viewport = document.querySelector('#messages');
        const ancestors = [];
        for (let element = viewport; element; element = element.parentElement) {
          const rect = element.getBoundingClientRect();
          ancestors.push({ tag: element.tagName, id: element.id, classes: element.className,
            hidden: element.hidden, display: getComputedStyle(element).display,
            top: rect.top, height: rect.height });
        }
        return { url: location.href, ancestors, scrollTop: viewport?.scrollTop,
          clientHeight: viewport?.clientHeight, scrollHeight: viewport?.scrollHeight,
          rows: [...(viewport?.querySelectorAll('[data-virtual-key]') ?? [])].map(element => {
            const rect = element.getBoundingClientRect();
            return { key: element.dataset.virtualKey, top: rect.top, height: rect.height };
          }) };
      }).catch(error => ({ diagnostic_error: String(error) }));
      writeFileSync(resolve(output, 'failure-dom.json'), boundedDiagnosticJson(diagnostic));
      const html = await page.content().catch(() => '');
      writeFileSync(resolve(output, 'failure.html'), Buffer.from(html).subarray(0, 1_000_000));
    }
    process.exitCode = 1;
  } finally {
    if (context) await context.setOffline(false).catch(() => {});
    if (browser) await browser.close();
    clearTimeout(hardStop);
    writeFileSync(resolve(output, 'browser-result.json'), JSON.stringify(result, null, 2) + '\n');
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await run();
}
