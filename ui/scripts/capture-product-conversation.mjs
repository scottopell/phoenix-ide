import { captureComposerFocus } from './capture-focus-regressions.mjs';
import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { buildLadleStoryUrl, runSurfaceCapture } from './capture-ladle-surface.mjs';

const measurements = [];
const trackingMeasurements = [];
const journeys = [];

async function assertMobileContinuation(page, id, viewport, outDir) {
  const input = page.locator('.product-conversation-page__composer #message-input');
  await input.waitFor();
  await input.fill('Unsent mobile draft');
  await input.evaluate(element => {
    window.__continuationComposer = element;
    element.setSelectionRange(3, 9);
  });
  const assertRetainedEditor = async () => {
    if (!await input.evaluate(element => element === window.__continuationComposer
      && document.activeElement === element && element.selectionStart === 3 && element.selectionEnd === 9)) {
      throw new Error('Continuation replaced the editor or lost native focus/selection');
    }
  };
  await page.evaluate(() => {
    window.dispatchEvent(new Event('product-conversation-fixture-hold-successor'));
    window.dispatchEvent(new Event('product-conversation-fixture-hold-successor-readback'));
  });
  const route = await page.locator('html').getAttribute('data-source-fixture-location');
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--before.png`, fullPage: true });
  await page.evaluate(() => window.dispatchEvent(new Event('product-conversation-fixture-continue')));
  await page.waitForFunction(() => {
    const streamUrl = document.documentElement.dataset.productConversationFixtureEventSourceLastUrl;
    if (!streamUrl || document.documentElement.dataset.productConversationFixtureActiveTranscriptSubscriptions !== '1') return false;
    const parsed = new URL(streamUrl, window.location.href);
    return parsed.pathname === '/api/conversation-reads/stream'
      && parsed.searchParams.get('reference') === 'fixture-successor';
  });
  await assertRetainedEditor();
  await page.evaluate(() => {
    window.dispatchEvent(new Event('product-conversation-fixture-release-successor-readback'));
    window.dispatchEvent(new Event('product-conversation-fixture-release-successor'));
  });
  await page.waitForFunction(() => document.documentElement.dataset.productConversationFixtureLastDeliveredInitMember === 'fixture-successor');
  await assertRetainedEditor();
  await page.evaluate(() => window.dispatchEvent(new Event('product-conversation-fixture-successor-message')));
  await page.getByText('Successor is streaming after the handoff.', { exact: true }).waitFor();
  if (await input.inputValue() !== 'Unsent mobile draft') throw new Error('Continuation lost the unsent draft');
  if (await page.locator('html').getAttribute('data-source-fixture-location') !== route) throw new Error('Continuation navigated the stable route');
  if (await page.getByRole('button', { name: 'Open continuation', exact: true }).count()) throw new Error('Redundant continuation action remains');
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--followed.png`, fullPage: true });
  await page.evaluate(() => window.dispatchEvent(new Event('product-conversation-fixture-stream-failure')));
  await page.waitForFunction(() => {
    const data = document.documentElement.dataset;
    return Number(data.productConversationFixtureLastDeliveredInitInstance) > Number(data.productConversationFixtureFailedTranscriptInstance)
      && data.productConversationFixtureLastDeliveredInitMember === 'fixture-successor'
      && data.productConversationFixtureActiveTranscriptSubscriptions === '1';
  });
  await page.evaluate(() => window.dispatchEvent(new Event('product-conversation-fixture-successor-message')));
  await page.getByText('Successor is streaming after reconnect.', { exact: true }).waitFor();
  if (await page.getByText('Successor is streaming after the handoff.', { exact: true }).count() !== 1) throw new Error('Reconnect duplicated/lost successor message');
  await assertRetainedEditor();
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--reconnected.png`, fullPage: true });
  journeys.push(`${id}/${viewport.name}: stable-route real SSE continuation, held cold hydration, retained native editor/focus/selection/draft, one subscription and new post-reconnect message; synthetic mobile browser only`);
}

async function assertSourceCall(page, id, viewport, outDir) {
  await page.evaluate(() => { window.__sourcePulseSeen = false; const observer = new MutationObserver(() => { if (document.querySelector('[data-tool-id="source-send-call"].jump-highlight')) window.__sourcePulseSeen = true; }); observer.observe(document.body, { subtree: true, attributes: true, attributeFilter: ["class"] }); });
  const expectedPath = id === 'source-call-global' ? '/global/source-member' : '/c/source-member';
  await page.getByRole('link', { name: /transcript ID .* · source call$/ }).click();
  await page.waitForFunction(path => document.documentElement.dataset.sourceFixtureLocation?.startsWith(path), expectedPath);
  const target = page.locator('[data-tool-id="source-send-call"]');
  await target.waitFor({ state: 'visible' });
  await page.waitForFunction(() => {
    const element = document.querySelector('[data-tool-id="source-send-call"]');
    const box = element?.getBoundingClientRect();
    return box && box.top >= 0 && box.bottom <= innerHeight && element.textContent.includes('Source jump fixture payload');
  });
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--source.png`, fullPage: true });
  if (!await page.evaluate(() => window.__sourcePulseSeen)) throw new Error("Exact source tool was not highlighted");
  const entry = await page.locator('html').getAttribute('data-source-fixture-location');
  const url = new URL(page.url()); url.searchParams.set('fixtureEntry', entry);
  await page.goto(url.href);
  await page.locator('[data-tool-id="source-send-call"]').waitFor({ state: 'visible' });
  if (await page.locator('[role="alert"]').count()) throw new Error('Source jump exposed an error alert');
  journeys.push(`${id}/${viewport.name}: receiver click and cold reload reach expanded exact source call`);
}

