import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { chromium } from 'playwright';

// Hosted-only: no server startup, browser install, build, or production API.
// LADLE_URL=https://host/ BASELINE=1 node ui/scripts/capture-review-reading-position.mjs
const base = process.env.LADLE_URL;
assert(base, 'LADLE_URL is required (serve the built fixtures on a hosted runner)');
assert(['http:', 'https:'].includes(new URL(base).protocol), 'LADLE_URL must be HTTP(S)');
const baseline = process.env.BASELINE === '1';
const out = path.resolve(process.env.REVIEW_READING_QA_OUT ?? 'qa-artifacts/review-reading-position');
await mkdir(out, { recursive: true });
const report = {
  baseline, base, cases: [], mismatches: [], errors: [], controls: [],
  limitations: [
    'HashRouter transports the product path/search/hash inside the hosted Ladle URL; not production-path cold reload acceptance.',
    'ViewerShell has no outside-click dismissal handler; outside dismissal is not a supported journey.',
    'Typed Proposal routing is covered by parent unit tests, not this capture.',
    'Opening a split pane changes transcript width; open-stage geometry is recorded, while exact reader geometry is compared after exit at the original width.',
  ],
};
const save = () => writeFile(path.join(out, 'report.json'), JSON.stringify(report, null, 2));
const watchdog = setTimeout(async () => {
  report.errors.push('Seven-minute total budget exceeded');
  await save();
  process.exit(1);
}, 415_000);
const browser = await chromium.launch({ headless: true, timeout: 15000 }).catch(async error => {
  clearTimeout(watchdog);
  report.errors.push(`Browser launch: ${error}`);
  await save();
  throw error;
});
const fixtures = [
  { kind: 'ordinary', story: 'review-reading-position--ordinary-long-history', ready: 'ordinary-lazy-history',
    entry: '/product-conversations/fixture-product-conversation?source_transcript=history-3#message-history-answer-3' },
  { kind: 'global', story: 'review-reading-position--global-long-history', ready: 'global-lazy-history',
    entry: '/global/history-3?source_transcript=history-3#message-history-answer-3' },
];
const cases = [];
for (const fixture of fixtures) for (const width of [390, 1100, 1440]) for (const position of ['middle', 'bottom']) {
  cases.push({ fixture, width, position, exit: 'close' });
  cases.push({ fixture, width, position, exit: (width === 1100) === (position === 'middle') ? 'send' : 'discard' });
}
for (const fixture of fixtures) {
  for (const width of [390, 1100]) cases.push({ fixture, width, position: 'middle', exit: 'escape' });
  cases.push({ fixture, width: 1440, position: 'middle', exit: 'fullscreen-return' });
}
assert.equal(cases.length, 30);

// Wait for geometry to remain stable over animation frames, with enough frames
// to include the short CSS transitions. No wall-clock journey sleeps.
async function settle(page) {
  await page.evaluate(() => new Promise((resolve, reject) => {
    let last = ''; let stable = 0; let frames = 0;
    function tick() {
      const el = document.querySelector('#messages');
      const rect = el?.getBoundingClientRect();
      const value = JSON.stringify([el?.scrollTop, el?.scrollHeight, rect?.width, rect?.height]);
      stable = value === last ? stable + 1 : 0; last = value;
      if (++frames >= 20 && stable >= 8) resolve();
      else if (frames > 180) reject(new Error('Transcript geometry did not settle'));
      else requestAnimationFrame(tick);
    }
    requestAnimationFrame(tick);
  }));
}

