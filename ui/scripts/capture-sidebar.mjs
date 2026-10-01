import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'sidebar',
  readyAttribute: 'data-sidebar-fixture',
  outDir: process.env.SIDEBAR_QA_OUT ?? 'qa-artifacts/sidebar',
  viewport: { width: 1180, height: 900 },
  viewportMatrix: [
    { name: 'desktop', width: 1180, height: 900 },
    { name: 'touch', width: 390, height: 844, hasTouch: true, isMobile: true },
  ],
  captureStory: async ({ page, id, outDir, viewport }) => {
    if (id !== 'product-actions-continued') return false;
    const row = page.locator('[data-product-conversation-id="pc-continued-fixture"]');
    await row.waitFor();
    if (viewport.name === 'touch') {
      const interaction = await page.evaluate(() => ({
        hasTouch: navigator.maxTouchPoints > 0,
        coarsePointer: window.matchMedia('(pointer: coarse)').matches,
      }));
      if (!interaction.hasTouch || !interaction.coarsePointer) {
        throw new Error(`Touch viewport did not expose touch/coarse-pointer input: ${JSON.stringify(interaction)}`);
      }
    }
    const dotCount = await row.locator('.conv-state-dot').count();
    if (dotCount !== 1) throw new Error(`Expected one product indicator, found ${dotCount}`);
    await page.getByLabel('Working').waitFor();
    const rename = page.getByRole('button', { name: /Rename product conversation Fixture Product Root/ });
    const close = page.getByRole('button', { name: /Close product conversation Fixture Product Root/ });
    for (const [name, locator] of [['rename', rename], ['close', close]]) {
      const box = await locator.boundingBox();
      if (!box || box.width < 44 || box.height < 44) {
        throw new Error(`Expected ${name} product action target to be at least 44px, got ${box ? `${box.width}x${box.height}` : 'no box'}`);
      }
    }
    await rename.focus();
    if (!(await rename.evaluate((el) => el === document.activeElement))) throw new Error('Rename action did not receive keyboard focus');
    await rename.click();
    await page.getByRole('textbox').waitFor();
    await page.getByRole('button', { name: 'Cancel' }).click();
    await close.focus();
    if (!(await close.evaluate((el) => el === document.activeElement))) throw new Error('Close action did not receive keyboard focus');
    await close.click();
    await page.getByText(/Close \"Fixture Product Root\"/).waitFor();
    await page.getByRole('button', { name: 'Cancel' }).click();
    const titleBox = await row.locator('.conv-item-title').boundingBox();
    const actionsBox = await row.locator('.conv-actions').boundingBox();
    if (!titleBox || titleBox.width < 24 || !actionsBox || actionsBox.width < 64) {
      throw new Error(`Product row title/actions are not visibly laid out at ${viewport.name ?? 'default'}`);
    }
    await page.screenshot({ path: `${outDir}/product-actions-continued--${viewport.name ?? 'default'}-verified.png`, fullPage: true });
    return true;
  },
});
