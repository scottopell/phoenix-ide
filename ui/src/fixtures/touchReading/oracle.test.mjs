// Pure Node/assert; no DOM, browser, server, dependencies, or test framework.
import assert from 'node:assert/strict';
import { calibrate, expectedDetection, inspectTimeline, phases } from './oracle.mjs';
import { cases, scheduleFor, seed } from './schedule.mjs';
import { installSemanticResolver } from './semanticAnchor.mjs';
const resolver = installSemanticResolver();
const anchor = { occurrence: 'member:message', fragmentId: 'agent-text-0', blockText: 'first second third', logicalOffset: 7, quoteStart: 0, quoteEnd: 18, quote: 'first second third' };
const block = segments => ({ occurrence: anchor.occurrence, fragmentId: anchor.fragmentId, segments });
assert.deepEqual(resolver.resolve([block(['first second third'])], anchor), { missing: null, segment: 0, offset: 7 });
assert.deepEqual(resolver.resolve([block(['first ', 'second', ' third'])], anchor), { missing: null, segment: 1, offset: 1 });
assert.deepEqual(resolver.resolve([block(['first s', '', 'econd third'])], anchor), { missing: null, segment: 2, offset: 0 });
assert.equal(resolver.resolve([block(['first second third']), block(['first second third'])], anchor).missing, 'exact-block-count:2');
assert.equal(resolver.resolve([{ ...block(['first second third']), occurrence: 'other:message' }], anchor).missing, 'exact-block-count:0');
assert.equal(resolver.resolve([block(['first altered third'])], anchor).missing, 'logical-text-offset-quote-lost');
// Constructed source fixtures only: no DOM, remount or browser execution is claimed.
const causalTrace = (missing = null) => ({
  frames: [0, 1, 2, 3, 4, 5].map(t => ({ phase: 'hold', t, x: 10, y: t >= 2 && !missing ? 80 : 20, expectedX: 10, expectedY: 20, missing: t >= 2 ? missing : null })),
  events: [{ kind: 'control-injection', name: 'forced-scroll-bottom', phase: 'hold', t: 1.5, fromFrame: 2 },
    { kind: 'control-ack', name: 'forced-scroll-bottom', phase: 'hold', t: 5.5, fromFrame: 2, toFrame: 6 }],
});
const causal = causalTrace();
assert.equal(expectedDetection('forced-scroll-bottom', inspectTimeline(causal.frames, 2), causal, 2), true);
const late = causalTrace(); late.frames.slice(2, 6).forEach(f => { f.y = 20; });
late.frames.push({ ...late.frames[5], t: 20, y: 80 });
assert.equal(expectedDetection('forced-scroll-bottom', inspectTimeline(late.frames, 2), late, 2), false);
const confounded = causalTrace(); confounded.events.push({ kind: 'schedule', type: 'reflow', t: 3 });
assert.equal(expectedDetection('forced-scroll-bottom', inspectTimeline(confounded.frames, 2), confounded, 2), false);
const slow = causalTrace(); slow.events[1].t = 202;
assert.equal(expectedDetection('forced-scroll-bottom', inspectTimeline(slow.frames, 2), slow, 2), false);
const prefailed = causalTrace(); prefailed.frames[0].y = 80;
assert.equal(expectedDetection('forced-scroll-bottom', inspectTimeline(prefailed.frames, 2), prefailed, 2), false);
const invalidControl = causalTrace('deliberate-oracle-anchor-invalid');
invalidControl.events.forEach(e => { e.name = 'oracle-anchor-invalid'; });
assert.equal(expectedDetection('oracle-anchor-invalid', inspectTimeline(invalidControl.frames, 2), invalidControl, 2), true);
assert.deepEqual(cases.slice(0, 4), ['no-stream', 'forced-scroll-bottom', 'uncompensated-above-height', 'oracle-anchor-invalid']);
for (const name of cases.slice(1, 4)) {
  const plan = scheduleFor(name, 'id');
  assert.equal(plan.at(-1).type, 'control');
  assert.equal(plan.some(s => s.type === 'reflow' || (s.type === 'tokens' && s.at > 300)), false);
}
const frames = () => phases.map((phase, t) => ({ phase, t, x: 10, y: 20, expectedX: 10, expectedY: 20, missing: null }));
assert.equal(calibrate(frames()), 2);
assert.equal(inspectTimeline(frames(), 2).ok, true);
const spike = frames(); spike.splice(1, 0, { ...spike[0], t: 0.5, y: 120 });
assert.equal(inspectTimeline(spike, 2).maxima.drag, 100);
assert.equal(inspectTimeline(spike, 2).ok, false); // A single bad frame cannot be averaged away.
assert.throws(() => calibrate(spike), /calibration failed/);
const missing = frames(); missing[1].missing = 'exact-occurrence-count:0';
assert.equal(inspectTimeline(missing, 2).ok, false);
assert.equal(expectedDetection('oracle-anchor-invalid', inspectTimeline(missing, 2)), false);
assert.equal(expectedDetection('uncompensated-above-height', inspectTimeline(missing, 2)), false);
missing[1].missing = 'deliberate-oracle-anchor-invalid';
assert.equal(expectedDetection('oracle-anchor-invalid', inspectTimeline(missing, 2)), false); // No causal window supplied.
missing[1].missing = 'exact-occurrence-count:2';
assert.equal(inspectTimeline(missing, 2).failures[0].kind, 'observability');
assert.equal(inspectTimeline(frames().slice(1), 2).ok, false);
const release = frames(); release[3].x += 9;
assert.equal(inspectTimeline(release, 2).maxima.post_release, 9);
assert.equal(expectedDetection('forced-scroll-bottom', inspectTimeline(release, 2)), false); // A later release jump cannot license a hold control.
assert.equal(expectedDetection('uncompensated-above-height', inspectTimeline(frames(), 2)), false);
assert.equal(expectedDetection('stream-chunks', inspectTimeline(release, 2)), false);
const invalid = frames(); invalid[0].x = NaN;
assert.equal(inspectTimeline(invalid, 2).failures[0].kind, 'observability');
const noisy = frames(); noisy[0].y += 6;
assert.equal(calibrate(noisy), 8); // Hard ceiling: a larger baseline is not licensed by a later failure.
assert.equal(seed, 1729); assert.equal(cases.length, 6);
assert.deepEqual(scheduleFor('stream-burst-reflow', 'id'), scheduleFor('stream-burst-reflow', 'id'));
assert.deepEqual(scheduleFor('no-stream', 'id'), []);
assert.equal(scheduleFor('forced-scroll-bottom', 'id').filter(s => s.type === 'control').length, 1);
console.log('touch reading pure oracle assertions passed');
