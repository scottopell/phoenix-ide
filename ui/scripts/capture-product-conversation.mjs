import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import { buildLadleStoryUrl, runSurfaceCapture } from './capture-ladle-surface.mjs';

const measurements = [];
const journeys = [];

async function fixtureValue(page, name) {
  return page.locator('html').getAttribute(`data-product-conversation-fixture-${name}`);
}

async function assertTranscriptGeometry(page, id, viewport, theme) {
  const geometry = await page.evaluate(() => {
    const required = {
      transcript: document.querySelector('.product-conversation-page__transcript'),
      mainArea: document.querySelector('#main-area'),
      chatView: document.querySelector('#chat-view'),
      virtualTranscript: document.querySelector('.virtual-transcript'),
      title: document.querySelector('.product-conversation-page__title'),
      composer: document.querySelector('[data-testid="product-conversation-composer"], [data-testid="product-conversation-history"]'),
    };
    const missing = Object.entries(required).filter(([, element]) => !element).map(([name]) => name);
    if (missing.length) return { missing };
    const box = (element) => {
      const rect = element.getBoundingClientRect();
      return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
    };
    const intersects = (a, b) => a.width > 0 && a.height > 0
      && a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y;
    const viewportBox = { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight };
    const contained = (rect) => rect.x >= 0 && rect.y >= 0
      && rect.x + rect.width <= window.innerWidth
      && rect.y + rect.height <= window.innerHeight;
    const rows = [...document.querySelectorAll('.virtual-transcript__row')];
    return {
      display: getComputedStyle(required.transcript).display,
      transcript: box(required.transcript), mainArea: box(required.mainArea), chatView: box(required.chatView),
      virtualTranscript: box(required.virtualTranscript), title: box(required.title), composer: box(required.composer),
      visibleRealMessageRow: rows.some((row) => row.querySelector('.message.user, .message.agent') && intersects(box(row), viewportBox)),
      positiveRows: rows.filter((row) => { const rect = row.getBoundingClientRect(); return rect.width > 0 && rect.height > 0; }).length,
      horizontalOverflow: document.documentElement.scrollWidth > window.innerWidth,
      documentOverflow: document.documentElement.scrollWidth > window.innerWidth || document.documentElement.scrollHeight > window.innerHeight,
      viewportContained: contained(box(required.title)) && contained(box(required.composer))
        && contained(box(required.transcript)) && contained(box(required.mainArea))
        && contained(box(required.chatView)) && contained(box(required.virtualTranscript)),
    };
  });
  if ('missing' in geometry) throw new Error(`${id}/${theme}: missing ${geometry.missing.join(', ')}`);
  for (const name of ['transcript', 'mainArea', 'chatView', 'virtualTranscript']) {
    if (geometry[name].height <= 0 || geometry[name].width <= 0) throw new Error(`${id}/${theme}: ${name} has non-positive geometry`);
  }
  if (geometry.display !== 'flex' || !geometry.visibleRealMessageRow || geometry.positiveRows === 0
    || geometry.horizontalOverflow || geometry.documentOverflow || !geometry.viewportContained) {
    throw new Error(`${id}/${theme}: transcript/viewport containment assertion failed: ${JSON.stringify(geometry)}`);
  }
  measurements.push({ id, viewport: viewport.name, theme, ...geometry });
}

async function assertContinuationHandoff(page, viewport) {
  const handoff = page.locator('.context-exhausted-handoff');
  await handoff.waitFor({ state: 'visible' });
  const actions = ['Continue', 'Edit first', 'Copy handoff'];
  for (const name of actions) {
    const button = handoff.getByRole('button', { name });
    await button.scrollIntoViewIfNeeded();
    const box = await button.boundingBox();
    if (!box || box.width <= 0 || box.height < 44 || box.x < 0 || box.y < 0
      || box.x + box.width > viewport.width || box.y + box.height > viewport.height) {
      throw new Error(`${name} is not visibly reachable at ${viewport.width}x${viewport.height}: ${JSON.stringify(box)}`);
    }
  }
  await handoff.getByRole('button', { name: 'Copy handoff' }).click();
  await handoff.getByRole('button', { name: 'Edit first' }).click();
  const editor = handoff.getByRole('textbox', { name: 'Handoff' });
  await editor.scrollIntoViewIfNeeded();
  const editorBox = await editor.boundingBox();
  if (!editorBox || editorBox.y < 0 || editorBox.y + editorBox.height > viewport.height) {
    throw new Error(`handoff review editor is not reachable: ${JSON.stringify(editorBox)}`);
  }
  journeys.push(`continuation ${viewport.name}: Continue, review, and Copy are viewport-reachable at ${viewport.width}x${viewport.height}`);
}

