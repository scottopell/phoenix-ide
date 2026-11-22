import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { runSurfaceCapture } from './capture-ladle-surface.mjs';

const storyIds = [
  'initial-failure',
  'alias-fallback-initial-failure',
  'loaded-stale-upcoming',
  'retry-mutation-rejection',  'retry-mutation-rejection',
  'compact-due',
  'compact-scheduled',
  'compact-cancelled',
  'compact-empty',
];
const viewports = [
  { name: 'mobile-375x667', width: 375, height: 667, isMobile: true, hasTouch: true },
  { name: 'mobile-390x844', width: 390, height: 844, isMobile: true, hasTouch: true },
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
    if (id.endsWith('initial-failure')) {
      await retry.waitFor({ state: 'visible' });
      assert.equal(await page.getByRole('region', { name: 'Reminders' }).count(), 0, 'initial GET failure must not produce a bordered panel');
      assert.ok(await page.locator('.reminders-fetch-status').evaluate((node) => node.getBoundingClientRect().height < 60), 'initial fetch failure must stay compact');
      const initialFailure = await page.evaluate(() => {
        const selectors = ['.reminders-fetch-status', '#input-area', '.product-conversation-page__composer', '.product-conversation-page__transcript', '.conversation-alias-fallback #app .conversation-column #chat-view'];
        const geometry = Object.fromEntries(selectors.map((selector) => {
          const rect = document.querySelector(selector)?.getBoundingClientRect();
          return [selector, rect ? { x: rect.x, y: rect.y, width: rect.width, height: rect.height, bottom: rect.bottom } : null];
        }));
        return { geometry, document: { width: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth } };
      });
      const composer = initialFailure.geometry['#input-area'];
      assert.ok(composer?.height > 0 && composer.y >= 0 && composer.bottom <= viewport.height, 'composer must remain inside the viewport on failed fetch');
      assert.ok(initialFailure.document.scrollWidth <= initialFailure.document.width + 1, 'failed fetch must not overflow horizontally');
      const transcriptSelector = id === 'alias-fallback-initial-failure' ? '.conversation-alias-fallback #app .conversation-column #chat-view' : '.product-conversation-page__transcript';
      assert.ok(initialFailure.geometry[transcriptSelector]?.height > 0, 'failed fetch must render the route transcript');
      const initialStem = `${id}--${viewport.name}--initial-failure`;
      await page.screenshot({ path: path.join(outDir, `${initialStem}.png`), fullPage: false });
      await writeFile(path.join(outDir, `${initialStem}.json`), JSON.stringify({ id, viewport, ...initialFailure }, null, 2));
      await retry.focus();
      assert.equal(await retry.evaluate((button) => document.activeElement === button), true, 'Retry must be keyboard-focusable');
      if (id === 'alias-fallback-initial-failure') {
        await page.screenshot({ path: path.join(outDir, `${id}--${viewport.name}--retry-focused.png`), fullPage: false });
      }
      if (id === 'alias-fallback-initial-failure') {
        await retry.press('Enter');
      } else {
        await retry.click();
      }
      await page.getByRole('region', { name: 'Reminders' }).waitFor({ state: 'visible' });
      journey.push('initial GET failure → retry → loaded reminders');
    } else if (id === 'loaded-stale-upcoming') {
      await page.getByRole('region', { name: 'Reminders' }).waitFor({ state: 'visible' });
      const disclosure = page.getByRole('button', { name: /Reminders/ });
      assert.equal(await disclosure.getAttribute('aria-expanded'), 'false', 'loaded reminder panel starts collapsed');
      await disclosure.press('Enter');
      await page.getByText('Reminder due').waitFor({ state: 'visible' });
      await page.evaluate(() => window.dispatchEvent(new Event('focus')));
      await retry.waitFor({ state: 'visible' });
      await page.getByText('Reminder due').waitFor({ state: 'visible' });
      await page.getByText('Check the release candidate').waitFor({ state: 'visible' });
      journey.push('loaded due/upcoming reminders → focus refresh failure retains both with stale warning');
    } else if (id.startsWith('compact-')) {
      await mkdir(outDir, { recursive: true });
      const empty = id === 'compact-empty';
      const panel = page.getByRole('region', { name: 'Reminders' });
      if (empty) {
        assert.equal(await panel.count(), 0, 'empty fixture must omit the reminders panel');
        const composer = page.locator('#input-area');
        const rect = await composer.boundingBox();
        assert.ok(rect && rect.y >= 0 && rect.y + rect.height <= viewport.height, 'empty fixture composer stays visible');
        journey.push('empty reminders → no panel; composer remains visible');
      } else {
        await panel.waitFor({ state: 'visible' });
        const toggle = page.getByRole('button', { name: /Reminders/ });
        assert.equal(await toggle.getAttribute('aria-expanded'), 'false');
        const expectedText = Array.from({ length: 18 }, (_, index) => `Line ${index + 1}: review the deployment checklist, confirm the service owner, inspect the latest monitoring evidence, and record any follow-up before the scheduled release window.`).join('\n');
        assert.ok(expectedText.length > 1000 && expectedText.includes('Line 18:'), 'fixture contains long multi-line reminder text');
        assert.equal(await page.locator('.reminder-card').count(), 0, 'collapsed panel hides details');
        assert.equal(await page.getByRole('button', { name: /Reschedule|Cancel reminder/ }).count(), 0, 'collapsed panel hides actions');
        const collapsed = await page.evaluate(() => ({
          panel: document.querySelector('.reminders-panel')?.getBoundingClientRect().height ?? null,
          composer: (() => { const r = document.querySelector('#input-area')?.getBoundingClientRect(); return r ? { x: r.x, y: r.y, width: r.width, height: r.height, bottom: r.bottom } : null; })(),
          transcript: (() => { const r = document.querySelector('.product-conversation-page__transcript')?.getBoundingClientRect(); return r ? { x: r.x, y: r.y, width: r.width, height: r.height } : null; })(),
          width: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth,
        }));
        assert.ok(collapsed.panel <= 60, `collapsed reminders height ${collapsed.panel}px exceeds 60px`);
        assert.ok(collapsed.composer?.y >= 0 && collapsed.composer.bottom <= viewport.height, 'composer stays within viewport');
        assert.ok(collapsed.scrollWidth <= collapsed.width + 1, 'collapsed page has no horizontal overflow');
        const stem = `${id}--${viewport.name}--collapsed`;
        await page.screenshot({ path: path.join(outDir, `${stem}.png`), fullPage: false });
        await writeFile(path.join(outDir, `${stem}.json`), JSON.stringify({ id, viewport, journey: ['initial collapsed disclosure'], geometry: collapsed }, null, 2));
        const collapsedRequestCount = Number(await page.locator('html').getAttribute('data-product-conversation-fixture-reminder-requests'));
        await page.evaluate(() => window.dispatchEvent(new Event('focus')));
        await page.waitForFunction((count) => Number(document.documentElement.dataset['productConversationFixtureReminderRequests']) > count, collapsedRequestCount);
        await page.waitForFunction(() => document.documentElement.dataset['productConversationFixtureReminderState'] === 'loaded');
        await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
        assert.equal(await toggle.getAttribute('aria-expanded'), 'false', 'background refresh preserves initial collapsed state');
        assert.equal(await page.locator('.reminder-card').count(), 0, 'background refresh keeps details hidden while collapsed');
        await toggle.press('Enter');
        assert.equal(await toggle.getAttribute('aria-expanded'), 'true');
        assert.equal(await page.locator('.reminder-card p').textContent(), expectedText, 'expanded disclosure renders exact full long text');
        const stateName = id === 'compact-due' ? 'Reminder due' : id === 'compact-cancelled' ? 'Cancelled' : 'Upcoming';
        await page.getByText(stateName, { exact: true }).waitFor({ state: 'visible' });
        if (id === 'compact-scheduled') {
          await page.getByRole('button', { name: 'Reschedule' }).waitFor({ state: 'visible' });
          await page.getByRole('button', { name: 'Cancel reminder' }).waitFor({ state: 'visible' });
        } else assert.equal(await page.getByRole('button', { name: /Reschedule|Cancel reminder/ }).count(), 0, 'non-scheduled states have no mutation actions');
        const textarea = page.locator('#input-area textarea').first();
        await textarea.focus();
        const transcript = page.locator('.product-conversation-page__transcript');
        await transcript.evaluate((node) => { node.scrollTop = Math.max(0, (node.scrollHeight - node.clientHeight) / 2); });
        const before = await page.evaluate(() => ({
          requests: Number(document.documentElement.dataset['productConversationFixtureReminderRequests']),
          transcript: (() => { const n = document.querySelector('.product-conversation-page__transcript'); const r = n?.getBoundingClientRect(); return { scrollTop: n?.scrollTop, x: r?.x, y: r?.y, width: r?.width, height: r?.height }; })(),
          composer: (() => { const r = document.querySelector('#input-area')?.getBoundingClientRect(); return r ? { x: r.x, y: r.y, width: r.width, height: r.height } : null; })(),
          active: document.activeElement?.tagName,
        }));
        await page.evaluate(() => window.dispatchEvent(new Event('focus')));
        await page.waitForFunction((count) => Number(document.documentElement.dataset['productConversationFixtureReminderRequests']) > count, before.requests);
        await page.waitForFunction(() => document.documentElement.dataset['productConversationFixtureReminderState'] === 'loaded');
        await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
        const after = await page.evaluate(() => ({
          requests: Number(document.documentElement.dataset['productConversationFixtureReminderRequests']),
          transcript: (() => { const n = document.querySelector('.product-conversation-page__transcript'); const r = n?.getBoundingClientRect(); return { scrollTop: n?.scrollTop, x: r?.x, y: r?.y, width: r?.width, height: r?.height }; })(),
          composer: (() => { const r = document.querySelector('#input-area')?.getBoundingClientRect(); return r ? { x: r.x, y: r.y, width: r.width, height: r.height } : null; })(),
          active: document.activeElement?.tagName,
          expanded: document.querySelector('.reminders-panel button[aria-expanded]')?.getAttribute('aria-expanded'),
        }));
        assert.equal(after.expanded, 'true', 'background refresh preserves expanded disclosure');
        assert.equal(after.active, before.active, 'background refresh preserves composer focus');
        assert.equal(after.transcript.scrollTop, before.transcript.scrollTop, 'background refresh preserves transcript scroll');
        assert.deepEqual(after.transcript, before.transcript, 'background refresh preserves transcript geometry');
        assert.deepEqual(after.composer, before.composer, 'background refresh preserves composer geometry');
        const expanded = await page.evaluate(() => ({ panel: document.querySelector('.reminders-panel')?.getBoundingClientRect().height, details: document.querySelector('.grounding-section-body')?.getBoundingClientRect().height ?? null, composer: document.querySelector('#input-area')?.getBoundingClientRect().toJSON(), width: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth }));
        assert.ok(expanded.composer.y >= 0 && expanded.composer.bottom <= viewport.height, 'composer stays visible after expanded refresh');
        assert.ok(expanded.scrollWidth <= expanded.width + 1, 'expanded page has no horizontal overflow');
        const expandedStem = `${id}--${viewport.name}--expanded`;
        await page.screenshot({ path: path.join(outDir, `${expandedStem}.png`), fullPage: false });
        await writeFile(path.join(outDir, `${expandedStem}.json`), JSON.stringify({ id, viewport, journey: ['collapsed → Enter expanded → background refresh preserves focus, scroll, geometry, and expansion'], geometry: expanded, refresh: { before, after } }, null, 2));
        await toggle.press('Space');
        assert.equal(await toggle.getAttribute('aria-expanded'), 'false', 'Space collapses the disclosure');
        journey.push('collapsed → Enter reveals full text and correct actions → background refresh retains expanded state and composer/transcript geometry → Space collapses');
      }
    } else if (id === 'retry-mutation-rejection') {
      await page.getByRole('region', { name: 'Reminders' }).waitFor({ state: 'visible' });
      const disclosure = page.getByRole('button', { name: /Reminders/ });
      assert.equal(await disclosure.getAttribute('aria-expanded'), 'false', 'loaded reminder panel starts collapsed');
      await disclosure.press('Enter');
      await page.getByText('Reminder due').waitFor({ state: 'visible' });
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
      await page.getByRole('alert').filter({ hasText: 'This reminder changed. Review its current time before trying again.' }).scrollIntoViewIfNeeded();
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

    if (id.startsWith('compact-')) return true;

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
