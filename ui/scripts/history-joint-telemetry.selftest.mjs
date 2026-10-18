import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, readFileSync, symlinkSync, realpathSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { EventEmitter } from 'node:events';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { runInNewContext } from 'node:vm';
import { assertBackgroundReport } from './qualify-view-telemetry.mjs';
import { APP_PIN, TELEMETRY_MODULE_SHA256, TELEMETRY_DIAGNOSTICS_SHA256, TELEMETRY_DIAGNOSTICS_RELATIVE_MODULE, UNIT_ONLY,
  prepareJointTelemetry, prepareJointFailureDiagnostic, validateJointFixtures, validateTelemetryResult, runJointTelemetryStage, authenticateOwnedContext,
  installObservedHistoryTelemetry, installHistoryRequestOwnership, installHistoryPaginationDiagnostic, appendObservedHistory } from './history-joint-telemetry.mjs';

// All positive fake browser/module/fixture values in this file are UNIT ONLY, not runtime proof.
function input() {
  return { runId: 'unit-run', serviceName: 'phoenix-view-qualification-unit-run', fixtures: {
    identity: { appSource: APP_PIN, frontendSource: APP_PIN, serverInstanceId: '12345678-1234-4234-8234-123456789abc' },
    aggregates: [
      { kind: 'ordinary', product_conversation_id: 'ordinary-product', route: '/c/ordinary-product', endpoint: '/api/product-conversations/ordinary-product', members: ['ordinary-root', 'ordinary-latest'], latest_transcript_row_id: 'ordinary-latest' },
      { kind: 'global', product_conversation_id: 'global-product', route: '/global/global-latest', endpoint: '/api/global/coordinator/history/global-root', members: ['global-root', 'global-latest'], latest_transcript_row_id: 'global-latest' },
    ], lifecycle: { route: '/c/ordinary-product', productConversationId: 'ordinary-product', spaControl: { kind: 'sidebar_home' } }, forbidden: [],
  } };
}
function stock() {
  const report = { open_id: 'unit-ready', outcome: 'ready', anchor_result: 'tail_pinned', marks: {
    meaningful_transcript: 1, breadcrumb_presented: 2, breadcrumb_enabled: 3, anchor_settled: 4 } };
  return { acceptance: 'partial_initial_terminal_and_lifecycle_export_raw_overhead_only', comparison: { overhead_clearance: false },
    trials: [{ reports: [report] }], journeys: [{ open_id: report.open_id, expected: {
      'browser.outcome': report.outcome, ...Object.fromEntries(Object.entries(report.marks).map(([k, v]) => [`browser.${k}_ms`, v])) } }] };
}
function unitFiles() {
  const root = realpathSync(mkdtempSync(resolve(tmpdir(), 'joint-unit-')));
  mkdirSync(resolve(root, 'ui/scripts'), { recursive: true });
  const module = resolve(root, 'ui/scripts/qualify-view-telemetry.mjs'), fixture = resolve(root, 'fixture.json');
  const diagnostics = resolve(root, TELEMETRY_DIAGNOSTICS_RELATIVE_MODULE);
  writeFileSync(module, 'UNIT_ONLY_not_the_donor_module'); writeFileSync(diagnostics, 'UNIT_ONLY_not_the_diagnostics_helper'); writeFileSync(fixture, JSON.stringify(input()));
  const env = { HISTORY_TELEMETRY_JOINT: '1', HISTORY_TELEMETRY_FIXTURE: fixture,
    HISTORY_TELEMETRY_MODULE: module, HISTORY_TELEMETRY_MODULE_SHA256: TELEMETRY_MODULE_SHA256 };
  const UNIT = { label: UNIT_ONLY, digest: path => path === diagnostics ? TELEMETRY_DIAGNOSTICS_SHA256 : TELEMETRY_MODULE_SHA256 };
  return { root, module, diagnostics, fixture, env, UNIT, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

test('owned browser login mints session before use, disposes responses, and never returns password (UNIT ONLY)', async () => {
  const f = unitFiles(); try {
    const path = resolve(f.root, 'qualification-password'), password = '12345678-1234-4234-8234-123456789abc'; writeFileSync(path, password, { mode: 0o600 });
    const calls = [], disposed = [];
    const context = { request: {
      post: async (url, options) => { calls.push(['login', url, options.data.password]); return { status: () => 200, dispose: async () => disposed.push('login') }; },
      get: async url => { calls.push(['status', url]); return { status: () => 200, json: async () => ({ auth_required: true, authenticated: true }), dispose: async () => disposed.push('status') }; },
    }, cookies: async () => [{ name: 'phoenix-auth', value: 'opaque_session' }] };
    const env = { HISTORY_AUTH_HOME: f.root, HISTORY_AUTH_CREDENTIAL_FILE: path };
    const cookie = await authenticateOwnedContext(context, 'http://127.0.0.1:12345', env);
    assert.equal(cookie, 'phoenix-auth=opaque_session'); assert.ok(!cookie.includes(password));
    assert.deepEqual(disposed, ['login', 'status']); assert.equal(calls[0][2], password);
    assert.deepEqual(calls.map(c => c[0]), ['login', 'status']);
    chmodSync(path, 0o644);
    await assert.rejects(authenticateOwnedContext(context, 'http://127.0.0.1:12345', env), /credential file ownership/);
    chmodSync(path, 0o600); rmSync(path); symlinkSync(f.fixture, path);
    await assert.rejects(authenticateOwnedContext(context, 'http://127.0.0.1:12345', env), /credential file ownership/);
    assert.equal(calls.length, 2);
  } finally { f.cleanup(); }
});

test('browser login rejects auth-disabled/wrong booleans/private JSON and sanitizes login transport (UNIT ONLY)', async () => {
  const f = unitFiles(); try {
    const path = resolve(f.root, 'qualification-password'), password = '22345678-1234-4234-8234-123456789abc'; writeFileSync(path, password, { mode: 0o600 });
    const env = { HISTORY_AUTH_HOME: f.root, HISTORY_AUTH_CREDENTIAL_FILE: path };
    for (const value of [{ auth_required: false, authenticated: true }, { auth_required: 1, authenticated: true }, { auth_required: true, authenticated: 1 }, 'invalid']) {
      let disposed = 0;
      const context = { request: {
        post: async () => ({ status: () => 200, dispose: async () => disposed++ }),
        get: async () => ({ status: () => 200, json: async () => { if (value === 'invalid') throw new Error(password); return value; }, dispose: async () => disposed++ }),
      } };
      await assert.rejects(authenticateOwnedContext(context, 'http://127.0.0.1:12345', env), error => !String(error).includes(password));
      assert.equal(disposed, 2);
    }
    await assert.rejects(authenticateOwnedContext({ request: { post: async () => { throw new Error(password); } } }, 'http://127.0.0.1:12345', env), error => !String(error).includes(password));
    const privateSession = 'SYNTHETIC_SESSION_SENTINEL';
    await assert.rejects(authenticateOwnedContext({ request: {
      post: async () => ({ status: () => 200, dispose: async () => {} }),
      get: async () => { throw new Error(`apiRequestContext.get: timeout\nCall log:\n - cookie: phoenix-auth=${privateSession}`); },
    } }, 'http://127.0.0.1:12345', env), error => error.message === 'owned browser authentication status transport failed' && !String(error.stack).includes(privateSession));
    await assert.rejects(authenticateOwnedContext({ request: {
      post: async () => ({ status: () => 200, dispose: async () => {} }),
      get: async () => ({ status: () => 401, dispose: async () => { throw new Error(privateSession); } }),
    } }, 'http://127.0.0.1:12345', env), error => /owned browser authentication status/.test(error.message)
      && error.owned_auth_cleanup_failure === 'owned authentication response disposal failed'
      && !String(error.stack).includes(privateSession));
  } finally { f.cleanup(); }
});

test('disabled stage does not admit imports or contexts (UNIT ONLY)', async () => {
  assert.equal(prepareJointTelemetry({ env: {} }), null);
  assert.equal(await runJointTelemetryStage({ config: null, browser: null }), null);
});
test('UNIT ONLY complete fixture and preparation carry no runtime credit', () => {
  assert.equal(validateJointFixtures(input()).runId, 'unit-run');
  const f = unitFiles(); try { assert.equal(prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT }).unitOnly, true); } finally { f.cleanup(); }
});
for (const key of ['HISTORY_TELEMETRY_JOINT', 'HISTORY_TELEMETRY_FIXTURE', 'HISTORY_TELEMETRY_MODULE', 'HISTORY_TELEMETRY_MODULE_SHA256'])
  test(`partial opt-in rejects missing ${key} (UNIT ONLY)`, () => {
    const f = unitFiles(); try { delete f.env[key]; assert.throws(() => prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT })); } finally { f.cleanup(); }
  });