async function assertHistoricalBoundary(page) {
  const boundary = page.getByRole('region', { name: 'Conversation continuation boundary' }).first();
  await boundary.waitFor({ state: 'visible' });
  await boundary.getByText('Conversation continued in the next segment').waitFor();
  const composer = page.locator('[data-testid="product-conversation-composer"] #message-input');
  await composer.waitFor({ state: 'visible' });
  journeys.push('completed predecessor: typed compacted boundary remains historical while latest successor composer stays active');
}

async function assertViewer(page, viewport, outDir) {
  const message = page.locator('.virtual-transcript__row .message.user, .virtual-transcript__row .message.agent').filter({ hasText: /./ }).first();
  await message.click({ button: 'right' });
  const contextMenu = page.locator('.msg-context-menu').filter({ visible: true });
  await contextMenu.waitFor({ state: 'visible' });
  await contextMenu.getByRole('button', {
    name: viewport.width >= 1280 ? 'Open in sidepanel' : 'Open in fullscreen',
  }).click();
  const viewer = page.getByRole(viewport.width >= 1280 ? 'region' : 'dialog', { name: /Message viewer/ });
  await viewer.waitFor();
  await page.screenshot({ path: `${outDir}/${viewport.name}--viewer-open.png`, fullPage: true });
  const box = await viewer.boundingBox();
  if (!box || box.x < 0 || box.y < 0 || box.x + box.width > viewport.width || box.y + box.height > viewport.height) {
    throw new Error(`message viewer escapes ${viewport.name} viewport`);
  }
  if (viewport.width >= 1280) {
    await page.locator('.product-conversation-page--split-pane .product-conversation-page__viewer-pane').waitFor();
    const transcript = await page.locator('.product-conversation-page__transcript').boundingBox();
    if (!transcript || transcript.width <= 0 || transcript.height <= 0) throw new Error('split pane removed transcript geometry');
  }
  let viewerClosedBySend = false;
  if (viewport.name === 'mobile-dark') {
    const uniqueNote = 'aggregate-viewer-note-5c47a2';
    await viewer.getByRole('button', { name: 'Add note to line 1' }).click({ force: true });
    await page.locator('.annotation-dialog .annotation-dialog-input').fill(uniqueNote);
    await page.getByRole('button', { name: 'Add Note', exact: true }).click();
    await viewer.getByRole('button', { name: 'Send notes' }).click();
    const composer = page.locator('[data-testid="product-conversation-composer"] #message-input');
    await composer.waitFor();
    if (!(await composer.inputValue()).includes(uniqueNote)) {
      throw new Error('aggregate viewer notes did not reach the real ProductConversation composer');
    }
    await viewer.waitFor({ state: 'hidden' });
    viewerClosedBySend = true;
    journeys.push('viewer notes: aggregate annotation reached the real latest-row composer draft');
  }
  if (!viewerClosedBySend) {
    await viewer.getByRole('button', { name: /Close/ }).click();
    await viewer.waitFor({ state: 'hidden' });
  }
  journeys.push(`viewer ${viewport.name}: overlay/pane in bounds and closes`);
}

async function assertComposer(page) {
  const unique = 'fixture-browser-send-7f4c7ad4';
  const input = page.locator('[data-testid="product-conversation-composer"] #message-input');
  await input.fill(unique);
  await page.getByRole('button', { name: 'Send message' }).click();
  await page.locator('html[data-product-conversation-fixture-send-count="1"]').waitFor();
  if (await fixtureValue(page, 'last-sent-text') !== unique) throw new Error('composer sent unexpected fixture payload');
  journeys.push('composer: real embedded composer submitted exactly one observed API send');
}

async function assertRetry(page, viewport, theme) {
  const before = Number(await fixtureValue(page, 'snapshot-requests') ?? '0');
  await page.getByRole('button', { name: 'Retry' }).click();
  await page.waitForFunction((previous) => Number(document.documentElement.dataset.productConversationFixtureSnapshotRequests ?? '0') > previous, before);
  await page.locator('[data-testid="product-conversation-transcript"]').waitFor();
  await assertTranscriptGeometry(page, 'error-recovered', viewport, theme);
  journeys.push('initial error: Retry made a second snapshot request and rendered ready transcript');
}

