#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const crypto = require('node:crypto');
const net = require('node:net');
const http = require('node:http');
const { spawn, spawnSync } = require('node:child_process');

class AcceptanceFailure extends Error {}
function requireCheck(condition, code) {
  if (!condition) throw new AcceptanceFailure(code);
}
function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}
function command(binary, args) {
  const result = spawnSync(binary, args, { encoding: 'utf8', timeout: 20000, maxBuffer: 1024 * 1024 });
  requireCheck(result.status === 0, 'artifact-command-failed');
  return (result.stdout || '') + (result.stderr || '');
}
function request(port, pathname, body, cookie, timeout = 10000) {
  return new Promise((resolve, reject) => {
    const req = http.request({
      host: '127.0.0.1', port, path: pathname, method: body ? 'POST' : 'GET',
      headers: {
        ...(body ? { 'content-type': 'application/json', 'content-length': Buffer.byteLength(body) } : {}),
        ...(cookie ? { cookie } : {}),
      },
    }, res => {
      let text = '';
      res.on('error', reject);
      res.on('data', chunk => {
        text += chunk;
        if (Buffer.byteLength(text) > 65536) req.destroy(new AcceptanceFailure('response-limit'));
      });
      res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, text }));
    });
    const timer = setTimeout(() => req.destroy(new AcceptanceFailure('request-timeout')), timeout);
    req.on('error', reject);
    req.on('close', () => clearTimeout(timer));
    req.end(body);
  });
}
function json(text) {
  try { return JSON.parse(text); } catch { throw new AcceptanceFailure('invalid-json-response'); }
}
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function allocatePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  requireCheck(port !== 8031, 'production-port-refused');
  return port;
}
function listenerClosed(port) {
  return new Promise(resolve => {
    const socket = net.connect({ host: '127.0.0.1', port });
    const timer = setTimeout(() => { socket.destroy(); resolve(false); }, 1000);
    socket.once('connect', () => { clearTimeout(timer); socket.destroy(); resolve(false); });
    socket.once('error', error => { clearTimeout(timer); resolve(error.code === 'ECONNREFUSED'); });
  });
}
async function waitForExit(exit, timeout) {
  let timer;
  try {
    return await Promise.race([exit, new Promise(resolve => { timer = setTimeout(() => resolve(null), timeout); })]);
  } finally { clearTimeout(timer); }
}