for (const [name, mutate] of Object.entries({
  mode: f => { f.env.HISTORY_TELEMETRY_JOINT = 'true'; },
  foreign_module: f => { f.env.HISTORY_TELEMETRY_MODULE = resolve(f.root, 'foreign.mjs'); },
  expected_hash: f => { f.env.HISTORY_TELEMETRY_MODULE_SHA256 = '0'.repeat(64); },
  actual_hash: f => { f.UNIT.digest = () => '0'.repeat(64); },
  symbolic_module: f => { rmSync(f.module); symlinkSync(f.fixture, f.module); },
  missing_diagnostics: f => { rmSync(f.diagnostics); },
  symbolic_diagnostics: f => { rmSync(f.diagnostics); symlinkSync(f.fixture, f.diagnostics); },
  changed_diagnostics: f => { f.UNIT.digest = path => path === f.diagnostics ? '0'.repeat(64) : TELEMETRY_MODULE_SHA256; },
  app_pin: f => { const i = input(); i.fixtures.identity.appSource = '0'.repeat(40); writeFileSync(f.fixture, JSON.stringify(i)); },
})) test(`pre-import guard rejects ${name} (UNIT ONLY)`, () => {
  const f = unitFiles(); try { mutate(f); assert.throws(() => prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT })); } finally { f.cleanup(); }
});
for (const [name, mutate] of Object.entries({
  global_product_equals_root: i => { i.fixtures.aggregates[1].product_conversation_id = 'global-root'; },
  global_foreign_route: i => { i.fixtures.aggregates[1].route = '/global/foreign'; },
  foreign_origin: i => { i.fixtures.aggregates[0].route = 'http://evil.test/c/ordinary-product'; },
  lifecycle_foreign_product: i => { i.fixtures.lifecycle.productConversationId = 'foreign'; },
  lifecycle_generic_link: i => { i.fixtures.lifecycle.spaControl = { kind: 'anchor', selector: 'a' }; },
  repeated_members: i => { i.fixtures.aggregates[1].members[0] = 'global-latest'; },
})) test(`actual-fixture shape rejects ${name} (UNIT ONLY)`, () => { const i = input(); mutate(i); assert.throws(() => validateJointFixtures(i)); });
for (const [name, mutate] of Object.entries({
  bad_anchor: r => { r.trials[0].reports[0].anchor_result = 'target_missing'; },
  absent_anchor: r => { r.trials[0].reports[0].anchor_result = null; },
  missing_mark: r => { delete r.trials[0].reports[0].marks.anchor_settled; },
  stale_export_mark: r => { r.journeys[0].expected['browser.anchor_settled_ms'] = 99; },
  invented_clearance: r => { r.comparison.overhead_clearance = true; },
  invented_acceptance: r => { r.acceptance = 'qualified'; },
  missing_export: r => { r.journeys = []; },
})) test(`current owning-report contract rejects ${name} (UNIT ONLY)`, () => { const r = stock(); mutate(r); assert.throws(() => validateTelemetryResult(r)); });
test('UNIT ONLY stage owns a fresh context and closes it without promoting partial scope', async () => {
  const f = unitFiles(); const calls = []; try {
    const config = prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT });
    const context = { newPage: async () => ({ unit: true }), close: async () => calls.push('close') };
    const result = await runJointTelemetryStage({ config, browser: { newContext: async () => { calls.push('fresh'); return context; } },
      baseURL: 'http://127.0.0.1:12345', outputDir: f.root, harnessPath: import.meta.filename,
      historyPhysical: { count: 1, visible: true, persistedMessageID: 'unit-exact' }, historyGlobalOwnedGetCount: 0,
      UNIT: { label: UNIT_ONLY, importModule: async () => ({ runTelemetryChecks: async () => stock() }) } });
    assert.deepEqual(calls, ['fresh', 'close']); assert.equal(result.receipt.classification.overhead_clearance, false);
    assert.match(result.receipt.scope.telemetry, /not telemetry lifecycle physical-target proof/);
  } finally { f.cleanup(); }
});
for (const phase of ['runtime_import', 'export_admission', 'context_acquisition'])
  test(`UNIT ONLY ${phase} failure retains before/after manifest without claiming checks executed`, async () => {
    const f = unitFiles(); try {
      const config = prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT });
      const primary = new Error('admission failure'); let checks = 0;
      const load = async () => {
        if (phase === 'runtime_import') throw primary;
        if (phase === 'export_admission') return {};
        return { runTelemetryChecks: async () => { checks++; } };
      };
      const browser = { newContext: async () => { throw primary; } };
      await assert.rejects(runJointTelemetryStage({ config, browser,
        baseURL: 'http://127.0.0.1:12345', outputDir: f.root, harnessPath: import.meta.filename,
        historyPhysical: { count: 1, visible: true, persistedMessageID: 'unit-exact' },
        UNIT: { label: UNIT_ONLY, importModule: load } }),
      error => phase === 'export_admission' ? /export required/.test(error.message) : error === primary);
      const manifest = JSON.parse(readFileSync(resolve(f.root, 'joint-telemetry-dependency-manifest.json'), 'utf8'));
      assert.equal(manifest.failure_phase, phase); assert.equal(manifest.context_acquired, false);
      assert.deepEqual(manifest.before, manifest.after); assert.equal(checks, 0);
      assert.equal(manifest.before[f.diagnostics], TELEMETRY_DIAGNOSTICS_SHA256);
    } finally { f.cleanup(); }
  });
