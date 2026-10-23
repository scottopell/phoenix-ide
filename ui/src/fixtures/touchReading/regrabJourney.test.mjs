import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runRegrabJourney } from './regrabJourney.mjs';

test('separate fixed journey sends no held move and preserves observation nonqualification', async () => {
  let clock = 0, evaluation = 0;
  const calls = [], pauses = [];
  const result = await runRegrabJourney({
    page: { evaluate: async () => [{ x: 10, y: 20 }, { x: 30, y: 40 },
      { status: 'OBSERVED_UNATTRIBUTED' }][evaluation++] },
    dispatch: async (type, x, y) => { calls.push({ type, x, y, clock }); },
    pause: async ms => { pauses.push(ms); clock += ms; }, now: () => clock,
  });
  assert.deepEqual(calls.map(c => c.type), ['touchStart', ...Array(5).fill('touchMove'), 'touchEnd', 'touchStart', 'touchEnd']);
  assert.deepEqual(pauses, [16, 16, 16, 16, 16, 240, 2600, 350]);
  assert.equal(calls[5].y - calls[0].y, 160);
  assert.deepEqual(calls.slice(7).map(({ x, y }) => [x, y]), [[30, 40], [30, 40]]);
  assert.equal(result.qualification, 'NOT_QUALIFIED');
  assert.equal(result.causalAttribution, 'UNKNOWN');
  assert.equal(result.error, null);
});
test('unobservable second contact never becomes success or a retry', async () => {
  let evaluations = 0, commands = 0;
  const result = await runRegrabJourney({
    page: { evaluate: async () => {
      if (++evaluations === 1) return { x: 10, y: 20 };
      if (evaluations === 2) throw new Error('anchor lost');
      return { status: 'UNOBSERVABLE' };
    } }, dispatch: async () => { commands++; }, pause: async () => {}, now: () => 0,
  });
  assert.equal(commands, 7); assert.equal(evaluations, 3);
  assert.ok(result.error); assert.equal(result.trace.status, 'UNOBSERVABLE');
});