async function assertReconnect(page) {
  const beforeRows = await page.locator('.virtual-transcript__row').count();
  const beforeOpens = Number(await fixtureValue(page, 'event-source-opens') ?? '0');
  const beforeInits = Number(await fixtureValue(page, 'event-source-inits') ?? '0');
  const beforeInstance = Number(await fixtureValue(page, 'event-source-last-instance') ?? '0');
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('product-conversation-fixture-stream-failure')));
  await page.waitForFunction(
    ({ opens, inits, instance }) => {
      const dataset = document.documentElement.dataset;
      return Number(dataset.productConversationFixtureEventSourceOpens ?? '0') === opens + 1
        && Number(dataset.productConversationFixtureEventSourceInits ?? '0') === inits + 1
        && Number(dataset.productConversationFixtureEventSourceLastInitializedInstance ?? '0') > instance;
    },
    { opens: beforeOpens, inits: beforeInits, instance: beforeInstance },
  );
  const replacementUrl = await fixtureValue(page, 'event-source-last-url') ?? '';
  const replacementParams = new URL(replacementUrl, page.url()).searchParams;
  if (Number(replacementParams.get('after_event_sequence') ?? '0') <= 0
    || replacementParams.get('transcript_generation') !== '1') {
    throw new Error(`replacement SSE omitted replay cursor/generation: ${replacementUrl}`);
  }
  await page.locator('[data-testid="product-conversation-composer"] .state-text').getByText('reconnected', { exact: true }).waitFor();
  const afterRows = await page.locator('.virtual-transcript__row').count();
  if (afterRows !== beforeRows) throw new Error(`reconnect duplicated transcript rows (${beforeRows} -> ${afterRows})`);
  journeys.push('ordinary latest-row SSE recovery: native failure caused production backoff, replacement EventSource, replay init, and no duplicate rows');
}

async function assertLongHistory(page) {
  const transcript = page.locator('.virtual-transcript');
  await page.getByText('Older deep-link target from the real cursor page.').waitFor();
  await transcript.evaluate((element) => element.scrollTo({ top: element.scrollHeight }));
  await page.getByText('Final status summary: deterministic fixture transcript content for chronology validation.').waitFor();
  await transcript.evaluate((element) => element.scrollTo({ top: 0 }));
  await page.getByText('Older deep-link target from the real cursor page.').waitFor();
  const boundary = page.getByRole('region', { name: 'Conversation continuation boundary' }).filter({ hasText: 'A historical handoff boundary survives pagination.' });
  await boundary.waitFor({ state: 'visible' });
  await boundary.getByText('Review handoff').click();
  await boundary.getByText('A historical handoff boundary survives pagination.').waitFor({ state: 'visible' });
  journeys.push('long transcript: deep-linked older row, controlled scroll to newest row, and paginated handoff boundary visible');
}

async function assertDeepLink(page) {
  await page.locator('html[data-product-conversation-fixture-older-snapshot-requests="1"]').waitFor();
  const target = page.locator('#message-long-older-target, [data-message-id="long-older-target"]');
  await target.waitFor();
  await target.evaluate((element) => element.scrollIntoView());
  await page.locator('#message-long-older-target.jump-highlight, [data-message-id="long-older-target"].jump-highlight').waitFor();
  journeys.push('deep link: hash target fetched via real older cursor, virtualized visible, and marked');
}