function pinnedDiagnosticConfig() {
  const modulePath = fileURLToPath(new URL('./qualify-view-telemetry.mjs', import.meta.url));
  const helper = fileURLToPath(new URL('./view-telemetry-diagnostics.mjs', import.meta.url));
  return { modulePath, dependencies: { [modulePath]: TELEMETRY_MODULE_SHA256, [helper]: TELEMETRY_DIAGNOSTICS_SHA256 } };
}
function pinnedAssertion() {
  try { assertBackgroundReport({}, {}); } catch (error) { return error; }
  assert.fail('UNIT ONLY pure pinned assertion must fail');
}
function syntheticAssertion(message, frames) {
  const error = new assert.AssertionError({ message });
  error.stack = `AssertionError [ERR_ASSERTION]: ${message}\n${frames.join('\n')}`;
  return error;
}

test('offline native Node assertion resolves exact pinned source token, not its message', () => {
  const diagnose = prepareJointFailureDiagnostic(pinnedDiagnosticConfig());
  assert.deepEqual(diagnose(pinnedAssertion(), 'module_checks'), {
    schema_version: 1, phase: 'module_checks', error_code: 'ERR_ASSERTION', resolution: 'verified_assertion_site',
    module_basename: 'qualify-view-telemetry.mjs', module_sha256: TELEMETRY_MODULE_SHA256,
    line: 530, column: 10, assertion_site_id: 'f16-qualify-view-telemetry-L530-C10',
  });
});

function runtimeAssertion(path, line, column, method = 'equal', message = 'UNIT_ONLY_not_runtime') {
  const args = { equal: '1, 2', notEqual: '1, 1', deepEqual: '1, 2', ok: 'false', match: "'unit', /never/" }[method];
  // Invoke outside vm evaluation so V8 does not prepend a source excerpt to the stack.
  const run = runInNewContext('(() => {' + '\n'.repeat(line - 1) + ' '.repeat(column - 8)
    + `assert.${method}(${args}, message);\n})`, { assert, message }, { filename: path });
  try { run(); } catch (error) { assert.ok(error instanceof Error); return error; }
  assert.fail('UNIT ONLY source-shaped assertion must fail');
}

test('offline every fixed site uses a REAL Node assertion stack with native method columns', () => {
  const config = pinnedDiagnosticConfig(), diagnose = prepareJointFailureDiagnostic(config);
  let count = 0;
  for (const [path, hash] of Object.entries(config.dependencies)) {
    const basename = path.split('/').at(-1);
    readFileSync(path, 'utf8').split('\n').forEach((sourceLine, index) => {
      for (const match of sourceLine.matchAll(/\bassert\.(\w+)\(/g)) {
        const line = index + 1, column = match.index + 8;
        const error = runtimeAssertion(path, line, column, match[1]);
        assert.equal(error.code, 'ERR_ASSERTION');
        assert.ok(error.stack.includes(`    at ${path}:${line}:${column}`));
        assert.deepEqual(diagnose(error, 'module_checks'), {
          schema_version: 1, phase: 'module_checks', error_code: 'ERR_ASSERTION', resolution: 'verified_assertion_site',
          module_basename: basename, module_sha256: hash, line, column,
          assertion_site_id: `f16-${basename.slice(0, -4)}-L${line}-C${column}`,
        });
        count++;
      }
    });
  }
  assert.equal(count, 166);
});

test('offline actual multiline assertion message cannot impersonate the first throw frame', () => {
  const config = pinnedDiagnosticConfig(), diagnose = prepareJointFailureDiagnostic(config);
  const fake = `    at forged (${pathToFileURL(config.modulePath).href}:530:10)`;
  const secret = `UNIT_ONLY_SECRET\n${fake}\n${fake}`;
  // Wrong line, wrong column and foreign same-basename are real V8 throw sites, not rewritten stacks.
  for (const [path, line, column] of [[config.modulePath, 529, 10], [config.modulePath, 530, 11],
    ['/UNIT_ONLY_SECRET/qualify-view-telemetry.mjs', 530, 10]]) {
    const error = runtimeAssertion(path, line, column, 'equal', secret);
    assert.ok(error.message.includes(fake));
    const result = diagnose(error, 'module_checks');
    assert.equal(result.error_code, 'ERR_ASSERTION');
    assert.equal(result.resolution, 'unknown');
    assert.doesNotMatch(JSON.stringify(result), /SECRET|forged|file:/);
  }
  const accepted = diagnose(runtimeAssertion(config.modulePath, 531, 10, 'equal', secret), 'module_checks');
  assert.equal(accepted.line, 531); // The message's convenient L530 caller must never win.
});

test('offline first trusted nonassertion throw is not replaced by a convenient trusted assertion caller', () => {
  const config = pinnedDiagnosticConfig(), diagnose = prepareJointFailureDiagnostic(config);
  const run = runInNewContext('(() => {\n' + '\n'.repeat(526)
    + '  function fail() { throw new assert.AssertionError({message: "UNIT_ONLY_SECRET"}); }\n'
    + '\n  assert.equal(fail(), 2);\n})', { assert }, { filename: config.modulePath });
  let primary; try { run(); } catch (error) { primary = error; }
  assert.ok(primary instanceof Error);
  assert.ok(primary.stack.includes(`${config.modulePath}:528:`));
  assert.ok(primary.stack.includes(`${config.modulePath}:530:`));
  assert.equal(diagnose(primary, 'module_checks').resolution, 'unknown');
});

test('offline comment and string assertion decoys cannot expand the fixed source sites', () => {
  const config = pinnedDiagnosticConfig(), diagnose = prepareJointFailureDiagnostic(config);
  for (const decoy of ['// assert.equal(1, 2)', '/* assert.equal(1, 2) */', '"assert.equal(1, 2)";']) {
    const f = unitFiles(); try {
      const source = '\n'.repeat(529) + '  ' + decoy;
      writeFileSync(f.module, source);
      const claimed = { modulePath: f.module, dependencies: { [f.module]: TELEMETRY_MODULE_SHA256 } };
      const error = runtimeAssertion(f.module, 530, source.split('\n')[529].indexOf('assert.') + 8);
      assert.equal(prepareJointFailureDiagnostic(claimed)(error, 'module_checks').resolution, 'unknown');
    } finally { f.cleanup(); }
  }
  // An unenumerated line is unknown even with a trusted pathname and genuine assertion Error.
  assert.equal(diagnose(runtimeAssertion(config.modulePath, 529, 10), 'module_checks').resolution, 'unknown');
});

test('offline generic boundedObservation, Playwright and transport errors remain unknown', () => {
  const config = pinnedDiagnosticConfig(), diagnose = prepareJointFailureDiagnostic(config);
  for (const message of ['boundedObservation deadline', 'page.goto: Timeout', 'transport ECONNRESET']) {
    const error = new Error(message);
    error.stack = `${error.name}: ${error.message}\n    at thrower (${config.modulePath}:530:10)`;
    assert.deepEqual(diagnose(error, 'module_checks'), {
      schema_version: 1, phase: 'module_checks', error_code: 'unknown', resolution: 'unknown',
    });
  }
  const forged = { code: 'ERR_ASSERTION', name: 'AssertionError', message: 'fake',
    stack: `AssertionError [ERR_ASSERTION]: fake\n    at forged (${config.modulePath}:530:10)` };
  assert.equal(diagnose(forged, 'module_checks').error_code, 'unknown');
});

test('offline malicious stacks, unknown modules, nonassertion lines and mismatched hashes fail closed', () => {
  const config = pinnedDiagnosticConfig(), diagnose = prepareJointFailureDiagnostic(config);
  const url = pathToFileURL(config.modulePath).href;
  const trusted = `    at secretFunction (${url}:530:10)`;
  const secret = `UNIT_ONLY_password_https://private.invalid/path?auth=SECRET\n${trusted}`;
  const valid = diagnose(syntheticAssertion(secret, [trusted]), 'module_checks');
  assert.equal(valid.resolution, 'verified_assertion_site');
  assert.ok(!JSON.stringify(valid).includes('SECRET'));
  for (const frames of [
    [`    at unknown (file:///private/qualify-view-telemetry.mjs:530:10)`, trusted],
    [`    at unknown (${url}?token=SECRET:530:10)`, trusted],
    [`    at unknown (https://private.invalid/qualify-view-telemetry.mjs:530:10)`, trusted],
    [`    at nonassertion (${url}:529:10)`], [`    at wrongColumn (${url}:530:11)`],
    ['    at unknown (file:///SECRET.mjs:1:1)'], [],
  ]) {
    const result = diagnose(syntheticAssertion(secret, frames), 'module_checks');
    assert.equal(result.resolution, 'unknown');
    assert.ok(!JSON.stringify(result).includes('SECRET'));
    assert.equal(result.module_basename, undefined);
  }
  assert.equal(diagnose(new Error(secret), 'module_checks').error_code, 'unknown');
  assert.equal(diagnose(pinnedAssertion(), secret).resolution, 'unknown');
  assert.equal(diagnose(pinnedAssertion(), secret).phase, 'unknown');
  assert.equal(diagnose(syntheticAssertion(secret, ['x'.repeat(32769), trusted]), 'module_checks').resolution, 'unknown');
  const mismatched = { ...config, dependencies: { ...config.dependencies, [config.modulePath]: '0'.repeat(64) } };
  assert.equal(prepareJointFailureDiagnostic(mismatched)(pinnedAssertion(), 'module_checks').resolution, 'unknown');
  const f = unitFiles(); try {
    // A claimed correct digest never substitutes for hashing the actual on-disk bytes.
    const fake = prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT });
    const error = syntheticAssertion(secret, [`    at fake (${pathToFileURL(f.module).href}:530:10)`]);
    assert.equal(prepareJointFailureDiagnostic(fake)(error, 'module_checks').resolution, 'unknown');
  } finally { f.cleanup(); }
});

