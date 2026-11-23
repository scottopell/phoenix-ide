import fs from 'node:fs/promises';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { chromium } from 'playwright';
import { qualifyQuestionSurface } from './question-surface-journeys.mjs';

// Synthetic HTTP isolation of the production UI, not server or iPhone latency.
const out = path.resolve(process.env.QUESTION_QA_OUT ?? 'qa-artifacts/question-loading');
const runs = Number(process.env.QUESTION_QA_RUNS ?? 8);
const versions = [['baseline', process.env.BASELINE_URL ?? 'http://127.0.0.1:4173'], ['candidate', process.env.CANDIDATE_URL ?? 'http://127.0.0.1:4174']];
const conditions = { cpu: 4, latency_ms: 400, bytes_per_second: 1048576, browser: 'hosted Chromium', cache: 'fresh context per trial; warm SPA has cached shell/feed only' };
const id = 'question-qualification';
const timestamp = '2026-01-01T12:00:00Z';
const marker = 'QUESTION QUALIFICATION TRANSCRIPT TAIL';
const conversation = { id, slug: id, title: 'Disposable long question history', model: 'fixture', connection: 'fixture', archived: false, favorited: false, task: false, agent_working: false, mode: 'explore', cwd: '/tmp/question-qualification', execution_mode: 'explore', work_scope_key: null, created_at: timestamp, updated_at: timestamp };
const messages = Array.from({ length: 100 }, (_, n) => ({ message_id: `question-message-${n}`, conversation_id: id, sequence_id: n + 1, message_type: n % 2 ? 'agent' : 'user', content: n % 2 ? [{ type: 'text', text: n === 99 ? marker : `Historical analysis ${n}.\n\n${'Bounded **synthetic** historical detail, not production data. '.repeat(100)}` }] : { text: `Synthetic request ${n}`, is_meta: false }, display_data: null, usage_data: null, event_presentation: null, created_at: timestamp, origin: { kind: n % 2 ? 'unknown_historical' : 'user_api' }, ...(n % 2 ? { state_snapshot: { type: 'idle' } } : {}) }));
const snapshot = { product_conversation_id: 'question-product', canonical_route: `/global/${id}`, requested_transcript_row_id: id, canonical_root: { transcript_row_id: id, slug: id, title: conversation.title }, latest_transcript_row_id: id, writable_transcript_row_id: id, latest_opening: { status: 'none' }, updated_at: timestamp, segments: [{ transcript_row_id: id, segment_ordinal: 0, slug: id, title: conversation.title, messages }], boundaries: [], before: null, has_older: false };
const questions = [{ request_id: 'question-request-1', coordinator_product_conversation_id: 'question-product', asked_at: timestamp, questions: [{ header: 'Scope', question: `Which bounded qualification scope should be used?\n\n${'This long synthetic description explains the choices without changing release scope or granting approval. '.repeat(24)}`, options: [{ label: 'Automatic', description: 'Use the bounded disposable qualification only.' }, { label: 'Manual', description: 'Keep release operations separately owned.' }], multiSelect: false }, { header: 'Surfaces', question: 'Select the requested surfaces.', options: [{ label: 'Mobile', description: '393px viewport.' }, { label: 'Desktop', description: '1440px viewport.' }], multiSelect: true }] }];
const exactReadReference = route => JSON.parse(route.request().postData() ?? '{}').reference;
const init = { opening: null, sequence_id: 100, conversation, transcript_generation: 1, transcript_coverage: 'complete', messages, steering_messages: [], agent_working: false, last_sequence_id: 100, stream_incarnation: 'question-qualification-stream', presentation_mode: 'idle', context_window_size: 128000, project_name: null, pending_anchor_sequence_id: 100, pending_events: [], pending_truncated: false };
const metrics = response => Object.fromEntries(response.metrics.map(({ name, value }) => [name, value]));
await fs.mkdir(out, { recursive: true });
const browser = await chromium.launch({ headless: true });
const results = [];
let failed = false;

