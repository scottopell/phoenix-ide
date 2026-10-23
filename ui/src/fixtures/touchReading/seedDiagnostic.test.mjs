import { test } from 'node:test';
import assert from 'node:assert/strict';
import { installSeedDiagnostic } from './seedDiagnostic.mjs';

test('failed synchronous seed retains bounded private-free diagnostics before teardown', () => {
  const previousWindow = globalThis.window;
  globalThis.window = {};
  try {
    installSeedDiagnostic();
    const receipt = { status: 'UNOBSERVED', phase: 'seed', reason: 'live connection unavailable', text: 'SECRET', requestId: 'SECRET' };
    const observer = { connectionState: 'connecting', notifications: 3, text: 'SECRET', requestId: 'SECRET' };
    const bridge = { status: () => ({ consumption: receipt }), diagnostic: () => ({
      initialized: true, sourceState: 1, tokenListeners: 1, emitted: 1, observer,
    }) };
    window.__touchCapture('before-arm', bridge);
    window.__touchCapture('after-arm', bridge);
    window.__touchCapture('after-emit', bridge);
    assert.throws(() => {
      try { throw new Error('UNOBSERVED'); }
      catch (error) { window.__touchCapture('failed', bridge); throw error; }
    }, /UNOBSERVED/);
    const retained = JSON.parse(JSON.stringify(window.__touchSeedDiagnostic));
    receipt.reason = 'observer disposed'; observer.notifications = 99;
    window.__touchCapture('failed', bridge);
    assert.equal(window.__touchSeedDiagnostic.length, 4);
    assert.equal(retained[3].receipt.reason, 'live connection unavailable');
    assert.equal(retained[3].receipt.status, 'UNOBSERVED');
    assert.equal(retained[3].observer.notifications, 3);
    assert.equal(JSON.stringify(retained).includes('SECRET'), false);
    assert.equal(window.__touchSeedDiagnostic[3].observer.notifications, 3);
    receipt.reason = 'SECRET';
    window.__touchCapture('before-arm', bridge);
    assert.equal(window.__touchSeedDiagnostic.length, 1);
    assert.equal(window.__touchSeedDiagnostic[0].receipt.reason, 'unrecognized reason');
  } finally {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  }
});
