import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import path from 'node:path';

export const findFinalizationId = 'find-identical-finalization';
export const findFinalizationViewport = { width: 1000, height: 800 };

// This observation window exceeds the original pending-reveal expiry (5s).
const drainMs = 5500;
const modes = ['streaming-active', 'historical-active', 'closed', 'streaming-active-no-wheel'];

function installProbe() {
  const commands = [];
  const restore = [];
  const record = (kind, element, args) => commands.push({
    time: performance.now(), kind, id: element.id, args,
  });
  for (const name of ['scrollIntoView', 'scrollTo', 'scrollBy']) {
    const original = Element.prototype[name];
    Element.prototype[name] = function (...args) {
      record(name, this, args);
      return original.apply(this, args);
    };
    restore.push(() => { Element.prototype[name] = original; });
  }
  const descriptor = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollTop');
  Object.defineProperty(Element.prototype, 'scrollTop', {
    ...descriptor,
    set(value) { record('scrollTop=', this, [value]); descriptor.set.call(this, value); },
  });
  restore.push(() => Object.defineProperty(Element.prototype, 'scrollTop', descriptor));
  const originalFocus = HTMLElement.prototype.focus;
  HTMLElement.prototype.focus = function (...args) {
    record('focus', this, args);
    return originalFocus.apply(this, args);
  };
  restore.push(() => { HTMLElement.prototype.focus = originalFocus; });

  function measure() {
    const scroller = document.querySelector('#messages');
    if (!scroller) throw new Error('Missing MessageList scroller');
    const texts = [...scroller.querySelectorAll('p')]
      .filter((element) => /Reading marker|alpha/.test(element.textContent))
      .map((element) => {
        const range = document.createRange();
        range.selectNodeContents(element);
        const rect = range.getBoundingClientRect();
        return { text: element.textContent, top: rect.top, left: rect.left, height: rect.height };
      });
    const highlights = [...scroller.querySelectorAll('[data-viewer-find-active-target]')].map((element) => ({
      text: element.textContent,
      connected: element.isConnected,
      ownerText: element.closest('p')?.textContent,
      top: element.getBoundingClientRect().top,
      bottom: element.getBoundingClientRect().bottom,
    }));
    // Streaming rows expose selection at row level rather than an inline mark.
    if (highlights.length === 0 && document.querySelector('[data-find-finalization="streaming"]')) {
      const row = scroller.querySelector('.viewer-find-row-match--active');
      const walker = row && document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
      for (let node = walker?.nextNode(); node; node = walker.nextNode()) {
        const start = node.textContent.indexOf('alpha');
        if (start < 0) continue;
        const range = document.createRange();
        range.setStart(node, start);
        range.setEnd(node, start + 5);
        const rect = range.getBoundingClientRect();
        highlights.push({
          text: range.toString(), connected: node.isConnected,
          ownerText: node.parentElement.closest('p')?.textContent,
          top: rect.top, bottom: rect.bottom,
        });
        break;
      }
    }
    const bounds = scroller.getBoundingClientRect();
    return {
      time: performance.now(), scrollTop: scroller.scrollTop, texts, highlights,
      bounds: { top: bounds.top, bottom: bounds.bottom },
      status: document.querySelector('.viewer-find-bar__status')?.textContent ?? null,
      finalized: document.querySelector('[data-find-finalization]')?.getAttribute('data-find-finalization') === 'final',
      commandCount: commands.length,
    };
  }
  window.__findFinalizationProbe = {
    commands, measure,
    restore: () => restore.splice(0).reverse().forEach((undo) => undo()),
    async settle(minMs) {
      const start = performance.now();
      let previous = '';
      let stable = 0;
      while (performance.now() - start < 12000) {
        await new Promise(requestAnimationFrame);
        const sample = measure();
        const signature = JSON.stringify([sample.scrollTop, sample.texts, sample.status, sample.commandCount]);
        stable = signature === previous ? stable + 1 : 0;
        previous = signature;
        if (performance.now() - start >= minMs && stable >= 12) return sample;
      }
      throw new Error('MessageList never reached 12 stable animation frames');
    },
    async finalize() {
      commands.length = 0;
      const before = measure();
      const frames = [];
      let recording = true;
      const sampleFrame = () => {
        if (!recording) return;
        frames.push(measure());
        requestAnimationFrame(sampleFrame);
      };
      requestAnimationFrame(sampleFrame);
      window.__messageListFinalizeFind();
      try {
        const after = await this.settle(1200);
        return { before, after, frames, commands: [...commands] };
      } finally { recording = false; }
    },
  };
}

function assertHighlight(sample, expected) {
  assert(sample.highlights.some((highlight) => highlight.connected && highlight.text === 'alpha'),
    `${expected}: active alpha highlight must remain connected`);
}

