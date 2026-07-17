import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'mobile-multi-pr-conversation',
  readyAttribute: 'data-mobile-multi-pr-conversation-fixture-ready',
  outDir: process.env.MOBILE_MULTI_PR_CONVERSATION_QA_OUT ?? 'qa-artifacts/mobile-multi-pr-conversation',
  viewport: { width: 390, height: 844 },
});