async function assertRecallAndWork(page, viewport, outDir) {
  const work = page.locator('.product-conversation-page__work');
  await work.locator('summary').click();
  await work.getByText(/task-|Branch|PR #/).first().waitFor();
  await work.locator('summary').click();

  const recall = page.getByRole('button', { name: 'Recall' });
  await recall.click();
  const panel = page.getByRole('dialog', { name: 'Recall' });
  await panel.waitFor();
  const box = await panel.boundingBox();
  if (!box || box.x < 0 || box.y < 0 || box.x + box.width > viewport.width || box.y + box.height > viewport.height) {
    throw new Error(`Recall panel escaped ${viewport.name}: ${JSON.stringify(box)}`);
  }
  await panel.getByText('Which invariants carried across the whole conversation?').waitFor();
  await page.screenshot({ path: `${outDir}/${viewport.name}--recall-open.png`, fullPage: true });
  await panel.getByRole('button', { name: 'Close Recall' }).click();
  await panel.waitFor({ state: 'detached' });
  journeys.push(`Work and Recall ${viewport.name}: typed content visible; Recall in bounds and closes`);
}

async function writeReport(outDir) {
  const report = [
    '# ProductConversation deterministic browser UAT', '',
    'Event-driven only: fixture-ready markers, locator state, and recorded fixture API calls. No journey sleeps.', '',
    '## Core geometry/theme matrix', '',
    '| Scenario | Viewport | Theme | transcript h | main h | chat h | virtual h | positive rows | overflow |',
    '| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |',
    ...measurements.map((item) => `| ${item.id} | ${item.viewport} | ${item.theme} | ${item.transcript.height} | ${item.mainArea.height} | ${item.chatView.height} | ${item.virtualTranscript.height} | ${item.positiveRows} | ${item.horizontalOverflow ? 'yes' : 'no'} |`),
    '', '## Deterministic interaction coverage', ...journeys.map((journey) => `- ${journey}`), '',
    'Screenshots: every scenario in light/dark at desktop/mobile, plus open viewer and Recall states. Viewport containment and CSS safe-area token use are checked separately; emulated zero-inset viewports cannot prove physical notch geometry. Shared runner fails on unexpected console, page, request, and >=400 response failures.',
    `Browser engine: ${process.env.PLAYWRIGHT_BROWSER ?? 'chromium'}.`, '',
  ].join('\n');
  await writeFile(path.join(outDir, 'UAT_REPORT.md'), report);
}

runSurfaceCapture({
  surface: 'product-conversation',
  readyAttribute: 'data-product-conversation-fixture-ready',
  outDir: process.env.PRODUCT_CONVERSATION_QA_OUT ?? 'qa-artifacts/product-conversation',
  viewportMatrix: [
    { name: 'desktop-light', width: 1440, height: 900 }, { name: 'desktop-dark', width: 1440, height: 900 },
    { name: 'mobile-light', width: 390, height: 844 }, { name: 'mobile-dark', width: 390, height: 844 },
  ],
  urlForStory: ({ storyKey, id, viewport }) => {
    const theme = viewport.name.endsWith('-light') ? 'light' : 'dark';
    const fixtureHash = id === 'long-history-110-messages' ? '#message-long-older-target' : '';
    const baseUrl = process.env.LADLE_URL ?? `http://127.0.0.1:${process.env.LADLE_PORT ?? 61123}/`;
    return buildLadleStoryUrl(baseUrl, storyKey, { fixtureTheme: theme, fixtureHash });
  },
  captureStory: async ({ page, id, outDir, viewport }) => {
    const theme = await fixtureValue(page, 'theme') ?? 'dark';
    if (!['loading', 'error'].includes(id)) await assertTranscriptGeometry(page, id, viewport, theme);
    await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--${theme}--initial.png`, fullPage: true });
    if (id === 'mobile-open' && viewport.name === 'mobile-dark') {
      await assertViewer(page, viewport, outDir);
      await assertReconnect(page);
      await assertComposer(page);
    }
    if (id === 'mobile-context-exhausted' && viewport.name === 'mobile-dark') {
      await assertContinuationHandoff(page, viewport);
    }
    if (id === 'awaiting-continuation' && viewport.name === 'mobile-dark') {
      await page.getByText('Compacting conversation...', { exact: true }).waitFor({ state: 'visible' });
      if (await page.locator('[data-testid="product-conversation-composer"] #message-input').count()) {
        throw new Error('awaiting continuation exposed an ordinary composer');
      }
      journeys.push('awaiting continuation: compacting progress replaces the aggregate composer');
    }
    if (id === 'desktop-multi-segment-qa-work' && viewport.name === 'desktop-dark') {
      await assertHistoricalBoundary(page);
    }
    if (id === 'desktop-multi-segment-qa-work' && viewport.name === 'desktop-light') {
      await assertViewer(page, viewport, outDir);
      await assertRecallAndWork(page, viewport, outDir);
    }
    if (id === 'desktop-multi-segment-qa-work' && viewport.name === 'mobile-dark') {
      await assertRecallAndWork(page, viewport, outDir);
    }
    if (id === 'error' && viewport.name === 'mobile-dark') await assertRetry(page, viewport, theme);
    if (id === 'long-history-110-messages') {
      if (viewport.name === 'mobile-dark') await assertLongHistory(page);
      if (viewport.name === 'desktop-dark') await assertDeepLink(page);
    }
    return false;
  },
  onComplete: writeReport,
});
