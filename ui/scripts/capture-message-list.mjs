import { runSurfaceCapture } from './capture-ladle-surface.mjs';
import { writeFile } from 'node:fs/promises';
import path from 'node:path';

const width = Number(process.env.MESSAGE_LIST_QA_WIDTH ?? 960);
const height = Number(process.env.MESSAGE_LIST_QA_HEIGHT ?? 900);
const narrowMarkdownTableMetrics = [];

async function verifyWideTable({ page, id, viewport }) {
  if (id !== 'wide-markdown-table' && id !== 'wide-markdown-table-light') return false;

  const desktop = await page.evaluate(() => {
    const chat = document.querySelector('#chat-view');
    const message = document.querySelector('.message.agent');
    const wrapper = document.querySelector('.markdown-table-scroll');
    const table = wrapper?.querySelector('table');
    if (!(chat instanceof HTMLElement)
      || !(message instanceof HTMLElement)
      || !(wrapper instanceof HTMLElement)
      || !(table instanceof HTMLTableElement)) {
      throw new Error('wide table fixture is missing expected layout elements');
    }
    const chatRect = chat.getBoundingClientRect();
    const messageRect = message.getBoundingClientRect();
    const wrapperRect = wrapper.getBoundingClientRect();
    return {
      chatLeft: chatRect.left,
      chatRight: chatRect.right,
      messageLeft: messageRect.left,
      messageRight: messageRect.right,
      wrapperLeft: wrapperRect.left,
      wrapperRight: wrapperRect.right,
      wrapperOverflowX: getComputedStyle(wrapper).overflowX,
      wrapperBackground: getComputedStyle(wrapper).backgroundColor,
      tableBackground: getComputedStyle(table).backgroundColor,
      messageBackground: getComputedStyle(message).backgroundColor,
      wrapperClientWidth: wrapper.clientWidth,
      wrapperScrollWidth: wrapper.scrollWidth,
      documentClientWidth: document.documentElement.clientWidth,
      documentScrollWidth: document.documentElement.scrollWidth,
    };
  });

  if (desktop.wrapperLeft < desktop.chatLeft || desktop.wrapperRight > desktop.chatRight) {
    throw new Error(`Wide table escaped chat bounds: ${JSON.stringify(desktop)}`);
  }
  if (desktop.wrapperClientWidth < Math.min(desktop.messageRight - desktop.messageLeft, 784)) {
    throw new Error(`Wide table wrapper is narrower than its owned table boundary: ${JSON.stringify(desktop)}`);
  }
  if (desktop.wrapperOverflowX !== 'auto' || desktop.wrapperScrollWidth <= desktop.wrapperClientWidth) {
    throw new Error(`Wide table wrapper does not own local overflow: ${JSON.stringify(desktop)}`);
  }
  if (desktop.tableBackground !== desktop.messageBackground || desktop.wrapperBackground !== 'rgba(0, 0, 0, 0)') {
    throw new Error(`Wide table does not own only its painted surface: ${JSON.stringify(desktop)}`);
  }
  if (desktop.documentScrollWidth !== desktop.documentClientWidth) {
    throw new Error(`Wide table created document overflow: ${JSON.stringify(desktop)}`);
  }

  await page.setViewportSize({ width: 375, height: viewport.height });
  const mobile = await page.evaluate(() => {
    const message = document.querySelector('.message.agent');
    const wrapper = document.querySelector('.markdown-table-scroll');
    const cell = wrapper?.querySelector('td:has(code)');
    const inlineCode = cell?.querySelector('code');
    if (!(message instanceof HTMLElement)
      || !(wrapper instanceof HTMLElement)
      || !(cell instanceof HTMLTableCellElement)
      || !(inlineCode instanceof HTMLElement)) {
      throw new Error('wide table fixture is missing mobile layout or typography elements');
    }
    const messageRect = message.getBoundingClientRect();
    const wrapperRect = wrapper.getBoundingClientRect();
    return {
      messageLeft: messageRect.left,
      messageRight: messageRect.right,
      wrapperLeft: wrapperRect.left,
      wrapperRight: wrapperRect.right,
      wrapperOverflowX: getComputedStyle(wrapper).overflowX,
      cellFontSize: getComputedStyle(cell).fontSize,
      inlineCodeFontSize: getComputedStyle(inlineCode).fontSize,
      documentClientWidth: document.documentElement.clientWidth,
      documentScrollWidth: document.documentElement.scrollWidth,
    };
  });
  if (mobile.wrapperLeft < mobile.messageLeft
    || mobile.wrapperRight > mobile.messageRight
    || mobile.wrapperOverflowX !== 'auto'
    || mobile.cellFontSize !== mobile.inlineCodeFontSize
    || mobile.documentScrollWidth !== mobile.documentClientWidth) {
    throw new Error(`Wide table mobile fallback regressed: ${JSON.stringify(mobile)}`);
  }

  await page.setViewportSize({ width: viewport.width, height: viewport.height });
  console.log(`  verified wide table surface, typography, and overflow geometry (${id})`);
  return false;
}

