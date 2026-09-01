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
    const maxScrollTop = await scroller.evaluate((element) => element.scrollHeight - element.clientHeight);
    let observed = false;
    for (let step = 0; step <= 20; step += 1) {
      await scroller.evaluate((element, scrollTop) => new Promise((resolve) => {
        element.scrollTop = scrollTop;
        requestAnimationFrame(() => requestAnimationFrame(resolve));
      }), maxScrollTop * step / 20);
      const count = await page.getByText(handoff, { exact: true }).count();
      if (count > 1) throw new Error(`expected at most one rendered handoff while scrolling, got ${count}`);
      observed ||= count === 1;
    }
    if (!observed) throw new Error('expected the handoff to render while scrolling the long transcript');
    await scroller.evaluate((element) => new Promise((resolve) => {
      element.scrollTop = 0;
      requestAnimationFrame(() => requestAnimationFrame(resolve));
    }));
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
