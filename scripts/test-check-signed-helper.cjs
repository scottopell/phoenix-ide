const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { spawn, execFileSync } = require('node:child_process');
const { checkSignedHelper } = require('./check-signed-helper.cjs');
const SHA = 'a'.repeat(40);
const VERSION = '0.13.0-rc.1';
const base = path.join(process.cwd(), 'target/signed-helper-tests');
fs.mkdirSync(base, { recursive: true });
const server = `
const fs = require('node:fs'), http = require('node:http');
const mode = process.argv[2];
const helper = process.argv[3];
if(mode==='early-exit')process.exit(3);
fs.writeFileSync(process.env.PHOENIX_DB_PATH, 'scripted database fixture');
const identity = {git_sha:'${SHA}',version:'${VERSION}',socket_activated:false};
if(mode==='runtime-sha')identity.git_sha='b'.repeat(40);
if(mode==='socket')identity.socket_activated=true;
const app=http.createServer((req,res)=>{
 if(req.url==='/api/version'){res.end(mode==='malformed'?'not-json':JSON.stringify(identity));return;}
 if(req.url==='/api/auth/login'){
  let body='';req.on('data',c=>body+=c);req.on('end',()=>{
   if(JSON.parse(body).password!==process.env.PHOENIX_PASSWORD)process.exit(7);
   if(mode==='login'){res.statusCode=403;res.end('{}');return;}
   res.setHeader('set-cookie','phoenix-auth=PRIVATE-COOKIE; HttpOnly; Path=/');
   if(mode==='delayed-login')setTimeout(()=>res.end('{}'),1200);else res.end('{}');
  });return;
 }
 if(req.url==='/api/deployment'){
  if(!req.headers.cookie){res.statusCode=mode==='auth-boundary'?200:401;res.end('{}');return;}
  res.end(JSON.stringify({instance_id:mode==='instance'?'wrong':process.env.PHOENIX_INSTANCE_ID,build:{git_sha:mode==='deployment-sha'?'wrong':'${SHA}',version:'${VERSION}'},private_payload:'MUST-NOT-APPEAR-IN-RECEIPT'}));return;
 }
 res.statusCode=404;res.end();
});
app.listen(Number(process.env.PHOENIX_PORT),'127.0.0.1');
process.on('SIGTERM',()=>app.close(()=>{if(mode==='mutate')fs.appendFileSync(helper,'changed');process.exit(mode==='stop'?2:0);}));
`;

