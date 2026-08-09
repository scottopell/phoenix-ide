import { runSurfaceCapture } from './capture-ladle-surface.mjs';

async function verifyCompactGrid({ page, id, viewport }) {
  if (id !== 'grid-compact') return false;
  const geometry = await page.evaluate(() => {
    const strip = document.querySelector('.compact-tool-group .compact-tool-strip');
    const cards = Array.from(strip?.querySelectorAll('.compact-tool-card') ?? []);
    const wide = strip?.querySelector('.compact-tool-card.wide');
    if (!(strip instanceof HTMLElement) || cards.length < 5 || !(wide instanceof HTMLElement)) {
      throw new Error('compact grid fixture is missing expected cards');
    }
    const rows = new Set(cards.filter((card) => !card.classList.contains('wide')).map((card) => Math.round(card.getBoundingClientRect().top)));
    const stripRect = strip.getBoundingClientRect();
    const wideRect = wide.getBoundingClientRect();
    return {
      lightweightRows: rows.size,
      stripWidth: stripRect.width,
      wideWidth: wideRect.width,
      documentClientWidth: document.documentElement.clientWidth,
      documentScrollWidth: document.documentElement.scrollWidth,
    };
  });
  if (geometry.documentScrollWidth !== geometry.documentClientWidth) {
    throw new Error(`compact grid created document overflow: ${JSON.stringify(geometry)}`);
  }
  if (Math.abs(geometry.stripWidth - geometry.wideWidth) > 2) {
    throw new Error(`bash card does not span compact grid: ${JSON.stringify(geometry)}`);
  }
  if (viewport.width >= 1000 && geometry.lightweightRows !== 1) {
    throw new Error(`desktop lightweight cards did not share one row: ${JSON.stringify(geometry)}`);
  }
  if (viewport.width <= 400 && geometry.lightweightRows < 2) {
    throw new Error(`mobile compact grid did not reduce its column count: ${JSON.stringify(geometry)}`);
  }
  return false;
}

runSurfaceCapture({
  surface: 'tool-results',
  readyAttribute: 'data-tool-results-fixture-ready',
  outDir: process.env.TOOL_RESULTS_QA_OUT ?? 'qa-artifacts/tool-results',
  captureStory: verifyCompactGrid,
  viewportMatrix: [
    { name: 'desktop', width: 1280, height: 900 },
    { name: 'mobile', width: 390, height: 844 },
  ],
});