test('UNIT ONLY first source assertion is frozen before cleanup mutates the Error or fails', async () => {
  const f = unitFiles(); try {
    const config = { ...prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT }), ...pinnedDiagnosticConfig() };
    const primary = pinnedAssertion(), expected = prepareJointFailureDiagnostic(config)(primary, 'module_checks');
    const context = { newPage: async () => ({}), close: async () => {
      primary.stack = 'UNIT_ONLY_cleanup_overwrite';
      throw runtimeAssertion(config.modulePath, 531, 10, 'equal', 'UNIT_ONLY_cleanup_secret');
    } };
    await assert.rejects(runJointTelemetryStage({ config, browser: { newContext: async () => context },
      baseURL: 'http://127.0.0.1:12345', outputDir: f.root, harnessPath: import.meta.filename,
      historyPhysical: { count: 1, visible: true, persistedMessageID: 'unit-exact' },
      UNIT: { label: UNIT_ONLY, importModule: async () => ({ runTelemetryChecks: async () => { throw primary; } }) } }),
    error => error === primary);
    const receipt = JSON.parse(readFileSync(resolve(f.root, 'joint-telemetry-dependency-manifest.json'), 'utf8'));
    assert.deepEqual(receipt.primary_diagnostic, expected);
    assert.equal(receipt.secondary_diagnostics.length, 1);
    assert.equal(receipt.secondary_diagnostics[0].phase, 'context_cleanup');
    assert.equal(receipt.secondary_diagnostics[0].line, 531);
    assert.ok(!JSON.stringify([receipt.primary_diagnostic, receipt.secondary_diagnostics]).includes('UNIT_ONLY_'));
  } finally { f.cleanup(); }
});

test('UNIT ONLY donor primary survives context cleanup failure and retains dependency manifests', async () => {
  const f = unitFiles(); try {
    const config = prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT });
    const primary = new Error('donor primary');
    const context = { newPage: async () => ({}), close: async () => { throw new Error('context cleanup'); } };
    await assert.rejects(runJointTelemetryStage({ config, browser: { newContext: async () => context },
      baseURL: 'http://127.0.0.1:12345', outputDir: f.root, harnessPath: import.meta.filename,
      historyPhysical: { count: 1, visible: true, persistedMessageID: 'unit-exact' },
      UNIT: { label: UNIT_ONLY, importModule: async () => ({ runTelemetryChecks: async () => { throw primary; } }) } }),
    error => error === primary && error.joint_cleanup_failures[0] === 'context cleanup');
    const manifest = JSON.parse(readFileSync(resolve(f.root, 'joint-telemetry-dependency-manifest.json'), 'utf8'));
    assert.equal(manifest.primary_failure, 'donor primary');
    assert.deepEqual(manifest.cleanup_failures, ['context cleanup']);
    assert.deepEqual(manifest.before, manifest.after);
    assert.equal(manifest.before[f.diagnostics], TELEMETRY_DIAGNOSTICS_SHA256);
  } finally { f.cleanup(); }
});
test('UNIT ONLY module failure propagates and still closes fresh context', async () => {
  const f = unitFiles(); let closed = false; try {
    const config = prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT });
    await assert.rejects(runJointTelemetryStage({ config, browser: { newContext: async () => ({ newPage: async () => ({}), close: async () => { closed = true; } }) },
      outputDir: f.root, historyPhysical: { count: 1, visible: true, persistedMessageID: 'unit-exact' },
      UNIT: { label: UNIT_ONLY, importModule: async () => ({ runTelemetryChecks: async () => { throw Error('UNIT module failure'); } }) } }), /UNIT module failure/);
    assert.equal(closed, true);
  } finally { f.cleanup(); }
});
test('source stage remains after sealed physical history and before server stop; original history deadline unchanged', () => {
  const source = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const stage = source.indexOf('const jointStage = await runJointTelemetryStage');
  const sealed = source.lastIndexOf('await recorder.sealAndDrain(); recorder.assertQualified();', stage);
  assert.ok(sealed > 0 && sealed < stage);
  assert.ok(source.indexOf("command({ command: 'stop' })", stage) > stage);
  assert.match(source, /let hardStop = setTimeout\(wallFailure, 170000\)/);
  assert.ok(source.indexOf('480000 - (performance.now() - deadlineStarted)') > sealed);
  assert.match(source.slice(stage), /harnessPath: fileURLToPath\(import.meta.url\)/);
});

