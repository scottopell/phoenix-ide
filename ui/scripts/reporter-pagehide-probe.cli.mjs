import { createRequire } from 'node:module';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { APP_PIN, SOURCE_PINS, MODULE_PINS, pinnedHashes, sha256, runSuite, functionalPass } from './reporter-pagehide-probe.mjs';

// Hosted-only. Importing this entry does not load Vite/Playwright or build/launch/listen.
async function prepare(setPhase) {
  const require = createRequire(import.meta.url), root = fileURLToPath(new URL('../../', import.meta.url));
  const pkgPath = require.resolve('playwright/package.json');
  const pkg = JSON.parse(await readFile(pkgPath, 'utf8'));
  const corePath = createRequire(pkgPath).resolve('playwright-core/package.json');
  const core = JSON.parse(await readFile(corePath, 'utf8'));
  const manifest = JSON.parse(await readFile(join(dirname(corePath), 'browsers.json'), 'utf8'));
  if (pkg.version !== '1.61.0' || core.version !== '1.61.0') throw Error('unsupported pinned Playwright');
  const sourceHashes = pinnedHashes(SOURCE_PINS), moduleHashes = pinnedHashes(MODULE_PINS);
  setPhase('setup_bundle');
  const { build } = await import('vite');
  const built = await build({ configFile: resolve(root, 'ui/vite.reporter-pagehide.config.ts'), logLevel: 'silent' });
  const outputs = Array.isArray(built) ? built : [built];
  const chunks = outputs.flatMap(output => output.output);
  if (chunks.length !== 1 || chunks[0].type !== 'chunk') throw Error('isolated single bundle required');
  const chunk = chunks[0];
  const expected = [...Object.keys(SOURCE_PINS), 'ui/qualification/reporter-pagehide.entry.ts'].map(path => resolve(root, path)).sort();
  if (JSON.stringify(Object.keys(chunk.modules).sort()) !== JSON.stringify(expected)) throw Error('unexpected bundle source inventory');
  const provenance = { node: process.version, playwright: pkg.version,
    chromiumRevision: manifest.browsers.find(b => b.name === 'chromium')?.revision, chromiumVersion: null,
    sourceCommit: process.env.PROBE_SOURCE_COMMIT, appPin: APP_PIN,
    lockSha256: sha256(await readFile(resolve(root, 'ui/pnpm-lock.yaml'))), bundleSha256: sha256(chunk.code), sourceHashes, moduleHashes };
  return { provenance, bundle: chunk.code, chromium: require('playwright').chromium };
}
export async function main(overrides = {}) {
  const result = await runSuite({ prepare,
    checkPins: () => { pinnedHashes(SOURCE_PINS); pinnedHashes(MODULE_PINS); },
    launch: prepared => prepared.chromium.launch({ headless: true, timeout: 10000 }),
    publish: async json => {
      await mkdir('reporter-pagehide-evidence', { recursive: true });
      await writeFile('reporter-pagehide-evidence/result.json', json, { flag: 'wx' });
    }, ...overrides });
  return functionalPass(result) ? 0 : 1;
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  // Let any owned late-acquisition cleanup settle; the hosted process fence remains authoritative.
  main().then(code => { process.exitCode = code; }, () => { process.exitCode = 1; });
}