async function measure(page, initialize = null) {
  return page.evaluate(initialize => {
    const el = document.querySelector('#messages');
    if (!el) throw new Error('Missing #messages');
    const rect = el.getBoundingClientRect();
    if (initialize) {
      const selectedRow = [...el.querySelectorAll('[data-render-unit-key]')].find(row => row.dataset.renderUnitKey === initialize);
      if (!selectedRow) throw new Error('Selected Review row is not mounted');
      const candidates = [...selectedRow.querySelectorAll('p')].filter(p => {
        const r = p.getBoundingClientRect();
        return r.top >= rect.top && r.bottom <= rect.bottom && r.height > 0;
      });
      const p = candidates[0];
      if (!p) throw new Error('No fully visible paragraph anchor');
      const row = p.closest('[data-render-unit-key]');
      window.__reviewReader = {
        node: el, anchorKey: row.dataset.renderUnitKey,
        paragraphIndex: [...row.querySelectorAll('p')].indexOf(p), text: p.textContent,
      };
    }
    const saved = window.__reviewReader;
    const row = [...el.querySelectorAll('[data-render-unit-key]')].find(r => r.dataset.renderUnitKey === saved.anchorKey);
    const p = row?.querySelectorAll('p')[saved.paragraphIndex];
    const ancestors = [];
    for (let parent = el.parentElement; parent; parent = parent.parentElement) {
      ancestors.push({ tag: parent.tagName, id: parent.id, className: parent.className,
        top: parent.scrollTop, left: parent.scrollLeft });
    }
    const location = document.documentElement.dataset.sourceFixtureLocation;
    const route = new URL(location, 'https://fixture.invalid');
    return {
      sameNode: saved.node === el, originalNodeConnected: saved.node.isConnected,
      scrollTop: el.scrollTop, scrollHeight: el.scrollHeight, clientHeight: el.clientHeight,
      width: rect.width, height: rect.height, top: rect.top,
      anchorKey: saved.anchorKey, paragraphIndex: saved.paragraphIndex, anchorText: p?.textContent ?? null,
      anchorOffset: p ? p.getBoundingClientRect().top - rect.top : null,
      anchorViewportTop: p?.getBoundingClientRect().top ?? null,
      location, route: { pathname: route.pathname, search: route.search, hash: route.hash },
      browser: { pathname: window.location.pathname, search: window.location.search, hash: window.location.hash },
      viewport: { width: innerWidth, height: innerHeight, x: scrollX, y: scrollY }, ancestors,
      sends: Number(document.documentElement.dataset.productConversationFixtureSendCount ?? 0),
    };
  }, initialize);
}
function mismatch(id, stage, reason, before, after) {
  report.mismatches.push({ id, stage, reason, before, after });
}
function compare(id, stage, before, after, geometry) {
  if (!after.sameNode || !after.originalNodeConnected) mismatch(id, stage, 'messages node replaced', before.sameNode, after.sameNode);
  const readerUrl = value => {
    const url = new URL(typeof value === 'string' ? value : `${value.pathname}${value.search}${value.hash}`, 'http://fixture.invalid');
    if (stage !== 'exit') for (const key of ['viewer', 'presentation', 'target', 'file', 'root', 'line', 'endLine', 'handle', 'message', 'review', 'message_id', 'occurrence']) url.searchParams.delete(key);
    return `${url.pathname}${url.search}${url.hash}`;
  };
  for (const field of ['location', 'browser']) if (readerUrl(before[field]) !== readerUrl(after[field])) {
    mismatch(id, stage, `${field} reader navigation changed`, before[field], after[field]);
  }
  if (after.sends !== before.sends) mismatch(id, stage, 'Review sent a network/API message', before.sends, after.sends);
  if (!geometry) return;
  for (const field of ['scrollTop', 'anchorOffset', 'anchorViewportTop']) {
    if (after[field] === null || Math.abs(before[field] - after[field]) > 2) mismatch(id, stage, `${field} jumped`, before[field], after[field]);
  }
  if (before.anchorText !== after.anchorText) mismatch(id, stage, 'paragraph anchor lost', before.anchorText, after.anchorText);
  if (before.viewport.x !== after.viewport.x || before.viewport.y !== after.viewport.y) mismatch(id, stage, 'viewport scrolled', before.viewport, after.viewport);
  const scrolls = value => value.ancestors.map(a => [a.tag, a.id, a.top, a.left]);
  if (JSON.stringify(scrolls(before)) !== JSON.stringify(scrolls(after))) mismatch(id, stage, 'ancestor scroll changed', scrolls(before), scrolls(after));
}