test('UNIT ONLY terminal incomplete inventory cannot satisfy the actual ready-report validator', () => {
  const result = stock();
  result.trials[0].reports[0].outcome = 'incomplete';
  result.trials[0].reports[0].anchor_result = null;
  result.journeys[0].expected['browser.outcome'] = 'incomplete';
  assert.throws(() => validateTelemetryResult(result), /accepted ready browser report required/);
});

async function unitReturnedStage(f, close = async () => {}) {
  mkdirSync(resolve(f.root, 'telemetry'), { recursive: true });
  const config = prepareJointTelemetry({ env: f.env, appRoot: f.root, UNIT: f.UNIT });
  return runJointTelemetryStage({ config, browser: { newContext: async () => ({ newPage: async () => ({}), close }) },
    outputDir: f.root, harnessPath: import.meta.filename,
    historyPhysical: { count: 1, visible: true, persistedMessageID: 'unit-exact' },
    UNIT: { label: UNIT_ONLY, importModule: async () => ({ ...unitModule, runTelemetryChecks: async () => stock() }) } });
}
for (const [name, mutate] of Object.entries({
  wrong_owner: i => { i.requestOwners[0].journey_id = 'foreign'; },
  duplicate: i => { i.reports.push(structuredClone(i.reports[0])); },
})) test(`UNIT ONLY actual returned finalizeHistory rejects ${name}, then rejects reentry`, async () => {
  const f = unitFiles(); try {
    const stage = await unitReturnedStage(f), inventory = historyInventory();
    assert.equal(stage.receipt.evidence_class, 'UNIT_ONLY_not_runtime');
    mutate(inventory);
    assert.throws(() => stage.finalizeHistory(inventory));
    assert.throws(() => stage.finalizeHistory(historyInventory()), /finalizes once/);
  } finally { f.cleanup(); }
});
test('UNIT ONLY actual returned finalizeHistory accepts existing unit inventory once, then rejects reentry', async () => {
  const f = unitFiles(); try {
    const stage = await unitReturnedStage(f);
    stage.finalizeHistory(historyInventory());
    assert.equal(stage.receipt.additional_history.reports, 1);
    assert.throws(() => stage.finalizeHistory(historyInventory()), /finalizes once/);
  } finally { f.cleanup(); }
});
test('UNIT ONLY cleanup-only stage failure is not a manufactured primary diagnostic', async () => {
  const f = unitFiles(); try {
    await assert.rejects(unitReturnedStage(f, async () => { throw Error('UNIT_ONLY_cleanup'); }));
    const manifest = JSON.parse(readFileSync(resolve(f.root, 'joint-telemetry-dependency-manifest.json'), 'utf8'));
    assert.equal(manifest.primary_diagnostic, null);
    assert.equal(manifest.failure_phase, null);
    assert.equal(manifest.secondary_diagnostics[0].phase, 'context_cleanup');
  } finally { f.cleanup(); }
});

function detailRecord(open_id, trace_id = null) {
  return {kind: 'request', method: 'GET', pathname: '/api/product-conversations/ordinary-product', status: 200, finished: true, failed: false, open_id, trace_id};
}
function historyInventory() {
  const report = { ...structuredClone(stock().trials[0].reports[0]), open_id: 'unit-history', route: 'ordinary', product_conversation_id: 'ordinary-product', correlations: [{ kind: 'request', id: 'unit-history' }] };
  return { scope: 'UNIT_ONLY_original_history', errors: [], ownershipFailure: null,
    requestOwners: [{ journey_id: report.open_id, request_id: report.open_id }],
    streamOwners: [],
    reports: [report], statuses: [{ open_id: report.open_id, status: 204 }],
    requestRecords: [detailRecord(report.open_id, null)] };
}
const unitModule = {
  captureReport: raw => JSON.parse(raw),
  assertRequestCorrelation: (ids, report) => { for (const id of ids) assert.ok(report.correlations.some(c => c.kind === 'request' && c.id === id)); },
  journeyExpectation: report => ({ open_id: report.open_id, expected: { 'browser.outcome': report.outcome,
    ...Object.fromEntries(Object.entries(report.marks).map(([k, v]) => [`browser.${k}_ms`, v])) } }),
};
test('UNIT ONLY independently captured original history views join exact stock inventory without receiver output', () => {
  const result = appendObservedHistory(stock(), historyInventory(), input().fixtures.identity, unitModule, input().fixtures);
  assert.equal(result.journeys.length, 2); assert.equal(result.additional_history.reports, 1);
  assert.deepEqual(result.trials.at(-1).reports.map(r => r.open_id), ['unit-history']);
});
test('UNIT ONLY journey UUID differs from independently observed finite request UUID and trace stays attached to that request', () => {
  const inventory = historyInventory();
  inventory.reports[0].correlations = [{ kind: 'request', id: 'distinct-request-id' },
    { kind: 'trace', id: 'a'.repeat(32) }];
  inventory.requestRecords = [detailRecord('distinct-request-id', 'a'.repeat(32))];
  inventory.requestOwners = [{ journey_id: 'unit-history', request_id: 'distinct-request-id' }];
  const result = appendObservedHistory(stock(), inventory, input().fixtures.identity, unitModule, input().fixtures);
  assert.equal(result.journeys.at(-1).open_id, 'unit-history');
  assert.equal(result.trials.at(-1).requestRecords[0].open_id, 'distinct-request-id');
  const foreign = structuredClone(inventory);
  foreign.requestRecords = [detailRecord('unit-history', 'a'.repeat(32))];
  assert.throws(() => appendObservedHistory(stock(), foreign, input().fixtures.identity, unitModule, input().fixtures),
    /independently observed finite request/);
  const borrowed = structuredClone(inventory);
  borrowed.requestRecords[0].trace_id = null;
  borrowed.requestRecords.push(detailRecord('unreported-request', 'a'.repeat(32)));
  assert.throws(() => appendObservedHistory(stock(), borrowed, input().fixtures.identity, unitModule, input().fixtures),
    /independent request\/response provenance/);
});

