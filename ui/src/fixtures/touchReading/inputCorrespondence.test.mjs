import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { installInputCorrespondence } from './inputCorrespondence.mjs';

test('intent never becomes observation; exact CSS coordinates fail closed', () => {
  const saved = Object.getOwnPropertyDescriptor(globalThis, 'window');
  globalThis.window = {};
  try {
    installInputCorrespondence();
    const intent = Object.freeze({ ordinal: 11, type: 'touchMove', x: 24.9296875, y: 171 });
    const observed = Object.freeze({ x: 24.9296875, y: 164 });
    const check = window.__touchInputCorrespondence;
    assert.equal(check(intent, observed).status, 'MISMATCH');
    assert.equal(check(intent, null).status, 'MISMATCH');
    assert.equal(check(intent, { x: intent.x, y: 171 }).status, 'MATCHED');
    assert.equal(check(intent, { x: intent.x + 0.000001, y: 171 }).status, 'MISMATCH');
    assert.equal(check({ ...intent, x: NaN }, observed).status, 'MISMATCH');
    assert.equal(check({ ...intent, ordinal: 0 }, observed).status, 'MISMATCH');
    assert.deepEqual(observed, { x: 24.9296875, y: 164 });
    assert.deepEqual(Object.keys(window), ['__touchInputCorrespondence']);
  } finally {
    if (saved) Object.defineProperty(globalThis, 'window', saved); else delete globalThis.window;
  }
});
test('mismatch check precedes baseline writes and does not replace existing acquisition gates', () => {
  const source = readFileSync(new URL('./browserProbe.mjs', import.meta.url), 'utf8');
  const start = source.indexOf('    acquire(intent) {');
  assert.ok(start >= 0);
  const body = source.slice(start, source.indexOf('    phase(value) {', start));
  const rejection = body.indexOf("throw new Error('Input intent/DOM observation mismatch before baseline')");
  assert.ok(rejection >= 0);
  for (const assignment of ['anchor =', 'baseline =']) assert.ok(body.indexOf(assignment) > rejection);
  assert.doesNotMatch(body.slice(0, rejection), /finger\s*=|anchor\s*=|baseline\s*=/);
  assert.match(body, /Native pan not acquired/);
  assert.match(body, /caretRangeFromPoint\(finger.x, finger.y\)/);
  assert.match(body, /Semantic anchor unobservable/);
  assert.doesNotMatch(body, /__touchTextContact|\.contact\(/);
});
