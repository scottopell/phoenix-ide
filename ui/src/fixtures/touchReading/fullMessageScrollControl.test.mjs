import test from 'node:test';
import assert from 'node:assert/strict';
import { inspectFullMessageScrollControl } from './fullMessageScrollControl.mjs';
const fixture = () => {
  const p = (t, y, scrollTop) => ({ t, x: 100, y, scrollTop });
  const sample = (t, dy) => ({ ...p(t, 100 + dy, 300 - dy), down: true, dx: 0, dy });
  return {
    trace: { failure: null, baseline: p(0, 100, 300), anchor: { occurrence: 'test:message', fragmentId: 'text', unitKey: 'row' },
      contacts: [{ t: 0, type: 'touchstart', trusted: true, owned: true, id: 1, x: 100, y: 100, active: 1 },
        { t: 2300, type: 'touchend', trusted: true, owned: true, id: 1, x: 100, y: 100, active: 0 }],
      samples: [sample(2100, 0), sample(2140, 12), sample(2160, 12), sample(2220, 0)] },
    receipt: { kind: 'SCROLL_DISPLACEMENT_DETECTOR_ONLY', status: 'PHYSICAL_ACTION_OBSERVED', failure: null,
      before: p(2110, 100, 300), applyRequestedAt: 2111, applied: p(2112, 112, 288),
      frames: [2120, 2130, 2140, 2150, 2160, 2170].map(t => ({ ...p(t, 112, 288), admitted: true,
        heldReady: true, ownerConnected: true, hitOwned: true, caretOwned: true,
        contactEvidence: 'ORIGINAL_TRUSTED_TRACE_CONTACT_NO_NEW_HARDWARE_EVENT' })),
      held: p(2200, 112, 288), restoreRequestedAt: 2201, restored: p(2202, 100, 300) },
  };
};
test('synthetic arithmetic accepts independently receipted 12px and restored trajectory, not browser qualification', () => {
  const { trace, receipt } = fixture();
  assert.equal(inspectFullMessageScrollControl(trace, receipt).status, 'SCROLL_DISPLACEMENT_DETECTOR_OBSERVED');
});
for (const [name, mutate] of [
  ['zero detector response', ({ trace }) => { trace.samples[1].y = 100; trace.samples[1].dy = 0; }],
  ['six-pixel response', ({ trace }) => { trace.samples[1].y = 106; trace.samples[1].dy = 6; }],
  ['clamped independent scroll', ({ receipt }) => { receipt.applied.scrollTop = 294; }],
  ['wrong identifier', ({ trace }) => { trace.contacts[1].id = 2; }],
  ['ended before control', ({ trace }) => { trace.contacts[1].t = 1000; }],
  ['untrusted start', ({ trace }) => { trace.contacts[0].trusted = false; }],
  ['lost anchor', ({ trace }) => { trace.failure = 'anchor-removed-no-reresolution'; }],
  ['missing independent receipt', ({ receipt }) => { receipt.applied = null; }],
  ['missing restore', ({ receipt }) => { receipt.restored = null; }],
  ['restoration mismatch', ({ receipt }) => { receipt.restored.scrollTop = 299; }],
  ['nonfinite geometry', ({ trace }) => { trace.samples[1].x = NaN; }],
  ['intermediate compensated frame', ({ receipt }) => { receipt.frames[2].scrollTop = 300; receipt.frames[2].y = 100; }],
  ['intermediate wrong geometry', ({ receipt }) => { receipt.frames[2].y = 106; }],
  ['missing sixth frame', ({ receipt }) => { receipt.frames.pop(); }],
  ['reversed control frame timestamps', ({ receipt }) => { receipt.frames[2].t = 2125; }],
  ['reversed raw trajectory timestamps', ({ trace }) => { [trace.samples[1], trace.samples[2]] = [trace.samples[2], trace.samples[1]]; }],
  ['nonfinite raw timestamp', ({ trace }) => { trace.samples[1].t = NaN; }],
  ['intermediate lifecycle loss', ({ receipt }) => { receipt.frames[2].admitted = false; }],
  ['intermediate foreign hit', ({ receipt }) => { receipt.frames[2].hitOwned = false; }],
  ['baseline reset', ({ trace }) => { trace.baseline.y = 112; }],
]) test(`synthetic arithmetic rejects ${name}`, () => {
  const f = fixture(); mutate(f);
  assert.equal(inspectFullMessageScrollControl(f.trace, f.receipt).status, 'UNOBSERVABLE');
});
