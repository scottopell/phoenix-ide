import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import path from 'node:path';

const imagePattern = 'https://message-list-fixture.invalid/delayed-image.svg?*';
const imageResponse = {
  status: 200,
  contentType: 'image/svg+xml',
  headers: { 'cache-control': 'no-store' },
  body: '<svg xmlns="http://www.w3.org/2000/svg" width="640" height="480"><rect width="640" height="480" fill="#386891"/></svg>',
};

export async function prepareDelayedImages({ page }) {
  // The shared runner's initial navigation waits for networkidle. Measured
  // cases reload below with domcontentloaded and a new held-response route.
  await page.route(imagePattern, (route) => route.fulfill(imageResponse));
}

export async function captureDelayedImages({ page, id, outDir }) {
  if (id !== 'delayed-image-scroll') return false;
  await page.setViewportSize({ width: 1100, height: 800 });
  const samples = [];
  const scroller = page.locator('#messages');
  const marker = page.locator('#messages p').filter({
    hasText: /^Keep this exact sentence visible while the image is pending and after its bytes load\.$/,
  });
  const settle = () => page.waitForTimeout(800);
  const sample = async (mode, phase) => {
    const geometry = await scroller.evaluate((element) => ({
      top: element.scrollTop,
      height: element.scrollHeight,
      client: element.clientHeight,
      gap: element.scrollHeight - element.clientHeight - element.scrollTop,
      scroller: element.getBoundingClientRect().toJSON(),
      rows: Array.from(element.querySelectorAll('[data-render-unit-key]')).map((row) => ({
        key: row.getAttribute('data-render-unit-key'),
        ...row.getBoundingClientRect().toJSON(),
      })),
      images: Array.from(element.querySelectorAll('img')).map((image) => ({
        id: new URL(image.src).searchParams.get('id'),
        complete: image.complete,
        width: image.naturalWidth,
        row: image.closest('[data-render-unit-key]')?.getAttribute('data-render-unit-key'),
        rect: image.getBoundingClientRect().toJSON(),
      })),
    }));
    const reading = await marker.evaluate((element) => {
      const row = element.closest('[data-render-unit-key]');
      const rect = element.getBoundingClientRect();
      return { y: rect.top, marker: rect.toJSON(), markerRow: row?.getAttribute('data-render-unit-key'),
        localOffset: row ? rect.top - row.getBoundingClientRect().top : null };
    });
    const value = { mode, phase, ...reading, ...geometry };
    samples.push(value);
    return value;
  };
  const reader = (value) => assert.ok(Math.abs(value.y - 152) <= 2, `Reader drift: ${JSON.stringify(value)}`);
  const tail = (value) => assert.ok(Math.abs(value.gap) <= 2, `Tail drift: ${JSON.stringify(value)}`);
  const readingGeometry = (value) => {
    const earlier = value.rows.find((row) => row.key === value.images.find((image) => image.id === 'earlier')?.row);
    const anchor = value.rows.find((row) => row.key === value.markerRow);
    const image = value.images.find((item) => item.id === 'anchor');
    const detail = JSON.stringify(value);
    assert.ok(earlier && anchor && image, `Missing reading geometry: ${detail}`);
    assert.ok(earlier.bottom <= value.scroller.top - 4, `Earlier row must be fully above the viewport: ${detail}`);
    assert.ok(anchor.top < value.scroller.top && anchor.bottom > value.scroller.top,
      `The sentence's own row must cross the reading edge: ${detail}`);
    assert.equal(image.row, value.markerRow, `Delayed image must share the sentence's row: ${detail}`);
    assert.ok(image.rect.bottom <= value.y, `Same-row image must precede the sentence: ${detail}`);
    assert.ok(value.marker.top >= value.scroller.top && value.marker.bottom <= value.scroller.bottom,
      `Whole sentence must be visible: ${detail}`);
    assert.ok(value.gap > 100, `Reading position must be well away from the tail: ${detail}`);
  };
  const images = (value, loaded) => {
    const tailAfterGrowth = ['follow-latest', 'end-navigation'].includes(value.mode) && loaded.length === 2;
    if (tailAfterGrowth) {
      assert.ok(value.images.some((image) => image.id === 'anchor'), 'The reading row remains mounted at the tail');
    } else {
      assert.equal(value.images.length, 2, 'Both real image elements must be mounted');
    }
    for (const image of value.images) {
      assert.equal(image.complete, loaded.includes(image.id), JSON.stringify(value));
      assert.equal(image.width, loaded.includes(image.id) ? 640 : 0, JSON.stringify(value));
    }
  };

  try {
    for (const mode of ['reader-1', 'reader-2', 'reader-3', 'reader-4', 'rapid-successive', 'concurrent', 'follow-latest', 'end-navigation']) {
      const pending = new Map();
      const counts = new Map();
      const hold = (route) => {
        const key = new URL(route.request().url()).searchParams.get('id');
        counts.set(key, (counts.get(key) ?? 0) + 1);
        pending.set(key, route);
      };
      await page.route(imagePattern, hold);
      try {
        // Routing disables HTTP cache; every case gets a fresh document and
        // actual pending image requests, not synthetic dimension mutations.
        await page.reload({ waitUntil: 'domcontentloaded' });
        await page.waitForSelector('[data-message-list-fixture-ready="delayed-image-scroll"]');
        await marker.waitFor();
        await page.waitForFunction(() => document.querySelectorAll('#messages img').length === 2);
        // Drain scrollMachine's 3000ms mount-rescue window and 150ms probe cadence
        // before the programmatic reader setup can race its pending native scroll event.
        // This is not positioning success: the before/quiet geometry checks below remain required.
        await page.waitForTimeout(3300);
        assert.deepEqual([...pending.keys()].sort(), ['anchor', 'earlier']);
        if (mode === 'follow-latest') {
          await scroller.evaluate((element) => { element.scrollTop = element.scrollHeight; });
        } else {
          const y = await marker.evaluate((element) => element.getBoundingClientRect().top);
          await scroller.evaluate((element, delta) => { element.scrollTop += delta; }, y - 152);
        }
        await settle();
        const before = await sample(mode, 'before');
        images(before, []);
        (mode === 'follow-latest' ? tail : reader)(before);
        await settle();
        const quiet = await sample(mode, 'quiet');
        images(quiet, []);
        (mode === 'follow-latest' ? tail : reader)(quiet);
        if (mode !== 'follow-latest') {
          readingGeometry(before);
          readingGeometry(quiet);
        }

        if (mode === 'end-navigation') {
          await scroller.focus();
          await page.keyboard.press('End');
          await settle();
          const end = await sample(mode, 'intentional-end');
          tail(end);
          assert.ok(end.top > before.top + 20, 'End must actually leave the reading position');
        }
        const release = async (keys, settleMs = 800) => {
          await Promise.all(keys.map(async (key) => {
            assert.ok(pending.has(key), `Missing held request: ${key}`);
            await pending.get(key).fulfill(imageResponse);
            pending.delete(key);
          }));
          await page.waitForFunction((loaded) => loaded.every((key) => {
            const image = Array.from(document.querySelectorAll('#messages img'))
              .find((node) => new URL(node.src).searchParams.get('id') === key);
            return image?.complete && image.naturalWidth === 640;
          }), keys);
          await page.waitForTimeout(settleMs);
        };
        const check = mode === 'follow-latest' || mode === 'end-navigation' ? tail : reader;
        if (mode === 'concurrent') {
          await release(['earlier', 'anchor']);
        } else {
          await release(['earlier'], mode === 'rapid-successive' ? 50 : 800);
          const earlier = await sample(mode, 'earlier-loaded');
          images(earlier, ['earlier']);
          check(earlier);
          await release(['anchor']);
        }
        const after = await sample(mode, 'both-loaded');
        images(after, ['earlier', 'anchor']);
        check(after);
        if (check === reader) {
          assert.equal(after.markerRow, before.markerRow, 'Reading row identity must survive both loads');
          assert.ok(after.localOffset - before.localOffset > 400,
            `Same-row image must actually move the sentence within its row: ${JSON.stringify({ before, after })}`);
        }
        assert.ok(after.height - before.height > 500, 'Images must cause substantial real layout growth');
        if (mode === 'concurrent') {
          assert.ok(Math.abs((after.top - before.top) - (after.height - before.height)) <= 2,
            `Concurrent correction doubled or lost: ${JSON.stringify({ before, after })}`);
        }
        assert.deepEqual(Object.fromEntries(counts), { earlier: 1, anchor: 1 });
        await settle();
        check(await sample(mode, 'settled-again'));
        await page.screenshot({ path: path.join(outDir, `${id}--${mode}.png`) });
      } finally {
        await Promise.all([...pending.values()].map((route) => route.fulfill(imageResponse)));
        await page.unroute(imagePattern, hold);
      }
    }
  } finally {
    await writeFile(path.join(outDir, `${id}--samples.json`), `${JSON.stringify(samples, null, 2)}\n`);
  }
  console.log('  verified cold-image reader continuity, concurrent correction, follow-latest and End');
  return true;
}
