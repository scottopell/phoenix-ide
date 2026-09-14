import assert from 'node:assert/strict';
import path from 'node:path';
import { runSurfaceCapture } from './capture-ladle-surface.mjs';

runSurfaceCapture({
  surface: 'ask-user-question',
  readyAttribute: 'data-ask-user-question-fixture',
  outDir: process.env.AUQ_QA_OUT ?? 'qa-artifacts/ask-user-question',
  viewportMatrix: [
    { name: 'small-phone', width: 320, height: 568 },
    { name: 'mobile', width: 390, height: 844 },
    { name: 'tablet', width: 600, height: 568 },
    { name: 'below-columns', width: 839, height: 900 },
    { name: 'columns', width: 840, height: 900 },
    { name: 'desktop-small', width: 1024, height: 900 },
    { name: 'desktop', width: 1440, height: 900 },
    { name: 'short', width: 390, height: 320 },
  ],
  captureStory: async ({ page, id, outDir, viewport }) => {
    const panel = page.locator('.question-panel');
    await panel.waitFor();
    const suffix = `${id}--${viewport.name}`;
    if (id === 'read-only') {
      assert.equal(await panel.locator('input, textarea, button').count(), 0);
    } else {
      assert.equal(await panel.locator('input:checked').count(), 0, 'Initial form must be unanswered');
      const controls = panel.locator('input[type=radio],input[type=checkbox]');
      await controls.first().check();
      if (id === 'long-preview') {
        await page.getByRole('button',{name:'Show full preview'}).waitFor();
        assert.ok(await page.locator('.question-preview-pane pre').evaluate(e=>e.clientHeight)<=168);
        await page.getByRole('button',{name:'Show full preview'}).click();
        assert.ok(await page.locator('.question-preview-pane pre').evaluate(e=>e.clientHeight)>168);
        await page.getByRole('button',{name:'Show less'}).click();
      }
      if (id === 'compact-headers') {
        for (const button of await page.getByRole('navigation',{name:'Questions'}).getByRole('button').all()) {
          const rect = await button.boundingBox(); assert.ok(rect.width>=44 && rect.height>=44);
        }
      }
      if (id === 'plain') {
        await controls.first().focus();
        await page.keyboard.press('ArrowDown');
        assert.equal(await controls.nth(1).isChecked(), true);
        await page.keyboard.press('Enter');
        assert.equal(await panel.count(), 1, 'Enter on a radio must not send');
        await page.keyboard.press('n');
        await page.getByLabel('Notes for the agent').fill('Notes survive collapse');
        await page.keyboard.press('Escape');
        await page.getByRole('button',{name:'Edit notes · included'}).evaluate(e => { if (e !== document.activeElement) throw new Error('Escape must focus notes disclosure'); });
        await page.keyboard.press('Escape');
        await page.getByLabel('Notes for the agent').waitFor({state:'detached'});
        assert.equal(await page.getByLabel('Notes for the agent').count(), 0);
      }
      if (id === 'multi-select') {
        await page.getByRole('checkbox', { name: 'Other', exact: true }).check();
        const editor = page.getByLabel('Custom answer');
        await editor.click(); await editor.fill('Custom scope');
        assert.equal(await page.getByRole('checkbox', {name:'Other',exact:true}).isChecked(), true);
        await page.keyboard.press('Escape');
        assert.equal(await page.getByRole('checkbox',{name:'Other',exact:true}).evaluate(e=>e===document.activeElement),true);
        await page.getByRole('checkbox',{name:'Other',exact:true}).uncheck();
        assert.equal(await page.getByRole('checkbox',{name:'Other',exact:true}).evaluate(e=>e===document.activeElement),true);
        await page.getByRole('button',{name:'Edit custom answer'}).click();
        assert.equal(await page.getByLabel('Custom answer').inputValue(),'Custom scope');
      }
      if (id === 'multiple') {
        await page.getByRole('radio',{name:'Other',exact:true}).check();
        await page.getByLabel('Custom answer').fill('Retained across questions');
        await page.getByRole('button',{name:'Next',exact:true}).click();
        await page.getByRole('button',{name:'Back',exact:true}).click();
        assert.equal(await page.getByLabel('Custom answer').inputValue(),'Retained across questions');
      }
      if (id === 'preview' || id === 'product-layout') {
        const columns = await panel.locator('.question-preview-layout').evaluate(e=>getComputedStyle(e).gridTemplateColumns.split(' ').length);
        const width = await panel.evaluate(e=>e.clientWidth);
        assert.equal(columns, width >= 840 ? 2 : 1);
        const optionTop = await panel.locator('.question-choice-block').nth(1).evaluate(e=>e.offsetTop);
        await page.getByRole('radio',{name:'Include ancestors',exact:true}).check();
        assert.equal(await panel.locator('.question-choice-block').nth(1).evaluate(e=>e.offsetTop),optionTop,'Choice positions remain stable');
        assert.equal(await page.getByText('No preview for this option.').count(),1);
      }
      if (id === 'plain' && viewport.width >= 480) {
        await page.getByRole('button',{name:'Expand questions'}).click();
        const geometry = await panel.evaluate(e=>({bottom:e.getBoundingClientRect().bottom,footer:e.querySelector('.question-actions').getBoundingClientRect().bottom,height:e.getBoundingClientRect().height,capacity:parseFloat(e.style.getPropertyValue('--question-available-height'))}));
        assert.ok(Math.abs(geometry.height-geometry.capacity)<2, 'Short content expands to capacity');
        assert.ok(Math.abs(geometry.bottom-geometry.footer)<2, 'Expanded footer anchors to bottom');
      }
      await page.getByRole('button',{name:'Use chat instead',exact:true}).click();
      const dialog = page.getByRole('dialog');
      await dialog.waitFor();
      assert.equal(await dialog.evaluate(e=>e.matches(':modal')),true);
      const rect = await dialog.boundingBox();
      assert.ok(rect.x>=0 && rect.x+rect.width<=viewport.width+1 && rect.y>=0 && rect.y+rect.height<=viewport.height+1, 'Dialog fits viewport');
      await page.screenshot({path:path.join(outDir,`${suffix}--dismiss.png`)});
      await page.getByRole('button',{name:'Keep answering',exact:true}).click();
    }
    const overflow = await panel.evaluate(e=>({width:e.clientWidth,scroll:e.scrollWidth,rect:e.getBoundingClientRect().toJSON()}));
    assert.ok(overflow.scroll <= overflow.width+1, 'No horizontal panel overflow');
    assert.ok(overflow.rect.bottom<=viewport.height+1, 'Panel stays in viewport');
    await page.screenshot({path:path.join(outDir,`${suffix}.png`)});
    return true;
  },
});
