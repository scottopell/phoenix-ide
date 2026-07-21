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
        conversation: measure('.coordinator-conversation'),
        brief: measure('.input-quick-action'),
        duplicateHeader: measure('.coordinator-header'),
        mobileNav: measure('.coordinator-mobile-nav'),
        work: measure('.coordinator-work-pane'),
        overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      };
    });
    if (geometry.overflow > 1) {
      throw new Error(`${id}/${viewport.name}: horizontal overflow: ${JSON.stringify(geometry)}`);
    }
    if (geometry.duplicateHeader || geometry.mobileNav || geometry.work) {
      throw new Error(`${id}/${viewport.name}: obsolete Coordinator chrome is mounted: ${JSON.stringify(geometry)}`);
    }
    if (!geometry.conversation || geometry.conversation.height < viewport.height - 1) {
      throw new Error(`${id}/${viewport.name}: conversation does not fill the viewport: ${JSON.stringify(geometry)}`);
    }
    if (!geometry.transcript || geometry.transcript.height <= 0 || !geometry.row || geometry.row.height <= 0 || !geometry.input || geometry.input.height <= 0) {
      throw new Error(`${id}/${viewport.name}: conversation geometry collapsed: ${JSON.stringify(geometry)}`);
    }
    if (!geometry.brief || geometry.brief.height < 32 || geometry.brief.bottom > viewport.height + 1) {
      throw new Error(`${id}/${viewport.name}: inline Brief me action is not usable: ${JSON.stringify(geometry)}`);
    }
    await page.screenshot({ path: `${outDir}/${id}--${viewport.name}.png`, fullPage: false });
    return true;
  },
});
