import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'tool-results',
  readyAttribute: 'data-tool-results-fixture-ready',
  outDir: process.env.TOOL_RESULTS_QA_OUT ?? 'qa-artifacts/tool-results',
  viewportMatrix: [
    { name: 'desktop', width: 1280, height: 900 },
    { name: 'mobile', width: 390, height: 844 },
  ],
});
