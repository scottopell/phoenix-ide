import { writeFile } from 'node:fs/promises';
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
    if (id.startsWith('sidebar-visual-')) {
      const rows = page.locator('.product-conversation-list-row');
      await rows.first().waitFor();
      await page.screenshot({ path: `${outDir}/${id}--${viewport.name}.png`, fullPage: true });
      await page.locator('.sidebar').screenshot({ path: `${outDir}/${id}--${viewport.name}-sidebar.png` });
      const geometry = await rows.evaluateAll(elements => elements.map(row => {
        const box = selector => {
          const el = selector ? row.querySelector(selector) : row;
          if (!el) return null;
          const r = el.getBoundingClientRect();
          const css = getComputedStyle(el);
          return { x: r.x, y: r.y, width: r.width, height: r.height, right: r.right, bottom: r.bottom,
            fontSize: css.fontSize, fontWeight: css.fontWeight, lineHeight: css.lineHeight,
            padding: css.padding, minWidth: css.minWidth, grid: css.gridTemplateColumns };
        };
        return { row: box(), main: box('.conv-item-main'), title: box('.conv-item-title'),
          dot: box('.conv-state-dot'), badge: box('.pr-badge'), menu: box('.conv-item-menu-btn') };
      }));
      await writeFile(`${outDir}/${id}--${viewport.name}.geometry.json`, JSON.stringify(geometry, null, 2));
      if (geometry.length !== 5) throw new Error('Missing visual state rows');
      if (process.env.SIDEBAR_VISUAL_BASELINE !== '1') {
        for (const g of geometry) {
          if (!g.title || !g.menu || g.title.width < 24) throw new Error('Title/action allocation missing');
          for (const b of [g.main, g.badge, g.menu].filter(Boolean)) {
            if (b.x < g.row.x - 1 || b.right > g.row.right + 1) throw new Error('Row content escapes bounds');
          }
          if (g.badge && g.badge.y < g.main.bottom && g.badge.bottom > g.main.y && g.badge.x < g.main.right - 1) throw new Error('PR overlaps main');
          if (g.badge && g.badge.y < g.menu.bottom && g.badge.bottom > g.menu.y && g.badge.right > g.menu.x + 1) throw new Error('PR overlaps menu');
          if (viewport.hasTouch && (g.menu.width < 44 || g.menu.height < 44 || g.main.height < 44)) throw new Error('Touch target smaller than 44px');
        }
      }
      const trigger = rows.first().getByRole('button', { name: /Actions for conversation/ });
      await trigger.focus();
      await page.keyboard.press('Enter');
      await rows.first().getByRole('button', { name: /Rename conversation/ }).waitFor();
      await page.screenshot({ path: `${outDir}/${id}--${viewport.name}-menu.png`, fullPage: true });
      return true;
    }
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
    const trigger = row.getByRole('button', { name: /Actions for conversation/ });
    await trigger.click();
    const rename = page.getByRole('button', { name: /Rename conversation Fixture Product Root/ });
    const close = page.getByRole('button', { name: /Close conversation Fixture Product Root/ });
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
    await trigger.click();
    await close.focus();
    if (!(await close.evaluate((el) => el === document.activeElement))) throw new Error('Close action did not receive keyboard focus');
    await close.click();
    await page.getByText(/Close \"Fixture Product Root\"/).waitFor();
    await page.getByRole('button', { name: 'Cancel' }).click();
    const titleBox = await row.locator('.conv-item-title').boundingBox();
    const actionsBox = await trigger.boundingBox();
    if (!titleBox || titleBox.width < 24 || !actionsBox || actionsBox.width < 44) {
      throw new Error(`Product row title/actions are not visibly laid out at ${viewport.name ?? 'default'}`);
    }
    await page.screenshot({ path: `${outDir}/product-actions-continued--${viewport.name ?? 'default'}-verified.png`, fullPage: true });
    return true;
  },
});