async function checkSignedHelper(options, dependencies = {}) {
  const run = dependencies.command || command;
  const launch = dependencies.spawn || spawn;
  const platform = dependencies.platform || process.platform;
  const hostArch = dependencies.arch || os.arch();
  let child, exit, report, output, stopRequested = false;
  try {
    requireCheck(platform === 'darwin', 'requires-macos-host');
    const { sha, version, target } = options;
    requireCheck(/^[a-f0-9]{40}$/.test(sha || ''), 'expected-full-sha-required');
    requireCheck(/^\d+\.\d+\.\d+(?:-rc\.\d+)?$/.test(version || ''), 'expected-version-required');
    const arch = { 'aarch64-apple-darwin': 'arm64', 'x86_64-apple-darwin': 'x86_64' }[target];
    requireCheck(arch && hostArch === (arch === 'arm64' ? 'arm64' : 'x64'), 'requires-native-matching-host');
    const workspace = fs.realpathSync(process.cwd());
    output = path.resolve(options.output);
    requireCheck(output.startsWith(workspace + path.sep), 'qa-workspace-boundary');
    requireCheck(!fs.existsSync(output), 'output-must-be-fresh');
    // Resolve the parent too, so an output symlink cannot escape the workspace.
    requireCheck(fs.realpathSync(path.dirname(output)).startsWith(workspace + path.sep), 'qa-output-parent-boundary');
    const production = path.join(os.homedir(), '.phoenix-ide');
    requireCheck(!(output === production || output.startsWith(production + path.sep)), 'production-path-refused');
    fs.mkdirSync(output, { mode: 0o700 });
    report = {
      schema: 1, kind: 'signed-helper-headless', observed_at: new Date().toISOString(),
      source_sha: sha, version, target, checks: {},
      scope: { gui: false, install_update_rollback: false, provider_acceptance: false },
    };
    const artifactPath = (file, preparedInput = false) => {
      const resolved = fs.realpathSync(file);
      const runnerTemp = preparedInput && process.env.RUNNER_TEMP ? fs.realpathSync(process.env.RUNNER_TEMP) : null;
      requireCheck(resolved.startsWith(workspace + path.sep) || (runnerTemp && resolved.startsWith(runnerTemp + path.sep)), 'qa-workspace-boundary');
      requireCheck(!(resolved === production || resolved.startsWith(production + path.sep)), 'production-path-refused');
      return resolved;
    };
    const standalone = artifactPath(options.standalone, true);
    let helper, archive, archiveDigest;
    if (options.archive) {
      archive = artifactPath(options.archive, true);
      archiveDigest = sha256(archive);
      report.archive_sha256 = archiveDigest;
      const extracted = path.join(output, 'artifacts');
      fs.mkdirSync(extracted, { mode: 0o700 });
      run('/usr/bin/ditto', ['-x', '-k', archive, extracted]);
      helper = artifactPath(path.join(extracted, 'Phoenix.app/Contents/Helpers/phoenix_ide'));
    } else {
      helper = artifactPath(options.helper);
    }
    requireCheck(helper.endsWith('/Phoenix.app/Contents/Helpers/phoenix_ide'), 'requires-bundled-helper');
    const digest = sha256(helper);
    report.helper_sha256 = digest;
    requireCheck(digest === sha256(standalone), 'helper-standalone-byte-mismatch');
    report.checks.standalone_identical = true;
    run('/usr/bin/codesign', ['--verify', '--strict', helper]);
    report.checks.strict_signature = true;
    const signature = run('/usr/bin/codesign', ['--display', '--verbose=4', helper]);
    requireCheck(signature.includes('Authority=Developer ID Application:') && /flags=.*runtime/.test(signature) && signature.includes('Timestamp='), 'developer-id-runtime-timestamp-required');
    report.checks.developer_id_runtime_timestamp = true;
    requireCheck(run('/usr/bin/lipo', ['-archs', helper]).trim().split(/\s+/).includes(arch), 'artifact-architecture-mismatch');
    const identity = json(run(helper, ['--build-identity']));
    requireCheck(identity.git_sha === sha && identity.version === version, 'artifact-identity-mismatch');
    report.checks.build_identity = true;
    for (const directory of ['home', 'state', 'data', 'tmp', 'logs']) fs.mkdirSync(path.join(output, directory), { mode: 0o700 });
    const port = await allocatePort();
    const password = crypto.randomBytes(32).toString('hex');
    const instance = crypto.randomUUID();
    const env = {
      PATH: process.env.PATH, HOME: path.join(output, 'home'), TMPDIR: path.join(output, 'tmp'),
      PHOENIX_STATE_DIR: path.join(output, 'state'), PHOENIX_DATA_DIR: path.join(output, 'data'),
      PHOENIX_DB_PATH: path.join(output, 'data/qa.db'), PHOENIX_TMP_DIR: path.join(output, 'tmp'),
      PHOENIX_PORT: String(port), PHOENIX_BIND_ADDR: '127.0.0.1', PHOENIX_TLS: 'off',
      PHOENIX_PASSWORD: password, PHOENIX_INSTANCE_ID: instance,
      PHOENIX_LOG_FILE: path.join(output, 'logs/server.log'), RUST_LOG: 'warn',
    };
    child = launch(helper, [], { env, stdio: 'ignore', cwd: output });
    exit = new Promise(resolve => {
      child.once('error', () => resolve({ code: null, signal: null, spawn_error: true }));
      child.once('exit', (code, signal) => resolve({ code, signal }));
    });
    let spawnFailed = false;
    child.once('error', () => { spawnFailed = true; });
    const alive = () => !spawnFailed && child.exitCode === null && child.signalCode === null;
    const deadline = Date.now() + 60000;
    let observed;
    while (Date.now() < deadline) {
      requireCheck(alive(), 'helper-exited-before-ready');
      try {
        const response = await request(port, '/api/version', null, null, 1000);
        if (response.status === 200) { observed = json(response.text); break; }
      } catch (error) {
        if (error instanceof AcceptanceFailure && error.message === 'invalid-json-response') throw error;
      }
      await pause(100);
    }
    requireCheck(observed && observed.git_sha === sha && observed.version === version && observed.socket_activated === false, 'runtime-identity-or-readiness-failed');
    const unauthorized = await request(port, '/api/deployment');
    requireCheck(unauthorized.status === 401, 'expected-auth-boundary');
    const login = await request(port, '/api/auth/login', JSON.stringify({ password }));
    const cookie = (login.headers['set-cookie'] || []).find(value => value.startsWith('phoenix-auth='));
    requireCheck(login.status === 200 && cookie, 'normal-isolated-login-failed');
    const deployment = await request(port, '/api/deployment', null, cookie.split(';')[0]);
    requireCheck(deployment.status === 200, 'authenticated-deployment-failed');
    const info = json(deployment.text);
    requireCheck(info.instance_id === instance && info.build?.git_sha === sha && info.build?.version === version && alive(), 'launched-instance-mismatch');
    report.checks.runtime_identity = true;
    report.checks.launched_instance = true;
    report.checks.auth_statuses = [unauthorized.status, login.status, deployment.status];
    stopRequested = true;
    child.kill('SIGTERM');
    const outcome = await waitForExit(exit, 40000);
    requireCheck(outcome && outcome.code === 0 && outcome.signal === null, 'graceful-sigterm-exit-not-zero');
    report.checks.graceful_exit_zero = true;
    requireCheck(await listenerClosed(port), 'listener-survived-stop');
    report.checks.listener_closed = true;
    const database = path.join(output, 'data/qa.db');
    requireCheck(fs.existsSync(database), 'qa-database-missing');
    requireCheck(run('/usr/bin/sqlite3', [database, 'PRAGMA integrity_check;']).trim() === 'ok', 'database-integrity-failed');
    requireCheck(run('/usr/bin/sqlite3', [database, 'PRAGMA foreign_key_check;']).trim() === '', 'database-foreign-keys-failed');
    requireCheck(sha256(helper) === digest && sha256(standalone) === digest, 'artifact-bytes-changed');
    if (archive) requireCheck(sha256(archive) === archiveDigest, 'archive-bytes-changed');
    report.checks.database_integrity = 'ok';
    report.checks.foreign_key_violations = 0;
    report.checks.artifacts_unchanged = true;
    report.result = 'passed';
    fs.writeFileSync(path.join(output, 'acceptance-receipt.json'), JSON.stringify(report, null, 2) + '\n', { mode: 0o600 });
    return report;
  } catch (error) {
    if (child && exit && child.exitCode === null && child.signalCode === null) {
      if (!stopRequested) { child.kill('SIGTERM'); await waitForExit(exit, 40000); }
      if (child.exitCode === null && child.signalCode === null) { child.kill('SIGKILL'); await waitForExit(exit, 5000); }
    }
    if (report) {
      report.result = 'failed';
      report.failure_code = error instanceof AcceptanceFailure ? error.message : 'check-failed-details-withheld';
      fs.writeFileSync(path.join(output, 'acceptance-receipt.json'), JSON.stringify(report, null, 2) + '\n', { mode: 0o600 });
    }
    throw new AcceptanceFailure(error instanceof AcceptanceFailure ? error.message : 'check-failed-details-withheld');
  }
}

module.exports = { checkSignedHelper };
if (require.main === module) {
  const [archive, standalone, sha, version, target, output, ...extra] = process.argv.slice(2);
  if (extra.length || !output) {
    console.error('Usage: check-signed-helper.cjs APP_ZIP STANDALONE SHA VERSION TARGET FRESH_OUTPUT');
    process.exitCode = 1;
  } else {
    checkSignedHelper({ archive, standalone, sha, version, target, output }).then(() => {
      console.log('Signed-helper headless acceptance passed; sanitized receipt saved.');
    }).catch(() => {
      console.error('Signed-helper acceptance failed; private details withheld.');
      process.exitCode = 1;
    });
  }
}
