import { createRequire } from 'node:module';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';
import { functionalPass, ProbeError, runProbe, safeJSON } from './context-close-probe.mjs';

// This entry point is hosted-only. Importing it does not resolve Playwright or launch anything.
export async function main() {
  const require = createRequire(import.meta.url);
  const pkgPath = require.resolve('playwright/package.json');
  const pkg = JSON.parse(await readFile(pkgPath, 'utf8'));
  const corePath = createRequire(pkgPath).resolve('playwright-core/package.json');
  const core = JSON.parse(await readFile(corePath, 'utf8'));
  const manifest = JSON.parse(await readFile(join(dirname(corePath), 'browsers.json'), 'utf8'));
  if (pkg.version !== '1.61.0' || core.version !== '1.61.0') throw new ProbeError('invalid_payload');
  const provenance = {
    node: process.version, playwright: pkg.version,
    lockSha256: createHash('sha256').update(await readFile(new URL('../pnpm-lock.yaml', import.meta.url))).digest('hex'),
    browserRevision: manifest.browsers.find(b => b.name === 'chromium')?.revision,
    sourceCommit: process.env.PROBE_SOURCE_COMMIT,
  };
  const initial = { version: 1, runId: randomUUID(), provenance, cases: [], suiteError: null, cleanupErrors: [] };
  safeJSON(initial); // Reject unexpected provenance before starting a browser.
  const { chromium } = require('playwright');
  const result = await runProbe({
    createBrowser: () => chromium.launch({ headless: true, timeout: 10000 }),
    provenance, runId: initial.runId, suiteMs: 48000,
  });
  await mkdir('context-close-evidence', { recursive: true });
  await writeFile('context-close-evidence/result.json', safeJSON(result), { flag: 'wx' });
  return functionalPass(result) ? 0 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  // Do not print exception messages/stacks; the result is the only runtime artifact.
  main().then(code => process.exit(code), () => process.exit(1));
}
