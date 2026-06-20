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
    'Failed to fetch task counts',
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
  // detached: `pnpm exec ladle serve` runs ladle/vite as a grandchild. Signalling
  // the pnpm wrapper alone leaves the vite server alive holding the inherited
  // stdio pipes open, which keeps this process's event loop from draining and
  // hangs the wrapper forever. Putting it in its own process group lets us signal
  // the whole tree by negating the pid.
  const ladle = spawn('pnpm', ['exec', 'ladle', 'serve', '--port', String(port), '--host', '127.0.0.1'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: process.env,
    detached: true,
  });
  ladle.stdout.on('data', (chunk) => process.stdout.write(chunk));
  ladle.stderr.on('data', (chunk) => process.stderr.write(chunk));

  let ladleStopped = false;
  const stopLadle = () => {
    if (ladleStopped || ladle.pid === undefined) return;
    ladleStopped = true;
    try {
      process.kill(-ladle.pid, 'SIGTERM');
    } catch {
      // group already gone
    }
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

main().then(
  () => {
    // Captures done and the ladle group signalled. Exit explicitly rather than
    // waiting for the event loop to drain: vite can take seconds to die, and its
    // lingering handles would otherwise stall the wrapper after work is complete.
    process.exit(0);
  },
  (error) => {
    console.error(error);
    process.exit(1);
  },
);
