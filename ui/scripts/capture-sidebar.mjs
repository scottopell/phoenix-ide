import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'sidebar',
  readyAttribute: 'data-sidebar-fixture',
  outDir: process.env.SIDEBAR_QA_OUT ?? 'qa-artifacts/sidebar',
  viewport: { width: 1180, height: 900 },
  viewportMatrix: [
    { name: 'desktop', width: 1180, height: 900 },
    { name: 'touch', width: 390, height: 844 },
  ],
  captureStory: async ({ page, id, outDir, viewport }) => {
    if (id !== 'product-actions-continued') return false;
    const row = page.locator('[data-product-conversation-id="pc-continued-fixture"]');
    await row.waitFor();
    const dotCount = await row.locator('.conv-state-dot').count();
    if (dotCount !== 1) throw new Error(`Expected one product indicator, found ${dotCount}`);
    await page.getByLabel('Working').waitFor();
    const rename = page.getByRole('button', { name: /Rename product conversation Fixture Product Root/ });
    const close = page.getByRole('button', { name: /Close product conversation Fixture Product Root/ });
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
