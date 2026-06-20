import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';

const port = Number(process.env.LADLE_PORT ?? 61123);
const baseUrl = process.env.LADLE_URL ?? `http://127.0.0.1:${port}`;
const outDir = path.resolve(process.env.GROUNDING_PANEL_QA_OUT ?? 'qa-artifacts/grounding-panel');
const stories = [
  ['full-dark', 'full-dark'],
  ['full-light', 'full-light'],
  ['empty-dark', 'empty-dark'],
  ['errors-dark', 'errors-dark'],
  ['collapsed-dark', 'collapsed-dark'],
  ['narrow-dark', 'narrow-dark'],
  ['skill-detail-dark', 'skill-detail-dark'],
  ['task-detail-dark', 'task-detail-dark'],
];

const expectedConsoleErrors = new Map([
  ['errors-dark', [
    'Failed to list tasks',
    'Failed to list skills',
    'Failed to fetch MCP status',
    'Failed to load work scope',
    'Failed to list files',
  ]],
]);

async function waitForLadle() {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(baseUrl);
      if (response.ok) return;
    } catch {
      // retry until Ladle finishes booting
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for Ladle at ${baseUrl}`);
}

async function main() {
  await mkdir(outDir, { recursive: true });
  const ladle = spawn('pnpm', ['exec', 'ladle', 'serve', '--port', String(port), '--host', '127.0.0.1'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: process.env,
  });
  ladle.stdout.on('data', (chunk) => process.stdout.write(chunk));
  ladle.stderr.on('data', (chunk) => process.stderr.write(chunk));

  const stopLadle = () => {
    if (!ladle.killed) ladle.kill('SIGTERM');
  };
  process.on('exit', stopLadle);
  process.on('SIGINT', () => { stopLadle(); process.exit(130); });
  process.on('SIGTERM', () => { stopLadle(); process.exit(143); });

  await waitForLadle();
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 960, height: 900 }, deviceScaleFactor: 1 });
  const consoleErrors = [];
  page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text());
  });
  page.on('pageerror', (error) => consoleErrors.push(error.message));

  try {
    for (const [story, name] of stories) {
      consoleErrors.length = 0;
      const url = `${baseUrl}/?story=grounding-panel--${story}`;
      await page.goto(url, { waitUntil: 'networkidle' });
      await page.waitForSelector(`[data-grounding-fixture-ready="${name}"]`, { timeout: 10_000 });
      await page.screenshot({ path: path.join(outDir, `${name}.png`), fullPage: true });
      const unexpectedErrors = consoleErrors.filter((error) => {
        const expected = expectedConsoleErrors.get(name) ?? [];
        return !expected.some((item) => error.includes(item));
      });
      if (unexpectedErrors.length > 0) {
        throw new Error(`Console errors while capturing ${name}:\n${unexpectedErrors.join('\n')}`);
      }
      console.log(`✓ captured ${name}`);
    }
  } finally {
    await browser.close();
    stopLadle();
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
