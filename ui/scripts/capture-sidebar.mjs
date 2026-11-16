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
      const media = await page.evaluate(() => ({
        coarse: matchMedia('(any-pointer: coarse)').matches,
        fine: matchMedia('(pointer: fine)').matches,
        hover: matchMedia('(hover: hover)').matches,
        width: innerWidth, height: innerHeight, deviceScaleFactor: devicePixelRatio,
      }));
      await writeFile(`${outDir}/${id}--${viewport.name}.environment.json`, JSON.stringify(media, null, 2));
      if (media.coarse !== !!viewport.hasTouch) throw new Error('Capture pointer media does not match requested context');
      const rows = page.locator('.product-conversation-list-row');
      await rows.first().waitFor();
      await page.screenshot({ path: `${outDir}/${id}--${viewport.name}.png`, fullPage: false });
      await page.locator('.sidebar').screenshot({ path: `${outDir}/${id}--${viewport.name}-sidebar.png` });
      const controls = await page.locator('.sidebar-header button, .sidebar-header a, .sidebar-lifecycle-tab, .ember-entry').evaluateAll(elements => elements.map(el => {
        const r = el.getBoundingClientRect();
        const sidebar = el.closest('.sidebar').getBoundingClientRect();
        return { name: el.getAttribute('aria-label') ?? el.textContent.trim(), contentFits: el.scrollWidth <= el.clientWidth, x: r.x, right: r.right,
          width: r.width, height: r.height, sidebarRight: sidebar.right, sidebarLeft: sidebar.x };
      }));
      await writeFile(`${outDir}/${id}--${viewport.name}.controls.json`, JSON.stringify(controls, null, 2));
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
      const mediaAfterScreenshot = await page.evaluate(() => ({ coarse: matchMedia('(any-pointer: coarse)').matches, fine: matchMedia('(pointer: fine)').matches }));
      await writeFile(`${outDir}/${id}--${viewport.name}.media-after-screenshot.json`, JSON.stringify(mediaAfterScreenshot, null, 2));
      if (mediaAfterScreenshot.coarse !== media.coarse) throw new Error('Screenshot changed pointer media');
      if (geometry.length !== 5) throw new Error('Missing visual state rows');
      if (process.env.SIDEBAR_VISUAL_BASELINE !== '1') {
        for (const c of controls) {
          if (c.name === '+ New' && !c.contentFits) throw new Error('New label clipped');
          if (c.x < c.sidebarLeft - 1 || c.right > c.sidebarRight + 1) throw new Error('Header control escapes sidebar');
          if (viewport.hasTouch && (c.width < 44 || c.height < 44)) throw new Error('Header touch target smaller than 44px');
        }
        for (const g of geometry) {
          if (!g.title || !g.menu || !g.dot || g.title.width < 40) throw new Error('Title/action allocation missing');
          for (const b of [g.main, g.badge, g.menu].filter(Boolean)) {
            if (b.x < g.row.x - 1 || b.right > g.row.right + 1) throw new Error('Row content escapes bounds');
          }
          if (g.badge && g.badge.y < g.main.bottom && g.badge.bottom > g.main.y && g.badge.x < g.main.right - 1) throw new Error('PR overlaps main');
          if (g.badge && g.badge.y < g.menu.bottom && g.badge.bottom > g.menu.y && g.badge.right > g.menu.x + 1) throw new Error('PR overlaps menu');
          if (g.dot.right > g.title.x || g.dot.x < g.main.x) throw new Error('Status dot overlaps title');
          if (!viewport.hasTouch && id === 'sidebar-visual-normal' && g.row.height > 40) throw new Error('Mouse row is not compact');
          if (viewport.hasTouch && g.badge && g.badge.height < 44) throw new Error('PR touch target smaller than 44px');
          if (viewport.hasTouch && (g.menu.width < 44 || g.menu.height < 44 || g.main.height < 44)) throw new Error('Touch target smaller than 44px');
        }
      }
      if (process.env.SIDEBAR_VISUAL_BASELINE !== '1' && id === 'sidebar-visual-normal') {
        const sweep = [];
        const sidebar = page.locator('.sidebar');
        for (const width of [160, 180, 200, 220, 230, 236, 237, 238, 239, 240, 260, 280, 320, 500]) {
          await sidebar.evaluate((el, value) => el.style.setProperty('--sidebar-pane-width', `${value}px`), width);
          const sample = await sidebar.evaluate(el => {
            const bounds = el.getBoundingClientRect();
            const rows = [...el.querySelectorAll('.product-conversation-list-row')].map(row => {
              const bounds = row.getBoundingClientRect();
              const main = row.querySelector('.conv-item-main').getBoundingClientRect();
              const title = row.querySelector('.conv-item-title').getBoundingClientRect();
              const menu = row.querySelector('.conv-item-menu-btn').getBoundingClientRect();
              const badge = row.querySelector('.pr-badge')?.getBoundingClientRect();
              return { titleWidth: title.width, mainWidth: main.width, inside: [main, menu, badge].filter(Boolean).every(r => r.x >= bounds.x && r.right <= bounds.right),
                badgeClear: !badge || badge.top >= main.bottom || badge.right <= menu.x && badge.x >= main.right };
            });
            const controls = [...el.querySelectorAll('.sidebar-header button, .sidebar-lifecycle-tab')].map(control => {
              const r = control.getBoundingClientRect();
              return { fits: control.scrollWidth <= control.clientWidth, inside: r.x >= bounds.x && r.right <= bounds.right + 1, x: r.x, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height };
            });
            return { rows, controls };
          });
          sweep.push({ width, ...sample });
        }
        await sidebar.evaluate(el => el.style.removeProperty('--sidebar-pane-width'));
        await writeFile(`${outDir}/${id}--${viewport.name}.width-sweep.json`, JSON.stringify(sweep, null, 2));
        for (const sample of sweep) {
          if (sample.rows.some(row => row.titleWidth < 40 || !row.inside || !row.badgeClear || viewport.hasTouch && row.mainWidth < 44)) throw new Error(`Row allocation failed at sidebar width ${sample.width}`);
          if (sample.controls.some(c => !c.fits || !c.inside || viewport.hasTouch && (c.width < 44 || c.height < 44))) throw new Error(`Header allocation failed at sidebar width ${sample.width}`);
          if (sample.controls.some((a, i) => sample.controls.slice(i + 1).some(b => a.x < b.right && b.x < a.right && a.top < b.bottom && b.top < a.bottom))) throw new Error(`Header controls overlap at sidebar width ${sample.width}`);
        }
      }
      await page.locator('.sidebar').evaluate(el => { el.tabIndex = -1; el.focus(); });
      await page.keyboard.press('Escape');
      await page.keyboard.press('ArrowDown');
      await page.waitForFunction(() => document.querySelectorAll('.product-conversation-list-row.keyboard-selected').length === 1);
      await page.keyboard.press('ArrowDown');
      if (!(await rows.nth(1).evaluate(el => el.classList.contains('keyboard-selected')))) throw new Error('Arrow navigation lost');
      await page.screenshot({ path: `${outDir}/${id}--${viewport.name}-keyboard.png`, fullPage: false });
      const trigger = rows.first().getByRole('button', { name: /Actions for conversation/ });
      await trigger.focus();
      await page.keyboard.press('Enter');
      await rows.first().getByRole('button', { name: /Rename conversation/ }).waitFor();
      await page.screenshot({ path: `${outDir}/${id}--${viewport.name}-menu.png`, fullPage: false });
      await page.keyboard.press('Escape');
      if (!(await trigger.evaluate(el => el === document.activeElement))) throw new Error('Action menu did not restore focus');
      await page.getByRole('button', { name: 'Settings', exact: true }).click();
      const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
      await settings.waitFor({ state: 'visible' });
      await settings.getByRole('radiogroup', { name: 'Model language' }).waitFor();
      await settings.getByRole('checkbox', { name: 'Disable browser notifications' }).waitFor();
      if (await settings.locator('.settings-section__error').count()) throw new Error('Settings fixture contains a load error');
      const popup = await settings.evaluate(el => {
        const r = el.getBoundingClientRect();
        const t = document.querySelector('.settings-dropdown-trigger').getBoundingClientRect();
        const hit = document.elementFromPoint(r.right - 8, r.top + 8);
        return { x: r.x, right: r.right, top: r.top, bottom: r.bottom, triggerBottom: t.bottom,
          viewportWidth: innerWidth, viewportHeight: innerHeight, unclipped: !!hit && el.contains(hit) };
      });
      await writeFile(`${outDir}/${id}--${viewport.name}.settings.json`, JSON.stringify(popup, null, 2));
      await page.screenshot({ path: `${outDir}/${id}--${viewport.name}-settings.png`, fullPage: false });
      if (Math.abs(popup.top - popup.triggerBottom - 6) > 1 || popup.x < 0 || popup.right > popup.viewportWidth || popup.bottom > popup.viewportHeight || !popup.unclipped) throw new Error('Settings popup shifted or clipped');
      const about = settings.getByRole('button', { name: 'About', exact: true });
      await about.scrollIntoViewIfNeeded();
      if (!(await about.evaluate(el => { const r = el.getBoundingClientRect(); return el.contains(document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2)); }))) throw new Error('Settings bottom control is clipped');
      await page.screenshot({ path: `${outDir}/${id}--${viewport.name}-settings-bottom.png`, fullPage: false });
      await page.keyboard.press('Escape');
      await settings.waitFor({ state: 'hidden' });
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
    if (!titleBox || titleBox.width < 24 || !actionsBox || actionsBox.width < (viewport.hasTouch ? 44 : 32)) {
      throw new Error(`Product row title/actions are not visibly laid out at ${viewport.name ?? 'default'}`);
    }
    await page.screenshot({ path: `${outDir}/product-actions-continued--${viewport.name ?? 'default'}-verified.png`, fullPage: true });
    return true;
  },
});