async function assertSupersededBoundary(page, id, viewport, outDir) {
  await page.evaluate(() => window.dispatchEvent(new Event('product-conversation-fixture-hold-older')));
  await page.getByRole('button', { name: 'Load context boundary 2', exact: true }).click();
  await page.waitForFunction(() => document.documentElement.dataset.productConversationFixtureOlderSnapshotRequests === '1');
  await page.getByRole('button', { name: /Member 3 request$/ }).click();
  await page.waitForFunction(() => document.documentElement.dataset.sourceFixtureLocation?.endsWith('#message-history-prompt-3'));
  await page.evaluate(() => window.dispatchEvent(new Event('product-conversation-fixture-release-older')));
  await page.getByRole('button', { name: 'Context boundary 2', exact: true }).waitFor();
  await page.waitForFunction(() => {
    const row = [...document.querySelectorAll('.virtual-transcript [data-render-unit-key]')].find(element => element.textContent.includes('Member 3 request'));
    const viewport = document.querySelector('.product-conversation-page__transcript .chat-main-area');
    if (!row || !viewport) return false;
    const rect = row.getBoundingClientRect(); const bounds = viewport.getBoundingClientRect();
    return document.documentElement.dataset.sourceFixtureLocation?.endsWith('#message-history-prompt-3') && rect.bottom > bounds.top && rect.top < bounds.bottom;
  });
  if (await fixtureValue(page, 'older-snapshot-requests') !== '1') throw new Error('Superseded boundary caused an unnecessary second page read');
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--superseded-boundary.png`, fullPage: true });
  if (!await page.locator('.product-conversation-page__composer #message-input').isEnabled() || !(await fixtureValue(page, 'event-source-last-url'))?.includes('history-3')) throw new Error('Reader selection changed newest-member composer ownership');
  journeys.push(`${id}/${viewport.name}: delayed unloaded-boundary read resolves after loaded prompt selection; actual shared reader retains new prompt URL and visible prompt, with newest composer unchanged`);
  const cleanUrl = new URL(page.url()); cleanUrl.hash = '';
  cleanUrl.searchParams.set('fixturePass', 'normal-history');
  await page.goto(cleanUrl.href);
  await page.waitForSelector(`[data-product-conversation-fixture-ready="${id}"]`);
}

async function assertLazyHistory(page, id, viewport, outDir) {
  const initialLocation = await page.locator('html').getAttribute('data-source-fixture-location');
  if (await fixtureValue(page, 'older-snapshot-requests') !== null) throw new Error('Older history was acquired before navigation');
  await page.getByRole('button', { name: 'Load context boundary 1', exact: true }).click();
  await page.waitForFunction(() => document.documentElement.dataset.productConversationFixtureOlderSnapshotRequests === '2');
  await page.getByRole('button', { name: 'Context boundary 1', exact: true }).waitFor();
  const boundary = page.locator('[data-render-unit-key="continue-history-1"] .conversation-history-boundary');
  await boundary.waitFor({ state: 'visible' });
  const disclosure = boundary.locator('details');
  if (await disclosure.getAttribute('open') !== null) throw new Error('Historical summary is not compact by default');
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--boundary-collapsed.png`, fullPage: true });
  await disclosure.locator('summary').click();
  await boundary.getByText('Preserved summary from member 1', { exact: true }).waitFor({ state: 'visible' });
  for (const value of ['history-1', 'history-2', 'continue-history-1']) await boundary.getByText(value, { exact: true }).waitFor();
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--boundary-expanded.png`, fullPage: true });
  await page.getByRole('button', { name: 'Context boundary 2', exact: true }).click();
  await page.waitForFunction(() => document.documentElement.dataset.sourceFixtureLocation?.endsWith('#message-continue-history-2'));
  if (await fixtureValue(page, 'older-snapshot-requests') !== '2') throw new Error('Loaded boundary requested a duplicate cursor page');
  await page.goBack();
  await page.waitForFunction(() => document.documentElement.dataset.sourceFixtureLocation?.endsWith('#message-continue-history-1'));
  await page.goBack();
  await page.waitForFunction(expected => document.documentElement.dataset.sourceFixtureLocation === expected, initialLocation);
  await page.goForward();
  await page.waitForFunction(() => document.documentElement.dataset.sourceFixtureLocation?.endsWith('#message-continue-history-1'));
  await page.goForward();
  await page.waitForFunction(() => document.documentElement.dataset.sourceFixtureLocation?.endsWith('#message-continue-history-2'));
  await page.getByRole('button', { name: /Member 2 answer/ }).click();
  const historicalSummary = page.locator('summary').filter({ hasText: /^Historical nonnested investigation$/ });
  await historicalSummary.waitFor({ state: 'visible' });
  const historicalDisclosure = historicalSummary.locator('..');
  if (await historicalDisclosure.getAttribute('open') !== null) throw new Error('Authored historical disclosure is not collapsed initially');
  await historicalSummary.click();
  await historicalDisclosure.locator('strong').filter({ hasText: /^historical prose marker$/ }).waitFor({ state: 'visible' });
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--authored-historical-disclosure.png`, fullPage: true });
  await page.getByRole('button', { name: /Member 1 answer/ }).click();
  const rejectedNested = page.locator('[data-render-unit-key="history-answer-1"]');
  await rejectedNested.getByText('<details><summary>Historical investigation</summary>', { exact: true }).waitFor({ state: 'visible' });
  await rejectedNested.locator('strong').filter({ hasText: /^nested prose marker$/ }).waitFor({ state: 'visible' });
  if (await rejectedNested.locator('.markdown-details').count()) throw new Error('Unsupported nested authored envelope was promoted');
  if (await fixtureValue(page, 'older-snapshot-requests') !== '2') throw new Error('Authored disclosure navigation reacquired loaded history');
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--nested-historical-fallback.png`, fullPage: true });
  journeys.push(`${id}/${viewport.name}: loaded historical prose uses one native nonnested Markdown disclosure; unsupported nested envelope remains visible literal syntax with ordinary Markdown body, independently of the literal persisted handoff summary and history-owned outer disclosure`);
  if (id.includes('global-lazy-history')) {
    await page.evaluate(() => { location.hash = '/global/history-3?source_transcript=history-1#message-history-answer-1'; });
    await page.waitForFunction(() => document.querySelector('.coordinator-conversation')?.hidden === false
      && document.documentElement.dataset.sourceFixtureLocation?.includes('source_transcript=history-1'));
    await page.getByRole('button', { name: /Member 2 request/ }).click();
    await page.waitForFunction(() => document.querySelector('.coordinator-conversation')?.hidden === false
      && document.documentElement.dataset.sourceFixtureLocation?.includes('source_transcript=history-2'));
    if (await fixtureValue(page, 'older-snapshot-requests') !== '2') throw new Error('Changing a validated Global source pin discarded acquired history');
    await page.getByRole('button', { name: /Member 1 request/ }).waitFor({ state: 'visible' });
    journeys.push(`${id}/${viewport.name}: changing validated Global member pins retains the acquired prefix and aggregate owner`);
  }
  const input = page.locator('.product-conversation-page__composer #message-input');
  await input.fill('Newest member remains writable after historical navigation');
  await input.press('Enter');
  await page.waitForFunction(() => document.documentElement.dataset.productConversationFixtureSendCount === '1');
  if (!(await fixtureValue(page, 'event-source-last-url'))?.includes('history-3')) throw new Error('Historical selection moved live/composer ownership');
  journeys.push(`${id}/${viewport.name}: unloaded exact boundary acquires two pages; collapsed/expanded original summary and provenance; loaded selection avoids duplicate read; browser Back/Forward crosses hashless entry using fixture HashRouter transport; newest member receives send`);
}

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
      visibleRealMessageRow: rows.some((row) => row.querySelector('.message.user, .message.agent, .message.meta') && intersects(box(row), viewportBox)),
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

async function assertAlignedPrefixTail(page) {
  const transcript = page.locator('.virtual-transcript');
  const terminal = page.getByText(/TRANSCRIPT_TAIL_TERMINAL_ASSISTANT_VISIBLE/);
  await terminal.waitFor({ state: 'visible' });
  const tailEvidence = await page.evaluate(() => {
    const scroller = document.querySelector('.virtual-transcript');
    const rows = [...document.querySelectorAll('[data-render-unit-key]')];
    const terminalRow = rows.find((row) => row.textContent?.includes('TRANSCRIPT_TAIL_TERMINAL_ASSISTANT_VISIBLE'));
    if (!(scroller instanceof HTMLElement) || !(terminalRow instanceof HTMLElement)) return null;
    const viewport = scroller.getBoundingClientRect();
    const marker = terminalRow.getBoundingClientRect();
    return {
      lastMountedKey: rows.at(-1)?.getAttribute('data-render-unit-key'),
      terminalKey: terminalRow.getAttribute('data-render-unit-key'),
      terminalVisible: marker.bottom > viewport.top && marker.top < viewport.bottom,
      distanceFromTail: scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight,
    };
  });
  if (!tailEvidence
    || tailEvidence.terminalKey !== 'aligned-prefix-terminal-assistant'
    || tailEvidence.lastMountedKey !== tailEvidence.terminalKey
    || !tailEvidence.terminalVisible
    || Math.abs(tailEvidence.distanceFromTail) > 1) {
    throw new Error(`terminal assistant is not the mounted physical tail: ${JSON.stringify(tailEvidence)}`);
  }

  const assertCompleted = async (name, tool) => {
    const text = await tool.textContent() ?? '';
    const completed = await tool.locator('.tool-block-status.success').count()
      || await tool.locator('.compact-tool-card-status').filter({ hasText: /done|success/i }).count();
    if (!completed || /waiting for tool result|result not received|running/i.test(text)) {
      throw new Error(`${name} source tool is not completed: ${text}`);
    }
  };
  const steering = page.locator('[data-tool-id="aligned-prefix-steering-tool"]');
  await steering.waitFor();
  await assertCompleted('steering', steering);

  await transcript.evaluate((element) => element.scrollTo({ top: 0 }));
  const boundary = page.locator('[data-tool-id="aligned-prefix-boundary-tool"]');
  await boundary.waitFor();
  await page.waitForFunction(() => (
    Number(document.documentElement.dataset.productConversationFixtureOlderSnapshotRequests ?? '0') === 1
      && document.documentElement.dataset.productConversationFixtureOlderSnapshotPending === 'true'
  ));
  const anchorBefore = await boundary.boundingBox();
  if (!anchorBefore) throw new Error('boundary reader anchor has no visible geometry');
  if (await page.locator('.virtual-transcript').getByText('OLDER_READER_ANCHOR_PRESERVED').count()) {
    throw new Error('older prefix rendered before the pre-restoration anchor measurement');
  }
  await page.evaluate(() => window.dispatchEvent(new Event('product-conversation-fixture-release-older')));
  await page.locator('.virtual-transcript').getByText('OLDER_READER_ANCHOR_PRESERVED').waitFor({ state: 'attached' });
  const anchorAfter = await boundary.boundingBox();
  if (!anchorAfter || Math.abs(anchorAfter.y - anchorBefore.y) > 1) {
    throw new Error(`reader prefix restoration moved the boundary anchor: ${JSON.stringify({ anchorBefore, anchorAfter })}`);
  }
  await assertCompleted('boundary', boundary);

  await transcript.evaluate((element) => element.scrollTo({ top: element.scrollHeight }));
  await terminal.waitFor({ state: 'visible' });
  journeys.push('aligned latest row: prefix owner and queued steering completed; terminal assistant is physical tail; reader prefix anchor preserved');
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

async function assertEmberTracking(page, id, viewport, outDir) {
  const scroller = page.locator('.virtual-transcript');
  const nav = page.locator('#conversation-nav');
  const cardLabel = 'Ember tracking · captured snapshot · 5';
  const duePills = nav.getByRole('button', { name: new RegExp(cardLabel) });
  await page.waitForFunction(() => document.querySelectorAll('#conversation-nav .conversation-nav-item').length >= 3);
  if (await duePills.count() !== (id === 'ember-tracking-fallback' ? 1 : 2)) throw new Error('Historical and current observations need truthful outline anchors');
  if (await nav.getByRole('button', { name: /Ember tracking · captured snapshot · 4/ }).count() !== (id === 'ember-tracking-fallback' ? 0 : 1)) {
    throw new Error('Null review cue has an incorrect outline label');
  }
  if (id === 'ember-tracking-fallback') {
    for (const target of ['tracking-spoof', 'tracking-malformed', 'tracking-unknown']) {
      const prefix = target === 'tracking-unknown' ? 'Unknown system observation:' : '[Saved Ember tracking observation:';
      const pill = nav.getByRole('button', { name: target === 'tracking-unknown' ? /Unknown system observation:/ : /Saved Ember tracking observation:/ });
      await pill.nth(target === 'tracking-malformed' ? 1 : 0).click();
      await page.waitForFunction(message => document.documentElement.dataset.sourceFixtureLocation?.endsWith(`#message-${message}`), target);
      const row = page.locator(`#message-${target}`);
      await row.locator('.message-content').waitFor({ state: 'visible' });
      if (await row.locator('.ember-tracking-card').count() || !await row.locator('.message-content').textContent().then(text => text?.startsWith(prefix))) {
        throw new Error(`${target} was promoted to a card or lost its exact original prefix`);
      }
      await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--${target}.png`, fullPage: true });
    }
    await duePills.first().click();
    await page.locator('#message-tracking-history-due .ember-tracking-card').waitFor({ state: 'visible' });
    if (await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)) throw new Error('Tracking lookalike caused horizontal overflow');
    await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--fallback.png`, fullPage: true });
    journeys.push(`${id}/${viewport.name}: user-origin exact payload spoof, malformed and unknown system-origin payloads remain original text; historical real observation is compact`);
    return true;
  }
  await duePills.last().click();
  await page.waitForFunction(() => document.documentElement.dataset.sourceFixtureLocation?.endsWith('#message-tracking-current-due'));
  const due = page.locator('#message-tracking-current-due');
  await due.locator('.ember-tracking-card').waitFor({ state: 'visible' });
  if (await due.locator('.ember-tracking-card details').first().getAttribute('open') !== null) throw new Error('Saved observation was not collapsed');
  const collapsedHeight = await due.locator('.ember-tracking-card').evaluate(element => element.getBoundingClientRect().height);
  if (collapsedHeight <= 0 || collapsedHeight > 240) throw new Error(`Collapsed observation is not compact: ${collapsedHeight}px`);
  trackingMeasurements.push({ id, viewport: viewport.name, collapsedTrackingCardHeight: collapsedHeight });
  const composer = page.locator('.product-conversation-page__composer #message-input');
  await composer.waitFor({ state: 'visible' });
  await composer.fill('Unsent tracking QA draft');
  await composer.focus();
  if (!await composer.evaluate(element => document.activeElement === element)) throw new Error('Composer focus was lost');
  if (await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)) throw new Error('Tracking presentation caused horizontal overflow');
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--collapsed.png`, fullPage: true });

  const details = due.locator('.ember-tracking-card > details');
  const summary = details.locator(':scope > summary');
  await summary.focus();
  await summary.press('Enter');
  await page.waitForFunction(() => document.querySelector('#message-tracking-current-due .ember-tracking-card > details')?.open === true);
  await details.getByText('3 entries omitted from captured page; more were available at capture.', { exact: true }).waitFor();
  await details.getByText('Review deployment evidence before release · requested work · tracking at capture · review flagged', { exact: true }).waitFor();
  const raw = details.locator(':scope > details');
  await raw.locator('summary').focus();
  await raw.locator('summary').press('Enter');
  await raw.locator('pre').waitFor({ state: 'visible' });
  const original = await raw.locator('pre').textContent();
  const expectedOriginal = await page.locator('[data-tracking-expected-payload]').getAttribute('data-tracking-expected-payload');
  if (!expectedOriginal || original !== expectedOriginal) {
    throw new Error('Raw original was not preserved byte-for-byte as a producer envelope and payload');
  }
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--raw.png`, fullPage: true });
  const copyButton = due.getByRole('button', { name: 'Copy system observation' });
  await copyButton.scrollIntoViewIfNeeded();
  await copyButton.focus();
  const beforeCopyScroll = await scroller.evaluate(element => element.scrollTop);
  await copyButton.click();
  await due.getByRole('button', { name: 'Copied!' }).waitFor();
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  if (copied !== original) throw new Error('Copy did not preserve the original producer payload');
  const afterCopyScroll = await scroller.evaluate(element => element.scrollTop);
  if (Math.abs(afterCopyScroll - beforeCopyScroll) > 3) throw new Error(`Copy jumped the transcript scroller: ${beforeCopyScroll} -> ${afterCopyScroll}`);
  trackingMeasurements.push({ id, viewport: viewport.name, beforeCopyScroll, afterCopyScroll });
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--expanded.png`, fullPage: true });
  await summary.focus();
  await summary.press('Enter');
  await page.waitForFunction(() => document.querySelector('#message-tracking-current-due .ember-tracking-card > details')?.open === false);

  await nav.getByRole('button', { name: /Ember tracking · captured snapshot · 4/ }).click();
  await page.locator('#message-tracking-current-null .ember-tracking-card').waitFor({ state: 'visible' });
  if (await page.locator('#message-tracking-current-null .ember-tracking-topic').count()) throw new Error('Null cue incorrectly advertises a review topic');
  await duePills.first().click();
  await page.waitForFunction(() => document.documentElement.dataset.sourceFixtureLocation?.endsWith('#message-tracking-history-due'));
  await page.locator('#message-tracking-history-due .ember-tracking-card').waitFor({ state: 'visible' });
  if (await composer.inputValue() !== 'Unsent tracking QA draft' || !await composer.isEnabled()) throw new Error('Historical selection changed latest composer ownership');
  const historicalLocation = await page.locator('html').getAttribute('data-source-fixture-location');
  if (!historicalLocation?.includes('#message-tracking-history-due')) throw new Error('Historical selection did not retain exact message target');
  await page.reload({ waitUntil: 'networkidle' });
  await page.waitForSelector(`[data-product-conversation-fixture-ready="${id}"]`);
  await page.waitForFunction((expected) => document.documentElement.dataset.sourceFixtureLocation === expected, historicalLocation);
  await page.locator('#message-tracking-history-due .ember-tracking-card').waitFor({ state: 'visible' });
  await page.screenshot({ path: `${outDir}/${id}--${viewport.name}--history-reload.png`, fullPage: true });
  journeys.push(`${id}/${viewport.name}: continued historical and current observations, compact outline/anchors, keyboard disclosure/raw original, exact copy, null cue, historical reload and latest composer ownership; virtual transcript scroller stays stable during copy`);
  return true;
}

async function writeReport(outDir) {
  const report = [
    '# ProductConversation deterministic browser UAT', '',
    'Event-driven only: fixture-ready markers, locator state, and recorded fixture API calls. No journey sleeps.', '',
    '## Core geometry/theme matrix', '',
    '| Scenario | Viewport | Theme | transcript h | main h | chat h | virtual h | positive rows | overflow |',
    '| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |',
    ...measurements.map((item) => `| ${item.id} | ${item.viewport} | ${item.theme} | ${item.transcript.height} | ${item.mainArea.height} | ${item.chatView.height} | ${item.virtualTranscript.height} | ${item.positiveRows} | ${item.horizontalOverflow ? 'yes' : 'no'} |`),
    '', '## Tracking measurements', ...trackingMeasurements.map(item => `- ${JSON.stringify(item)}`),
    '', '## Deterministic interaction coverage', ...journeys.map((journey) => `- ${journey}`), '',
    'Screenshots and journeys cover only the scenarios selected for this run. Geometry, viewer, and Recall coverage is present only where recorded above. Lazy-history fixtures use HashRouter so browser Back/Forward exercises actual browser history without a Phoenix server; this is not production-path cold-reload or physical-device acceptance. Emulated zero-inset viewports cannot prove physical notch geometry. Shared runner fails on unexpected console, page, request, and >=400 response failures.',
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
    { name: 'mobile-light', width: 390, height: 844, isMobile: true, hasTouch: true }, { name: 'mobile-dark', width: 390, height: 844, isMobile: true, hasTouch: true },
    { name: 'tracking-393-dark', width: 393, height: 852, isMobile: true, hasTouch: true },
  ],
  urlForStory: ({ storyKey, id, viewport }) => {
    const theme = viewport.name.endsWith('-light') ? 'light' : 'dark';
    const fixtureHash = id === 'long-history-110-messages' ? '#message-long-older-target' : '';
    const baseUrl = process.env.LADLE_URL ?? `http://127.0.0.1:${process.env.LADLE_PORT ?? 61123}/`;
    return buildLadleStoryUrl(baseUrl, storyKey, { fixtureTheme: theme, fixtureHash,
      ...(['ordinary-lazy-history', 'global-lazy-history', 'ember-tracking-observations', 'ember-tracking-fallback'].includes(id) ? { fixtureBrowserHistory: '1' } : {}),
    });
  },
  preparePage: async ({ page }) => {
    await page.route('**/api/**', route => route.abort('blockedbyclient'));
  },
  captureStory: async ({ page, id, outDir, viewport }) => {
    if (id === 'ember-tracking-observations' || id === 'ember-tracking-fallback') {
      const theme = await fixtureValue(page, 'theme') ?? 'dark';
      await assertTranscriptGeometry(page, id, viewport, theme);
      return assertEmberTracking(page, id, viewport, outDir);
    }
    if (id === 'composer-focus') {
      await captureComposerFocus({ page, outDir, viewport });
      return false;
    }
    if (id === 'mobile-continuation-ordinary' || id === 'mobile-continuation-global') {
      await assertMobileContinuation(page, id, viewport, outDir);
      return;
    }
    const theme = await fixtureValue(page, 'theme') ?? 'dark';
    if (id === 'ordinary-lazy-history' || id === 'global-lazy-history') {
      await assertSupersededBoundary(page, id, viewport, outDir);
      await assertLazyHistory(page, id, viewport, outDir);
      return false;
    }
    if (id === 'source-call-global' || id === 'source-call-ordinary') { await assertSourceCall(page, id, viewport, outDir); return false; }
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
    if (id === 'latest-row-aligned-prefix-tail' && viewport.name === 'mobile-dark') {
      await assertAlignedPrefixTail(page);
    }
    if (id === 'long-history-110-messages') {
      if (viewport.name === 'mobile-dark') await assertLongHistory(page);
      if (viewport.name === 'desktop-dark') await assertDeepLink(page);
    }
    return false;
  },
  onComplete: writeReport,
});
