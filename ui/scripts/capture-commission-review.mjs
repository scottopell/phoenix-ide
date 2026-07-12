import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'commission-review',
  readyAttribute: 'data-commission-review-fixture-ready',
  outDir: process.env.COMMISSION_REVIEW_QA_OUT ?? 'qa-artifacts/commission-review',
  viewportMatrix: [
    { name: 'desktop', width: 1280, height: 900 },
    { name: 'mobile', width: 390, height: 844 },
  ],
});
