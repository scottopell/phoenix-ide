import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'product-conversation',
  readyAttribute: 'data-product-conversation-fixture-ready',
  outDir: process.env.PRODUCT_CONVERSATION_QA_OUT ?? 'qa-artifacts/product-conversation',
  viewportMatrix: [
    { name: 'desktop', width: 1440, height: 900 },
    { name: 'mobile', width: 390, height: 844 },
  ],
  expectedConsoleErrors: new Map([
    ['error', ['Fixture failed to fetch product conversation snapshot']],
    ['desktop-open-multi-segment-qa-work', [
      'Failed to load resource: the server responded with a status of 404 (Not Found)',
      'WebSocket connection to',
    ]],
  ]),
});
