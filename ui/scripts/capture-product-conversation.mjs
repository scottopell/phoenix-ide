import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'product-conversation',
  readyAttribute: 'data-product-conversation-fixture-ready',
  outDir: process.env.PRODUCT_CONVERSATION_QA_OUT ?? 'qa-artifacts/product-conversation',
  viewportMatrix: [
    { name: 'desktop', width: 1440, height: 900 },
    { name: 'mobile', width: 390, height: 844 },
  ],
  captureStory: async ({ page, id, outDir, viewport }) => {
    const capturesRecall = id === 'desktop-multi-segment-qa-work' || id === 'mobile-open';
    if (!capturesRecall) return false;

    await page.screenshot({ path: `${outDir}/${id}--${viewport.name}.png`, fullPage: true });
    await page.getByRole('button', { name: 'Recall' }).click();
    await page.getByRole('dialog', { name: 'Recall' }).waitFor();
    await page.screenshot({
      path: `${outDir}/${id}--${viewport.name}--recall-expanded.png`,
      fullPage: true,
    });
    await page.getByRole('button', { name: 'Close Recall' }).click();

    if (id === 'desktop-multi-segment-qa-work' && viewport.name === 'desktop') {
      await page.getByText('Work', { exact: true }).click();
      await page.locator('[data-testid="product-conversation-work"][open] .chain-work-identity').waitFor();
      await page.screenshot({
        path: `${outDir}/${id}--${viewport.name}--work-expanded.png`,
        fullPage: true,
      });
    }
    return true;
  },
});
