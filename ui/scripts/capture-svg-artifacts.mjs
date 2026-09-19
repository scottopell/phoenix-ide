import assert from 'node:assert/strict';
import path from 'node:path';
import { runSurfaceCapture } from './capture-ladle-surface.mjs';

const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="800" height="400" viewBox="0 0 800 400"><rect width="800" height="400" fill="white"/><g font-family="sans-serif" font-size="24" fill="#111"><text x="30" y="48">Measured directory sizes</text><text x="30" y="110">Projects</text><text x="30" y="180">Caches</text><text x="30" y="300">Free space</text><text x="680" y="110">48 GiB</text><text x="460" y="180">24 GiB</text><text x="560" y="300">36 GiB</text></g><rect x="180" y="75" width="480" height="45" fill="#3268a8"/><rect x="180" y="145" width="240" height="45" fill="#3268a8"/><line x1="30" y1="230" x2="770" y2="230" stroke="#aaa"/><rect x="180" y="265" width="360" height="45" fill="#609878"/></svg>';
const hostileSource = '<svg onload="window.svgExecuted=true"><script>window.svgExecuted=true</script><image href="https://hostile.invalid/track"/></svg>';
const unexpectedRequests = [];
runSurfaceCapture({
  surface: 'svg-artifacts', readyAttribute: 'data-svg-artifacts-ready', outDir: 'qa-artifacts/svg-artifacts',
  viewportMatrix: [{ name: 'desktop', width: 1280, height: 900 }, { name: 'mobile', width: 390, height: 844 }],
  preparePage: async ({ page }) => {
    page.on('request', (request) => { if (request.url().includes('hostile.invalid')) unexpectedRequests.push(request.url()); });
    await page.route('**/api/conversations/svg-fixture/svg-artifacts/**', (route) => {
      const url = route.request().url();
      const chart = url.includes('/tall') ? svg.replace('width="800" height="400"', 'width="400" height="1600"') : url.includes('/wide') ? svg.replace('width="800" height="400"', 'width="8000" height="400"') : svg;
      return route.fulfill({ status: 200, contentType: url.endsWith('/source') ? 'text/plain' : 'image/svg+xml', body: url.endsWith('/source') ? hostileSource : chart });
    });
  },
  captureStory: async ({ page, id, viewport, outDir }) => {
    const image = page.getByRole('img', { name: /Measured directory sizes/ });
    await image.evaluate((element) => element.decode());
    assert.equal(await image.count(), 1);
    const geometry = await page.evaluate(() => ({ width: document.documentElement.clientWidth, scroll: document.documentElement.scrollWidth, height: document.querySelector('.svg-artifact-image img').getBoundingClientRect().height, bg: getComputedStyle(document.querySelector('.svg-artifact-image')).backgroundColor }));
    assert.equal(geometry.scroll, geometry.width);
    assert.ok(geometry.height <= 360);
    assert.equal(geometry.bg, 'rgb(255, 255, 255)');
    await page.screenshot({ path: path.join(outDir, `${id}-${viewport.name}.png`), fullPage: true });
    if (id.endsWith('compact')) {
      await page.getByRole('button', { name: /present_svg:.*expand tool detail/ }).click();
      assert.equal(await image.count(), 1);
      await page.getByRole('button', { name: 'Collapse expanded tool detail' }).click();
      assert.equal(await image.count(), 1);
    }
    const expand = page.getByRole('button', { name: 'Expand visualization' });
    await expand.click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('img').evaluate((element) => element.decode());
    await page.getByRole('button', { name: 'Zoom in' }).click();
    assert.equal(await page.getByLabel('Zoom level').textContent(), '150%');
    await page.screenshot({ path: path.join(outDir, `${id}-${viewport.name}-expanded.png`), fullPage: true });
    await page.keyboard.press('Escape');
    await expand.evaluate((element) => new Promise((resolve) => requestAnimationFrame(() => resolve(document.activeElement === element)))).then((focused) => assert.ok(focused));
    await page.getByRole('button', { name: 'View source' }).click();
    await page.getByText(hostileSource, { exact: true }).waitFor();
    assert.equal(await page.locator('.svg-artifact-source svg').count(), 0);
    assert.equal(await page.evaluate(() => window.svgExecuted), undefined);
    assert.deepEqual(unexpectedRequests, []);
    await page.screenshot({ path: path.join(outDir, `${id}-${viewport.name}-source.png`), fullPage: true });
    await page.keyboard.press('Escape');
    return true;
  },
});
