// Existing Ladle only: LADLE_URL=http://127.0.0.1:<port> node scripts/capture-composer-rejection.mjs
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';

assert.ok(process.env.LADLE_URL, 'LADLE_URL must name an already-running Ladle server');
const out = path.resolve(process.env.COMPOSER_REJECTION_QA_OUT_DIR ?? 'qa-artifacts/composer-rejection');
await mkdir(out, { recursive: true });
const browser = await chromium.launch();
const report = { coverage: 'Real ConnectedInputArea and DraftStore; synthetic deferred ExpansionError and file metadata. No upload, recording, image, or scope-switch coverage.', samples: [] };
try {
  for (const width of [375, 1280]) for (const theme of ['dark', 'light']) {
    const context = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: theme, hasTouch: width === 375,
      permissions: ['clipboard-read', 'clipboard-write'] });
    const sample = { width, theme, screenshots: [], errors: [] };
    report.samples.push(sample);
    try {
      const page = await context.newPage();
      page.on('pageerror', error => sample.errors.push(error.message));
      const url = new URL(process.env.LADLE_URL);
      url.searchParams.set('story', 'composer-rejection--deferred-rejection');
      url.searchParams.set('mode', 'preview');
      url.searchParams.set('fixtureTheme', theme);
      await page.goto(url.href);
      await page.waitForSelector('[data-testid="composer-rejection-fixture"][data-ready="true"]');
      assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), theme);
      await page.evaluate(() => document.fonts.ready);
      const input = page.getByRole('textbox');
      const original = await input.inputValue();
      assert.ok(original.startsWith('  Synthetic draft A\n') && original.endsWith('\n  '));
      assert.equal(await page.locator('.file-attachment-chip').count(), 1);
      await page.getByRole('button', { name: 'Send message', exact: true }).click();
      await page.waitForFunction(() => document.querySelector('textarea')?.value === '');
      assert.equal(await page.locator('.file-attachment-chip').count(), 0);
      const newer = 'Newer draft B — must survive rejection of A';
      await input.fill(newer);
      await page.getByRole('button', { name: 'Reject pending A', exact: true }).click();
      await page.getByText('Unsent message', { exact: true }).click();
      const unsent = page.locator('.input-unsent-message');
      assert.equal(await unsent.count(), 1);
      assert.equal(await input.inputValue(), newer);
      assert.equal(await unsent.locator('pre').textContent(), original);
      assert.match(await unsent.locator('li').textContent(), /synthetic-recovery-notes\.txt/);
      const restore = page.getByRole('button', { name: 'Restore unsent message', exact: true });
      assert.equal(await restore.isDisabled(), true);
      await unsent.getByRole('button', { name: 'Copy text', exact: true }).click();
      assert.equal(await page.evaluate(() => navigator.clipboard.readText()), original);
      sample.geometry = await page.evaluate(() => ({
        width: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth,
        recovery: Array.from(document.querySelectorAll('.input-unsent-message'), node => ({ client: node.clientWidth, scroll: node.scrollWidth })),
        buttons: Array.from(document.querySelectorAll('.input-unsent-actions button'), node => {
          const rect = node.getBoundingClientRect();
          return { name: node.textContent, x: rect.x, right: rect.right, width: rect.width, height: rect.height };
        }),
      }));
      const capture = async stage => {
        const filename = `${width}-${theme}-${stage}.png`;
        await page.screenshot({ path: path.join(out, filename), fullPage: true });
        sample.screenshots.push(filename);
      };
      await capture('retained-A-newer-B');
      assert.ok(sample.geometry.scrollWidth <= sample.geometry.width, 'No page horizontal overflow');
      for (const region of sample.geometry.recovery) assert.ok(region.scroll <= region.client, 'No recovery horizontal overflow');
      for (const button of sample.geometry.buttons) {
        assert.ok(button.x >= 0 && button.right <= width, 'Recovery action inside viewport');
        if (width === 375) assert.ok(button.width >= 44 && button.height >= 44, 'Mobile recovery target at least 44px');
      }
      await input.fill('');
      assert.equal(await restore.isEnabled(), true);
      await restore.click();
      assert.equal(await input.inputValue(), original);
      assert.equal(await unsent.count(), 0);
      assert.equal(await page.locator('.file-attachment-chip').count(), 1);
      assert.equal(await page.locator('.file-attachment-name').textContent(), 'synthetic-recovery-notes.txt');
      assert.equal(await page.getByTestId('composer-rejection-fixture').getAttribute('data-send-count'), '1');
      await capture('restored-A-once');
      assert.deepEqual(sample.errors, []);
      sample.passed = true;
    } catch (error) {
      sample.failure = String(error);
      throw error;
    } finally { await context.close(); }
  }
} finally {
  await writeFile(path.join(out, 'report.json'), JSON.stringify(report, null, 2));
  await browser.close();
}
console.log(`Composer rejection evidence: ${out}`);