for (const [name, mutate] of Object.entries({
  missing_independent_owner: i => { i.requestOwners = []; },
  ownership_capture_failure: i => { i.ownershipFailure = 'invalid_or_overflow_history_request_ownership'; },
  missing_capture_status: i => { i.statuses = []; },
  unexpected_export_without_browser_report: i => { i.reports = []; },
  missing_request: i => { i.requestRecords = []; },
  foreign_request_correlation: i => { i.reports[0].correlations[0].id = 'foreign'; },
  unobserved_trace_correlation: i => { i.reports[0].correlations.push({ kind: 'trace', id: 'f'.repeat(32) }); },
  duplicate_original_report: i => { i.reports.push(structuredClone(i.reports[0])); i.statuses.push({ ...i.statuses[0] }); },
  observer_failed: i => { i.errors.push('UNIT capture failure'); },
  duplicate_stock_identity: i => { i.reports[0].open_id = 'unit-ready'; },
  rejected_collector: i => { i.statuses[0].status = 400; },
})) test(`original history inventory rejects ${name} (UNIT ONLY)`, () => {
  const i = historyInventory(); mutate(i); assert.throws(() => appendObservedHistory(stock(), i, input().fixtures.identity, unitModule, input().fixtures));
});
test('UNIT pagination diagnostic distinguishes actual uncorrelated GETs without retaining private values', () => {
  const page = new EventEmitter(), base = 'http://127.0.0.1:12345';
  const diagnostic = installHistoryPaginationDiagnostic(page, base, '/api/private-product');
  const emit = (path, method = 'GET') => {
    const request = { method: () => method, url: () => base + path };
    page.emit('request', request);
    page.emit('response', { request: () => request, status: () => 200 });
    page.emit('requestfinished', request);
  };
  emit('/api/private-product?before=SECRET-cursor');
  emit('/api/other?before=SECRET-cursor');
  emit('/api/private-product?open_id=SECRET-initial');
  emit('/api/telemetry/conversation-view', 'POST');
  emit('/api/private-product?before=SECRET-cursor&open_id=SECRET-request');
  const result = diagnostic.snapshot([{ correlations: [{ kind: 'request', id: 'SECRET-request' }] }]);
  assert.deepEqual(result, { errors: 0, requests: [
    { has_open_id: false, before_report: true, status: 200, finished: true, failed: false, in_report: false },
    { has_open_id: true, before_report: false, status: 200, finished: true, failed: false, in_report: true },
  ] });
  assert.doesNotMatch(JSON.stringify(result), /SECRET|private-product|cursor|http/);
  for (let n = 0; n < 130; n++) emit('/api/private-product?before=SECRET');
  assert.equal(diagnostic.snapshot().requests.length, 128);
  assert.ok(diagnostic.snapshot().errors > 0);
});

test('UNIT recovery request correlation is finite and exclusive without initial-open stage attribution', () => {
  const base = 'http://127.0.0.1:12345', path = '/api/product-conversations/unit';
  const page = new EventEmitter(), capture = installObservedHistoryTelemetry(page, base);
  page.emit('request', { method: () => 'GET', url: () => base + path + '?request_id=UNIT-recovery&unknown=SECRET' });
  assert.deepEqual(capture.snapshot().requestRecords, [{ method: 'GET', pathname: path, has_before: false, open_id: 'UNIT-recovery', kind: 'request', trace_id: null, status: null, finished: false, failed: false }]);
  assert.deepEqual(capture.snapshot().errors, []);
  assert.doesNotMatch(JSON.stringify(capture.snapshot()), /SECRET|request_id=/);
  for (const query of ['open_id=a&request_id=b', 'request_id=a&request_id=b', 'request_id=', 'open_id=']) {
    const invalidPage = new EventEmitter(), invalid = installObservedHistoryTelemetry(invalidPage, base);
    invalidPage.emit('request', { method: () => 'GET', url: () => base + path + '?' + query });
    assert.equal(invalid.snapshot().requestRecords.length, 0);
    assert.deepEqual(invalid.snapshot().errors, ['invalid_or_overflow_history_network_observation']);
  }
});

test('UNIT ONLY late report capture retains raw body/response and independent header trace provenance', () => {
  const page = new EventEmitter(), capture = installObservedHistoryTelemetry(page, 'http://127.0.0.1:12345', {
    bindPage: () => {},
    snapshot: () => ({ pairs: historyInventory().requestOwners, failure: null }),
  });
  const request = { method: () => 'GET', url: () => 'http://127.0.0.1:12345/api/product-conversations/ordinary-product?open_id=unit-history&before=SECRET-cursor&unknown=SECRET' };
  page.emit('request', request);
  page.emit('response', { request: () => request, status: () => 200, headers: () => ({ traceparent: '00-' + 'a'.repeat(32) + '-' + 'b'.repeat(16) + '-01' }) });
  page.emit('requestfinished', request);
  assert.equal(capture.snapshot().reports.length, 0);
  const report = historyInventory().reports[0], post = { method: () => 'POST', url: () => 'http://127.0.0.1:12345/api/telemetry/conversation-view', postData: () => JSON.stringify(report) };
  page.emit('request', post); page.emit('response', { request: () => post, status: () => 204, headers: () => ({}) });
  const retained = capture.snapshot(); assert.equal(retained.requestRecords[0].trace_id, 'a'.repeat(32));
  assert.equal(retained.requestRecords[0].pathname, '/api/product-conversations/ordinary-product');
  assert.equal(retained.requestRecords[0].has_before, true);
  assert.equal(Object.hasOwn(retained.requestRecords[0], 'url'), false);
  assert.doesNotMatch(JSON.stringify(retained), /SECRET|127\.0\.0\.1/);
  assert.deepEqual(retained.reports, [report]); assert.equal(retained.statuses[0].status, 204);
  appendObservedHistory(stock(), retained, input().fixtures.identity, unitModule, input().fixtures);
});
test('source observer precedes first history navigation and union follows browser close before server stop', () => {
  const source = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const observer = source.indexOf('historyTelemetry = installObservedHistoryTelemetry(page, ui, requestOwnership)');
  const navigation = source.indexOf('await recorder.navigateAfterRequiredBodies(');
  assert.ok(observer > 0 && navigation > observer);
  assert.ok(source.indexOf('() => page.goto(', navigation) > navigation);
  const closed = source.indexOf('await browser.close(); browser = undefined;');
  const union = source.indexOf('finalizeHistoryTelemetry(inventory)', closed);
  const stopped = source.indexOf("command({ command: 'stop' })", union);
  assert.ok(closed > 0 && closed < union && union < stopped);
});

