// Uses an already-running Ladle. Never starts a server or installs a browser.
// cd ui && LADLE_URL=http://127.0.0.1:<port> node scripts/capture-assistant-finalization.mjs
import { chromium, webkit } from 'playwright';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';
import { isDeepStrictEqual } from 'node:util';

// Run against prepatch production components to require a reproduced geometry mismatch.
const baseline = process.env.FINALIZATION_QA_BASELINE === '1';

assert.ok(process.env.LADLE_URL, 'Set LADLE_URL to an already-running Ladle instance');
const outDir = path.resolve(process.env.FINALIZATION_QA_OUT_DIR ?? `qa-artifacts/assistant-finalization${baseline ? '-baseline' : ''}`);
await mkdir(outDir, { recursive: true });
const engine = process.env.FINALIZATION_QA_BROWSER ?? 'chromium';
assert.ok(['chromium', 'webkit'].includes(engine), 'FINALIZATION_QA_BROWSER must be chromium or webkit');
const browser = await (engine === 'webkit' ? webkit : chromium).launch();
const samples = [];

function measure() {
  const rect = (node, origin) => {
    if (!node) return null;
    const box = node.getBoundingClientRect();
    return { left: box.left - origin.left, top: box.top - origin.top, width: box.width, height: box.height };
  };
  const visibility = node => {
    const style = getComputedStyle(node);
    const ancestors = [];
    for (let parent = node.parentElement; parent; parent = parent.parentElement) {
      const computed = getComputedStyle(parent);
      if (computed.display === 'none' || Number(computed.opacity) === 0) {
        ancestors.push({ className: parent.className, display: computed.display, opacity: computed.opacity });
      }
    }
    const box = node.getBoundingClientRect();
    return {
      display: style.display, visibility: style.visibility, opacity: style.opacity,
      pointerEvents: style.pointerEvents, disabled: node.disabled, hiddenAncestors: ancestors,
      visible: box.width > 0 && box.height > 0 && style.display !== 'none'
        && style.visibility === 'visible' && Number(style.opacity) > 0 && ancestors.length === 0,
    };
  };
  const user = document.querySelector('.message.user');
  const userBox = user.getBoundingClientRect();
  return {
    user: { width: userBox.width, height: userBox.height },
    media: {
      hoverNone: matchMedia('(hover: none)').matches,
      hoverHover: matchMedia('(hover: hover)').matches,
      anyPointerCoarse: matchMedia('(any-pointer: coarse)').matches,
      narrowViewport: matchMedia('(width <= 768px)').matches,
    },
    columnWidth: document.querySelector('.assistant-finalization-fixture').getBoundingClientRect().width,
    documentWidth: document.documentElement.clientWidth,
    documentScrollWidth: document.documentElement.scrollWidth,
    cases: Array.from(document.querySelectorAll('[data-geometry-case]'), section => {
      const message = section.querySelector('.message.agent');
      const content = message.querySelector('.message-content');
      const origin = section.getBoundingClientRect();
      const proseOrigin = content.getBoundingClientRect();
      const paragraphs = Array.from(content.querySelectorAll('p'), paragraph => {
        const range = document.createRange();
        range.selectNodeContents(paragraph);
        return Array.from(range.getClientRects(), box => ({
          left: box.left - proseOrigin.left, top: box.top - proseOrigin.top, width: box.width, height: box.height,
        }));
      });
      return {
        id: section.dataset.geometryCase,
        geometry: {
          message: rect(message, origin), content: rect(content, origin),
          toolbar: rect(message.querySelector('.assistant-message-toolbar')
            ?? message.querySelector('.message-header') ?? message.querySelector('.message-mobile-copy-row'), origin),
          downstream: rect(section.querySelector('[data-geometry-downstream]'), origin), paragraphs,
        },
        toolbarSelector: message.querySelector('.assistant-message-toolbar') ? '.assistant-message-toolbar'
          : message.querySelector('.message-header') ? '.message-header'
            : message.querySelector('.message-mobile-copy-row') ? '.message-mobile-copy-row' : null,
        actions: Array.from(message.querySelectorAll('.assistant-message-actions button, .message-header-actions button, .message-mobile-copy-row button'), button => ({
          name: button.getAttribute('aria-label'), ...rect(button, origin), ...visibility(button),
        })),
      };
    }),
  };
}

