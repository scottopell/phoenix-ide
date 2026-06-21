import { runSurfaceCapture } from './capture-ladle-surface.mjs';

// Surface config for the meta-viewer QA capture. Most scenarios are payload-in
// (MetaViewer never fetches), so no console errors are expected; the engine
// fails the run loudly if any appear.
runSurfaceCapture({
  surface: 'meta-viewer',
  readyAttribute: 'data-meta-viewer-fixture-ready',
  outDir: process.env.META_VIEWER_QA_OUT ?? 'qa-artifacts/meta-viewer',
  // MetaViewer is a full-screen overlay; a wider viewport shows the header
  // chrome (toggles, banners, notes badge) without horizontal cramping.
  viewport: { width: 1200, height: 900 },
});
