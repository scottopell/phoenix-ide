import { test } from 'node:test';
import assert from 'node:assert/strict';
import { installRegrabObserver } from './regrabObserver.mjs';
function setup(run) {
  const saved = Object.getOwnPropertyDescriptor(globalThis, 'window');
  globalThis.window = {};
  const listeners = new Map(), events = [];
  const owner = { scrollTop: 1000, scrollHeight: 2000, clientHeight: 600,
    addEventListener: (k, f) => listeners.set(k, f), removeEventListener: k => listeners.delete(k) };
  let geometry = { connected: true, occurrences: 1, fragments: 1, identityMatches: true, ownedHit: true,
    x: 100, y: 200, left: 90, right: 110, top: 190, bottom: 210 }, clock = 0;
  let observer;
  try {
    installRegrabObserver();
    observer = window.__createRegrabObserver(owner, () => ({ ...geometry }), e => events.push(e), () => ++clock);
    const event = (type, options = {}) => {
      const contact = { identifier: options.id ?? 1, clientX: options.x ?? 100, clientY: options.y ?? 200 };
      listeners.get(type)({ type, isTrusted: options.trusted ?? true,
        composedPath: () => options.foreign ? [] : [owner],
        changedTouches: [contact], touches: type === 'touchend' ? [] : Array(options.count ?? 1).fill(contact) });
    };
    const momentum = () => { event('touchstart'); event('touchend'); observer.frame(); owner.scrollTop = 990; observer.frame(); };
    const arm = candidate => {
      const result = observer.arm(candidate);
      if (result) { owner.scrollTop -= 10; observer.frame(); }
      return result;
    };
    run({ observer, arm, events, owner, event, momentum, geometry, listeners, advance: ms => { clock += ms; } });
  } finally {
    observer?.stop(); assert.equal(listeners.size, 0);
    if (saved) Object.defineProperty(globalThis, 'window', saved); else delete globalThis.window;
  }
}
test('PRE baseline survives AT and RAF displacement across generation two', () => setup(r => {
  r.momentum(); assert.equal(r.arm({ key: 'point' }), true);
  r.geometry.y = 203; r.event('touchstart', { id: 2 });
  r.geometry.y = 207; r.observer.frame(); r.advance(2600); r.observer.frame();
  r.event('touchend', { id: 2 }); r.observer.frame();
  assert.equal(r.events.find(e => e.kind === 'pre-dispatch').dy, 0);
  assert.equal(r.events.find(e => e.kind === 'at-touchstart').dy, 3);
  assert.equal(r.events.find(e => e.kind === 'first-raf').dy, 7);
  assert.equal(r.observer.finish().status, 'OBSERVED_UNATTRIBUTED');
}));
for (const [name, action] of [
  ['untrusted', r => r.event('touchstart', { trusted: false })],
  ['foreign target', r => r.event('touchstart', { foreign: true })],
  ['multiple contact', r => r.event('touchstart', { count: 2 })],
  ['wrong held identifier', r => { r.event('touchstart', { id: 2 }); r.event('touchmove', { id: 3 }); }],
  ['held coordinate moved', r => { r.event('touchstart', { id: 2 }); r.event('touchmove', { id: 2, y: 201 }); }],
  ['duplicate occurrence', r => { r.geometry.occurrences = 2; r.event('touchstart'); }],
  ['disconnected', r => { r.geometry.connected = false; r.event('touchstart'); }],
  ['identity changed', r => { r.geometry.identityMatches = false; r.event('touchstart'); }],
  ['frozen glyph moved off contact', r => { r.geometry.top = 220; r.geometry.bottom = 240; r.event('touchstart'); }],
]) test(`rejects ${name}`, () => setup(r => {
  r.momentum(); r.arm({ key: 'point' }); action(r);
  assert.equal(r.observer.finish().status, 'UNOBSERVABLE');
}));
test('no command or first contact alone can arm; no observed momentum fails', () => setup(r => {
  assert.throws(() => r.observer.arm({}), /not armable/);
  r.event('touchstart'); assert.throws(() => r.observer.arm({}), /not armable/);
  r.event('touchend'); r.observer.frame(); r.observer.frame();
  assert.equal(r.observer.arm({}), false);
  assert.equal(r.events.at(-1).reason, 'momentum-not-observed');
}));
test('final stopped momentum does not qualify and node replacement keeps same semantic geometry', () => {
  setup(r => { r.momentum(); r.observer.frame(); assert.equal(r.observer.arm({}), false); });
  setup(r => { r.momentum(); r.arm({ key: 'point' }); r.event('touchstart', { id: 2 });
    r.geometry.nodeInstance = 'replacement'; r.observer.frame();
    assert.equal(r.events.at(-1).dy, 0); r.advance(2600); r.observer.frame(); r.event('touchend', { id: 2 });
    assert.equal(r.observer.finish().status, 'OBSERVED_UNATTRIBUTED'); });
});

