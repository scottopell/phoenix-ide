import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'product-conversation',
  readyAttribute: 'data-product-conversation-fixture-ready',
  outDir: process.env.PRODUCT_CONVERSATION_QA_OUT ?? 'qa-artifacts/product-conversation',
  viewportMatrix: [
    { name: 'desktop', width: 1440, height: 900 },
    { name: 'mobile', width: 390, height: 844 },
  ],
  captureStory: async ({ page, id }) => {
    if (id !== 'long-history-110-messages') return false;
    const scroller = page.locator('[aria-label="Conversation transcript"]');
    const handoff = 'Approved handoff: keep exactly one persisted handoff summary between predecessor history and the successor transcript.';
    const tail = page.locator('[data-render-unit-key="long-110"]');
    await scroller.evaluate((element) => { element.style.height = '400px'; });
    const scrollTo = (scrollTop) => scroller.evaluate((element, top) => new Promise((resolve) => {
      element.scrollTop = top;
      element.dispatchEvent(new Event('scroll', { bubbles: true }));
      requestAnimationFrame(() => requestAnimationFrame(resolve));
    }), scrollTop);
    const initialMaxScrollTop = await scroller.evaluate((element) => element.scrollHeight - element.clientHeight);
    let boundaryScrollTop = null;
    for (let step = 0; step <= 20; step += 1) {
      await scrollTo(initialMaxScrollTop * step / 20);
      const count = await page.getByText(handoff, { exact: true }).count();
      if (count > 1) throw new Error(`expected at most one handoff while locating its segment boundary, got ${count}`);
      if (count === 1) {
        boundaryScrollTop = await scroller.evaluate((element) => element.scrollTop);
        break;
      }
    }
    if (boundaryScrollTop === null) throw new Error('expected to locate exactly one initial rendered handoff boundary');
    let tailMounted = false;
    for (let attempt = 0; attempt < 10; attempt += 1) {
      const maxScrollTop = await scroller.evaluate((element) => element.scrollHeight - element.clientHeight);
      await scrollTo(maxScrollTop);
      tailMounted = await tail.count() === 1;
      if (tailMounted) break;
    }
    if (!tailMounted) throw new Error('expected the transcript tail to render after scrolling the virtualizer');
    if (await page.getByText(handoff, { exact: true }).count() !== 0) {
      throw new Error('expected the handoff boundary to be virtualized away at the transcript tail');
    }

    await scrollTo(boundaryScrollTop);
    if (await page.getByText(handoff, { exact: true }).count() !== 1) {
      throw new Error('expected exactly one handoff after remounting the recorded segment boundary');
    }
    if (await page.locator('a[href*="product-handoff"]').count() !== 1) {
      throw new Error('expected exactly one handoff card at the rendered segment boundary');
    }
    if (await tail.count() !== 0) {
      throw new Error('expected the transcript tail to be virtualized away at the handoff boundary');
    }
    return false;
  },
  expectedConsoleErrors: new Map([
    ['error', [
      'Fixture failed to fetch product conversation snapshot',
      'Failed to load resource: the server responded with a status of 404 (Not Found)',
      'WebSocket connection to',
    ]],
    ['desktop-open-multi-segment-qa-work', [
      'Failed to load resource: the server responded with a status of 404 (Not Found)',
      'WebSocket connection to',
    ]],
    ['mobile-open', [
      'Failed to load resource: the server responded with a status of 404 (Not Found)',
      'WebSocket connection to',
    ]],
  ]),
});
