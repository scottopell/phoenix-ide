import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'ask-user-question',
  readyAttribute: 'data-ask-user-question-fixture',
  outDir: process.env.AUQ_QA_OUT ?? 'qa-artifacts/ask-user-question',
  viewportMatrix: [
    { name: 'desktop', width: 1440, height: 900 },
    { name: 'mobile', width: 390, height: 844 },
  ],
});
