import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';

/**
 * Shared Ladle screenshot-capture engine for the `./dev.py qa <surface>`
 * workflows. A surface (grounding-panel, meta-viewer, …) supplies a small
 * config; everything else — booting Ladle, discovering the story set from
 * Ladle's own manifest, waiting on each fixture's settled-DOM marker, failing
 * on unexpected console errors, and tearing the server down cleanly — is shared
 * here. Adding a surface is "add a config", not "copy the engine".
 *
 * @typedef {Object} SurfaceConfig
 * @property {string} surface              Surface name (used in logs + default out dir).
 * @property {string} [storyPrefix]        Ladle story-key prefix; defaults to `${surface}--`.
 * @property {string} readyAttribute       data-* attribute the fixture sets to the scenario id when settled.
 * @property {string} outDir               Output directory for PNGs (resolved against cwd).
 * @property {{width:number,height:number}} [viewport]   Capture viewport; defaults to 960x900.
 * @property {Map<string,string[]>} [expectedConsoleErrors]  scenario id → console-error substrings to tolerate.
 */

const port = Number(process.env.LADLE_PORT ?? 61123);
const baseUrl = process.env.LADLE_URL ?? `http://127.0.0.1:${port}`;

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

// Derive the capture set from Ladle's own story manifest so it tracks the
// stories (and through them the shared scenarios) as the single source of
// truth — a scenario added or removed can't silently fall out of capture.
async function discoverStories(storyPrefix) {
  const response = await fetch(`${baseUrl}/meta.json`);
  if (!response.ok) {
    throw new Error(`Could not fetch Ladle story manifest at ${baseUrl}/meta.json (${response.status})`);
  }
  const meta = await response.json();
  const stories = Object.keys(meta.stories ?? {})
    .filter((key) => key.startsWith(storyPrefix))
    .map((storyKey) => ({ storyKey, id: storyKey.slice(storyPrefix.length) }))
    .sort((a, b) => a.id.localeCompare(b.id));
  if (stories.length === 0) {
    throw new Error(`No '${storyPrefix}*' stories in Ladle manifest — stories missing?`);
  }
  return stories;
}

/**
 * Run the capture for one surface. Resolves on success; rejects on any failure.
 * Owns the Ladle process lifecycle (its own process group, so the whole
 * vite/ladle tree is signalled on teardown — signalling the pnpm wrapper alone
 * leaves vite holding the inherited stdio pipes open and hangs forever).
 *
 * @param {SurfaceConfig} config
 */
export async function captureSurface(config) {
  const {
    surface,
    storyPrefix = `${surface}--`,
    readyAttribute,
    outDir,
    viewport = { width: 960, height: 900 },
    expectedConsoleErrors = new Map(),
  } = config;
  const resolvedOut = path.resolve(outDir);
  await mkdir(resolvedOut, { recursive: true });

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
  const stories = await discoverStories(storyPrefix);
  console.log(`Capturing ${stories.length} ${surface} stories`);
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport, deviceScaleFactor: 1 });
  const consoleErrors = [];
  page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text());
  });
  page.on('pageerror', (error) => consoleErrors.push(error.message));

  try {
    for (const { storyKey, id } of stories) {
      consoleErrors.length = 0;
      const url = `${baseUrl}/?story=${storyKey}`;
      await page.goto(url, { waitUntil: 'networkidle' });
      await page.waitForSelector(`[${readyAttribute}="${id}"]`, { timeout: 10_000 });
      await page.screenshot({ path: path.join(resolvedOut, `${id}.png`), fullPage: true });
      const unexpectedErrors = consoleErrors.filter((error) => {
        const expected = expectedConsoleErrors.get(id) ?? [];
        return !expected.some((item) => error.includes(item));
      });
      if (unexpectedErrors.length > 0) {
        throw new Error(`Console errors while capturing ${id}:\n${unexpectedErrors.join('\n')}`);
      }
      console.log(`✓ captured ${id}`);
    }
  } finally {
    await browser.close();
    stopLadle();
  }
}

/**
 * Entry-point wrapper for a per-surface capture script: runs the capture and
 * exits with the right code. Exits explicitly rather than draining the event
 * loop — vite can take seconds to die, and its lingering handles would
 * otherwise stall the process after work is complete.
 *
 * @param {SurfaceConfig} config
 */
export function runSurfaceCapture(config) {
  captureSurface(config).then(
    () => process.exit(0),
    (error) => {
      console.error(error);
      process.exit(1);
    },
  );
}