function assertStable(result, mode) {
  assert(result.after.finalized, `${mode}: fixture did not finalize`);
  assert.equal(result.commands.length, 0, `${mode}: finalization issued commands: ${JSON.stringify(result.commands)}`);
  assert(result.frames.length >= 12, `${mode}: missing RAF samples`);
  const visible = result.before.texts.filter((text) => text.top >= result.before.bounds.top && text.top < result.before.bounds.bottom);
  assert(visible.length > 0, `${mode}: no reading text measured`);
  for (const frame of [...result.frames, result.after]) {
    assert(Math.abs(frame.scrollTop - result.before.scrollTop) <= 2, `${mode}: transient scroll drift`);
    for (const before of visible) {
      const after = frame.texts.find((text) => text.text === before.text);
      assert(after, `${mode}: reading text disappeared: ${before.text}`);
      assert(Math.abs(after.top - before.top) <= 2 && Math.abs(after.left - before.left) <= 2,
        `${mode}: transient text drift for ${before.text}: ${after.top - before.top}px`);
    }
  }
  if (mode !== 'closed') {
    assert.equal(result.after.status, result.before.status, `${mode}: active selection changed`);
    assertHighlight(result.after, mode);
    assert.deepEqual(result.after.highlights.map((item) => item.ownerText), result.before.highlights.map((item) => item.ownerText), `${mode}: highlighted owner changed`);
  } else {
    assert.equal(result.after.status, null, 'Closed Find reopened');
    assert.equal(result.after.highlights.length, 0, 'Closed Find gained highlights');
  }
}

export async function captureFindFinalization({ page, id, outDir }) {
  if (id !== findFinalizationId) return false;
  await page.setViewportSize(findFinalizationViewport);
  const results = [];
  try {
    for (const mode of modes) {
      // Fresh React state, same capture-owned browser/page and error listeners.
      await page.reload({ waitUntil: 'networkidle' });
      await page.waitForSelector('[data-find-finalization="streaming"]');
      await page.waitForFunction(() => typeof window.__messageListFinalizeFind === 'function');
      await page.evaluate(installProbe);
      await page.evaluate(() => window.__findFinalizationProbe.settle(250));
      await page.evaluate(() => window.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', metaKey: true, bubbles: true })));
      await page.getByRole('textbox', { name: 'Find in viewer' }).fill('alpha');
      await page.waitForFunction(() => document.querySelector('.viewer-find-bar__status')?.textContent === '1 of 2');
      if (mode !== 'historical-active') {
        await page.getByRole('button', { name: 'Next', exact: true }).click();
        await page.waitForFunction(() => document.querySelector('.viewer-find-bar__status')?.textContent === '2 of 2');
      }
      if (mode === 'closed') await page.getByRole('textbox', { name: 'Find in viewer' }).press('Escape');
      const settled = await page.evaluate((ms) => window.__findFinalizationProbe.settle(ms), drainMs);
      if (mode !== 'closed') assertHighlight(settled, mode);
      if (mode !== 'streaming-active-no-wheel') {
        await page.locator('#messages').hover();
        await page.mouse.wheel(0, 180);
        await page.waitForFunction((top) => document.querySelector('#messages').scrollTop > top + 20, settled.scrollTop);
      }
      const reading = await page.evaluate((ms) => window.__findFinalizationProbe.settle(ms), drainMs);
      await page.screenshot({ path: path.join(outDir, `${id}--${mode}--before.png`) });
      const result = await page.evaluate(() => window.__findFinalizationProbe.finalize());
      results.push({ mode, settled, reading, ...result, navigation: [] });
      assertStable(result, mode);
      await page.screenshot({ path: path.join(outDir, `${id}--${mode}--after.png`) });
      if (mode !== 'closed') {
        for (const direction of ['Next', 'Previous']) {
          const previousStatus = await page.locator('.viewer-find-bar__status').textContent();
          await page.evaluate(() => { window.__findFinalizationProbe.commands.length = 0; });
          await page.getByRole('button', { name: direction, exact: true }).click();
          await page.waitForFunction((previous) => document.querySelector('.viewer-find-bar__status')?.textContent !== previous, previousStatus);
          const navigation = await page.evaluate(() => window.__findFinalizationProbe.settle(600));
          results.at(-1).navigation.push({ direction, ...navigation });
          assertHighlight(navigation, `${mode} ${direction}`);
          assert(navigation.commandCount > 0, `${mode} ${direction}: no reveal command`);
          assert(navigation.highlights.some((item) => item.top >= navigation.bounds.top && item.bottom <= navigation.bounds.bottom), `${mode} ${direction}: active match not revealed in viewport`);

        }
      }
      await page.evaluate(() => window.__findFinalizationProbe.restore());
    }
  } finally {
    await writeFile(path.join(outDir, `${id}--trace.json`), `${JSON.stringify(results, null, 2)}\n`);
    await page.evaluate(() => window.__findFinalizationProbe?.restore()).catch(() => {});
  }
  console.log('  verified settled Find finalization: active wheel, historical, closed, and no-wheel transient controls');
  return true;
}
