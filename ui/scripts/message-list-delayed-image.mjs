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
      images: Array.from(element.querySelectorAll('img')).map((image) => ({
        id: new URL(image.src).searchParams.get('id'),
        complete: image.complete,
        width: image.naturalWidth,
      })),
    }));
    const value = { mode, phase, y: await marker.evaluate((element) => element.getBoundingClientRect().top), ...geometry };
    samples.push(value);
    return value;
  };
  const reader = (value) => assert.ok(Math.abs(value.y - 152) <= 2, `Reader drift: ${JSON.stringify(value)}`);
  const tail = (value) => assert.ok(Math.abs(value.gap) <= 2, `Tail drift: ${JSON.stringify(value)}`);
  const images = (value, loaded) => {
    assert.equal(value.images.length, 2, 'Both real image elements must be mounted');
    for (const image of value.images) {
      assert.equal(image.complete, loaded.includes(image.id), JSON.stringify(value));
      assert.equal(image.width, loaded.includes(image.id) ? 640 : 0, JSON.stringify(value));
    }
  };

  try {
    for (const mode of ['reader-1', 'reader-2', 'reader-3', 'reader-4', 'concurrent', 'follow-latest', 'end-navigation']) {
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
        await settle();
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

        if (mode === 'end-navigation') {
          await scroller.focus();
          await page.keyboard.press('End');
          await settle();
          const end = await sample(mode, 'intentional-end');
          tail(end);
          assert.ok(end.top > before.top + 20, 'End must actually leave the reading position');
        }
        const release = async (keys) => {
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
          await settle();
        };
        const check = mode === 'follow-latest' || mode === 'end-navigation' ? tail : reader;
        if (mode === 'concurrent') {
          await release(['earlier', 'anchor']);
        } else {
          await release(['earlier']);
          const earlier = await sample(mode, 'earlier-loaded');
          images(earlier, ['earlier']);
          check(earlier);
          await release(['anchor']);
        }
        const after = await sample(mode, 'both-loaded');
        images(after, ['earlier', 'anchor']);
        check(after);
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
