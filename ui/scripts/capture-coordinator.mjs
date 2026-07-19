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
    const geometry = await page.evaluate(() => {
      const measure = (selector) => {
        const element = document.querySelector(selector);
        if (!element) return null;
        const rect = element.getBoundingClientRect();
        return { width: rect.width, height: rect.height, top: rect.top, bottom: rect.bottom, right: rect.right };
      };
      return {
        transcript: measure('#messages'),
        row: measure('[data-render-unit-key]'),
        input: measure('#input-area'),
        mobileNav: measure('.coordinator-mobile-nav'),
        work: measure('.coordinator-work-pane:not([hidden])'),
        workItem: measure('.coordinator-item'),
        retry: measure('.coordinator-work-error button'),
        overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      };
    });
    if (geometry.overflow > 1) {
      throw new Error(`${id}/${viewport.name}: horizontal overflow: ${JSON.stringify(geometry)}`);
    }
    if (viewport.width <= 1024 && (!geometry.mobileNav || geometry.mobileNav.bottom > viewport.height + 1)) {
      throw new Error(`${id}/${viewport.name}: mobile navigation is outside the viewport: ${JSON.stringify(geometry)}`);
    }
    if (viewport.width <= 1024 && geometry.input && geometry.mobileNav && geometry.input.bottom > geometry.mobileNav.top + 1) {
      throw new Error(`${id}/${viewport.name}: mobile navigation overlaps the composer: ${JSON.stringify(geometry)}`);
    }
    if (id.startsWith('conversation-')) {
      if (!geometry.transcript || geometry.transcript.height <= 0 || !geometry.row || geometry.row.height <= 0 || !geometry.input || geometry.input.height <= 0) {
        throw new Error(`${id}/${viewport.name}: conversation geometry collapsed: ${JSON.stringify(geometry)}`);
      }
    } else if (!geometry.work || geometry.work.height <= 0 || geometry.work.right > viewport.width + 1) {
      throw new Error(`${id}/${viewport.name}: Work pane geometry collapsed: ${JSON.stringify(geometry)}`);
    } else if (!id.endsWith('error') && (!geometry.workItem || geometry.workItem.width <= 0)) {
      throw new Error(`${id}/${viewport.name}: Work item geometry collapsed: ${JSON.stringify(geometry)}`);
    } else if (id === 'fleet-error' && (!geometry.retry || geometry.retry.height < 32)) {
      throw new Error(`${id}/${viewport.name}: Work retry is not usable: ${JSON.stringify(geometry)}`);
    }
    await page.screenshot({ path: `${outDir}/${id}--${viewport.name}.png`, fullPage: false });
    return true;
  },
});