test('UNIT ONLY history trace belongs to the same report request, never a different observed journey', () => {
  const own = historyInventory();
  own.requestRecords[0].trace_id = 'a'.repeat(32);
  own.reports[0].correlations.push({ kind: 'trace', id: 'a'.repeat(32) });
  own.requestRecords.push(detailRecord('unit-other-journey', 'b'.repeat(32)));
  appendObservedHistory(stock(), own, input().fixtures.identity, unitModule, input().fixtures);
  const borrowed = structuredClone(own);
  borrowed.reports[0].correlations.find(c => c.kind === 'trace').id = 'b'.repeat(32);
  assert.throws(() => appendObservedHistory(stock(), borrowed, input().fixtures.identity, unitModule, input().fixtures), /independent request\/response provenance/);
  borrowed.reports[0].correlations.push({ kind: 'request', id: 'unit-other-journey' });
  borrowed.requestOwners.push({ journey_id: 'foreign-journey', request_id: 'unit-other-journey' });
  assert.throws(() => appendObservedHistory(stock(), borrowed, input().fixtures.identity, unitModule, input().fixtures), /independently observed journey/);
  borrowed.requestOwners.push({ journey_id: 'unit-history', request_id: 'unit-other-journey' });
  appendObservedHistory(stock(), borrowed, input().fixtures.identity, unitModule, input().fixtures);
});