test('release without an actual held frame beyond two seconds fails', () => setup(r => {
  r.momentum(); r.arm({ key: 'point' }); r.event('touchstart', { id: 2 });
  r.observer.frame(); r.advance(2600); r.event('touchend', { id: 2 });
  assert.equal(r.events.at(-1).reason, 'held-window-incomplete');
  assert.equal(r.observer.finish().status, 'UNOBSERVABLE');
}));
for (const field of ['connected', 'identityMatches']) test(`lost ${field} during hold fails`, () => setup(r => {
  r.momentum(); r.arm({ key: 'point' }); r.event('touchstart', { id: 2 });
  r.geometry[field] = false; r.observer.frame();
  assert.equal(r.observer.finish().status, 'UNOBSERVABLE');
}));
test('second contact cancellation fails and does not publish a successful release', () => setup(r => {
  r.momentum(); r.arm({ key: 'point' }); r.event('touchstart', { id: 2 });
  r.event('touchcancel', { id: 2 });
  assert.equal(r.events.at(-1).reason, 'contact-cancelled');
  assert.equal(r.observer.finish().status, 'UNOBSERVABLE');
}));

test('old pre-arm changing pair cannot qualify second start without an armed frame', () => setup(r => {
  r.momentum(); r.observer.arm({ key: 'point' }); r.event('touchstart', { id: 2 });
  assert.equal(r.events.at(-1).reason, 'final-prestart-momentum-not-observed');
}));
test('armed sample older than fixed 50ms ceiling fails without waiting or retry', () => setup(r => {
  r.momentum(); r.arm({ key: 'point' }); r.advance(51); r.event('touchstart', { id: 2 });
  assert.equal(r.events.at(-1).reason, 'prestart-momentum-sample-stale');
}));
test('fresh changing armed frame qualifies observed contact but not causality', () => setup(r => {
  r.momentum(); r.arm({ key: 'point' }); r.event('touchstart', { id: 2 });
  assert.equal(r.events.at(-1).kind, 'at-touchstart');
  assert.equal(r.events.find(e => e.kind === 'prestart-sample-check').armedSamples, 1);
}));
test('owned scroller path and glyph geometry cannot substitute for actual fragment hit', () => setup(r => {
  r.momentum(); r.arm({ key: 'point' }); r.geometry.ownedHit = false;
  r.event('touchstart', { id: 2 });
  assert.equal(r.events.at(-1).reason, 'frozen-fragment-not-hit');
}));

test('rejected actual contact retains geometry without admitting a held sample', () => setup(r => {
  r.momentum(); assert.equal(r.arm({ key: 'point' }), true);
  assert.equal(r.events.at(-1).kind, 'armed-frame');
  r.geometry.y = 220; r.geometry.top = 210; r.geometry.bottom = 230;
  r.event('touchstart', { id: 2 });
  const contact = r.events.find(e => e.kind === 'actual-second-contact');
  const geometry = r.events.find(e => e.kind === 'second-contact-geometry');
  assert.equal(contact.contactY, 200);
  assert.equal(geometry.y, 220); assert.equal(geometry.dy, 20);
  assert.equal(geometry.top, 210); assert.equal(geometry.bottom, 230);
  assert.equal(geometry.qualification, 'NOT_ADMITTED');
  assert.equal(r.events.at(-1).reason, 'frozen-point-not-under-contact');
  assert.ok(!r.events.some(e => e.kind === 'at-touchstart'));
  assert.equal(r.observer.finish().status, 'UNOBSERVABLE');
}));
test('lost identity retains actual contact but never invents semantic geometry', () => setup(r => {
  r.momentum(); assert.equal(r.arm({ key: 'point' }), true);
  assert.equal(r.events.at(-1).kind, 'armed-frame');
  r.geometry.connected = false;
  r.event('touchstart', { id: 2 });
  assert.ok(r.events.some(e => e.kind === 'actual-second-contact'));
  assert.ok(!r.events.some(e => e.kind === 'second-contact-geometry'));
  assert.equal(r.events.at(-1).reason, 'semantic-point-unobservable');
  assert.equal(r.observer.finish().status, 'UNOBSERVABLE');
}));
