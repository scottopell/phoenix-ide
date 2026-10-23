import { test } from 'node:test';
import assert from 'node:assert/strict';
import { inspectRegrabAccuracy, installRegrabAccuracyControl } from './regrabAccuracy.mjs';
function fixture(dy = 12) {
  const frame = (kind, t, delta) => ({ kind, t, x: 100, y: 200 + delta, dx: 0, dy: delta });
  return { trace: { status: 'OBSERVED_UNATTRIBUTED', overflow: false, events: [
    frame('pre-dispatch', 0, 0), frame('at-touchstart', 10, 0),
    frame('first-raf', 20, dy), frame('hold-frame', 2020, dy),
  ] }, receipt: { kind: 'deliberate-accuracy-control', mode: dy === 0 ? 'stable' : 'touchdown-shift',
    connected: true, commandedDy: dy, measuredDy: dy, measuredDx: 0, t: 11 } };
}
for (const dy of [0, 12]) test(`exact ${dy}px control is observed, never product-qualified`, () => {
  const r = fixture(dy), result = inspectRegrabAccuracy(r.trace, r.receipt);
  assert.equal(result.status, 'CONTROL_OBSERVED'); assert.equal(result.measuredDy, dy);
  assert.equal(result.productQualification, 'NOT_QUALIFIED');
});
test('first RAF baseline reset cannot mask a known jump', () => {
  const r = fixture(); r.trace.events[2].dy = 0;
  const result = inspectRegrabAccuracy(r.trace, r.receipt);
  assert.equal(result.status, 'FAILED'); assert.ok(result.failures.includes('baseline-displacement-inconsistent'));
});
test('wrong magnitude, absent receipt and incomplete identity are not passes', () => {
  const r = fixture(); r.receipt.measuredDy = 11;
  assert.equal(inspectRegrabAccuracy(r.trace, r.receipt).status, 'FAILED');
  assert.equal(inspectRegrabAccuracy(r.trace, null).status, 'UNOBSERVABLE');
  r.trace.status = 'UNOBSERVABLE';
  assert.equal(inspectRegrabAccuracy(r.trace, r.receipt).status, 'UNOBSERVABLE');
});
test('stable negative rejects actual displacement instead of expanding tolerance', () => {
  const r = fixture(0); r.trace.events[2].y++; r.trace.events[2].dy++;
  assert.equal(inspectRegrabAccuracy(r.trace, r.receipt).status, 'FAILED');
});

for (const [name, change] of [
  ['infinite hold', r => { r.trace.events[3].t = Infinity; }],
  ['NaN hold', r => { r.trace.events[3].t = NaN; }],
  ['backwards hold', r => { r.trace.events[3].t = 5; }],
  ['hold before first RAF in array', r => { r.trace.events.splice(2, 0, r.trace.events.pop()); }],
  ['duplicate hold timestamp', r => { r.trace.events.push({ ...r.trace.events[3] }); }],
  ['infinite receipt', r => { r.receipt.t = Infinity; }],
  ['hold after release', r => { r.trace.events.splice(3, 0, { ...r.trace.events[2], kind: 'at-release', t: 30 }); }],
]) test(`rejects ${name}`, () => {
  const r = fixture(); change(r);
  assert.equal(inspectRegrabAccuracy(r.trace, r.receipt).status, 'UNOBSERVABLE');
});

