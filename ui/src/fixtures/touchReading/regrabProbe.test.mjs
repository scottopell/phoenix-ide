import { test } from 'node:test';
import assert from 'node:assert/strict';
import { installRegrabProbe } from './regrabProbe.mjs';
import { installSemanticResolver } from './semanticAnchor.mjs';

function fixture(run) {
  const keys = ['window', 'document', 'Node', 'NodeFilter', 'getComputedStyle', 'requestAnimationFrame', 'cancelAnimationFrame', '__touchSemanticResolver', '__touchTextContact', '__createRegrabObserver', '__regrabProbe'];
  const saved = keys.map(k => [k, Object.getOwnPropertyDescriptor(globalThis, k)]);
  let resolver, candidate, point, messages, blocks, nodes, hit;
  const row = { dataset: { renderUnitKey: 'unit' } };
  const block = { dataset: { fragmentId: 'block' }, isConnected: true, contains: element => element === block };
  hit = block;
  const message = { isConnected: true, dataset: { messageId: 'm', messageOccurrence: 't:m' },
    closest: () => row, contains: b => b === block, querySelectorAll: () => blocks };
  const parent = { closest: selector => selector.includes('data-message-id') ? message : block };
  const makeNode = data => ({ data, length: data.length, nodeType: 3, isConnected: true, parentElement: parent });
  nodes = [makeNode('abc')]; blocks = [block]; messages = [message];
  const owner = { isConnected: true, clientHeight: 600, scrollTop: 900,
    querySelectorAll: () => messages, contains: m => m === message,
    getBoundingClientRect: () => ({ bottom: 600 }) };
  try {
    globalThis.window = globalThis;
    globalThis.Node = { TEXT_NODE: 3 }; globalThis.NodeFilter = { SHOW_TEXT: 4 };
    globalThis.getComputedStyle = () => ({ overflowY: 'auto' });
    globalThis.requestAnimationFrame = () => 1; globalThis.cancelAnimationFrame = () => {};
    globalThis.document = {
      querySelectorAll: () => [owner],
      elementFromPoint: () => hit,
      caretRangeFromPoint: () => ({ startContainer: nodes[0], startOffset: 1 }),
      createTreeWalker: () => { let i = -1; return { nextNode() { return ++i < nodes.length; }, get currentNode() { return nodes[i]; } }; },
      createRange: () => ({ setStart() {}, setEnd() {}, getClientRects: () => [{ left: 90, right: 110, top: 190, bottom: 210 }] }),
    };
    installSemanticResolver();
    window.__touchTextContact = () => ({ x: 100, y: 200 });
    window.__createRegrabObserver = (_owner, resolve) => {
      resolver = resolve;
      return { frame() {}, arm(a) { candidate = a; point = resolve(a); return !point.missing; },
        finish: () => ({ status: 'OBSERVED_UNATTRIBUTED' }), stop() {} };
    };
    installRegrabProbe({ transcriptId: 't' });
    window.__regrabProbe.prepare();
    run({ resolve: () => resolver(candidate), initial: point,
      contact: (inside = true) => resolver(candidate, { x: 100, y: 200, path: inside ? [block, message, owner] : [owner] }),
      occlude: () => { hit = { toolbar: true }; },
      replace: () => { nodes = [makeNode('a'), makeNode('bc')]; },
      duplicateMessage: () => { messages = [message, message]; },
      duplicateBlock: () => { blocks = [block, block]; },
      disconnect: () => { owner.isConnected = false; },
      changeText: () => { nodes = [makeNode('xyz')]; },
      changeUnit: () => { row.dataset.renderUnitKey = 'other'; } });
  } finally {
    for (const [key, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}
test('semantic resolver re-resolves exact text after node segmentation replacement', () => fixture(r => {
  r.replace(); assert.deepEqual(r.resolve(), r.initial);
}));
for (const action of ['duplicateMessage', 'duplicateBlock', 'disconnect', 'changeText', 'changeUnit']) {
  test(`DOM adapter rejects ${action}`, () => fixture(r => { r[action](); assert.ok(r.resolve().missing); }));
}

test('actual fragment path and hit both required at unchanged glyph coordinates', () => fixture(r => {
  assert.equal(r.contact().ownedHit, true);
  assert.equal(r.contact(false).ownedHit, false);
  r.occlude(); assert.equal(r.contact().ownedHit, false);
}));
