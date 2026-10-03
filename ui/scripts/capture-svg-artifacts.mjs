import assert from 'node:assert/strict';
import path from 'node:path';
import { readFile, writeFile } from 'node:fs/promises';
import { runSurfaceCapture } from './capture-ladle-surface.mjs';

const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="800" height="400" viewBox="0 0 800 400"><rect width="800" height="400" fill="white"/><g font-family="sans-serif" font-size="24" fill="#111"><text x="30" y="48">Measured directory sizes</text><text x="30" y="110">Projects</text><text x="30" y="180">Caches</text><text x="30" y="300">Free space</text><text x="680" y="110">48 GiB</text><text x="460" y="180">24 GiB</text><text x="560" y="300">36 GiB</text></g><rect x="180" y="75" width="480" height="45" fill="#3268a8"/><rect x="180" y="145" width="240" height="45" fill="#3268a8"/><line x1="30" y1="230" x2="770" y2="230" stroke="#aaa"/><rect x="180" y="265" width="360" height="45" fill="#609878"/></svg>';
const hostileSource = `<svg xmlns="http://www.w3.org/2000/svg" width="800" height="400" onload="window.svgExecuted=true; window.location='https://hostile.invalid/navigation'"><script>window.svgExecuted=true; parent.document.body.dataset.svgOwned="true"</script><style>@import url("https://hostile.invalid/style");</style><image href="https://hostile.invalid/track"/><a href="https://hostile.invalid/link"><rect width="800" height="400" fill="white"/><text x="30" y="70" font-size="26">Hostile SVG image-context isolation probe</text></a></svg>`;
const unexpectedRequests = [];
const hostileEvidence = [];
const privateShareRequests = [];
let serveHostileImage = false;
const artifactHeaders = {
  'x-content-type-options': 'nosniff',
  'content-security-policy': "sandbox; default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'",
  'cross-origin-resource-policy': 'same-origin',
  'cache-control': 'private, no-store',
};
runSurfaceCapture({
  surface: 'svg-artifacts', readyAttribute: 'data-svg-artifacts-ready', outDir: 'qa-artifacts/svg-artifacts',
  viewportMatrix: [{ name: 'desktop', width: 1280, height: 900 }, { name: 'mobile', width: 390, height: 844 }],
  preparePage: async ({ page }) => {
    page.context().on('request', (request) => { if (request.url().includes('hostile.invalid')) unexpectedRequests.push(request.url()); });
    await page.context().route('**/api/share/anonymous-svg/events', (route) => {
      const reference = { artifact_id: 'chart', conversation_id: 'svg-fixture', title: 'Shared chart', description: 'Measured directory sizes in GiB; free space is shown separately.', width: 800, height: 400, validation: 'accepted_static_svg' };
      const base = { conversation_id: 'svg-fixture', created_at: '2026-01-01T00:00:00Z', display_data: null };
      const init = {
        sequence_id: 2, transcript_generation: 1, transcript_coverage: 'complete',
        conversation: { id: 'svg-fixture', slug: 'shared-chart', model: 'fixture' },
        messages: [
          { ...base, message_id: 'agent', sequence_id: 1, message_type: 'agent', content: [{ type: 'tool_use', id: 'publish', name: 'present_svg', input: { path: '/server/chart.svg' } }] },
          { ...base, message_id: 'tool', sequence_id: 2, message_type: 'tool', content: { tool_use_id: 'publish', content: JSON.stringify(reference), is_error: false } },
        ],
        steering_messages: [], agent_working: false, last_sequence_id: 2,
        stream_incarnation: 'test-stream', presentation_mode: 'idle', context_window_size: 0,
        project_name: null, pending_anchor_sequence_id: 0, pending_events: [], pending_truncated: false,
      };
      return route.fulfill({ status: 200, contentType: 'text/event-stream', body: `retry: 60000\nevent: init\ndata: ${JSON.stringify(init)}\n\n` });
    });
    await page.context().route(/\/api\/(?:conversations\/svg-fixture|share\/anonymous-svg)\/svg-artifacts\//, (route) => {
      const url = route.request().url();
      if ((route.request().headers()['referer'] ?? '').includes('svg-artifacts--shared') && url.includes('/api/conversations/')) {
        privateShareRequests.push(url);
        return route.fulfill({ status: 401, body: 'Authentication required' });
      }
      const chart = url.includes('/tall') ? svg.replace('width="800" height="400"', 'width="400" height="1600"') : url.includes('/wide') ? svg.replace('width="800" height="400"', 'width="8000" height="400"') : svg;
      const source = url.endsWith('/source');
      return route.fulfill({
        status: 200,
        contentType: source ? 'text/plain; charset=utf-8' : 'image/svg+xml',
        headers: { ...artifactHeaders, 'content-disposition': source ? 'inline' : 'attachment; filename=visualization.svg' },
        body: source || serveHostileImage ? hostileSource : chart,
      });
    });
  },
  captureStory: async ({ page, id, viewport, outDir }) => {
    const image = page.getByRole('img', { name: /Measured directory sizes/ });
    await image.evaluate((element) => element.decode());
    assert.equal(await image.count(), 1);
    if (id === 'shared') {
      assert.equal(await image.getAttribute('src'), '/api/share/anonymous-svg/svg-artifacts/chart');
      assert.equal(await page.getByRole('link', { name: 'Download SVG' }).getAttribute('href'), '/api/share/anonymous-svg/svg-artifacts/chart/download');
      assert.deepEqual(privateShareRequests, []);
    }
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
    // Route mocking deliberately bypasses ingestion to exercise browser isolation.
    serveHostileImage = true;
    try {
      await page.reload({ waitUntil: 'networkidle' });
      await image.evaluate((element) => element.decode());
      assert.equal(await page.evaluate(() => window.svgExecuted), undefined);
      assert.equal(await page.evaluate(() => document.body.dataset.svgOwned), undefined);
      await page.evaluate(() => { window.svgExecuted = 'untouched'; });
      const appUrl = page.url();
      const assertIsolated = async () => {
        assert.equal(await page.evaluate(() => window.svgExecuted), 'untouched');
        assert.equal(await page.evaluate(() => document.body.dataset.svgOwned), undefined);
        assert.equal(await page.locator('.svg-artifact-image svg, .svg-artifact-image script, .svg-artifact-image object, .svg-artifact-image iframe').count(), 0);
        assert.equal(page.url(), appUrl);
        assert.deepEqual(unexpectedRequests, []);
      };
      await image.click();
      await assertIsolated();
      await page.screenshot({ path: path.join(outDir, `${id}-${viewport.name}-hostile-preview.png`), fullPage: true });
      await expand.click();
      await dialog.getByRole('img').evaluate((element) => element.decode());
      await dialog.getByRole('img').click();
      await assertIsolated();
      await page.screenshot({ path: path.join(outDir, `${id}-${viewport.name}-hostile-expanded.png`), fullPage: true });
      await page.keyboard.press('Escape');
      const artifactUrl = await image.getAttribute('src');
      const directContext = await page.context().browser().newContext();
      const directPage = await directContext.newPage();
      directPage.on('request', (request) => { if (request.url().includes('hostile.invalid')) unexpectedRequests.push(request.url()); });
      await directPage.route(/\/api\/(?:conversations\/svg-fixture|share\/anonymous-svg)\/svg-artifacts\//, (route) => route.fulfill({
        status: 200, contentType: 'image/svg+xml', body: hostileSource,
        headers: { ...artifactHeaders, 'content-disposition': 'attachment; filename=visualization.svg' },
      }));
      try {
        const downloadPromise = directPage.waitForEvent('download');
        await directPage.goto(new URL(artifactUrl, appUrl).href).catch((error) => {
          assert.match(error.message, /Download is starting|net::ERR_ABORTED/);
        });
        const download = await downloadPromise;
        assert.equal(download.suggestedFilename(), 'visualization.svg');
        const downloadedPath = await download.path();
        assert.equal(await readFile(downloadedPath, 'utf8'), hostileSource);
        assert.equal(directPage.url(), 'about:blank');
        assert.equal(await directPage.locator('svg, script').count(), 0);
        assert.equal(await directPage.evaluate(() => window.svgExecuted), undefined);
        await assertIsolated();
      } finally {
        await directContext.close();
      }
      hostileEvidence.push({ story: id, viewport: viewport.name, preview: 'inert image', expansion: 'inert image', directNavigation: 'download; blank document retained', hostileRequests: 0, appMutation: false });
    } finally {
      serveHostileImage = false;
    }
    return true;
  },
  onComplete: async (outDir) => {
    assert.deepEqual(privateShareRequests, []);
    await writeFile(path.join(outDir, 'hostile-browser-evidence.json'), JSON.stringify(hostileEvidence, null, 2));
  },
});