function collectTableMetrics(label) {
  const wrapper = document.querySelector('.conversation-markdown-table-scroll');
  const table = wrapper?.querySelector('table');
  const prose = document.querySelector('.agent-text-block');
  const th = table?.querySelector('th');
  const td = table?.querySelector('td');
  const strong = table?.querySelector('strong');
  const code = table?.querySelector('code');
  if (!(wrapper instanceof HTMLElement)
    || !(table instanceof HTMLTableElement)
    || !(prose instanceof HTMLElement)
    || !(th instanceof HTMLTableCellElement)
    || !(td instanceof HTMLTableCellElement)
    || !(strong instanceof HTMLElement)
    || !(code instanceof HTMLElement)) {
    throw new Error('narrow markdown table fixture is missing expected measurement elements');
  }
  const sizes = (element) => {
    const style = getComputedStyle(element);
    return { fontSize: style.fontSize, lineHeight: style.lineHeight };
  };
  const rect = wrapper.getBoundingClientRect();
  const rootStyle = getComputedStyle(document.documentElement);
  return {
    label,
    viewportWidth: window.innerWidth,
    colorScheme: document.documentElement.classList.contains('dark') ? 'dark' : 'light',
    prose: sizes(prose),
    th: sizes(th),
    td: sizes(td),
    strong: sizes(strong),
    code: sizes(code),
    textSizeAdjust: rootStyle.webkitTextSizeAdjust || rootStyle.textSizeAdjust || '',
    tableClientWidth: table.clientWidth,
    tableScrollWidth: table.scrollWidth,
    wrapperClientWidth: wrapper.clientWidth,
    wrapperScrollWidth: wrapper.scrollWidth,
    wrapperInitialScrollLeft: wrapper.scrollLeft,
    wrapperLeft: rect.left,
    wrapperRight: rect.right,
    wrapperOverflowX: getComputedStyle(wrapper).overflowX,
    tableLayout: getComputedStyle(table).tableLayout,
    tableWidth: getComputedStyle(table).width,
    cellOverflowWrap: getComputedStyle(td).overflowWrap,
    documentClientWidth: document.documentElement.clientWidth,
    documentScrollWidth: document.documentElement.scrollWidth,
    documentOverflowX: rootStyle.overflowX,
    bodyClientWidth: document.body.clientWidth,
    bodyScrollWidth: document.body.scrollWidth,
    selectorHasSupported: CSS.supports('selector(:has(*))'),
  };
}

async function collectNarrowMarkdownTableMetrics(page, viewport, label) {
  await page.setViewportSize(viewport);
  await page.waitForSelector('.conversation-markdown-table-scroll table');
  const patched = await page.evaluate(collectTableMetrics, `${label}:post`);
  await page.addStyleTag({
    content: `
      .agent-text-block .conversation-markdown-table-scroll > table {
        width: max-content !important;
        table-layout: auto !important;
      }
      .agent-text-block .conversation-markdown-table-scroll :where(th, td) {
        overflow-wrap: normal !important;
      }
    `,
  });
  const baseline = await page.evaluate(collectTableMetrics, `${label}:pre-simulated`);
  return { baseline, patched };
}

