import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { chromium } from 'playwright';

// Uses an already-running Ladle; never starts servers or installs browsers.
const outDir = path.resolve(process.env.SUBAGENT_TASK_DETAILS_QA_OUT ?? 'qa-artifacts/subagent-task-details');
const scenarios = ['ready', 'connecting', 'error', 'model-default', 'missing-effort', 'long-model', 'dock'];
const report = { browser: 'chromium', clipboard: 'writeText-only test double (not OS clipboard acceptance)', captures: [], errors: [] };
await mkdir(outDir, { recursive: true });
let browser;
try {
  assert.ok(process.env.LADLE_URL, 'LADLE_URL must identify an existing Ladle server');
  const base = new URL(process.env.LADLE_URL);
  assert.ok(['http:', 'https:'].includes(base.protocol), 'LADLE_URL must use HTTP(S)');
  if (!base.pathname.endsWith('/')) base.pathname += '/';
  const response = await fetch(new URL('meta.json', base), { signal: AbortSignal.timeout(15_000) });
  assert.ok(response.ok, `Ladle manifest returned ${response.status}`);
  const manifest = await response.json();
  const storyKeys = Object.keys(manifest.stories ?? {}).filter(key => key.startsWith('subagent-task-details--'));
  assert.deepEqual(storyKeys.map(key => key.slice('subagent-task-details--'.length)).sort(), [...scenarios].sort());
  browser = await chromium.launch({ timeout: 15_000 });
  for (const width of [375, 1280]) {
    for (const theme of ['dark', 'light']) {
      const context = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: theme });
      // No other browser/API mocking: only the permission-dependent clipboard sink.
      await context.addInitScript(() => {
        window.__subagentCopied = [];
        Object.defineProperty(navigator, 'clipboard', {
          configurable: true,
          value: { writeText: async text => { window.__subagentCopied.push(text); } },
        });
      });
      try {
        for (const scenario of scenarios) {
          const page = await context.newPage();
          page.setDefaultTimeout(10_000);
          page.setDefaultNavigationTimeout(15_000);
          const name = `${scenario}-${width}-${theme}`;
          const capture = { scenario, width, theme, screenshots: [], errors: [] };
          report.captures.push(capture);
          page.on('pageerror', error => capture.errors.push(`pageerror: ${error.message}`));
          page.on('console', message => {
            if (message.type() === 'error') capture.errors.push(`console: ${message.text()}`);
          });
          page.on('requestfailed', request => capture.errors.push(`requestfailed: ${request.url()} ${request.failure()?.errorText}`));
          page.on('response', res => {
            if (res.status() >= 400) capture.errors.push(`HTTP ${res.status()}: ${res.url()}`);
          });
          page.on('dialog', async dialog => {
            capture.errors.push(`Unexpected dialog: ${dialog.message()}`);
            await dialog.dismiss();
          });
          try {
            const url = new URL(base);
            url.search = '';
            url.hash = '';
            url.searchParams.set('story', `subagent-task-details--${scenario}`);
            url.searchParams.set('mode', 'preview');
            url.searchParams.set('fixtureTheme', theme);
            await page.goto(url.href, { waitUntil: 'networkidle' });
            const fixture = page.locator(`[data-subagent-task-fixture="${scenario}"]`);
            await fixture.waitFor({ state: 'visible' });
            await page.waitForFunction(expected => document.documentElement.dataset.theme === expected, theme);
            const task = await fixture.getAttribute('data-expected-task');
            assert.ok(task?.startsWith('  SYNTHETIC QA TASK'));
            const model = await fixture.getAttribute('data-expected-model');
            const effort = await fixture.getAttribute('data-expected-effort');
            assert.deepEqual(await fixture.locator('.subagent-task-execution > span').allTextContents(), [`Model: ${model}`, `Effort: ${effort}`]);
            const details = fixture.locator('details');
            const summary = details.locator('summary');
            assert.equal(await details.getAttribute('open'), null, 'task starts collapsed');
            const collapsed = `${name}-collapsed.png`;
            await page.screenshot({ path: path.join(outDir, collapsed), fullPage: true });
            capture.screenshots.push(collapsed);
            await summary.click();
            assert.notEqual(await details.getAttribute('open'), null, 'native summary expands task');
            const text = fixture.locator('.subagent-delegated-task-text');
            assert.equal(await text.textContent(), task, 'full exact task, including whitespace');
            assert.equal(await text.locator('*').count(), 0, 'HTML-looking task remains literal text');
            const copy = fixture.getByRole('button', { name: 'Copy delegated task', exact: true });
            await copy.click();
            await page.waitForFunction(() => window.__subagentCopied.length === 1);
            assert.deepEqual(await page.evaluate(() => window.__subagentCopied), [task], 'copy uses exact original task');
            assert.notEqual(await details.getAttribute('open'), null, 'copy leaves disclosure open');
            capture.geometry = await fixture.evaluate(root => {
              const container = root.querySelector('[data-task-container]');
              const bounds = container.getBoundingClientRect();
              const selectors = ['.subagent-task-execution', '.subagent-task-execution > span', '.subagent-delegated-task', '.subagent-delegated-task-text'];
              const boxes = selectors.flatMap(selector => [...root.querySelectorAll(selector)].map(element => {
                const rect = element.getBoundingClientRect();
                return { selector, left: rect.left, right: rect.right, width: rect.width, clientWidth: element.clientWidth, scrollWidth: element.scrollWidth };
              }));
              const text = root.querySelector('.subagent-delegated-task-text');
              const style = getComputedStyle(text);
              const range = document.createRange();
              range.selectNodeContents(text);
              const selection = window.getSelection();
              selection.removeAllRanges();
              selection.addRange(range);
              const selected = selection.toString();
              selection.removeAllRanges();
              text.scrollTop = text.scrollHeight;
              const reachableEnd = text.scrollHeight - text.clientHeight - text.scrollTop <= 1;
              text.scrollTop = 0;
              const targets = ['summary', '.copy-btn'].map(selector => {
                const rect = root.querySelector(selector).getBoundingClientRect();
                return { selector, width: rect.width, height: rect.height };
              });
              return { container: { left: bounds.left, right: bounds.right, width: bounds.width }, boxes, targets, selected, reachableEnd, userSelect: style.userSelect, whiteSpace: style.whiteSpace, overflowY: style.overflowY, textOverflow: style.textOverflow, lineClamp: style.webkitLineClamp, documentWidth: document.documentElement.scrollWidth, viewportWidth: innerWidth };
            });
            const geometry = capture.geometry;
            assert.equal(geometry.selected, task, 'all task text can be selected');
            assert.notEqual(geometry.userSelect, 'none');
            assert.ok(['pre-wrap', 'break-spaces'].includes(geometry.whiteSpace), 'preserve task whitespace');
            assert.ok(geometry.reachableEnd, 'last task line is reachable without truncation');
            assert.ok(['auto', 'scroll', 'visible'].includes(geometry.overflowY), 'no clipped task text');
            assert.notEqual(geometry.textOverflow, 'ellipsis');
            assert.ok(['none', '0'].includes(geometry.lineClamp), 'no line clamp');
            assert.ok(geometry.documentWidth <= geometry.viewportWidth + 1, 'no page horizontal overflow');
            for (const box of geometry.boxes) {
              assert.ok(box.left >= geometry.container.left - 1 && box.right <= geometry.container.right + 1, `${box.selector} stays inside container`);
              assert.ok(box.scrollWidth <= box.clientWidth + 1, `${box.selector} has no horizontal overflow`);
            }
            if (width === 375) {
              for (const target of geometry.targets) {
                assert.ok(target.width >= 44 && target.height >= 44, `${target.selector} mobile target must be at least 44×44; got ${target.width}×${target.height}`);
              }
            }
            const expanded = `${name}-expanded.png`;
            await page.screenshot({ path: path.join(outDir, expanded), fullPage: true });
            capture.screenshots.push(expanded);
            await summary.click();
            assert.equal(await details.getAttribute('open'), null, 'native summary collapses again');
          } catch (error) {
            capture.errors.push(error.stack ?? String(error));
            const failure = `${name}-failure.png`;
            try {
              await page.screenshot({ path: path.join(outDir, failure), fullPage: true, timeout: 5000 });
              capture.screenshots.push(failure);
            } catch (screenshotError) {
              capture.errors.push(`Failure screenshot: ${screenshotError.message}`);
            }
          } finally {
            await page.close();
          }
          capture.passed = capture.errors.length === 0;
          await writeFile(path.join(outDir, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
        }
      } finally {
        await context.close();
      }
    }
  }
} catch (error) {
  report.errors.push(error.stack ?? String(error));
} finally {
  await browser?.close();
  report.passed = report.errors.length === 0 && report.captures.length === scenarios.length * 4 && report.captures.every(capture => capture.passed);
  await writeFile(path.join(outDir, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
}
console.log(`Sub-agent task details QA: ${report.passed ? 'PASS' : 'FAIL'}; ${path.join(outDir, 'report.json')}`);
if (!report.passed) process.exitCode = 1;