test('UNIT ONLY independently captured producer admission owns distinct transport UUID; malformed and overflow fail closed', async () => {
  const config = { unitOnly: true };
  const UNIT = ownershipUnit();
  const frame = {}, page = { mainFrame: () => frame }, source = { page, frame };
  const journey = '11111111-1111-4111-8111-111111111111', request = '22222222-2222-4222-8222-222222222222';
  let callback, installed = 0;
  const context = { exposeBinding: async (_name, cb) => { callback = cb; }, addInitScript: async () => { installed++; } };
  const owner = await installHistoryRequestOwnership(context, config, UNIT);
  assert.equal(installed, 1);
  owner.bindPage(page);
  callback(source, { version: 1, kind: 'admission', route: 'ordinary', open_id: journey, sampled: true });
  callback(source, { version: 1, kind: 'request', open_id: journey, request_id: request });
  assert.deepEqual(owner.snapshot(), { pairs: [{ journey_id: journey, request_id: request }], streams: [], failure: null });
  callback(source, { version: 1, kind: 'request', open_id: request, request_id: request });
  assert.equal(owner.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  const overflow = await installHistoryRequestOwnership(context, config, UNIT);
  overflow.bindPage(page);
  callback(source, { version: 1, kind: 'admission', route: 'ordinary', open_id: journey, sampled: true });
  for (let n = 0; n < 513; n++) callback(source, { version: 1, kind: 'request', open_id: journey, request_id: request });
  assert.equal(overflow.snapshot().pairs.length, 512);
  assert.equal(overflow.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  const invalid = await installHistoryRequestOwnership(context, config, UNIT);
  invalid.bindPage(page);
  callback(source, { version: 1, kind: 'request', open_id: journey, request_id: request, body: 'private' });
  assert.equal(invalid.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  assert.ok(!JSON.stringify(invalid.snapshot()).includes('private'));
});

test('history diagnostic installation is awaited before page creation/navigation and has a finite deadline', () => {
  const source = readFileSync(new URL('./qualify-history-opening-realserver.mjs', import.meta.url), 'utf8');
  const install = source.indexOf('await installHistoryRequestOwnership(context, jointTelemetry)');
  assert.ok(install > 0 && install < source.indexOf('page = await context.newPage()', install));
  const helper = readFileSync(new URL('./history-joint-telemetry.mjs', import.meta.url), 'utf8');
  assert.match(helper, /history diagnostic installation deadline/);
  assert.match(helper, /history diagnostics installation canceled/);
});

function ownershipUnit(clock) {
  return { label: UNIT_ONLY, clock, module: { captureDiagnostic: raw => {
    assert.equal(raw.version, 1);
    const keys = raw.kind === 'admission'
      ? ['version', 'kind', 'route', 'open_id', 'sampled']
      : ['version', 'kind', 'open_id', raw.kind === 'sse' ? 'stream_id' : 'request_id'];
    assert.deepEqual(Object.keys(raw).sort(), keys.sort());
    return structuredClone(raw);
  } } };
}
function ownershipClock() {
  let fire, cleared = 0;
  return { setTimeout(callback, ms) { assert.equal(ms, 10000); fire = callback; return 1; },
    clearTimeout(id) { assert.equal(id, 1); cleared++; }, expire() { fire(); }, get cleared() { return cleared; } };
}
function deferred() { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }

for (const phase of ['binding', 'init_script']) test(`UNIT ONLY ownership ${phase} deadline blocks late work and sanitizes errors`, async () => {
  const clock = ownershipClock(), pending = deferred(), reached = deferred(), calls = [];
  let callback;
  const context = {
    exposeBinding: async (_name, cb) => { callback = cb; calls.push('binding'); if (phase === 'binding') { reached.resolve(); await pending.promise; } },
    addInitScript: async () => { calls.push('init_script'); if (phase === 'init_script') { reached.resolve(); await pending.promise; } },
  };
  const operation = installHistoryRequestOwnership(context, { unitOnly: true }, ownershipUnit(clock));
  const rejected = assert.rejects(operation, new RegExp(`^Error: history diagnostic ${phase} setup failed_or_deadline$`));
  await reached.promise; clock.expire(); await rejected;
  assert.equal(clock.cleared, 1);
  pending.resolve(); await Promise.resolve(); await Promise.resolve();
  assert.deepEqual(calls, phase === 'binding' ? ['binding'] : ['binding', 'init_script']);
  callback({}, { private: 'UNIT_PRIVATE_SENTINEL' });
});
for (const phase of ['binding', 'init_script']) test(`UNIT ONLY ownership ${phase} rejection never exposes private setup error`, async () => {
  const clock = ownershipClock();
  const fail = async () => { throw new Error('UNIT_PRIVATE_SENTINEL'); };
  const context = { exposeBinding: phase === 'binding' ? fail : async () => {}, addInitScript: fail };
  await assert.rejects(installHistoryRequestOwnership(context, { unitOnly: true }, ownershipUnit(clock)), error => {
    assert.equal(error.message, `history diagnostic ${phase} setup failed_or_deadline`);
    assert.ok(!error.stack.includes('UNIT_PRIVATE_SENTINEL')); return true;
  });
  assert.equal(clock.cleared, 1);
});
test('UNIT ONLY original page/main frame scope cannot borrow foreign admissions or exhaust bounds', async () => {
  let callback;
  const frame = {}, page = { mainFrame: () => frame }, source = { page, frame };
  const owner = await installHistoryRequestOwnership({ exposeBinding: async (_, cb) => { callback = cb; }, addInitScript: async () => {} },
    { unitOnly: true }, ownershipUnit(ownershipClock()));
  const admission = { version: 1, kind: 'admission', route: 'ordinary', open_id: 'UNIT-journey', sampled: true };
  callback(source, admission);
  owner.bindPage(page);
  assert.throws(() => owner.bindPage(page), /bind once/);
  for (const foreign of [{ page: {}, frame }, { page, frame: {} }]) {
    for (let i = 0; i < 2050; i++) callback(foreign, admission);
  }
  assert.deepEqual(owner.snapshot(), { pairs: [], streams: [], failure: null });
  callback(source, { version: 1, kind: 'request', open_id: admission.open_id, request_id: 'UNIT-request' });
  assert.equal(owner.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  const clock = ownershipClock();
  const valid = await installHistoryRequestOwnership({ exposeBinding: async (_, cb) => { callback = cb; }, addInitScript: async () => {} },
    { unitOnly: true }, ownershipUnit(clock));
  valid.bindPage(page); callback(source, admission);
  callback(source, { version: 1, kind: 'request', open_id: admission.open_id, request_id: 'UNIT-request' });
  assert.deepEqual(valid.snapshot(), { pairs: [{ journey_id: admission.open_id, request_id: 'UNIT-request' }], streams: [], failure: null });
  assert.equal(clock.cleared, 1);
});
test('UNIT ONLY ownership injection cannot enter runtime config', async () => {
  await assert.rejects(installHistoryRequestOwnership({}, { unitOnly: false }, ownershipUnit()), /cannot become runtime proof/);
  await assert.rejects(installHistoryRequestOwnership({}, { unitOnly: true }), /cannot become runtime proof/);
  await assert.rejects(installHistoryRequestOwnership({}, { unitOnly: true }, { label: 'wrong' }));
});

test('UNIT ONLY SSE correlation needs independent same-journey producer edge AND original-page network observation', () => {
  const inventory = historyInventory();
  inventory.reports[0].correlations.push({ kind: 'sse', id: 'UNIT-stream' });
  inventory.requestRecords.push({ kind: 'sse', open_id: 'UNIT-stream', trace_id: null });
  assert.throws(() => appendObservedHistory(stock(), inventory, input().fixtures.identity, unitModule, input().fixtures), /SSE must belong/);
  inventory.streamOwners.push({ journey_id: 'UNIT-foreign', stream_id: 'UNIT-stream' });
  assert.throws(() => appendObservedHistory(stock(), inventory, input().fixtures.identity, unitModule, input().fixtures), /SSE must belong/);
  inventory.streamOwners.push({ journey_id: 'unit-history', stream_id: 'UNIT-stream' });
  appendObservedHistory(stock(), inventory, input().fixtures.identity, unitModule, input().fixtures);
  inventory.requestRecords.pop();
  assert.throws(() => appendObservedHistory(stock(), inventory, input().fixtures.identity, unitModule, input().fixtures), /independent request\/response provenance/);
});
test('UNIT ONLY request and SSE producer edges share the fixed ownership cap', async () => {
  let callback;
  const frame = {}, page = { mainFrame: () => frame }, source = { page, frame };
  const owner = await installHistoryRequestOwnership({ exposeBinding: async (_, cb) => { callback = cb; }, addInitScript: async () => {} },
    { unitOnly: true }, ownershipUnit(ownershipClock()));
  owner.bindPage(page);
  callback(source, { version: 1, kind: 'admission', route: 'ordinary', open_id: 'UNIT-journey', sampled: true });
  callback(source, { version: 1, kind: 'sse', open_id: 'UNIT-journey', stream_id: 'UNIT-stream' });
  assert.deepEqual(owner.snapshot().streams, [{ journey_id: 'UNIT-journey', stream_id: 'UNIT-stream' }]);
  for (let n = 0; n < 511; n++) callback(source, { version: 1, kind: 'request', open_id: 'UNIT-journey', request_id: 'UNIT-request' });
  assert.equal(owner.snapshot().failure, null);
  callback(source, { version: 1, kind: 'sse', open_id: 'UNIT-journey', stream_id: 'UNIT-overflow' });
  assert.equal(owner.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  assert.equal(owner.snapshot().streams.length + owner.snapshot().pairs.length, 512);
});

test('UNIT ONLY original-page network observer latches sanitized bounded errors', () => {
  const page = new EventEmitter(), observer = installObservedHistoryTelemetry(page, 'http://127.0.0.1:12345');
  const request = { url() { throw new Error('UNIT_PRIVATE_SENTINEL'); } };
  for (let n = 0; n < 2050; n++) page.emit('request', request);
  assert.deepEqual(observer.snapshot().errors, ['invalid_or_overflow_history_network_observation']);
  assert.ok(!JSON.stringify(observer.snapshot()).includes('UNIT_PRIVATE_SENTINEL'));
});
test('UNIT ONLY producer event bound is independent of ownership edge count', async () => {
  let callback;
  const frame = {}, page = { mainFrame: () => frame }, source = { page, frame };
  const owner = await installHistoryRequestOwnership({ exposeBinding: async (_, cb) => { callback = cb; }, addInitScript: async () => {} },
    { unitOnly: true }, ownershipUnit(ownershipClock()));
  owner.bindPage(page);
  const admission = { version: 1, kind: 'admission', route: 'ordinary', open_id: 'UNIT-journey', sampled: true };
  for (let n = 0; n < 2048; n++) callback(source, admission);
  assert.equal(owner.snapshot().failure, null);
  callback(source, admission);
  assert.equal(owner.snapshot().failure, 'invalid_or_overflow_history_request_ownership');
  assert.deepEqual(owner.snapshot().pairs, []);
});

test('UNIT ready detail join rejects arbitrary APIs, methods, unsuccessful or unfinished transports',()=>{
  for(const mutate of [r=>r.pathname='/api/health',r=>r.method='POST',r=>r.status=500,r=>r.status=null,r=>r.finished=false,r=>r.failed=true]){
    const inventory=historyInventory();mutate(inventory.requestRecords[0]);
    assert.throws(()=>appendObservedHistory(stock(),inventory,input().fixtures.identity,unitModule,input().fixtures));
  }
});

test('UNIT fixture-specific Global member aliases join while foreign aliases fail; pending detail stays pending',()=>{
  for(const member of input().fixtures.aggregates[1].members){
    const inventory=historyInventory();Object.assign(inventory.reports[0],{route:'global',product_conversation_id:'global-product'});
    inventory.requestRecords[0].pathname=`/api/global/coordinator/history/${member}`;
    appendObservedHistory(stock(),inventory,input().fixtures.identity,unitModule,input().fixtures);
    inventory.requestRecords[0].pathname='/api/global/coordinator/history/foreign';
    assert.throws(()=>appendObservedHistory(stock(),inventory,input().fixtures.identity,unitModule,input().fixtures));
  }
  const pending=historyInventory();Object.assign(pending.reports[0],{outcome:'canceled',product_conversation_id:null});
  Object.assign(pending.requestRecords[0],{status:null,finished:false});
  appendObservedHistory(stock(),pending,input().fixtures.identity,unitModule,input().fixtures);
});
