import path from 'node:path';
import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'new-conversation',
  readyAttribute: 'data-new-conversation-fixture-ready',
  outDir: process.env.NEW_CONVERSATION_QA_OUT ?? 'qa-artifacts/new-conversation',
  async preparePage({ page }) {
    await page.addInitScript(() => {
      window.EventSource = class FixtureEventSource {
        static CONNECTING = 0;
        static OPEN = 1;
        static CLOSED = 2;
        readyState = FixtureEventSource.OPEN;
        addEventListener() {}
        close() { this.readyState = FixtureEventSource.CLOSED; }
      };
    });
  },
  async captureStory({ page, id, outDir, viewport }) {
    if (id !== 'crowded-repositories') return false;
    const file = (state) => path.join(outDir, `${id}--${viewport.name}--${state}.png`);
    const picker = page.locator(viewport.name === 'mobile' ? '.mobile-only .directory-picker' : '.desktop-only .directory-picker');
    const toggle = picker.getByRole('button', { name: 'Choose recent location (20)' });
    await page.screenshot({ path: file('collapsed'), fullPage: true });
    if (viewport.hasTouch) await toggle.tap();
    else {
      await toggle.focus();
      await page.keyboard.press('Enter');
    }
    const search = picker.getByRole('searchbox', { name: 'Search recent locations' });
    await search.waitFor();
    if (await picker.locator('.directory-picker-recent-entry').count() !== 20) {
      throw new Error('Crowded fixture must expose every recent location');
    }
    await page.screenshot({ path: file('expanded'), fullPage: true });
    if (viewport.hasTouch) {
      await search.tap();
      const toggleHeight = await toggle.evaluate(element => element.getBoundingClientRect().height);
      const entryHeight = await picker.locator('.directory-picker-recent-entry').first().evaluate(element => element.getBoundingClientRect().height);
      if (toggleHeight < 44 || entryHeight < 44) throw new Error('Mobile location controls must be at least 44px high');
    }
    await search.fill('repository-20');
    const result = picker.getByRole('button', { name: /repository-20/ });
    if (viewport.hasTouch) await result.tap();
    else {
      await result.focus();
      await page.keyboard.press('Enter');
    }
    if (await picker.getByRole('textbox', { name: 'Directory' }).inputValue() !== '/Users/alex/projects/long-named-repository-20-for-crowded-layout-testing') {
      throw new Error('Selection must set the chosen location');
    }
    return true;
  },
  viewportMatrix: [
    { name: 'desktop', width: 1280, height: 900 },
    { name: 'mobile', width: 390, height: 844, hasTouch: true, isMobile: true },
  ],
});
