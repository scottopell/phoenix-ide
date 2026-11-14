import { writeFile } from 'node:fs/promises';
import path from 'node:path';
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

async function captureMobileExpandedSpacing({ page, id, outDir, viewport }) {
  if (id !== 'mobile-spacing-compact') return false;
  const sendCard = page.locator('[data-tool-id="mobile-send-message"] .compact-tool-card-expand');
  await sendCard.waitFor({ state: 'visible' });
  await sendCard.click();
  await page.getByRole('button', { name: 'Collapse expanded tool detail' }).waitFor({ state: 'visible' });
  const evidence = await page.evaluate(() => {
    const selectors = [
      '.compact-tool-group',
      '.compact-tool-group > .message-header',
      '.compact-tool-group > .message-content',
      '.compact-tool-selected-detail > .compact-tool-owner-copy',
      '.compact-tool-selected-detail > .compact-tool-detail-collapse',
      '.compact-tool-selected-detail > .message',
      '.compact-tool-selected-detail > .message > .assistant-message-toolbar',
      '.compact-tool-selected-detail > .message > .message-content',
      '.compact-tool-selected-detail > .message > .message-content > *',
      '.compact-tool-selected-detail > .message .tool-block',
      '.compact-tool-selected-detail > .message .tool-block-header',
      '.compact-tool-group',
      '.compact-tool-group .message-content',
      '.compact-tool-selected-detail',
      '.compact-tool-selected-detail > .message',
      '.tool-block-input',
      '.tool-block-output',
      '[data-tool-id="mobile-next-read"]',
      '.compact-tool-detail-collapse',
    ];
    return Object.fromEntries(selectors.map((selector) => {
      const element = document.querySelector(selector);
      if (!(element instanceof HTMLElement)) return [selector, null];
      const rect = element.getBoundingClientRect();
      const style = getComputedStyle(element);
      return [selector, {
        top: Math.round(rect.top + scrollY),
        bottom: Math.round(rect.bottom + scrollY),
        height: Math.round(rect.height),
        marginTop: style.marginTop,
        marginBottom: style.marginBottom,
        paddingTop: style.paddingTop,
        paddingBottom: style.paddingBottom,
        lineHeight: style.lineHeight,
        display: style.display,
        gap: style.gap,
        flexDirection: style.flexDirection,
        tagName: element.tagName,
        className: element.className,
        text: element.children.length === 0 ? element.textContent?.trim().slice(0, 80) : undefined,
      }];
    }));
  });
  const experimentStyle = await page.addStyleTag({
    content: '.compact-tool-selected-detail > .message > .assistant-message-toolbar { display: none !important; }',
  });
  const toolbarHiddenExperiment = await page.evaluate(() => {
    const toolbar = document.querySelector('.compact-tool-selected-detail > .message > .assistant-message-toolbar');
    const content = document.querySelector('.compact-tool-selected-detail > .message > .message-content');
    const rect = (element) => element instanceof HTMLElement
      ? { top: Math.round(element.getBoundingClientRect().top + scrollY), height: Math.round(element.getBoundingClientRect().height) }
      : null;
    return { toolbar: rect(toolbar), content: rect(content) };
  });
  await experimentStyle.evaluate((element) => element.remove());
  const stem = `mobile-spacing-compact--${viewport.name ?? `${viewport.width}x${viewport.height}`}`;
  await page.screenshot({ path: path.join(outDir, `${stem}.png`), fullPage: true });
  await writeFile(path.join(outDir, `${stem}.geometry.json`), `${JSON.stringify({ viewport, evidence, toolbarHiddenExperiment }, null, 2)}\n`);
  return true;
}

runSurfaceCapture({
  surface: 'tool-results',
  readyAttribute: 'data-tool-results-fixture-ready',
  outDir: process.env.TOOL_RESULTS_QA_OUT ?? 'qa-artifacts/tool-results',
  captureStory: async (context) => (await captureMobileExpandedSpacing(context)) || verifyCompactGrid(context),
  viewportMatrix: [
    { name: 'desktop', width: 1280, height: 900 },
    { name: 'mobile', width: 393, height: 852, hasTouch: true, isMobile: true },
  ],
});
