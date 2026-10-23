import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { installTextContactSelector } from './textContact.mjs';

function fixture(change = () => {}) {
  const keys = ['window', 'document', 'Node', 'NodeFilter'];
  const saved = keys.map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]);
  const node = { nodeType: 3, data: 'A', isConnected: true };
  const hit = {};
  const row = { isConnected: true, dataset: { renderUnitKey: 'unit' }, contains: item => item === message };
  const message = { isConnected: true, dataset: { messageId: 'reading-1', messageOccurrence: 'fixture:reading-1' },
    closest: () => row, contains: item => item === block || item === hit };
  const block = { isConnected: true, dataset: { fragmentId: 'fragment' }, closest: () => message,
    contains: item => item === node || item === hit };
  node.parentElement = { closest: () => block };
  const owner = { isConnected: true, contains: item => item === row || item === hit,
    getBoundingClientRect: () => ({ left: 0, right: 390, top: 0, bottom: 600, width: 390, height: 600 }) };
  const state = { node, block, message, row, owner, hit, traversal: [node], offset: 0,
    rect: { left: 100, top: 100, width: 8, height: 16 },
    caret: () => ({ startContainer: node, startOffset: state.offset }), hitAt: () => hit };
  try {
    change(state);
    globalThis.window = {};
    globalThis.Node = { TEXT_NODE: 3 }; globalThis.NodeFilter = { SHOW_ALL: -1 };
    globalThis.document = {
      createTreeWalker: () => { let index = 0; return { nextNode: () => state.traversal[index++] ?? null }; },
      createRange: () => ({ setStart: (_, offset) => { state.offset = offset; }, setEnd() {}, getClientRects: () => [state.rect] }),
      caretRangeFromPoint: () => state.caret(), elementFromPoint: () => state.hitAt(),
    };
    installTextContactSelector();
    return window.__touchTextContact(owner, 'fixture');
  } finally {
    for (const [key, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}
test('deterministic positive BMP code-unit contact; boundary geometry not universal grapheme proof', () => {
  assert.deepEqual(fixture(), fixture());
  assert.equal(fixture().x, 104);
  assert.equal(fixture(s => { s.caret = () => ({ startContainer: s.node, startOffset: s.offset + 1 }); }).y, 108);
  assert.equal(fixture(s => { s.node.data = '\u0301'; }).x, 104);
});
for (const [name, change] of [
  ['toolbar', s => { s.node.parentElement.closest = () => null; }],
  ['blank', s => { s.node.data = '   '; }],
  ['surrogate', s => { s.node.data = '\ud83d\ude00'; }],
  ['wrong occurrence', s => { s.message.dataset.messageOccurrence = 'foreign'; }],
  ['missing unit', s => { s.row.dataset.renderUnitKey = ''; }],
  ['occluded', s => { s.hitAt = () => ({}); }],
  ['virtualized during hit', s => { s.hitAt = () => { s.node.isConnected = false; return s.hit; }; }],
  ['out of viewport', s => { s.rect.left = 500; }],
  ['path room', s => { s.rect.top = 550; }],
  ['other caret', s => { s.caret = () => ({ startContainer: {}, startOffset: 0 }); }],
  ['adjacent offset', s => { s.caret = () => ({ startContainer: s.node, startOffset: 2 }); }],
]) test(`rejects ${name}`, () => assert.throws(() => fixture(change), /no-eligible-code-unit/));
test('bounds traversal even with no candidate or huge whitespace', () => {
  assert.throws(() => fixture(s => { s.traversal = Array(8193).fill({ nodeType: 1 }); }), /node-budget/);
  assert.throws(() => fixture(s => { s.node.data = ' '.repeat(16385); }), /code-unit-budget/);
  assert.throws(() => fixture(s => { s.node.data = 'A'.repeat(4097); s.rect.width = 0; }), /range-budget/);
});
test('selector is pregesture only, with no layout writes or acquire reselection', () => {
  const helper = readFileSync(new URL('./textContact.mjs', import.meta.url), 'utf8');
  assert.doesNotMatch(helper, /scrollTop\s*=|scrollTo\(|getSelection\(|appendChild\(/);
  const probe = readFileSync(new URL('./browserProbe.mjs', import.meta.url), 'utf8');
  const acquire = probe.slice(probe.indexOf('    acquire(intent) {'), probe.indexOf('    phase(value) {'));
  assert.doesNotMatch(acquire, /__touchTextContact|\.contact\(/);
  assert.match(acquire, /caretRangeFromPoint\(finger.x, finger.y\)/);
  assert.equal((probe.match(/__touchTextContact\(/g) ?? []).length, 1);
});
