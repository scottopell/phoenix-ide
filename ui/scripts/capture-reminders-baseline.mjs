import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runSurfaceCapture } from './capture-ladle-surface.mjs';

process.env.CAPTURE_STORIES = 'initial-failure';

runSurfaceCapture({
  surface: 'product-conversation-reminders',
  storyPrefix: 'product-conversation--reminders-',
  readyAttribute: 'data-reminders-fixture-ready',
  outDir: process.env.REMINDERS_BASELINE_QA_OUT ?? 'qa-artifacts/reminders-baseline',
  viewportMatrix: [
    { name: 'mobile-375x667', width: 375, height: 667, isMobile: true, hasTouch: true },
    { name: 'mobile-393x852', width: 393, height: 852, isMobile: true, hasTouch: true },
    { name: 'desktop', width: 1440, height: 900 },
  ],
  expectedConsoleErrors: new Map([['initial-failure', ['Network response 503']]]),
  captureStory: async ({ page, id, outDir, viewport }) => {
    await page.getByText('Failed to load reminders').waitFor({ state: 'visible' });
    const evidence = await page.evaluate(() => {
      const geometry = Object.fromEntries(
        ['.reminders-panel', '#input-area', '.product-conversation-page__composer', '.product-conversation-page__transcript'].map((selector) => {
          const element = document.querySelector(selector);
          const bounds = element?.getBoundingClientRect();
          return [selector, bounds ? { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height, bottom: bounds.bottom, border: getComputedStyle(element).borderTopWidth } : null];
        }),
      );
      return { geometry, document: { width: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth } };
    });
    assert.ok(evidence.geometry['.reminders-panel']?.height > 0, 'baseline must render the failure panel');
    assert.notEqual(evidence.geometry['.reminders-panel']?.border, '0px', 'baseline must have a border');
    const composer = evidence.geometry['#input-area'];
    assert.ok(composer?.height > 0 && composer.y >= 0 && composer.bottom <= viewport.height, 'baseline composer must remain in viewport');
    assert.ok(evidence.document.scrollWidth <= evidence.document.width + 1, 'baseline must not overflow horizontally');
    await mkdir(outDir, { recursive: true });
    const stem = `${id}--${viewport.name}--initial-failure`;
    await page.screenshot({ path: path.join(outDir, `${stem}.png`), fullPage: false });
    await writeFile(path.join(outDir, `${stem}.json`), JSON.stringify({ id, viewport, ...evidence }, null, 2));
    return true;
  },
});
