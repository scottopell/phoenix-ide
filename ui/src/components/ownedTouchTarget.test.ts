import { expect, it, vi } from 'vitest';
import { OwnedTouchTargets } from './ownedTouchTarget';

function requiredTouchEvidence<T>(value: T | undefined, label: string): T {
  if (value === undefined) throw new Error('Missing touch regression evidence: ' + label);
  return value;
}

// Callback-level models, not browser trust or DOM propagation evidence.
function model() {
  const listeners: Array<{ type: string; fn: EventListener }> = [];
  const target = { addEventListener: vi.fn((type: string, fn: EventListener) => listeners.push({ type, fn })), removeEventListener: vi.fn() } as unknown as EventTarget;
  const scroller = {} as HTMLElement;
  const receive = vi.fn(); const controller = new OwnedTouchTargets(receive);
  controller.bind(scroller, 'A');
  const event = (type: string, ids = [1], trusted = true) => ({ type, target, isTrusted: trusted,
    changedTouches: ids.map(identifier => ({ identifier, target })),
    touches: type === 'touchstart' ? ids.map(identifier => ({ identifier, target })) : [],
    composedPath: () => [target, scroller],
  }) as unknown as TouchEvent;
  return { controller, receive, target, scroller, event, listeners };
}
for (const type of ['touchend', 'touchcancel']) it(`accepts ${type} once after same-owner rebind`, () => {
  const m = model(); m.controller.start(m.event('touchstart'));
  m.controller.bind(m.scroller, 'A');
  const stop = m.listeners.find(x => x.type === type)!.fn; const end = m.event(type);
  stop(end); stop(end);
  expect(m.receive).toHaveBeenCalledTimes(1);
  expect(requiredTouchEvidence(m.receive.mock.calls[0], 'first admitted receipt')[0].identifiers).toEqual([1]);
  expect(m.target.removeEventListener).toHaveBeenCalledTimes(2);
});
it('rejects untrusted start before it can prune a trusted registration', () => {
  const m = model(); m.controller.start(m.event('touchstart'));
  m.controller.start(m.event('touchstart', [2], false));
  requiredTouchEvidence(m.listeners[0], 'first target listener').fn(m.event('touchend'));
  expect(m.receive).toHaveBeenCalledTimes(1);
});
it('invalidates A to B to A and recycled identifiers', () => {
  const m = model(); m.controller.start(m.event('touchstart')); const old = requiredTouchEvidence(m.listeners[0], 'first target listener').fn;
  m.controller.bind(m.scroller, 'B'); m.controller.bind(m.scroller, 'A');
  m.controller.start(m.event('touchstart')); old(m.event('touchend'));
  expect(m.receive).not.toHaveBeenCalled();
  const recycled = m.listeners.at(-2)!.fn;
  m.controller.start(m.event('touchstart')); recycled(m.event('touchend'));
  expect(m.receive).not.toHaveBeenCalled();
  m.listeners.at(-2)!.fn(m.event('touchend')); expect(m.receive).toHaveBeenCalledTimes(1);
});
it('admits two matching identifiers on one target once', () => {
  const m = model(); m.controller.start(m.event('touchstart', [1, 2]));
  const callbacks = m.listeners.filter(x => x.type === 'touchend').map(x => x.fn);
  const end = m.event('touchend', [1, 2]); callbacks.forEach(fn => fn(end));
  expect(m.receive).toHaveBeenCalledTimes(1); expect(requiredTouchEvidence(m.receive.mock.calls[0], 'first admitted receipt')[0].identifiers).toEqual([1, 2]);
});
it('foreign or untrusted receipts cannot consume a matching receipt', () => {
  const m = model(); m.controller.start(m.event('touchstart'));
  const fn = requiredTouchEvidence(m.listeners[0], 'first target listener').fn;
  fn({ ...m.event('touchend', [2]), touches: [{ identifier: 1, target: m.target }] } as unknown as TouchEvent);
  fn(m.event('touchend', [1], false));
  expect(m.receive).not.toHaveBeenCalled(); fn(m.event('touchend')); expect(m.receive).toHaveBeenCalledTimes(1);
});
for (const foreignTarget of [false, true]) it(`prunes trusted live-list absence without a receipt, foreignTarget=${foreignTarget}`, () => {
  const m = model(); m.controller.start(m.event('touchstart'));
  const stale = requiredTouchEvidence(m.listeners[0], 'first target listener').fn;
  const end = { ...m.event('touchend', [2]), target: foreignTarget ? {} : m.target } as TouchEvent;
  expect(m.controller.stop(end)).toBe(false);
  expect(m.target.removeEventListener).toHaveBeenCalledTimes(2);
  expect(m.receive).not.toHaveBeenCalled();
  stale(m.event('touchend'));
  expect(m.receive).not.toHaveBeenCalled();
});
it('preserves a still-live owned contact on a trusted foreign-target release', () => {
  const m = model(); m.controller.start(m.event('touchstart'));
  const end = { ...m.event('touchend', [2]), target: {},
    touches: [{ identifier: 1, target: m.target }] } as unknown as TouchEvent;
  expect(m.controller.stop(end)).toBe(false);
  expect(m.target.removeEventListener).not.toHaveBeenCalled();
  requiredTouchEvidence(m.listeners[0], 'first target listener').fn(m.event('touchend'));
  expect(m.receive).toHaveBeenCalledTimes(1);
});
it('untrusted live-list absence cannot release owned resources', () => {
  const m = model(); m.controller.start(m.event('touchstart'));
  expect(m.controller.stop(m.event('touchend', [2], false))).toBe(false);
  expect(m.target.removeEventListener).not.toHaveBeenCalled();
  requiredTouchEvidence(m.listeners[0], 'first target listener').fn(m.event('touchend'));
  expect(m.receive).toHaveBeenCalledTimes(1);
});
it('clear disposes outstanding resources and invalidates captured callbacks without a synthetic release', () => {
  const m = model(); m.controller.start(m.event('touchstart')); const fn = requiredTouchEvidence(m.listeners[0], 'first target listener').fn;
  m.controller.clear(); fn(m.event('touchend'));
  expect(m.receive).not.toHaveBeenCalled(); expect(m.target.removeEventListener).toHaveBeenCalledTimes(2);
});
it('cleanup failure is reported but cannot prevent the real receipt', () => {
  const m = model(); m.controller.start(m.event('touchstart'));
  const log = vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.mocked(m.target.removeEventListener).mockImplementation(() => { throw new Error('cleanup'); });
  try {
    requiredTouchEvidence(m.listeners[0], 'first target listener').fn(m.event('touchend'));
    expect(m.receive).toHaveBeenCalledTimes(1); expect(log).toHaveBeenCalledTimes(2);
  } finally { log.mockRestore(); }
});
it('consumer failure is reported and rethrown after resource release, never retried as another end', () => {
  const m = model(); m.controller.start(m.event('touchstart'));
  const log = vi.spyOn(console, 'error').mockImplementation(() => {});
  const error = new Error('consumer'); m.receive.mockImplementation(() => { throw error; });
  try {
    const end = m.event('touchend');
    expect(() => requiredTouchEvidence(m.listeners[0], 'first target listener').fn(end)).toThrow(error);
    expect(m.target.removeEventListener).toHaveBeenCalledTimes(2);
    requiredTouchEvidence(m.listeners[0], 'first target listener').fn(end); expect(m.receive).toHaveBeenCalledTimes(1); expect(log).toHaveBeenCalledTimes(1);
  } finally { log.mockRestore(); }
});
for (const failureAt of [1, 2]) for (const cleanupThrows of [false, true]) {
  it(`rolls back listener install ${failureAt} failure with cleanupThrows=${cleanupThrows}`, () => {
    const m = model(); const log = vi.spyOn(console, 'error').mockImplementation(() => {});
    let installed = 0;
    vi.mocked(m.target.addEventListener).mockImplementation((type, listener) => {
      if (typeof listener === 'function') m.listeners.push({ type, fn: listener });
      if (++installed === failureAt) throw new Error('install');
    });
    if (cleanupThrows) vi.mocked(m.target.removeEventListener).mockImplementation(() => { throw new Error('cleanup'); });
    try {
      expect(() => m.controller.start(m.event('touchstart'))).not.toThrow();
      expect(m.target.removeEventListener).toHaveBeenCalledTimes(2);
      requiredTouchEvidence(m.listeners[0], 'first target listener').fn(m.event('touchend')); expect(m.receive).not.toHaveBeenCalled();
      expect(log).toHaveBeenCalledTimes(cleanupThrows ? 3 : 1);
    } finally { log.mockRestore(); }
  });
}
it('one end batch spanning two original targets admits both identifiers exactly once', () => {
  const m = model(); m.controller.start(m.event('touchstart'));
  const otherListeners: EventListener[] = [];
  const other = { addEventListener: (_: string, fn: EventListener) => otherListeners.push(fn), removeEventListener: vi.fn() } as unknown as EventTarget;
  const one = { identifier: 1, target: m.target }, two = { identifier: 2, target: other };
  m.controller.start({ isTrusted: true, target: other, touches: [one, two], changedTouches: [two],
    composedPath: () => [other, m.scroller] } as unknown as TouchEvent);
  const end = { isTrusted: true, type: 'touchend', target: m.target, touches: [], changedTouches: [one, two] } as unknown as TouchEvent;
  requiredTouchEvidence(m.listeners[0], 'first target listener').fn(end); requiredTouchEvidence(otherListeners[0], 'second target listener')(end);
  expect(m.receive).toHaveBeenCalledTimes(1);
  expect(requiredTouchEvidence(m.receive.mock.calls[0], 'first admitted receipt')[0]).toMatchObject({ identifiers: [1, 2], remainingIdentifiers: [] });
  expect(other.removeEventListener).toHaveBeenCalledTimes(2);
});
