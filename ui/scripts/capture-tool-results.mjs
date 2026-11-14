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
      overflowElements: Array.from(document.querySelectorAll('body *')).filter((el) => { const r = el.getBoundingClientRect(); return r.right > document.documentElement.clientWidth + 1 && r.width > 0; }).slice(-25).map((el) => ({ tag: el.tagName, cls: el.className, text: el.textContent?.slice(0, 120), parent: el.parentElement?.className, right: el.getBoundingClientRect().right, width: el.getBoundingClientRect().width, whiteSpace: getComputedStyle(el).whiteSpace, minWidth: getComputedStyle(el).minWidth })),
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
  const collapse = page.getByRole('button', { name: 'Collapse expanded tool detail' });
  await collapse.waitFor({ state: 'visible' });

  const evidence = await page.evaluate(() => {
    const detail = document.querySelector('.compact-tool-selected-detail');
    const message = detail?.querySelector(':scope > .message');
    const content = message?.querySelector(':scope > .message-content');
    const toolbar = message?.querySelector(':scope > .assistant-message-toolbar');
    const send = detail?.querySelector('[data-tool-id="mobile-send-message"]');
    const result = send?.querySelector('.tool-block-output');
    const query = document.querySelector('[data-tool-id="mobile-query-database"]');
    const continuation = document.querySelector('[data-tool-id="mobile-next-read"]');
    const copyInput = send?.querySelector('.tool-block-input button');
    const rect = (element) => element instanceof HTMLElement
      ? { top: Math.round(element.getBoundingClientRect().top + scrollY), height: Math.round(element.getBoundingClientRect().height), scrollHeight: element.scrollHeight, clientHeight: element.clientHeight }
      : null;
    return {
      detail: rect(detail),
      message: rect(message),
      content: rect(content),
      toolbarPresent: toolbar !== null,
      inputText: send?.querySelector('.tool-block-input')?.textContent?.trim() ?? null,
      resultText: result?.textContent?.trim() ?? null,
      copyInputPresent: copyInput !== null,
      queryDatabasePresent: query !== null,
      continuationPresent: continuation !== null,
      documentClientWidth: document.documentElement.clientWidth,
      documentScrollWidth: document.documentElement.scrollWidth,
      overflowElements: Array.from(document.querySelectorAll('body *')).filter((el) => { const r = el.getBoundingClientRect(); return r.right > document.documentElement.clientWidth + 1 && r.width > 0; }).slice(-25).map((el) => ({ tag: el.tagName, cls: el.className, text: el.textContent?.slice(0, 120), parent: el.parentElement?.className, right: el.getBoundingClientRect().right, width: el.getBoundingClientRect().width, whiteSpace: getComputedStyle(el).whiteSpace, minWidth: getComputedStyle(el).minWidth })),
    };
  });
  if (evidence.toolbarPresent) throw new Error('expanded detail still contains a redundant message toolbar');
  if (!evidence.message || !evidence.content || evidence.content.height <= 0) {
    throw new Error('expanded message and content must have visible geometry');
  }
  const contentGap = evidence.content.top - evidence.message.top;
  if (contentGap < 0 || contentGap > 16) {
    throw new Error(`expanded content exceeds its 14px message inset (2px tolerance): ${contentGap}px`);
  }
  if (!evidence.inputText?.includes('@conv:fixture-review') || !evidence.inputText.includes('message_id')) {
    throw new Error(`expanded send input is missing expected structured fields: ${JSON.stringify(evidence.inputText)}`);
  }
  if (!evidence.resultText?.includes('Delivered') || !evidence.resultText.includes('8f53edc3-0711-46d8-8f3b-004201a2f410')) {
    throw new Error(`expanded send result is missing its delivery outcome: ${JSON.stringify(evidence.resultText)}`);
  }
  if (!evidence.copyInputPresent || !evidence.queryDatabasePresent || !evidence.continuationPresent) {
    throw new Error(`expanded detail lost a result control or later tool: ${JSON.stringify(evidence)}`);
  }
  if (evidence.documentScrollWidth !== evidence.documentClientWidth) {
    throw new Error(`expanded detail created page overflow: ${JSON.stringify(evidence)}`);
  }
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.locator('.compact-tool-selected-detail [data-tool-id="mobile-send-message"] .tool-block-input button').click();
  const copiedInput = await page.evaluate(() => navigator.clipboard.readText());
  if (!copiedInput.includes('@conv:fixture-review') || !copiedInput.includes('8f53edc3-0711-46d8-8f3b-004201a2f410')) {
    throw new Error(`copy input action did not copy the structured tool input: ${copiedInput}`);
  }

  const stem = `mobile-spacing-compact--${viewport.name ?? `${viewport.width}x${viewport.height}`}`;
  await page.screenshot({ path: path.join(outDir, `${stem}.png`), fullPage: true });
  await writeFile(path.join(outDir, `${stem}.geometry.json`), `${JSON.stringify({ viewport, evidence, copiedInput }, null, 2)}\n`);
  await collapse.click();
  if (await page.locator('.compact-tool-selected-detail').count() !== 0) {
    throw new Error('collapse control did not restore compact details');
  }
  if (await page.locator('[data-tool-id="mobile-next-read"]').count() !== 1) {
    throw new Error('collapse removed or duplicated the following continuation tool');
  }
  await page.locator('[data-tool-id="mobile-send-message"] .compact-tool-card-expand').click();
  await page.locator('.compact-tool-selected-detail [data-tool-id="mobile-send-message"] .tool-block-input').waitFor({ state: 'visible' });
  const reopened = await page.locator('.compact-tool-selected-detail .message-content').boundingBox();
  if (!reopened || Math.abs(reopened.height - evidence.content.height) > 1) {
    throw new Error('collapse/reopen changed expanded content geometry');
  }
  await page.locator('.compact-tool-selected-detail .compact-tool-detail-collapse').click();
  await page.locator('[data-tool-id="mobile-query-database"] .compact-tool-card-expand').click();
  const copyOutput = page.locator('.compact-tool-selected-detail').getByRole('button', { name: 'Copy output', exact: true });
  await copyOutput.click();
  const copiedOutput = await page.evaluate(() => navigator.clipboard.readText());
  if (!copiedOutput.includes('fixture-review-transcript') || !copiedOutput.includes('idle')) {
    throw new Error(`generic result copy lost its full output: ${copiedOutput}`);
  }
  await page.locator('.compact-tool-selected-detail .compact-tool-detail-collapse').click();
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
    { name: 'mobile-narrow', width: 320, height: 760, hasTouch: true, isMobile: true },
  ],
});
