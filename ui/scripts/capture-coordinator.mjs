import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'coordinator',
  readyAttribute: 'data-coordinator-fixture-ready',
  outDir: process.env.COORDINATOR_QA_OUT ?? 'qa-artifacts/coordinator',
  viewportMatrix: [
    { name: 'phone', width: 390, height: 844 },
    { name: 'small-phone', width: 360, height: 640 },
    { name: 'tablet', width: 820, height: 900 },
    { name: 'desktop', width: 1440, height: 900 },
  ],
  captureStory: async ({ page, id, outDir, viewport }) => {
    if (id.startsWith('conversation-')) {
      const geometry = await page.evaluate(() => {
        const measure = (selector) => {
          const element = document.querySelector(selector);
          if (!element) return null;
          const rect = element.getBoundingClientRect();
          return { width: rect.width, height: rect.height };
        };
        return {
          transcript: measure('#messages'),
          row: measure('[data-render-unit-key]'),
          input: measure('#input-area'),
          overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        };
      });
      if (!geometry.transcript || geometry.transcript.height <= 0 || !geometry.row || geometry.row.height <= 0) {
        throw new Error(`${id}/${viewport.name}: transcript geometry collapsed: ${JSON.stringify(geometry)}`);
      }
      if (!geometry.input || geometry.input.height <= 0 || geometry.overflow > 1) {
        throw new Error(`${id}/${viewport.name}: unusable Coordinator geometry: ${JSON.stringify(geometry)}`);
      }
    }
    await page.screenshot({ path: `${outDir}/${id}--${viewport.name}.png`, fullPage: false });
    return true;
  },
});