function installerFixture(mode, run, { noOp = false, existingTransform = 'none' } = {}) {
  const keys = ['window', 'document', 'Element', 'getComputedStyle', '__regrabAccuracyControl'];
  const saved = keys.map(k => [k, Object.getOwnPropertyDescriptor(globalThis, k)]);
  const listeners = [], order = [], writes = [];
  let transform = '', controller;
  class FakeElement {}
  const block = new FakeElement();
  block.matches = () => true; block.contains = n => n === block; block.isConnected = true;
  block.style = {};
  Object.defineProperty(block.style, 'transform', { get: () => transform, set: v => { writes.push(v); transform = v; } });
  block.getBoundingClientRect = () => ({ left: 100, top: 200 + (!noOp && transform === 'translateY(12px)' ? 12 : 0) });
  const owner = { contains: n => n === block };
  const observed = () => { order.push(['observer', transform]); };
  listeners.push(observed);
  try {
    globalThis.window = globalThis; globalThis.Element = FakeElement;
    globalThis.getComputedStyle = () => ({ transform: existingTransform, translate: 'none', transitionDuration: '0s' });
    globalThis.document = {
      querySelector: () => owner, elementFromPoint: () => block,
      addEventListener: (_type, callback, options) => { assert.deepEqual(options, { capture: true, passive: true }); listeners.push(callback); },
      removeEventListener: (_type, callback, capture) => { assert.equal(capture, true); const i = listeners.indexOf(callback); if (i >= 0) listeners.splice(i, 1); },
    };
    installRegrabAccuracyControl({ mode }); controller = window.__regrabAccuracyControl;
    const start = (foreign = false) => {
      const contact = { clientX: 100, clientY: 200 };
      const event = { isTrusted: true, touches: [contact], changedTouches: [contact], composedPath: () => foreign ? [block] : [block, owner] };
      for (const listener of [...listeners]) listener(event);
    };
    run({ start, controller, writes, order, listeners });
  } finally {
    controller?.stop(); assert.deepEqual(listeners, [observed]); assert.equal(transform, '');
    for (const [key, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}
test('control follows observer, counts only owned starts, writes exactly once and restores', () => installerFixture('touchdown-shift', r => {
  r.start(true); r.start(); assert.deepEqual(r.writes, []);
  r.start(); assert.deepEqual(r.writes, ['translateY(12px)']);
  assert.equal(r.order.at(-1)[1], '');
  assert.equal(r.controller.receipt().measuredDy, 12);
  r.start(); assert.equal(r.writes.length, 1);
}));
test('stable control makes no measurement-window write', () => installerFixture('stable', r => {
  r.start(); r.start(); assert.deepEqual(r.writes, []);
  assert.equal(r.controller.receipt().measuredDy, 0);
}));
test('physical no-op is retained as zero and oracle rejects claimed12', () => installerFixture('touchdown-shift', r => {
  r.start(); r.start(); const receipt = r.controller.receipt();
  assert.equal(receipt.measuredDy, 0);
  const f = fixture(); receipt.t = 11;
  assert.equal(inspectRegrabAccuracy(f.trace, receipt).status, 'FAILED');
}, { noOp: true }));
test('preexisting transform is rejected without writes or receipt', () => installerFixture('touchdown-shift', r => {
  r.start(); r.start(); assert.deepEqual(r.writes, []); assert.equal(r.controller.receipt(), null);
}, { existingTransform: 'matrix(1,0,0,1,0,2)' }));

test('installer diagnostic distinguishes foreign target, first ordinal and published stable receipt', () => installerFixture('stable', r => {
  assert.equal(r.controller.diagnostic().installed, true);
  r.start(true);
  assert.equal(r.controller.diagnostic().reason, 'target-mismatch');
  assert.equal(r.controller.diagnostic().ownedStarts, 0);
  r.start(); assert.equal(r.controller.diagnostic().reason, 'not-second-owned');
  r.start();
  const d = r.controller.diagnostic();
  assert.equal(d.receiptPublished, true); assert.equal(d.reason, 'receipt-published');
  assert.equal(d.counts['target-mismatch'], 1);
  assert.equal(d.counts['not-second-owned'], 1);
  assert.deepEqual(r.writes, []);
  r.controller.stop();
  assert.equal(r.controller.diagnostic().restoreResult, 'inline-value-restored');
  assert.equal(r.controller.diagnostic().listenerRemoved, true);
}));
test('null receipt with style rejection is diagnostic only, never control success', () => installerFixture('stable', r => {
  r.start(); r.start();
  assert.equal(r.controller.receipt(), null);
  assert.equal(r.controller.diagnostic().reason, 'style-precondition');
  assert.equal(r.controller.diagnostic().receiptPublished, false);
  r.controller.stop();
  assert.equal(r.controller.diagnostic().restoreAttempted, false);
  assert.equal(r.controller.diagnostic().restoreResult, 'no-target');
  assert.equal(inspectRegrabAccuracy(fixture(0).trace, null).status, 'UNOBSERVABLE');
}, { existingTransform: 'matrix(1,0,0,1,0,2)' }));
