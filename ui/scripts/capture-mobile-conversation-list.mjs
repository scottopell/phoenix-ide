import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'mobile-conversation-list',
  readyAttribute: 'data-mobile-conversation-list-fixture-ready',
  outDir: process.env.MOBILE_CONVERSATION_LIST_QA_OUT ?? 'qa-artifacts/mobile-conversation-list',
  viewport: { width: 390, height: 844 },
});
