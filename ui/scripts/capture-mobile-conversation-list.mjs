import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'mobile-conversation-list',
  readyAttribute: 'data-mobile-conversation-list-fixture-ready',
  outDir: process.env.MOBILE_CONVERSATION_LIST_QA_OUT ?? 'qa-artifacts/mobile-conversation-list',
  viewport: { width: 393, height: 852 },
  viewportMatrix: [
    { name: 'mobile', width: 393, height: 852, hasTouch: true, isMobile: true },
    { name: 'desktop', width: 1180, height: 900 },
  ],
  captureStory: async ({ page, id, outDir, viewport }) => {
    if (!id.startsWith('product-conversations-')) return false;
    const isHistory = id.includes('-history-');
    const productConversationId = isHistory ? 'mobile-product-history' : 'mobile-product-open';
    const row = page.locator(`[data-product-conversation-id="${productConversationId}"]`);
    await row.waitFor();
    const main = row.locator('.conv-item-main');
    const title = row.locator('.conv-item-title');
    const meta = row.locator('.product-conversation-list-row__meta');
    const actions = row.locator('.conv-actions');
    const actionButtons = actions.locator('.conv-action-btn');
    const metrics = await row.evaluate((element) => {
      const mainElement = element.querySelector('.conv-item-main');
      const titleElement = element.querySelector('.conv-item-title');
      const metaElement = element.querySelector('.product-conversation-list-row__meta');
      const actionsElement = element.querySelector('.conv-actions');
      if (!(mainElement instanceof HTMLElement)
        || !(titleElement instanceof HTMLElement)
        || !(metaElement instanceof HTMLElement)
        || !(actionsElement instanceof HTMLElement)) throw new Error('Product row structure missing');
      const rowRect = element.getBoundingClientRect();
      const mainRect = mainElement.getBoundingClientRect();
      const titleRect = titleElement.getBoundingClientRect();
      const actionsRect = actionsElement.getBoundingClientRect();
      const mainStyle = getComputedStyle(mainElement);
      return {
        rowHeight: rowRect.height,
        rowRight: rowRect.right,
        mainRight: mainRect.right,
        actionsLeft: actionsRect.left,
        titleWidth: titleRect.width,
        metaDisplay: getComputedStyle(metaElement).display,
        mainBackground: mainStyle.backgroundColor,
        mainBorderWidth: mainStyle.borderWidth,
        mainBorderRadius: mainStyle.borderRadius,
        mainTextAlign: mainStyle.textAlign,
        documentWidth: document.documentElement.clientWidth,
        documentScrollWidth: document.documentElement.scrollWidth,
      };
    });
    if (metrics.metaDisplay !== 'flex'
      || metrics.mainBackground !== 'rgba(0, 0, 0, 0)'
      || metrics.mainBorderWidth !== '0px'
      || metrics.mainBorderRadius !== '0px'
      || metrics.mainTextAlign !== 'left'
      || metrics.mainRight > metrics.actionsLeft
      || metrics.titleWidth < 24
      || metrics.rowRight > metrics.documentWidth
      || metrics.documentScrollWidth !== metrics.documentWidth) {
      throw new Error(`Product aggregate row layout regressed at ${viewport.name}: ${JSON.stringify(metrics)}`);
    }
    if (viewport.name === 'mobile' && metrics.rowHeight > 80) {
      throw new Error(`Product aggregate row lost compact mobile density: ${JSON.stringify(metrics)}`);
    }
    const expectedActionCount = isHistory ? 1 : 2;
    if ((await actionButtons.count()) !== expectedActionCount) {
      throw new Error(`${isHistory ? 'History' : 'Open'} aggregate must expose ${expectedActionCount} action target(s)`);
    }
    for (const button of await actionButtons.all()) {
      const box = await button.boundingBox();
      if (!box || box.width < 44 || box.height < 44) throw new Error(`Product action target is smaller than 44px: ${JSON.stringify(box)}`);
    }
    await main.focus();
    if (!(await main.evaluate((element) => element === document.activeElement))) throw new Error('Aggregate navigation control did not receive focus');
    await actionButtons.first().focus();
    if (!(await actionButtons.first().evaluate((element) => element === document.activeElement))) throw new Error('Aggregate action did not receive focus');
    await page.screenshot({ path: `${outDir}/${id}--${viewport.name}-verified.png`, fullPage: true });
    return true;
  },
});