function fixture(t, mode = 'success', target = 'aarch64-apple-darwin', archiveInput = false) {
  const root = fs.mkdtempSync(path.join(base, 'case-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const helper = path.join(root, 'Phoenix.app/Contents/Helpers/phoenix_ide');
  fs.mkdirSync(path.dirname(helper), { recursive: true });
  fs.writeFileSync(helper, 'scripted signed artifact fixture');
  const standalone = path.join(root, 'standalone');
  fs.copyFileSync(helper, standalone);
  const archive = path.join(root, 'app.zip');
  fs.writeFileSync(archive, 'scripted prepared app archive');
  let launchedHelper = helper;
  const script = path.join(root, 'server.cjs');
  fs.writeFileSync(script, server);
  let launched = false, password, child;
  const command = (binary, args) => {
    if (binary.endsWith('/ditto')) {
      assert.deepEqual(args.slice(0, 3), ['-x', '-k', archive]);
      if (mode === 'extraction') throw Error('PRIVATE-DIAGNOSTIC');
      launchedHelper = path.join(args[3], 'Phoenix.app/Contents/Helpers/phoenix_ide');
      fs.mkdirSync(path.dirname(launchedHelper), { recursive: true });
      if (mode !== 'missing-extracted-helper') fs.copyFileSync(helper, launchedHelper);
      return '';
    }
    if (binary.endsWith('/codesign')) {
      if (mode === 'signature') throw Error('PRIVATE-DIAGNOSTIC');
      return mode === 'adhoc' ? 'Signature=adhoc' : 'Authority=Developer ID Application: Fixture\nflags=0x10000(runtime)\nTimestamp=Fixture\n';
    }
    if (binary.endsWith('/lipo')) return mode === 'binary-arch' ? 'other' : target.startsWith('x86') ? 'x86_64' : 'arm64';
    if (binary === launchedHelper) return JSON.stringify({ git_sha: mode === 'build-sha' ? 'b'.repeat(40) : mode === 'dirty' ? `${SHA}-dirty` : SHA, version: VERSION, private_extra: 'MUST-NOT-APPEAR-IN-RECEIPT' });
    if (binary.endsWith('/sqlite3')) {
      assert.equal(args[0], path.join(root, 'runtime/data/qa.db'));
      if (args[1].includes('integrity')) return mode === 'integrity' ? 'not ok' : 'ok\n';
      return mode === 'foreign-keys' ? 'violated' : '';
    }
    throw Error('unexpected command');
  };
  const dependencies = {
    command, platform: 'darwin', arch: target.startsWith('x86') ? 'x64' : 'arm64',
    spawn: (binary, args, options) => {
      launched = true;
      assert.equal(binary, launchedHelper);
      assert.deepEqual(args, []);
      assert.equal(options.env.PHOENIX_TLS, 'off');
      assert.equal(options.env.PHOENIX_BIND_ADDR, '127.0.0.1');
      assert.notEqual(options.env.PHOENIX_PORT, '8031');
      assert.equal(options.env.HOME, path.join(root, 'runtime/home'));
      assert.equal(options.env.OPENAI_API_KEY, undefined);
      assert.equal(options.env.PHOENIX_SIDECAR_LEASE_PATH, undefined);
      assert.equal(options.cwd, path.join(root, 'runtime'));
      password = options.env.PHOENIX_PASSWORD;
      child = spawn(process.execPath, [script, mode, launchedHelper], options);
      return child;
    },
  };
  const options = { helper, ...(archiveInput ? { archive } : {}), standalone, sha: SHA, version: VERSION, target, output: path.join(root, 'runtime') };
  return { options, dependencies, root, launched: () => launched, password: () => password, child: () => child };
}

for (const target of ['aarch64-apple-darwin', 'x86_64-apple-darwin']) {
  test(`isolated startup/auth/DB/stop logic, scripted ${target} fixture`, async t => {
    const f = fixture(t, 'success', target, true);
    const before = fs.readFileSync(f.options.helper);
    const report = await checkSignedHelper(f.options, f.dependencies);
    assert.equal(report.result, 'passed');
    assert.deepEqual(report.checks.auth_statuses, [401, 200, 200]);
    assert.equal(report.checks.launched_instance, true);
    assert.equal(report.checks.graceful_exit_zero, true);
    assert.equal(report.checks.listener_closed, true);
    assert.equal(report.checks.artifacts_unchanged, true);
    assert.deepEqual(report.scope, { gui: false, install_update_rollback: false, provider_acceptance: false });
    assert.deepEqual(fs.readFileSync(f.options.helper), before);
    const receipt = path.join(f.options.output, 'acceptance-receipt.json');
    assert.equal(fs.statSync(receipt).mode & 0o777, 0o600);
    const text = fs.readFileSync(receipt, 'utf8');
    for (const secret of [f.password(), 'PRIVATE-COOKIE', 'MUST-NOT-APPEAR-IN-RECEIPT']) assert.ok(!text.includes(secret));
    assert.equal(f.child().exitCode, 0);
  });
}

test('post-readiness login tolerates a bounded busy-host response', async t => {
  const f = fixture(t, 'delayed-login');
  const report = await checkSignedHelper(f.options, f.dependencies);
  assert.equal(report.result, 'passed');
  assert.deepEqual(report.checks.auth_statuses, [401, 200, 200]);
  assert.equal(f.child().exitCode, 0);
});

for (const [mode, code] of [
  ['adhoc', 'developer-id-runtime-timestamp-required'], ['signature', 'check-failed-details-withheld'],
  ['binary-arch', 'artifact-architecture-mismatch'], ['build-sha', 'artifact-identity-mismatch'], ['dirty', 'artifact-identity-mismatch'],
  ['runtime-sha', 'runtime-identity-or-readiness-failed'], ['socket', 'runtime-identity-or-readiness-failed'],
  ['malformed', 'invalid-json-response'], ['auth-boundary', 'expected-auth-boundary'], ['login', 'normal-isolated-login-failed'],
  ['instance', 'launched-instance-mismatch'], ['deployment-sha', 'launched-instance-mismatch'],
  ['stop', 'graceful-sigterm-exit-not-zero'], ['integrity', 'database-integrity-failed'], ['foreign-keys', 'database-foreign-keys-failed'],
  ['mutate', 'artifact-bytes-changed'],
  ['early-exit', 'helper-exited-before-ready'],
]) {
  test(`fails closed for ${mode} without private diagnostics`, async t => {
    const f = fixture(t, mode);
    await assert.rejects(checkSignedHelper(f.options, f.dependencies), { message: code });
    if (f.launched()) assert.notEqual(f.child().exitCode, null);
    const text = fs.readFileSync(path.join(f.options.output, 'acceptance-receipt.json'), 'utf8');
    assert.equal(JSON.parse(text).result, 'failed');
    assert.equal(JSON.parse(text).failure_code, code);
    for (const secret of [f.password(), 'PRIVATE-COOKIE', 'MUST-NOT-APPEAR-IN-RECEIPT', 'PRIVATE-DIAGNOSTIC'].filter(Boolean)) assert.ok(!text.includes(secret));
  });
}

test('rejects payload mismatch before launching', async t => {
  const f = fixture(t);
  fs.appendFileSync(f.options.standalone, 'different');
  await assert.rejects(checkSignedHelper(f.options, f.dependencies), { message: 'helper-standalone-byte-mismatch' });
  assert.equal(f.launched(), false);
});

test('rejects cross-runtime and bad expected identity before launching', async t => {
  const f = fixture(t);
  await assert.rejects(checkSignedHelper(f.options, { ...f.dependencies, arch: 'x64' }), { message: 'requires-native-matching-host' });
  await assert.rejects(checkSignedHelper({ ...f.options, sha: 'short' }, f.dependencies), { message: 'expected-full-sha-required' });
  assert.equal(f.launched(), false);
});

test('rejects existing output and symlinked output parent before launching', async t => {
  const f = fixture(t);
  fs.mkdirSync(f.options.output);
  await assert.rejects(checkSignedHelper(f.options, f.dependencies), { message: 'output-must-be-fresh' });
  const link = path.join(f.root, 'escape');
  fs.symlinkSync(path.dirname(process.cwd()), link);
  await assert.rejects(checkSignedHelper({ ...f.options, output: path.join(link, 'not-created') }, f.dependencies), { message: 'qa-output-parent-boundary' });
  assert.equal(f.launched(), false);
});

for (const mode of ['missing-helper', 'missing-standalone', 'missing-archive', 'extraction', 'missing-extracted-helper']) {
  test(`retains sanitized prestart receipt for ${mode}`, async t => {
    const f = fixture(t, mode, 'aarch64-apple-darwin', mode !== 'missing-helper');
    if (mode === 'missing-helper') fs.unlinkSync(f.options.helper);
    if (mode === 'missing-standalone') fs.unlinkSync(f.options.standalone);
    if (mode === 'missing-archive') fs.unlinkSync(f.options.archive);
    await assert.rejects(checkSignedHelper(f.options, f.dependencies), { message: 'check-failed-details-withheld' });
    assert.equal(f.launched(), false);
    const receipt = fs.readFileSync(path.join(f.options.output, 'acceptance-receipt.json'), 'utf8');
    assert.equal(JSON.parse(receipt).result, 'failed');
    assert.ok(!receipt.includes('PRIVATE-DIAGNOSTIC'));
  });
}

test('CLI with invalid admission exits with no private output', () => {
  assert.throws(() => execFileSync(process.execPath, ['scripts/check-signed-helper.cjs'], { encoding: 'utf8', stdio: 'pipe' }), error => {
    assert.equal(error.status, 1);
    assert.ok(error.stderr.startsWith('Usage:'));
    return true;
  });
});
