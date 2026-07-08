import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'work-actions',
  readyAttribute: 'data-work-actions-fixture',
  outDir: process.env.WORK_ACTIONS_QA_OUT ?? 'qa-artifacts/work-actions',
  viewport: { width: 960, height: 360 },
});
