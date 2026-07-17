import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'desktop-multi-pr-conversation',
  readyAttribute: 'data-desktop-multi-pr-conversation-fixture-ready',
  outDir: process.env.DESKTOP_MULTI_PR_CONVERSATION_QA_OUT ?? 'qa-artifacts/desktop-multi-pr-conversation',
  viewportMatrix: [
    { name: 'compact', width: 1024, height: 768 },
    { name: 'wide', width: 1440, height: 900 },
  ],
});
