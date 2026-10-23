import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { installFullMessageProbe } from '../src/fixtures/touchReading/fullMessageProbe.mjs';
import { installTextContactSelector } from '../src/fixtures/touchReading/textContact.mjs';
import { runFullMessageScrollControl, inspectFullMessageScrollControl } from '../src/fixtures/touchReading/fullMessageScrollControl.mjs';
const modes = process.argv.includes('--detector-only') ? ['detector'] : ['stable', 'natural'];
if (!process.argv.includes('--admitted-host') || process.env.PHOENIX_TOUCH_HOST_ADMITTED !== '1') throw new Error('Host admission required');
const url = process.env.TOUCH_FIXTURE_URL;
if (url !== 'http://127.0.0.1:4179/src/fixtures/touchReading/index.html') throw new Error('Exact fixture URL required');
const out = process.env.TOUCH_QA_OUT;
if (!out || !path.isAbsolute(out)) throw new Error('Owned absolute output required');
await mkdir(out);
const { chromium } = await import('playwright');
const browser = await chromium.launch({ headless: true, timeout: 15000 });
const timer = setTimeout(() => void browser.close(), 240000);
const results = [];
const seed = 'TOUCH_STREAM_SEED_1729.\n\n';
const workload = Array.from({ length: 8 }, (_, i) => `Full-message growth paragraph ${i}. ${'Actual store consumption is distinct from committed text and physical row layout. '.repeat(3)}\n\n`).join('') + 'FULL_MESSAGE_GROWTH_END';
try {
  for (const mode of modes) {
    const context = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 1, hasTouch: true, isMobile: true, serviceWorkers: 'block' });
    const page = await context.newPage(); page.setDefaultTimeout(8000);
    const errors = []; page.on('pageerror', e => { if (errors.length < 16) errors.push(e.message.slice(0, 500)); });
    let controlReceipt = null, controlResult = null;
    let trace = null, error = null, beforeContact = null, before = null, after = null, consumed = null, streamRowTransition = null, status = 'UNOBSERVABLE';
    try {
      await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 15000 });
      await page.waitForFunction(() => window.__touchFixture?.ready() && document.querySelector('[data-product-conversation-fixture-ready]'));
      await page.evaluate(() => document.fonts.ready);
      await page.evaluate(installTextContactSelector);
      await page.evaluate(installFullMessageProbe);
      await page.evaluate(seed => window.__fullMessage.consume('seed', [seed]), seed);
      await page.waitForFunction(() => document.querySelector('#messages')?.textContent.includes('TOUCH_STREAM_SEED_1729'));
      // Setup-only scroll exercise records actual row additions/removals, not held-anchor remount coverage.
      await page.evaluate(() => window.__fullMessage.setupScroll('top'));
      await page.waitForFunction(() => window.__fullMessage.lifecycle().some(e => e.kind === 'dom-removed') && document.querySelector('.message[data-message-id="touch-reading-0"]'));
      await page.evaluate(() => window.__fullMessage.setupScroll('bottom'));
      await page.waitForFunction(() => document.querySelector('#messages')?.textContent.includes('TOUCH_STREAM_SEED_1729') && window.__fullMessage.lifecycle().some(e => e.kind === 'dom-added'));
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      const point = await page.evaluate(() => window.__fullMessage.point());
      beforeContact = await page.evaluate(() => window.__fullMessage.stream());
      const cdp = await context.newCDPSession(page);
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: point.x, y: point.y, id: 1 }] });
      if (!await page.evaluate(() => window.__fullMessage.admitted())) throw new Error('Actual owned contact not admitted');
      await page.waitForFunction(() => window.__fullMessage.heldReady());
      before = await page.evaluate(() => window.__fullMessage.stream());
      if (mode === 'detector') controlReceipt = await page.evaluate(runFullMessageScrollControl);
      if (mode === 'natural') {
        consumed = await page.evaluate(text => {
          if (!window.__fullMessage.heldReady()) throw new Error('Held contact lost before workload');
          return window.__fullMessage.consume('held-growth', [text], true);
        }, workload);
        await page.waitForFunction(() => document.querySelector('#messages')?.textContent.includes('FULL_MESSAGE_GROWTH_END'));
        after = await page.evaluate(() => window.__fullMessage.stream());
        if (!Number.isFinite(after.height) || after.height <= before.height || !after.text.includes('FULL_MESSAGE_GROWTH_END')) throw new Error('DOM text/row growth absent');
      } else after = await page.evaluate(() => window.__fullMessage.stream());
      streamRowTransition = before.node === after.node && before.key === after.key ? 'SAME_ROW' : 'STREAM_ROW_REPLACED';
      if (mode === 'natural' && streamRowTransition !== 'SAME_ROW') throw new Error('Continuation changed stream-row identity');
      await page.evaluate(() => new Promise(resolve => { let n = 0; const f = () => ++n >= 12 ? resolve() : requestAnimationFrame(f); requestAnimationFrame(f); }));
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
      trace = await page.evaluate(() => window.__fullMessage.result());
      if (trace.failure || errors.length) throw new Error('Contact/anchor/runtime observation incomplete');
      if (!trace.lifecycle.some(e => e.kind === 'dom-added') || !trace.lifecycle.some(e => e.kind === 'dom-removed')) throw new Error('Actual row lifecycle exercise missing');
      const held = trace.samples.filter(s => s.down);
      if (!held.length || held.some(s => !Number.isFinite(s.dx) || !Number.isFinite(s.dy))) throw new Error('Invalid raw trajectory');
      if (mode === 'natural' && (!consumed || consumed.t <= trace.baseline.t + 2000
        || !trace.layouts.some(e => e.node === after.node && e.key === after.key && e.t >= consumed.t
          && Number.isFinite(e.borderHeight) && e.borderHeight === after.height && e.borderHeight > before.height)
        || !trace.domMutations.some(e => e.t >= consumed.t) || !held.some(s => s.t > after.t)
        || (streamRowTransition === 'STREAM_ROW_REPLACED' && (!trace.lifecycle.some(e => e.kind === 'dom-removed' && e.node === before.node && e.t >= consumed.t)
          || !trace.lifecycle.some(e => e.kind === 'dom-added' && e.node === after.node && e.t >= consumed.t))))) throw new Error('Consumption/DOM-observation/layout/held boundary incomplete');
      if (mode === 'detector') {
        controlResult = inspectFullMessageScrollControl(trace, controlReceipt);
        if (controlResult.status !== 'SCROLL_DISPLACEMENT_DETECTOR_OBSERVED') throw new Error('Physical detector unobservable');
      }
      const moved = held.some(s => s.dx !== 0 || s.dy !== 0);
      status = mode === 'detector' ? controlResult.status : moved ? (mode === 'stable' ? 'STABLE_CONTROL_FAILED' : 'SAMPLED_POSITION_DELTA_UNQUALIFIED') : 'NO_SAMPLED_POSITION_DELTA_UNQUALIFIED';
      if (mode === 'stable' && moved) throw new Error('Stable control moved');
    } catch (e) {
      error = String(e); trace ??= await page.evaluate(() => window.__fullMessage?.result() ?? null).catch(() => null);
    } finally { await context.close(); }
    results.push({ mode, status, error, errors, controlReceipt, controlResult, beforeContact, before, after, consumed, streamRowTransition, trace,
      domEvidence: 'DIRECT_STREAM_TEXT_AND_HEIGHT_OBSERVATION; GENERIC_MO_DELIVERY_NOT_REACT_COMMIT',
      stressClassification: before?.relativeToAnchorGlyph === 'BELOW' ? 'BELOW_ANCHOR_NOT_ABOVE_CONTACT_STRESS' : before?.relativeToAnchorGlyph ?? 'UNOBSERVED',
      physicalDetector: controlResult?.status ?? 'UNAVAILABLE_NOT_A_PASS', claim: 'SAMPLED_ONLY_NOT_PAINT_IPHONE_OR_FLICK_REPRO',
      mountCoverage: 'ACTUAL_DOM_ROW_ADD_REMOVE_SETUP; HELD_ANCHOR_REMOUNT_NOT_REQUIRED_OR_CLAIMED' });
    const bytes = JSON.stringify(results); if (Buffer.byteLength(bytes) > 2000000) throw new Error('Artifact budget');
    await writeFile(path.join(out, 'result.json'), bytes);
    if (error) break;
  }
} finally { clearTimeout(timer); await browser.close(); }
process.exitCode = results.length === modes.length && results.every(r => !r.error) ? 0 : 1;