async function trial(version, base, viewport, mode, run, isolation = false) {
  const name = `${version}-${viewport.width}-${mode}-${isolation ? 'isolation' : run}`;
  const context = await browser.newContext({ viewport, ...(viewport.width < 768 ? { isMobile: true, hasTouch: true } : {}), serviceWorkers: 'block' });
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await cdp.send('Network.enable');
  await cdp.send('Performance.enable');
  // Keep fixture streams open logically, as the existing Ladle fixtures do.
  // HTTP snapshots/assets remain real requests; SSE transport is not measured.
  await page.addInitScript(({ init, latency }) => {
    class FixtureEventSource extends EventTarget {
      static CONNECTING = 0; static OPEN = 1; static CLOSED = 2;
      CONNECTING = 0; OPEN = 1; CLOSED = 2; readyState = 0;
      onopen = null; onmessage = null; onerror = null;
      constructor(url) {
        super(); this.url = String(url); this.withCredentials = false;
        this.timer = setTimeout(() => {
          if (this.readyState === 2) return;
          this.readyState = 1;
          const open = new Event('open'); this.onopen?.(open); this.dispatchEvent(open);
          if (this.url.includes('/conversation-reads/stream')) {
            this.dispatchEvent(new MessageEvent('init', { data: JSON.stringify(init) }));
          }
        }, latency);
      }
      close() { clearTimeout(this.timer); this.readyState = 2; }
    }
    window.EventSource = FixtureEventSource;
  }, { init, latency: conditions.latency_ms });
  await cdp.send('Emulation.setCPUThrottlingRate', { rate: conditions.cpu });
  await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: conditions.latency_ms, downloadThroughput: conditions.bytes_per_second, uploadThroughput: conditions.bytes_per_second });
  const requests = new Map();
  const diagnostics = [];
  const unknown = [];
  const longTasks = [];
  let start = performance.now();
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  let historyPending = false;
  let historyReplied = false;
  let feedLoaded = false;
  let answerError = false;
  let pendingQuestions = structuredClone(questions);
  await page.addInitScript(() => {
    window.__questionLongTasks = [];
    new PerformanceObserver(list => {
      window.__questionLongTasks.push(...list.getEntries().map(e => ({ start: e.startTime, duration: e.duration })));
    }).observe({ type: 'longtask', buffered: true });
  });
  page.on('pageerror', error => diagnostics.push({ type: 'pageerror', message: error.message }));
  page.on('console', message => { if (message.type() === 'error' || message.type() === 'warning') diagnostics.push({ type: message.type(), message: message.text() }); });
  cdp.on('Network.requestWillBeSent', event => requests.set(event.requestId, { url: event.request.url, method: event.request.method, started_ms: performance.now() - start, resource: event.type }));
  cdp.on('Network.responseReceived', event => { const item = requests.get(event.requestId); if (item) Object.assign(item, { status: event.response.status, response_ms: performance.now() - start, mime: event.response.mimeType, from_cache: event.response.fromDiskCache ?? false }); });
  cdp.on('Network.loadingFinished', event => { const item = requests.get(event.requestId); if (item) Object.assign(item, { completed_ms: performance.now() - start, encoded_bytes: event.encodedDataLength }); });
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url());
    const p = url.pathname;
    const json = data => route.fulfill({ contentType: 'application/json', body: JSON.stringify(data) });
    // CDP throttles real bundle requests; intercepted API latency is modeled explicitly.
    await new Promise(resolve => setTimeout(resolve, conditions.latency_ms));
    if (p === '/api/telemetry/product-conversation-open' || p === '/api/telemetry/conversation-open' || p === '/api/telemetry/conversation-view') return route.fulfill({ status: 204, body: '' });
    if (p === '/api/product-conversations/events') return route.fulfill({ contentType: 'text/event-stream', body: ': synthetic aggregate stream\n\n' });
    if (p === '/api/product-conversations/question-product/read') return json({ state: 'read' });
    if (p.endsWith('/proposals')) return json({ proposals: [] });
    if (p === '/api/auth/status') return json({ auth_required: false, authenticated: true });
    if (p === '/api/coordinator/questions' && route.request().method() === 'GET') { feedLoaded = true; return json(pendingQuestions); }
    if (/\/api\/coordinator\/questions\/[^/]+\/(answer|dismiss)$/.test(p) && route.request().method() === 'POST') {
      if (answerError) return route.fulfill({ status: 409, contentType: 'application/json', body: JSON.stringify({ error: 'Question is obsolete. Refresh before answering.' }) });
      pendingQuestions = []; return json({ success: true });
    }
    if (p === '/api/global/coordinator') return json({ conversation, created: false });
    if (p === '/api/global/coordinator/reads/route' && route.request().method() === 'POST') {
      if (exactReadReference(route) !== id) throw new Error('Coordinator route fixture received an unexpected reference');
      return json({ coordinator_id: id });
    }
    if (p === '/api/global/coordinator/reads/history' && route.request().method() === 'POST') {
      if (exactReadReference(route) !== id) throw new Error('Coordinator history fixture received an unexpected reference');
      historyPending = true;
      if (isolation) await gate;
      historyReplied = true;
      return json(snapshot);
    }
    if ((p === '/api/conversation-reads/route' || p === '/api/conversation-reads/route/by-slug') && route.request().method() === 'POST') {
      if (exactReadReference(route) !== id) throw new Error('Conversation route fixture received an unexpected reference');
      return json({ id, slug: id });
    }
    if (p === '/api/conversation-reads/snapshot' && route.request().method() === 'POST') return json({ conversation, messages, agent_working: false, presentation_mode: 'idle', context_window_size: 128000 });
    if (p === '/api/conversation-reads/stream' && route.request().method() === 'GET') return route.fulfill({ contentType: 'text/event-stream', body: `event: init\ndata: ${JSON.stringify(init)}\n\n` });
    if (p === '/api/conversations' || p === '/api/conversations/archived' || p === '/api/coordinator/watches' || p === '/api/coordinator/close-watches' || p === '/api/coordinator/bash/live' || p.endsWith('/reminders') || p.endsWith('/fork-proposals')) return json([]);
    if (p === '/api/product-conversations') return json({ product_conversations: [], coordinator: null });
    if (p === '/api/models') return json({ models: [], default: '', llm_configured: false, credential_status: 'not_configured' });
    if (p === '/api/codex/login/preflight') return json({ auth_path: '/tmp/disposable', already_signed_in: false, bridge_loaded_at_startup: false, restart_required_after_login: false, account_id: null, account_email: null });
    if (p === '/api/global/coordinator/language' || p === '/api/settings/llm-language') return json({ language: 'en', available: ['en'], languages: [{ id: 'en', label: 'English' }] });
    if (p.endsWith('/automatic-continuation')) return json({ aggregate: { kind: 'coordinator', product_conversation_id: 'question-product' }, auto_continue_on_context_exhaustion: false, admission: null });
    if (p === '/api/reminders/due') return json({ cursor: timestamp, instances: [] });
    if (p === '/api/settings/notifications') return json({ enabled: false, sound: false, desktop: false, push: false });
    if (p.endsWith('/system-prompt')) return json({ prompt: '' });
    if (p.endsWith('/inventory')) return json({ scope_key: 'fixture', bash: [], tmux: null, browser: null });
    if (p.endsWith('/read-state')) return json({});
    unknown.push({ method: route.request().method(), path: p });
    return route.fulfill({ status: 501, contentType: 'application/json', body: JSON.stringify({ error: `Unmodelled fixture request ${p}` }) });
  });
  const record = { name, version, viewport, mode, run, isolation, conditions, synthetic: true, requests: [], diagnostics, unknown };
  try {
    if (mode === 'warm-notification') {
      await page.goto(`${base}/conversations`, { waitUntil: 'domcontentloaded' });
      await page.waitForFunction(() => document.querySelector('.desktop-layout, .app-layout, nav') !== null);
      if (!feedLoaded) await page.waitForResponse(response => response.url().includes('/api/coordinator/questions'));
      if (!feedLoaded) throw new Error('Warm shell did not acquire question feed');
    }
    const before = metrics(await cdp.send('Performance.getMetrics'));
    start = performance.now();
    if (mode === 'warm-notification') await page.evaluate(() => window.dispatchEvent(new CustomEvent('phoenix:navigate-to-conversation', { detail: { route: '/global' } })));
    else await page.goto(`${base}${mode === 'cold-link' ? `/global/${id}` : '/global'}`, { waitUntil: 'commit' });
    // The pinned baseline renders the card without the candidate-only dock wrapper.
    const panel = page.locator('.question-panel');
    await panel.waitFor({ state: 'visible', timeout: 45000 });
    record.question_visible_ms = performance.now() - start;
    const automatic = panel.locator('.question-option').filter({ hasText: 'Automatic' }).first();
    await automatic.click();
    if (!await automatic.evaluate(el => el.classList.contains('selected'))) throw new Error('Option click did not update selection');
    const custom = panel.getByRole('textbox', { name: 'Write a custom answer' });
    await custom.fill('Disposable retained custom answer');
    if (await custom.inputValue() !== 'Disposable retained custom answer') throw new Error('Custom answer was not usable');
    record.question_usable_ms = performance.now() - start;
    record.history_replied_at_question_usable = historyReplied;
    if (isolation && historyReplied) throw new Error('Controlled history replied before question interaction');
    release();
    // Return without dismissing on candidate mobile; baseline has no modal.
    const returnButton = page.getByRole('button', { name: 'Return to conversation', exact: true });
    if (await returnButton.isVisible()) await returnButton.click();
    await page.getByText(marker, { exact: true }).waitFor({ state: 'visible', timeout: 45000 });
    record.transcript_ready_ms = performance.now() - start;
    const after = metrics(await cdp.send('Performance.getMetrics'));
    record.metrics_delta = Object.fromEntries(['TaskDuration', 'ScriptDuration', 'LayoutDuration', 'RecalcStyleDuration', 'JSHeapUsedSize'].map(key => [key, after[key] - before[key]]));
    record.history_requested = historyPending;
    longTasks.push(...await page.evaluate(() => window.__questionLongTasks));
    record.long_tasks = longTasks;
    await page.screenshot({ path: path.join(out, `${name}-conversation.png`) });
    const launcher = page.getByRole('button', { name: /Answer Ember/ });
    if (await launcher.isVisible()) await launcher.click();
    if (await custom.inputValue() !== 'Disposable retained custom answer') throw new Error('Return lost custom answer');
    await page.screenshot({ path: path.join(out, `${name}-question.png`) });
    if (version === 'candidate' && isolation && viewport.width === 393) {
      await qualifyQuestionSurface({ page, out, questions,
        setQuestions: next => { pendingQuestions = next; },
        setAnswerError: value => { answerFails = value; },
      });
    }
    if (unknown.length) throw new Error(`Unmodelled requests: ${JSON.stringify(unknown)}`);
    if (diagnostics.some(item => item.type === 'pageerror')) throw new Error('Production UI reported page errors');
    record.ok = true;
  } catch (error) {
    record.ok = false; record.error = String(error); failed = true;
    await page.screenshot({ path: path.join(out, `${name}-failure.png`) }).catch(() => {});
  } finally {
    release();
    record.requests = [...requests.values()];
    await fs.writeFile(path.join(out, `${name}.json`), JSON.stringify(record, null, 2));
    results.push(record);
    await context.close();
  }
  console.log(JSON.stringify({ name, ok: record.ok, question_ms: record.question_usable_ms, transcript_ms: record.transcript_ready_ms, error: record.error }));
  return record.ok;
}
try {
  for (const viewport of [{ width: 393, height: 852 }, { width: 1440, height: 900 }]) {
    for (const mode of ['cold-global', 'cold-link', 'warm-notification']) {
      // Alternate exact base/candidate within each trial to bound host drift.
      for (let run = -1; run < runs; run++) {
        for (const [version, url] of run % 2 ? [...versions].reverse() : versions) {
          if (!await trial(version, url, viewport, mode, run)) throw new Error('Stop on first invalid qualification sample; artifacts retained');
        }
      }
    }
    for (const [version, url] of versions) if (!await trial(version, url, viewport, 'cold-global', 0, true)) throw new Error('History isolation failed');
  }
} catch (error) { failed = true; console.error(error); }
finally {
  await fs.writeFile(path.join(out, 'results.json'), JSON.stringify({ baseline_sha: process.env.BASELINE_SHA, candidate_sha: process.env.CANDIDATE_SHA, conditions, scope: 'Synthetic intercepted HTTP, production bundles, real render and request order; excludes server/database and physical keyboard latency', results }, null, 2));
  await browser.close();
}
if (failed) process.exitCode = 1;
