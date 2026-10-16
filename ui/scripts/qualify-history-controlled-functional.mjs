import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { prepareJointTelemetry } from './history-joint-telemetry.mjs';
import { runControlledFunctional } from './history-controlled-functional.mjs';

export async function verifyServedArtifact(baseURL, receipt, fetcher = fetch) {
  const deadline = performance.now() + 15000;
  assert.equal(receipt.evidence_class, 'controlled_functional');
  assert.equal(receipt.sample_rate, 1);
  assert.ok(receipt.ui_files && Object.keys(receipt.ui_files).length > 0 && Object.keys(receipt.ui_files).length <= 10000);
  for (const [relative, expected] of Object.entries(receipt.ui_files)) {
    assert.ok(!relative.startsWith('/') && !relative.split('/').some(p => p === '..' || p === '.') && !relative.includes('\\'));
    assert.match(expected, /^[a-f0-9]{64}$/);
    const url = new URL(relative, new URL('/', baseURL));
    assert.equal(url.origin, new URL(baseURL).origin);
    const left = Math.floor(deadline - performance.now());
    assert.ok(left > 0, 'served asset verification absolute deadline');
    const response = await fetcher(url, { signal: AbortSignal.timeout(Math.min(5000, left)), redirect: 'error' });
    assert.equal(response.status, 200);
    const chunks = []; let count = 0;
    if (response.body) {
      for await (const chunk of response.body) {
        count += chunk.length;
        assert.ok(count <= 32 * 1024 * 1024 && performance.now() < deadline, 'served asset exceeds byte/deadline bound');
        chunks.push(chunk);
      }
    } else {
      const chunk = Buffer.from(await response.arrayBuffer()); count = chunk.length;
      assert.ok(count <= 32 * 1024 * 1024); chunks.push(chunk);
    }
    const bytes = Buffer.concat(chunks);
    assert.equal(createHash('sha256').update(bytes).digest('hex'), expected, 'served controlled artifact bytes must match admitted build');
  }
}

async function boundedOperation(operation, milliseconds) {
  let timer;
  try {
    return await Promise.race([operation(), new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error('controlled lifecycle deadline')), milliseconds);
    })]);
  } finally { clearTimeout(timer); }
}

export async function captureOwnedBrowser({ launch, capture, negative, publish, timeoutMs = 205000, closeMs = 4000 }) {
  let browser, result, primary, canceled = false;
  const failed = () => ({ evidence_class: 'controlled_functional', status: 'failed',
    failure: 'controlled_driver_failed', overhead_clearance: false, full_acceptance: false });
  try {
    await boundedOperation(async () => {
      browser = await launch();
      if (canceled) { void browser.close().catch(() => {}); return; }
      result = await capture(browser);
      if (canceled) return;
      await negative(browser, result);
    }, timeoutMs);
  } catch (error) {
    primary = error;
    result = error.controlledReceipt ?? result ?? failed();
    result = { ...result, status: 'failed', failure: 'controlled_driver_failed' };
  } finally {
    canceled = true;
    result ??= failed();
    // Retain capture evidence before attempting a possibly stalled browser shutdown.
    publish(result);
    try { if (browser) await boundedOperation(() => browser.close(), closeMs); }
    catch {
      result.status = 'failed'; result.cleanup_failure = 'controlled_browser_close_failed';
    }
    publish(result);
  }
  if (primary || result.cleanup_failure) throw new Error('controlled functional qualification failed; see retained sanitized receipt');
  return result;
}

async function main() {
  const app = resolve(process.env.APP_ROOT), baseURL = process.env.HISTORY_BASE_URL;
  assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(new URL(baseURL).hostname));
  const admission = JSON.parse(readFileSync(process.env.HISTORY_CONTROLLED_ADMISSION, 'utf8'));
  assert.equal(admission.evidence_class, 'controlled_functional');
  const config = await prepareJointTelemetry({ env: process.env, appRoot: app });
  assert.ok(config && !config.unitOnly);
  assert.equal(admission.source_sha, config.input.fixtures.identity.appSource);
  await verifyServedArtifact(baseURL, admission);
  const module = await import(pathToFileURL(config.modulePath).href);
  const require = createRequire(resolve(app, 'ui/package.json'));
  const { chromium } = require('@playwright/test');
  await captureOwnedBrowser({
    launch: () => chromium.launch({ headless: true, timeout: 10000 }),
    capture: async browser => {
      const result = await runControlledFunctional({ browser, baseURL, fixtures: config.input.fixtures,
        config: { ...config, ...config.input, evidence_class: 'controlled_functional' }, module,
        historyTargets: admission.history_targets });
      result.artifact = { evidence_class: 'controlled_functional', manifest_sha256: admission.manifest_sha256,
        source_sha: admission.source_sha, source_tree: admission.source_tree };
      return result;
    },
    negative: async (browser, result) => {
      const context = await browser.newContext();
      let primary;
      try {
        const response = await context.request.post(new URL('/api/telemetry/conversation-view', baseURL).href,
          { data: result.trials[0].reports[0], timeout: 5000 });
        assert.equal(response.status(), 401, 'unauthenticated controlled POST must not be accepted');
        result.auth_negative = { attempts: 1, response_status: 401, expected_acceptances: 0 };
      } catch (error) { primary = error; throw error; }
      finally {
        try { await boundedOperation(() => context.close(), 2000); }
        catch { result.auth_negative_cleanup_failure = 'controlled_negative_context_close_failed';
          if (!primary) throw new Error('controlled negative context cleanup failed'); }
      }
    },
    publish: result => writeFileSync(resolve(process.env.HISTORY_OUTPUT, 'browser-result.json'), JSON.stringify(result, null, 2)),
  });
}
if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url)
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
