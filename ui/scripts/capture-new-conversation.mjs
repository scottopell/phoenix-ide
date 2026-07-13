import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'new-conversation',
  readyAttribute: 'data-new-conversation-fixture-ready',
  outDir: process.env.NEW_CONVERSATION_QA_OUT ?? 'qa-artifacts/new-conversation',
  viewportMatrix: [
    { name: 'desktop', width: 1280, height: 900 },
    { name: 'mobile', width: 390, height: 844 },
  ],
});
