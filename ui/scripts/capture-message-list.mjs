import { runSurfaceCapture } from './capture-ladle-surface.mjs';
import { writeFile } from 'node:fs/promises';
import path from 'node:path';

const width = Number(process.env.MESSAGE_LIST_QA_WIDTH ?? 960);
const height = Number(process.env.MESSAGE_LIST_QA_HEIGHT ?? 900);

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
  if (desktop.wrapperLeft >= desktop.messageLeft || desktop.wrapperRight <= desktop.messageRight) {
    throw new Error(`Wide table did not break out on both sides: ${JSON.stringify(desktop)}`);
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
    if (!(message instanceof HTMLElement) || !(wrapper instanceof HTMLElement)) {
      throw new Error('wide table fixture is missing mobile layout elements');
    }
    const messageRect = message.getBoundingClientRect();
    const wrapperRect = wrapper.getBoundingClientRect();
    return {
      messageLeft: messageRect.left,
      messageRight: messageRect.right,
      wrapperLeft: wrapperRect.left,
      wrapperRight: wrapperRect.right,
      wrapperOverflowX: getComputedStyle(wrapper).overflowX,
      documentClientWidth: document.documentElement.clientWidth,
      documentScrollWidth: document.documentElement.scrollWidth,
    };
  });
  if (mobile.wrapperLeft < mobile.messageLeft
    || mobile.wrapperRight > mobile.messageRight
    || mobile.wrapperOverflowX !== 'auto'
    || mobile.documentScrollWidth !== mobile.documentClientWidth) {
    throw new Error(`Wide table mobile fallback regressed: ${JSON.stringify(mobile)}`);
  }

  await page.setViewportSize({ width: viewport.width, height: viewport.height });
  console.log(`  verified wide table surface and overflow geometry (${id})`);
  return false;
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
    await verifyWideTable(context) || await captureContinuityReproduction(context)
  ),
});
