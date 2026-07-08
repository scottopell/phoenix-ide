import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'sidebar',
  readyAttribute: 'data-sidebar-fixture',
  outDir: process.env.SIDEBAR_QA_OUT ?? 'qa-artifacts/sidebar',
  viewport: { width: 1180, height: 900 },
});
