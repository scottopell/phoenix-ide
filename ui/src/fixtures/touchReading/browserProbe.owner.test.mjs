import { test } from 'node:test';
import assert from 'node:assert/strict';
import { installTouchProbe } from './browserProbe.mjs';

test('probe selects the production owner and rejects insufficient independent geometry', () => {
  const names = ['window', 'document', 'getComputedStyle', 'MutationObserver', 'ResizeObserver', 'requestAnimationFrame', 'cancelAnimationFrame'];
  const saved = new Map(names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)]));
  let writes = 0, count = 1, overflowY = 'auto', innerHeight = 2400, top = 1200;
  const owner = { get scrollTop() { return top; }, set scrollTop(value) { writes++; },
    scrollHeight: 2400, clientHeight: 662,
    querySelector: selector => { assert.equal(selector, ':scope > .virtual-transcript__inner'); return { getBoundingClientRect: () => ({ height: innerHeight }) }; },
    addEventListener() {}, removeEventListener() {},
    getBoundingClientRect: () => ({ left: 0, top: 80, width: 390, height: 662 }),
  };
  try {
    globalThis.window = {};
    globalThis.document = { querySelectorAll: selector => {
      assert.equal(selector, '.product-conversation-page__transcript #messages.virtual-transcript');
      return Array(count).fill(owner);
    }, addEventListener() {}, removeEventListener() {} };
    globalThis.getComputedStyle = element => { assert.equal(element, owner); return { overflowY }; };
    globalThis.MutationObserver = globalThis.ResizeObserver = class { observe() {} disconnect() {} };
    globalThis.requestAnimationFrame = () => 1;
    globalThis.cancelAnimationFrame = () => {};
    for (count of [0, 2]) assert.throws(() => installTouchProbe({ transcriptId: 'fixture' }), /Expected one/);
    count = 1;
    overflowY = 'hidden';
    assert.throws(() => installTouchProbe({ transcriptId: 'fixture' }), /lacks upward-pan geometry/);
    overflowY = 'auto'; owner.scrollHeight = 662;
    assert.throws(() => installTouchProbe({ transcriptId: 'fixture' }), /lacks upward-pan geometry/);
    owner.scrollHeight = 2400; innerHeight = 662;
    assert.throws(() => installTouchProbe({ transcriptId: 'fixture' }), /lacks upward-pan geometry/);
    innerHeight = 2400; top = 0;
    assert.throws(() => installTouchProbe({ transcriptId: 'fixture' }), /lacks upward-pan geometry/);
    top = 1200;
    installTouchProbe({ transcriptId: 'fixture' });
    assert.equal(window.__touchProbe.snapshot().events[0].kind, 'scroll-owner');
    assert.equal(window.__touchProbe.snapshot().events[0].innerHeight, 2400);
    assert.deepEqual(window.__touchProbe.bounds(), { x: 0, y: 80, width: 390, height: 662 });
    assert.equal(writes, 0);
    assert.throws(() => window.__touchProbe.acquire(), /Native pan not acquired/);
    window.__touchProbe.stop();
  } finally {
    for (const [name, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
  }
});