try {
  for (const density of ['full', 'compact']) {
    for (const viewport of [
      { width: 375, height: 900 }, { width: 390, height: 900 },
      { width: 768, height: 900 }, { width: 769, height: 900 },
      { width: 1280, height: 900 }, { width: 1280, height: 900, hasTouch: true },
      { width: 1280, height: 900, column: 'narrow' },
    ]) {
      const name = `${engine}-${density}-${viewport.width}${viewport.hasTouch ? '-touch' : ''}${viewport.column ? '-narrow-column' : ''}`;
      const context = await browser.newContext({ viewport: { width: viewport.width, height: viewport.height }, hasTouch: viewport.hasTouch ?? false });
      try {
      const page = await context.newPage();
      const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      const url = new URL(process.env.LADLE_URL);
      url.searchParams.set('story', `assistant-finalization--${density}`);
      url.searchParams.set('mode', 'preview');
      if (viewport.column) url.searchParams.set('column', viewport.column);
      await page.goto(url.href);
      await page.waitForSelector('[data-finalization-ready="true"]');
      await page.waitForFunction(() => Array.from(document.querySelectorAll('[data-geometry-case] .message-content')).length === 8
        && Array.from(document.querySelectorAll('[data-geometry-case] .message-content')).every(node => node.querySelectorAll('p').length === 2));
      await page.evaluate(() => document.fonts.ready);
      const before = await page.evaluate(measure);
      if (viewport.width === 375) await page.screenshot({ path: path.join(outDir, `${name}-streaming.png`), fullPage: true });
      await page.getByTestId('finalize-assistant').click();
      await page.waitForSelector('[data-finalized="true"]');
      const after = await page.evaluate(measure);
      const geometryMismatches = after.cases.flatMap((result, index) => {
        const fields = Object.keys(result.geometry).filter(key =>
          !isDeepStrictEqual(result.geometry[key], before.cases[index].geometry[key]));
        return fields.length ? [{ id: result.id, fields }] : [];
      });
      samples.push({ name, baseline, viewport, before, after, geometryMismatches, errors });
      await writeFile(path.join(outDir, 'geometry.json'), JSON.stringify(samples, null, 2));
      if (viewport.width === 375) await page.screenshot({ path: path.join(outDir, `${name}-finalized.png`), fullPage: true });
      assert.deepEqual(errors, [], `${name}: browser errors`);
      if (baseline) {
        console.log(`BASELINE ${name}: geometry mismatch in ${geometryMismatches.length}/8 cases (not a fixed-behavior pass)`);
        continue;
      }
      assert.deepEqual(after.user, before.user, `${name}: user message geometry changed`);
      if (viewport.column === 'narrow') {
        assert.ok(after.columnWidth <= 390 && after.documentWidth > 768, `${name}: narrow column was not applied at desktop viewport width`);
      }
      assert.equal(after.documentWidth, after.documentScrollWidth, `${name}: horizontal document overflow`);
      for (const [index, result] of after.cases.entries()) {
        assert.deepEqual(result.geometry, before.cases[index].geometry, `${name}/${result.id}: finalization changed geometry`);
        const first = result.id.startsWith('first-');
        const mobileActions = after.media.narrowViewport || after.media.hoverNone;
        const suppressed = result.id.endsWith('-suppressed');
        const expectedNames = suppressed ? [] : result.id.endsWith('-review')
          ? ['Open message reviewer', 'Copy Phoenix message'] : ['Copy Phoenix message'];
        assert.deepEqual(result.actions.map(action => action.name), expectedNames, `${name}/${result.id}: action suppression changed`);
        for (const action of result.actions) {
          const expectedVisible = action.name === 'Copy Phoenix message' ? mobileActions : first || mobileActions;
          assert.equal(action.visible, expectedVisible, `${name}/${result.id}/${action.name}: computed visibility violates existing display policy`);
          if (action.visible) {
            assert.equal(action.disabled, false, `${name}/${result.id}: visible action disabled`);
            assert.notEqual(action.pointerEvents, 'none', `${name}/${result.id}: visible action ignores pointer events`);
          }
        }
        if (mobileActions) {
          for (const action of result.actions.filter(action => action.visible)) {
            assert.ok(action.width >= 44 && action.height >= 44, `${name}/${result.id}: action target smaller than 44px`);
            assert.ok(action.top + action.height <= result.geometry.content.top, `${name}/${result.id}: action overlaps prose`);
          }
        }
      }
      console.log(`PASS ${name}: eight streaming/finalized cases, stable line boxes and downstream rows`);
      } finally {
        await context.close();
      }
    }
  }
  if (baseline) {
    assert.ok(samples.some(sample => sample.geometryMismatches.some(mismatch =>
      mismatch.fields.some(field => field !== 'toolbar'))),
    'Baseline did not reproduce message/content/line-box/downstream movement; toolbar DOM differences alone are not regression evidence');
    console.log('BASELINE reproduced geometry mismatch; see geometry.json for per-case evidence');
  }
} finally {
  await browser.close();
}
