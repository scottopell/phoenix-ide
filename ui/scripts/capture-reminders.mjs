import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runSurfaceCapture } from './capture-ladle-surface.mjs';

const storyIds = [
  'initial-failure',
  'loaded-stale-upcoming',
  'retry-mutation-rejection',
];
const viewports = [
  { name: 'mobile-375x667', width: 375, height: 667, isMobile: true, hasTouch: true },
  { name: 'mobile-393x852', width: 393, height: 852, isMobile: true, hasTouch: true },
  { name: 'desktop', width: 1440, height: 900 },
];

runSurfaceCapture({
  surface: 'product-conversation-reminders',
  storyPrefix: 'product-conversation--reminders-',
  readyAttribute: 'data-reminders-fixture-ready',
  outDir: process.env.REMINDERS_QA_OUT ?? 'qa-artifacts/reminders',
  viewportMatrix: viewports,
  expectedConsoleErrors: new Map(storyIds.map((id) => [id, ['Network response 503', 'Network response 409']])),
  captureStory: async ({ page, id, outDir, viewport }) => {
    assert.ok(storyIds.includes(id), `Unexpected Reminders story: ${id}`);
    const journey = [];
    const retry = page.getByRole('button', { name: 'Retry reminders' });
    if (id === storyIds[0]) {
      await retry.waitFor({ state: 'visible' });
      assert.equal(await page.getByRole('region', { name: 'Reminders' }).count(), 0, 'initial GET failure must not produce a bordered panel');
      assert.ok(await page.locator('.reminders-fetch-status').evaluate((node) => node.getBoundingClientRect().height < 60), 'initial fetch failure must stay compact');
      const initialFailure = await page.evaluate(() => {
        const selectors = ['.reminders-fetch-status', '#input-area', '.product-conversation-page__composer', '.product-conversation-page__transcript'];
        const geometry = Object.fromEntries(selectors.map((selector) => {
          const rect = document.querySelector(selector)?.getBoundingClientRect();
          return [selector, rect ? { x: rect.x, y: rect.y, width: rect.width, height: rect.height, bottom: rect.bottom } : null];
        }));
        return { geometry, document: { width: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth } };
      });
      const composer = initialFailure.geometry['#input-area'];
      assert.ok(composer?.height > 0 && composer.y >= 0 && composer.bottom <= viewport.height, 'composer must remain inside the viewport on failed fetch');
      assert.ok(initialFailure.document.scrollWidth <= initialFailure.document.width + 1, 'failed fetch must not overflow horizontally');
      const initialStem = `${id}--${viewport.name}--initial-failure`;
      await page.screenshot({ path: path.join(outDir, `${initialStem}.png`), fullPage: false });
      await writeFile(path.join(outDir, `${initialStem}.json`), JSON.stringify({ id, viewport, ...initialFailure }, null, 2));
      await retry.click();
      await page.getByRole('region', { name: 'Reminders' }).waitFor({ state: 'visible' });
      journey.push('initial GET failure → retry → loaded reminders');
    } else if (id === storyIds[1]) {
      await page.getByRole('region', { name: 'Reminders' }).waitFor({ state: 'visible' });
      await page.evaluate(() => window.dispatchEvent(new Event('focus')));
      await retry.waitFor({ state: 'visible' });
      await page.getByText('Reminder due').waitFor({ state: 'visible' });
      await page.getByText('Check the release candidate').waitFor({ state: 'visible' });
      journey.push('loaded due/upcoming reminders → focus refresh failure retains both with stale warning');
    } else if (id === storyIds[2]) {
      await page.getByRole('region', { name: 'Reminders' }).waitFor({ state: 'visible' });
      await page.evaluate(() => window.dispatchEvent(new Event('focus')));
      await retry.waitFor({ state: 'visible' });
      await page.getByText('Reminder due').waitFor({ state: 'visible' });
      await page.screenshot({ path: path.join(outDir, `${id}--${viewport.name}--loaded-stale.png`), fullPage: false });
      await retry.focus();
      assert.equal(await page.evaluate(() => document.activeElement?.textContent?.trim()), 'Retry reminders');
      await page.screenshot({ path: path.join(outDir, `${id}--${viewport.name}--retry-focused.png`), fullPage: false });
      await retry.press('Enter');
      await page.locator('.reminders-fetch-status').waitFor({ state: 'hidden' });
      const beforeConflictRequests = Number(await page.locator('html').getAttribute('data-product-conversation-fixture-reminder-requests'));
      await page.getByRole('button', { name: 'Cancel reminder' }).click();
      await page.getByRole('alert').filter({ hasText: 'This reminder changed. Review its current time before trying again.' }).waitFor({ state: 'visible' });
      await page.getByText('Check the release candidate — revised on server').waitFor({ state: 'visible' });
      const conflictEvidence = await page.evaluate(() => ({
        requests: Number(document.documentElement.dataset['productConversationFixtureReminderRequests']),
        mutations: Number(document.documentElement.dataset['productConversationFixtureReminderMutations']),
      }));
      assert.ok(conflictEvidence.requests > beforeConflictRequests, '409 must refresh the changed server record');
      assert.equal(conflictEvidence.mutations, 1, '409 must not replay the rejected mutation');
      await page.screenshot({ path: path.join(outDir, `${id}--${viewport.name}--mutation-conflict.png`), fullPage: false });
      await page.getByRole('button', { name: 'Cancel reminder' }).click();
      await page.getByText('Cancelled').waitFor({ state: 'visible' });
      await page.waitForFunction(() => document.documentElement.dataset['productConversationFixtureReminderServedRevision'] === '6');
      const retriedMutation = await page.evaluate(() => ({
        revision: Number(document.documentElement.dataset['productConversationFixtureLastReminderRevision']),
        mutations: Number(document.documentElement.dataset['productConversationFixtureReminderMutations']),
        outcome: document.documentElement.dataset['productConversationFixtureReminderMutation'],
        acceptedRevision: Number(document.documentElement.dataset['productConversationFixtureReminderAcceptedRevision']),
      }));
      assert.deepEqual(retriedMutation, { revision: 5, mutations: 2, outcome: 'accepted', acceptedRevision: 6 }, 'explicit user retry must use the fresh revision and advance it');
      journey.push('loaded → focus refresh failure → keyboard retry → 409 alert → server row refetched without replay → explicit retry at revision 5 succeeds');
    }

    const evidence = await page.evaluate(() => {
      const selectors = ['.reminders-panel', '.reminder-card', '.reminders-fetch-status', '#input-area'];
      const geometry = Object.fromEntries(selectors.map((selector) => {
        const element = document.querySelector(selector);
        if (!element) return [selector, null];
        const rect = element.getBoundingClientRect();
        return [selector, { x: rect.x, y: rect.y, width: rect.width, height: rect.height, bottom: rect.bottom }];
      }));
      return {
        geometry,
        document: { width: document.documentElement.clientWidth, height: document.documentElement.clientHeight, scrollWidth: document.documentElement.scrollWidth },
        accessibility: {
          landmarks: [...document.querySelectorAll('main, section[aria-label], [role="region"]')].map((node) => ({ role: node.getAttribute('role') ?? node.tagName.toLowerCase(), name: node.getAttribute('aria-label') })),
          buttons: [...document.querySelectorAll('button')].map((button) => ({ name: button.innerText.trim(), disabled: button.disabled })),
          liveRegions: [...document.querySelectorAll('[aria-live]')].map((node) => ({ politeness: node.getAttribute('aria-live'), text: node.textContent?.trim() })),
        },
        focus: { tag: document.activeElement?.tagName ?? null, name: document.activeElement?.textContent?.trim() ?? null },
      };
    });
    assert.ok(evidence.geometry['.reminders-panel']?.height > 0, `${id}: reminders panel geometry missing after retry`);
    assert.ok(evidence.document.scrollWidth <= evidence.document.width + 1, `${id}/${viewport.name}: horizontal overflow`);
    assert.ok(evidence.accessibility.landmarks.some((landmark) => landmark.name === 'Reminders'), `${id}: reminders landmark missing`);
    assert.ok(evidence.accessibility.liveRegions.some((region) => region.politeness === 'polite'), `${id}: fetch-status live region missing`);
    await mkdir(outDir, { recursive: true });
    const stem = `${id}--${viewport.name}`;
    await page.screenshot({ path: path.join(outDir, `${stem}.png`), fullPage: false });
    await writeFile(path.join(outDir, `${stem}.json`), JSON.stringify({ id, viewport, journey, ...evidence }, null, 2));
    return true;
  },
});
