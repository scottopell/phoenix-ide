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

async function captureCompactChronology({ page, id, outDir }) {
  if (id !== 'compact-expanded-tool-chronology') return false;

  const measureExpandedA = async (label) => page.evaluate((sampleLabel) => {
    const scroller = document.querySelector('.message-list-fixture-shell #messages');
    const expanded = document.querySelector('[data-tool-id="chronology-tool-a"]');
    if (!(scroller instanceof HTMLElement) || !(expanded instanceof HTMLElement)) {
      throw new Error(`chronology A measurement target missing for ${sampleLabel}`);
    }
    const scrollerRect = scroller.getBoundingClientRect();
    const expandedRect = expanded.getBoundingClientRect();
    return {
      label: sampleLabel,
      top: expandedRect.top - scrollerRect.top,
      bottom: expandedRect.bottom - scrollerRect.top,
      scrollTop: scroller.scrollTop,
      scrollHeight: scroller.scrollHeight,
      clientHeight: scroller.clientHeight,
      atTail: Math.abs(scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop) <= 4,
    };
  }, label);

  const assertStableExpandedA = (before, after) => {
    const drift = Math.abs(after.top - before.top);
    if (drift > 4) {
      throw new Error(`Expanded A viewport position drifted ${drift.toFixed(1)}px: ${JSON.stringify({ before, after })}`);
    }
    if (after.atTail) {
      throw new Error(`Chronology capture returned to tail-follow instead of reader-owned scroll: ${JSON.stringify(after)}`);
    }
  };

  const assertResizePreservedExpandedA = (before, after) => {
    const drift = Math.abs(after.top - before.top);
    if (drift > 160 || after.bottom <= 0 || after.top >= after.clientHeight) {
      throw new Error(`Expanded A was not preserved through resize: ${JSON.stringify({ before, after, drift })}`);
    }
  };

  const assertSameCompactGroup = async (label) => {
    const order = await page.evaluate((sampleLabel) => ({
      label: sampleLabel,
      toolDomOrder: Array.from(document.querySelectorAll('[data-tool-id]')).map((node) => node.getAttribute('data-tool-id')),
      groupCount: document.querySelectorAll('.compact-tool-group').length,
    }));
    const compactOrder = order.toolDomOrder.filter((toolId) => toolId?.startsWith('chronology-tool-'));
    if (compactOrder.join(',') !== 'chronology-tool-a,chronology-tool-b,chronology-tool-c' || order.groupCount !== 1) {
      throw new Error(`Chronology tools are not in one compact group: ${JSON.stringify(order)}`);
    }
    return order;
  };

  const assertCompletedPairing = async (label) => {
    const pairing = await page.evaluate((sampleLabel) => {
      const textFor = (toolId) => document.querySelector(`[data-tool-id="${toolId}"]`)?.textContent ?? '';
      return {
        label: sampleLabel,
        bText: textFor('chronology-tool-b'),
        cText: textFor('chronology-tool-c'),
      };
    }, label);
    if (!pairing.bText.includes('done') || pairing.bText.includes('pending') || !pairing.cText.includes('exit 0') || !pairing.cText.includes('C_OK')) {
      throw new Error(`Chronology B/C results are not paired with completed cards: ${JSON.stringify(pairing)}`);
    }
    return pairing;
  };

  const runChronologyFlow = async ({ label, width, height, resizedWidth, resizedHeight }) => {
    await page.setViewportSize({ width, height });
    await page.reload({ waitUntil: 'networkidle' });
    await page.waitForSelector(`[data-message-list-fixture-ready="${id}"]`, { timeout: 10_000 });

    await page.waitForSelector('[data-tool-id="chronology-tool-a"] .compact-tool-card-expand');
    await page.locator('[data-tool-id="chronology-tool-a"] .compact-tool-card-expand').click();
    await page.waitForSelector('.compact-tool-selected-detail [data-tool-id="chronology-tool-a"]');
    await page.locator('.message-list-fixture-shell #messages').evaluate((scroller) => {
      scroller.scrollTop = Math.max(1, scroller.scrollTop - 120);
      scroller.dispatchEvent(new Event('scroll'));
    });
    await page.waitForFunction(() => {
      const scroller = document.querySelector('.message-list-fixture-shell #messages');
      if (!(scroller instanceof HTMLElement)) return false;
      return scroller.scrollTop > 0
        && Math.abs(scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop) > 4;
    });
    const expandedBeforeAppend = await measureExpandedA(`${label}:before-append`);

    if (await page.locator('.jump-to-newest').count() !== 0) {
      throw new Error(`${label}: unread jump appeared before tail advanced`);
    }
    await page.setViewportSize({ width: resizedWidth, height: resizedHeight });
    await page.waitForSelector('.compact-tool-selected-detail [data-tool-id="chronology-tool-a"]');
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const expandedAfterResize = await measureExpandedA(`${label}:after-mounted-resize`);
    assertResizePreservedExpandedA(expandedBeforeAppend, expandedAfterResize);
    if (await page.locator('.jump-to-newest').count() !== 0) {
      throw new Error(`${label}: unread jump appeared during resize without tail advance`);
    }
    await page.locator('.message-list-fixture-shell #messages').hover();
    await page.mouse.wheel(0, -160);
    await page.waitForFunction(() => {
      const scroller = document.querySelector('.message-list-fixture-shell #messages');
      if (!(scroller instanceof HTMLElement)) return false;
      return Math.abs(scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop) > 4;
    });
    const expandedAfterResizeReaderOwned = await measureExpandedA(`${label}:after-resize-reader-owned`);

    await page.getByTestId('chronology-append-bc').click();
    await page.waitForFunction(() => document.documentElement.dataset['messageListChronologyPhase'] === 'appended-bc');
    const sameGroup = await assertSameCompactGroup(`${label}:appended-bc`);
    const expandedAfterAppend = await measureExpandedA(`${label}:after-append-bc`);
    assertStableExpandedA(expandedAfterResizeReaderOwned, expandedAfterAppend);

    await page.getByTestId('chronology-complete-bc').click();
    await page.waitForFunction(() => document.documentElement.dataset['messageListChronologyPhase'] === 'completed-bc');
    const expandedAfterComplete = await measureExpandedA(`${label}:after-complete-bc`);
    assertStableExpandedA(expandedAfterResizeReaderOwned, expandedAfterComplete);

    await page.getByTestId('chronology-final').click();
    await page.waitForFunction(() => document.documentElement.dataset['messageListChronologyPhase'] === 'final-prose');
    await page.waitForSelector('.jump-to-newest');
    const expandedAfterFinal = await measureExpandedA(`${label}:after-final-prose`);
    assertStableExpandedA(expandedAfterResizeReaderOwned, expandedAfterFinal);

    await page.locator('.compact-tool-detail-collapse').click();
    await page.waitForFunction(() => {
      const active = document.activeElement;
      return active instanceof HTMLElement
        && active.closest('[data-message-id="chronology-agent-a"]')
        && active.matches('.compact-tool-card-expand');
    });

    await page.locator('.jump-to-newest').evaluate((button) => {
      if (!(button instanceof HTMLButtonElement)) throw new Error('jump-to-newest is not a button');
      button.click();
    });
    await page.waitForFunction(() => !document.querySelector('.jump-to-newest'));
    await page.waitForSelector('#message-chronology-agent-final');
    await page.waitForSelector('[data-tool-id="chronology-tool-c"]');
    const completedPairing = await assertCompletedPairing(`${label}:completed-bc`);
    const latestReachability = await page.evaluate(() => {
      const scroller = document.querySelector('.message-list-fixture-shell #messages');
      const finalMessage = document.querySelector('#message-chronology-agent-final');
      return {
        latestReachable: finalMessage instanceof HTMLElement && scroller instanceof HTMLElement
          && finalMessage.offsetTop <= scroller.scrollHeight - finalMessage.offsetHeight,
        finalText: finalMessage?.textContent ?? '',
      };
    });
    if (!latestReachability.latestReachable || !latestReachability.finalText.includes('Final prose after B and C completed')) {
      throw new Error(`Compact chronology final prose was not reachable: ${JSON.stringify(latestReachability)}`);
    }

    const samples = [expandedBeforeAppend, expandedAfterResize, expandedAfterResizeReaderOwned, expandedAfterAppend, expandedAfterComplete, expandedAfterFinal];
    await page.screenshot({ path: path.join(outDir, `${id}--${label}.png`), fullPage: true });
    return { label, initialViewport: { width, height }, resizedViewport: { width: resizedWidth, height: resizedHeight }, latestReachability, expandedA: samples, completedPairing, sameGroup };
  };

  const runs = [
    await runChronologyFlow({ label: 'mobile-to-desktop', width: 390, height: 900, resizedWidth: 960, resizedHeight: 900 }),
  ];
  await writeFile(path.join(outDir, `${id}--metrics.json`), `${JSON.stringify(runs, null, 2)}\n`);
  console.log('  verified compact chronology append/complete/final flow at mobile and desktop widths');
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
    await verifyWideTable(context) || await captureCompactChronology(context) || await captureContinuityReproduction(context)
  ),
});