async function verifyNarrowMarkdownTable({ page, id, outDir }) {
  if (id !== 'narrow-webkit-markdown-table' && id !== 'narrow-webkit-markdown-table-light') return false;

  const theme = id.endsWith('-light') ? 'light' : 'dark';
  await page.evaluate((scenarioTheme) => {
    document.documentElement.classList.toggle('dark', scenarioTheme === 'dark');
  }, theme);
  const desktop = await collectNarrowMarkdownTableMetrics(page, { width: 960, height: 900 }, `${theme}:desktop`);
  await page.reload({ waitUntil: 'networkidle' });
  await page.waitForSelector(`[data-message-list-fixture-ready="${id}"]`, { timeout: 10_000 });
  await page.evaluate((scenarioTheme) => {
    document.documentElement.classList.toggle('dark', scenarioTheme === 'dark');
  }, theme);
  const narrow = await collectNarrowMarkdownTableMetrics(page, { width: 375, height: 900 }, `${theme}:narrow`);
  narrowMarkdownTableMetrics.push(desktop.baseline, desktop.patched, narrow.baseline, narrow.patched);

  for (const sample of [desktop.patched, narrow.patched]) {
    if (sample.th.fontSize !== sample.td.fontSize
      || sample.strong.fontSize !== sample.td.fontSize
      || sample.code.fontSize !== sample.td.fontSize) {
      throw new Error(`Markdown table relative sizing is incoherent: ${JSON.stringify(sample)}`);
    }
    if (sample.th.lineHeight !== sample.td.lineHeight || sample.code.lineHeight !== sample.td.lineHeight) {
      throw new Error(`Markdown table line heights diverged: ${JSON.stringify(sample)}`);
    }
    if (sample.wrapperInitialScrollLeft !== 0 || sample.wrapperLeft < -0.5) {
      throw new Error(`Markdown table initial left edge is inaccessible: ${JSON.stringify(sample)}`);
    }
    if (sample.documentScrollWidth !== sample.documentClientWidth || sample.bodyScrollWidth !== sample.bodyClientWidth) {
      throw new Error(`Markdown table created document overflow: ${JSON.stringify(sample)}`);
    }
    if (sample.wrapperOverflowX !== 'auto') {
      throw new Error(`Markdown table wrapper does not own horizontal overflow: ${JSON.stringify(sample)}`);
    }
  }
  if (narrow.patched.tableLayout !== 'fixed' || narrow.patched.cellOverflowWrap !== 'anywhere') {
    throw new Error(`Narrow table did not receive the narrow readability rule: ${JSON.stringify(narrow.patched)}`);
  }
  await page.screenshot({ path: path.join(outDir, `${id}--narrow-table.png`), fullPage: true });
  console.log(`  verified narrow markdown table metrics (${id})`);
  return true;
}

async function captureContinuityReproduction({ page, id, outDir }) {
  if (id !== 'prefix-continuity-offset-bug') return false;

  const scroller = page.locator('.message-list-fixture-shell #messages, .message-list-fixture-shell [data-testid="virtuoso-scroller"], .message-list-fixture-shell [data-virtuoso-scroller="true"]').first();
  const anchor = page.getByText('Continuity marker 01:', { exact: false }).first();
  await anchor.waitFor();
  await scroller.evaluate((element) => {
    const marker = Array.from(element.querySelectorAll('[data-render-unit-key]'))
      .find((row) => row.textContent?.includes('Continuity marker 01'));
    if (!(marker instanceof HTMLElement)) throw new Error('continuity anchor row not mounted');
    element.scrollTop = marker.offsetTop + Math.min(marker.offsetHeight * 0.55, 700);
    element.dispatchEvent(new Event('scroll'));
  });
  await page.waitForFunction(() => {
    const scroller = document.querySelector('.message-list-fixture-shell #messages, .message-list-fixture-shell [data-testid="virtuoso-scroller"], .message-list-fixture-shell [data-virtuoso-scroller="true"]');
    const marker = Array.from(document.querySelectorAll('[data-render-unit-key]'))
      .find((row) => row.textContent?.includes('Continuity marker 01'));
    if (!(scroller instanceof HTMLElement) || !(marker instanceof HTMLElement)) return false;
    const offset = marker.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    return offset < -20;
  });

  await page.screenshot({ path: path.join(outDir, `${id}--before-prefix.png`), fullPage: true });
  await page.getByTestId('reproduce-prefix-jump').click();
  await page.waitForSelector('[data-continuity-milestone="before-prefix"]');
  await page.waitForSelector('[data-continuity-milestone="after-restore"]', { timeout: 10_000 });
  await page.screenshot({ path: path.join(outDir, `${id}--after-restore.png`), fullPage: true });

  const trace = await page.evaluate(() => window.__messageListContinuityTrace ?? []);
  await writeFile(path.join(outDir, `${id}--trace.json`), `${JSON.stringify(trace, null, 2)}\n`);
  const drift = trace.find((milestone) => milestone.name === 'after-restore')?.drift;
  if (typeof drift !== 'number' || Math.abs(drift) > 2) {
    throw new Error(`Expected prefix continuity drift <=2px; observed ${String(drift)}`);
  }
  console.log(`  verified prefix continuity drift: ${drift.toFixed(1)}px`);
  return true;
}

runSurfaceCapture({
  surface: 'message-list',
  readyAttribute: 'data-message-list-fixture-ready',
  outDir: process.env.MESSAGE_LIST_QA_OUT ?? 'qa-artifacts/message-list',
  viewport: { width, height },
  captureStory: async (context) => (
    await verifyWideTable(context) || await verifyNarrowMarkdownTable(context) || await captureContinuityReproduction(context)
  ),
  onComplete: async (outDir) => {
    if (narrowMarkdownTableMetrics.length > 0) {
      await writeFile(
        path.join(outDir, 'narrow-webkit-markdown-table-metrics.json'),
        `${JSON.stringify(narrowMarkdownTableMetrics, null, 2)}\n`,
      );
    }
  },
});