try {
  for (const test of cases) {
    const { fixture, width, position, exit } = test;
    const id = `${fixture.kind}-${width}-${position}-${exit}`;
    const result = { id, stages: {} };
    report.cases.push(result);
    const context = await browser.newContext({ viewport: { width, height: width === 390 ? 844 : 900 } });
    const page = await context.newPage();
    page.setDefaultTimeout(6500);
    page.setDefaultNavigationTimeout(15000);
    page.on('pageerror', error => report.errors.push(`${id}: ${error.message}`));
    page.on('console', message => { if (message.type() === 'error') report.errors.push(`${id}: console: ${message.text()}`); });
    await page.route('**/api/**', route => {
      report.errors.push(`${id}: unexpected network API ${route.request().url()}`);
      return route.abort('blockedbyclient');
    });
    try {
      const url = new URL(base);
      url.search = ''; url.hash = '';
      for (const [key, value] of Object.entries({ story: fixture.story, mode: 'preview', fixtureTheme: 'dark', fixtureBrowserHistory: '1', fixtureEntry: fixture.entry })) url.searchParams.set(key, value);
      await page.goto(url.href);
      await page.locator(`main[data-product-conversation-fixture-ready="${fixture.ready}"]`).waitFor();
      await page.locator('#messages').waitFor();
      await page.waitForFunction(entry => document.documentElement.dataset.sourceFixtureLocation === entry, fixture.entry);
      await settle(page);
      const scroller = page.locator('#messages');
      await scroller.evaluate((el, position) => {
        if (el.scrollHeight < el.clientHeight * 3) throw new Error('Fixture is not long history');
        el.scrollTop = position === 'bottom' ? el.scrollHeight : (el.scrollHeight - el.clientHeight) * 0.52;
      }, position);
      await settle(page);
      // Middle setup explicitly aligns a mounted Review header, then freezes the
      // baseline. This is header-adjacent reading, not arbitrary paragraph reading.
      if (position === 'middle') {
        await scroller.evaluate(el => {
          const box = el.getBoundingClientRect();
          const buttons = [...el.querySelectorAll('button[aria-label="Open message reviewer"]')];
          const button = buttons.sort((a, b) => Math.abs(a.getBoundingClientRect().top - box.top - 64)
            - Math.abs(b.getBoundingClientRect().top - box.top - 64))[0];
          if (!button) throw new Error('No mounted Review header near middle');
          el.scrollTop += button.getBoundingClientRect().top - box.top - 64;
        });
        await settle(page);
      }
      // Bottom never aligns a header: the short final answer must be visible at
      // the physical tail. Missing controls fail instead of scrolling the reader.
      const key = await page.evaluate(position => {
        const root = document.querySelector('#messages'); const box = root.getBoundingClientRect();
        const button = [...root.querySelectorAll('button[aria-label="Open message reviewer"]')].find(b => {
          const r = b.getBoundingClientRect();
          const row = b.closest('[data-render-unit-key]');
          return (position !== 'bottom' || row.dataset.renderUnitKey === 'review-tail-answer')
            && r.width > 0 && r.height > 0 && r.top >= Math.max(box.top, 0) + 2
            && r.bottom <= Math.min(box.bottom, innerHeight) - 2
            && r.left >= Math.max(box.left, 0) && r.right <= Math.min(box.right, innerWidth);
        });
        if (!button) throw new Error('No native Review button inside reader viewport');
        return button.closest('[data-render-unit-key]').dataset.renderUnitKey;
      }, position);
      result.setup = { position, reviewRow: key, alignment: position === 'middle' ? 'Review header near top, then frozen' : 'physical tail, no header alignment' };
      const before = result.stages.before = await measure(page, key);
      assert.equal(before.location, fixture.entry, 'Setup must preserve the exact source pin/hash');
      const tailDistance = before.scrollHeight - before.clientHeight - before.scrollTop;
      if (position === 'bottom') assert(Math.abs(tailDistance) <= 2, 'Bottom case must start at the physical tail');
      else assert(before.scrollTop > before.clientHeight && tailDistance > before.clientHeight, 'Middle case must be away from either edge');
      await page.screenshot({ path: path.join(out, `${id}-before.png`) });
      const clickPoint = await page.locator(`[data-render-unit-key=${JSON.stringify(key)}]`).getByRole('button', { name: 'Open message reviewer', exact: true }).evaluate(button => {
        const r = button.getBoundingClientRect();
        const box = document.querySelector('#messages').getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0 || r.top < Math.max(box.top, 0) || r.bottom > Math.min(box.bottom, innerHeight)
          || r.left < Math.max(box.left, 0) || r.right > Math.min(box.right, innerWidth)) throw new Error('Review button left the frozen viewport');
        const x = r.left + r.width / 2; const y = r.top + r.height / 2;
        if (!button.contains(document.elementFromPoint(x, y))) throw new Error('Review button is obscured');
        return { x, y };
      });
      // Real pointer input without Locator.click's automatic scrollIntoView.
      await page.mouse.click(clickPoint.x, clickPoint.y);
      const viewer = page.locator('.viewer-shell').filter({ has: page.locator('.viewer-shell-title') });
      await viewer.waitFor({ state: 'visible' });
      await settle(page);
      const opened = result.stages.open = await measure(page);
      compare(id, 'open', before, opened, false);
      await page.screenshot({ path: path.join(out, `${id}-open.png`) });
      if (exit === 'discard' || exit === 'send') {
        await viewer.getByRole('button', { name: 'Add note to line 1', exact: true }).click({ force: true });
        const note = `reading-position-note-${id}`;
        await page.locator('.annotation-dialog-input').fill(note);
        await page.getByRole('button', { name: 'Add Note', exact: true }).click();
        if (exit === 'send') {
          await viewer.getByRole('button', { name: 'Send notes', exact: true }).click();
          const input = page.locator('.product-conversation-page__composer #message-input');
          await page.waitForFunction(text => document.querySelector('.product-conversation-page__composer #message-input')?.value.includes(text), note);
          result.draft = await input.inputValue();
        } else {
          await viewer.getByRole('button', { name: 'Close viewer', exact: true }).click();
          await page.getByRole('button', { name: 'Discard notes and close', exact: true }).click();
        }
      } else if (exit === 'escape') {
        await page.keyboard.press('Escape');
      } else if (exit === 'fullscreen-return') {
        await viewer.getByRole('button', { name: 'Fullscreen', exact: true }).click();
        await page.locator('.viewer-shell--takeover').waitFor();
        await settle(page);
        result.stages.fullscreen = await measure(page);
        compare(id, 'fullscreen', before, result.stages.fullscreen, false);
        await viewer.getByRole('button', { name: 'Return to pane', exact: true }).click();
        await page.locator('.viewer-shell--inline').waitFor();
        await settle(page);
        result.stages.returned = await measure(page);
        compare(id, 'returned', before, result.stages.returned, false);
        await viewer.getByRole('button', { name: 'Close viewer', exact: true }).click();
      } else {
        await viewer.getByRole('button', { name: 'Close viewer', exact: true }).click();
      }
      await viewer.waitFor({ state: 'hidden' });
      await settle(page);
      const after = result.stages.exit = await measure(page);
      compare(id, 'exit', before, after, true);
      await page.screenshot({ path: path.join(out, `${id}-exit.png`) });
      if (fixture.kind === 'global' && width === 1440 && exit === 'close' && position === 'middle') {
        const locationBefore = after.location;
        await page.getByRole('button', { name: /Member 3 request$/ }).click();
        await page.waitForFunction(() => document.documentElement.dataset.sourceFixtureLocation?.endsWith('#message-history-prompt-3'));
        const locationAfter = await page.locator('html').getAttribute('data-source-fixture-location');
        assert.notEqual(locationAfter, locationBefore, 'True chapter navigation must change the anchor');
        await page.waitForFunction(() => {
          const root = document.querySelector('#messages');
          const row = root.querySelector('[data-render-unit-key="history-prompt-3"]');
          if (!row) return false;
          const r = row.getBoundingClientRect(); const b = root.getBoundingClientRect();
          return r.bottom > b.top && r.top < b.bottom;
        });
        report.controls.push({ id, locationBefore, locationAfter, targetVisible: true });
      }
    } catch (error) {
      result.error = String(error);
      report.errors.push(`${id}: ${error}`);
      await page.screenshot({ path: path.join(out, `${id}-error.png`) }).catch(() => {});
    } finally {
      await context.close();
      await save();
    }
  }
  assert.equal(report.errors.length, 0, `Harness/interaction errors: ${report.errors.join('\n')}`);
  assert.equal(report.controls.length, 1, 'Missing true chapter navigation control');
  if (baseline) assert(report.mismatches.length > 0, 'BASELINE must reproduce at least one position/route/node mismatch');
  else assert.equal(report.mismatches.length, 0, `Reading-position regressions: ${report.mismatches.length}; see ${out}/report.json`);
  console.log(`${baseline ? 'BASELINE reproduced' : 'PASS'}: ${report.cases.length} cases, ${report.mismatches.length} mismatches; ${out}`);
} finally {
  clearTimeout(watchdog);
  await save();
  await browser.close();
}
