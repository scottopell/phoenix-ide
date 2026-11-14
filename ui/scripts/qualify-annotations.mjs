import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

assert.ok(process.env.LADLE_URL, 'LADLE_URL must name an existing synthetic Ladle server');
const out = path.resolve('qa-artifacts/annotation-review');
await mkdir(out, { recursive: true });
const browser = await chromium.launch();
const report = { coverage: 'Production components in synthetic Ladle fixtures; Chromium, not physical iOS. No production data.', samples: [] };
try {
  for (const width of [390, 1440]) for (const theme of ['light', 'dark']) {
    const context = await browser.newContext({ viewport: { width, height: width === 390 ? 844 : 900 }, hasTouch: width === 390, colorScheme: theme });
    const page = await context.newPage();
    try {
      for (const state of ['01-empty', '02-selected', '03-writing', '04-discard', '05-one-annotation', '06-several-annotations', '07-long-comment', '08-sending', '09-failed-send', '10-sent', '11-two-annotations', '12-editing', '13-after-delete', '14-many-annotations']) {
        const sample = { width, theme, state, errors: [] };
        report.samples.push(sample);
        const onError = error => sample.errors.push(error.message);
        page.on('pageerror', onError);
        const url = new URL(process.env.LADLE_URL);
        url.searchParams.set('story', `annotations--${state}`);
        url.searchParams.set('mode', 'preview');
        url.searchParams.set('fixtureTheme', theme);
        await page.goto(url.href);
        await page.waitForSelector(`html[data-annotation-design-ready="${state}"]`);
        sample.geometry = await page.evaluate(() => ({
          width: innerWidth, scrollWidth: document.documentElement.scrollWidth,
          cards: document.querySelectorAll('.draft-annotation').length,
          highlights: CSS.highlights?.get('phoenix-pending-annotations')?.size ?? 0,
          quotes: [...(CSS.highlights?.get('phoenix-pending-annotations') ?? [])].map(range => range.toString()),
          composer: document.querySelector('#input-area').getBoundingClientRect().toJSON(),
        }));
        assert.ok(sample.geometry.scrollWidth <= width, JSON.stringify(sample.geometry));
        assert.ok(sample.geometry.composer.height > 0);
        if (state === '10-sent') assert.equal(sample.geometry.cards, 0);
        sample.screenshot = `${state}-${width}-${theme}.png`;
        await page.screenshot({ path: path.join(out, sample.screenshot) });
        if (state === '06-several-annotations') {
          const trigger = page.getByRole('button', { name: 'Edit annotation 2', exact: true });
          await trigger.click();
          const comment = page.getByRole('textbox', { name: 'Annotation 2 comment', exact: true });
          await comment.fill('<script>not executed</script> & "quoted" synthetic comment');
          await page.keyboard.press('Escape');
          await comment.waitFor({ state: 'hidden' });
          await assert.equal(await trigger.evaluate(el => el === document.activeElement), true);
          await trigger.click();
          assert.equal(await comment.inputValue(), '<script>not executed</script> & "quoted" synthetic comment');
          await page.screenshot({ path: path.join(out, `edited-${width}-${theme}.png`) });
          await page.getByRole('button', { name: 'Delete annotation', exact: true }).click();
          await trigger.waitFor({ state: 'hidden' });
          assert.equal(await page.getByRole('button', { name: 'Edit annotation 1', exact: true }).count(), 1);
          assert.equal(await page.getByRole('button', { name: 'Edit annotation 3', exact: true }).count(), 1);
          assert.equal(await page.getByRole('button', { name: 'Edit annotation 3', exact: true }).evaluate(el => el === document.activeElement), true);
          sample.editDeleteFocus = 'passed';
        }
        assert.deepEqual(sample.errors, []);
        page.off('pageerror', onError);
        console.log(`PASS ${state} ${width} ${theme}`);
      }
    } finally { await context.close(); }
  }
} finally {
  await writeFile(path.join(out, 'report.json'), JSON.stringify(report, null, 2));
  await browser.close();
}
