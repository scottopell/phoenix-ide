import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'mobile-multi-pr-conversation',
  readyAttribute: 'data-mobile-multi-pr-conversation-fixture-ready',
  outDir: process.env.MOBILE_MULTI_PR_CONVERSATION_QA_OUT ?? 'qa-artifacts/mobile-multi-pr-conversation',
  viewport: { width: 390, height: 844 },
  captureStory: async ({ page, id }) => {
    if (id === 'chooser-open') {
      await page.waitForSelector('.active-pr-dialog[open]');
      const choiceCount = await page.locator('.active-pr-dialog [role="option"]').count();
      if (choiceCount !== 2) throw new Error(`Expected 2 StateBar PR choices, found ${choiceCount}`);
    }
    if (id === 'model-dialog') {
      await page.waitForSelector('.model-selection-dialog[open]');
      await page.waitForSelector('.model-selection-dialog [aria-label="Select effort"]');
    }
    if (id === 'model-locked') {
      await page.waitForSelector('.conv-model-lock-reason');
      if (await page.locator('.model-selection-dialog[open]').count()) {
        throw new Error('Locked model scenario unexpectedly opened the model dialog');
      }
    }
    return false;
  },
});
