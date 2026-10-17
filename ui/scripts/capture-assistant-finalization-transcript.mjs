import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

assert.ok(process.env.LADLE_URL, 'LADLE_URL must identify an existing fixture server');
const out = path.resolve(process.env.FINALIZATION_QA_OUT_DIR ?? 'qa-artifacts/finalization-transcript');
await mkdir(out, { recursive: true });
const browser = await chromium.launch();
const results = [];
function snapshot(marker) {
  const scroller = document.querySelector('#messages');
  const paragraphs = [...scroller.querySelectorAll('p')].filter(p => p.textContent.startsWith('Reading marker'));
  const box = scroller.getBoundingClientRect();
  const paragraph = marker ? paragraphs.find(p => p.textContent === marker)
    : paragraphs.find(p => p.getBoundingClientRect().top >= box.top && p.getBoundingClientRect().bottom <= box.bottom);
  return { top: scroller.scrollTop, gap: scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop,
    marker: paragraph?.textContent, y: paragraph?.getBoundingClientRect().top,
    width: paragraph?.getBoundingClientRect().width, height: paragraph?.getBoundingClientRect().height,
    text: paragraphs.map(p => p.textContent) };
}
try {
  for (const width of [375, 1280]) for (const density of ['full', 'compact']) {
    for (const retry of [false, true]) for (const mode of ['reader', 'pinned']) {
      const context = await browser.newContext({ viewport: { width, height: 900 } });
      const result = { width, density, retry, mode, errors: [], samples: [] };
      results.push(result);
      try {
        const page = await context.newPage();
        page.on('pageerror', e => result.errors.push(e.message));
        const url = new URL(process.env.LADLE_URL);
        url.searchParams.set('story', `assistant-finalization-transcript--${density}${retry ? '-retry' : ''}`);
        url.searchParams.set('mode', 'preview');
        await page.goto(url.href);
        await page.waitForSelector('[data-transcript-ready="true"]');
        await page.waitForFunction(() => document.querySelectorAll('#messages p').length >= 24);
        await page.waitForFunction(() => {
          const e = document.querySelector('#messages');
          return e.scrollHeight > e.clientHeight && e.scrollHeight - e.clientHeight - e.scrollTop < 2;
        });
        if (mode === 'reader') {
          await page.locator('#messages').hover();
          await page.mouse.wheel(0, -550);
          await page.waitForFunction(() => {
            const e = document.querySelector('#messages');
            return e.scrollHeight - e.clientHeight - e.scrollTop > 500;
          });
        }
        // Observe the real scroll policy's 3000ms settle window; this is a negative
        // delayed-snap observation after readiness, not a substitute for readiness.
        await page.waitForTimeout(3500);
        result.before = await page.evaluate(snapshot);
        assert.ok(result.before.marker, 'a fully visible reading paragraph must be present');
        await page.getByTestId('finalize-transcript').click();
        await page.waitForSelector('[data-transcript-finalized="true"] #message-finalization-response');
        await page.waitForFunction(() => !document.querySelector('#messages .streaming-cursor'));
        for (let sample = 0; sample < 36; sample++) {
          result.samples.push(await page.evaluate(snapshot, result.before.marker));
          await page.waitForTimeout(100);
        }
        result.after = result.samples.at(-1);
        assert.deepEqual(result.errors, []);
        assert.equal(await page.locator('#message-finalization-response .message-review-action').count(), 1);
        assert.equal(await page.locator('#message-finalization-response .message-mobile-copy').count(), 1);
        assert.equal(await page.locator('#message-finalization-response .message-retry-badge').count(), retry ? 1 : 0);
        for (const sample of result.samples) {
          assert.deepEqual(sample.text, result.before.text, 'finalization changed prose');
          assert.equal(sample.width, result.before.width, 'finalization changed prose width');
          assert.equal(sample.height, result.before.height, 'finalization changed paragraph wrapping');
          if (mode === 'reader') {
            assert.ok(Math.abs(sample.top - result.before.top) <= 1, 'reader scrollTop moved');
            assert.ok(Math.abs(sample.y - result.before.y) <= 1, 'reader paragraph moved');
          } else assert.ok(sample.gap <= 2, `bottom-follow lost: ${sample.gap}`);
        }
        console.log(`PASS ${width}/${density}/${retry ? 'retry' : 'normal'}/${mode}`);
      } catch (error) {
        result.failure = String(error);
        throw error;
      } finally {
        await writeFile(path.join(out, 'transcript.json'), JSON.stringify(results, null, 2));
        await context.close();
      }
    }
  }
} finally { await browser.close(); }
