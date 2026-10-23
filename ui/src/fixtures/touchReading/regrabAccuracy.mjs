export function installRegrabAccuracyControl({ mode }) {
  if (!['stable', 'touchdown-shift'].includes(mode)) throw new Error('Unknown accuracy control');
  let starts = 0, target = null, prior = null, receipt = null;
  const diagnostic = { entered: true, installed: false, targetMatched: false, receiptPublished: false,
    eventCount: 0, ownedStarts: 0, reason: 'awaiting-contact', restoreAttempted: false,
    restoreResult: 'not-attempted', listenerRemoved: false };
  const counts = { 'invalid-contact': 0, 'target-mismatch': 0, 'hit-mismatch': 0, 'not-second-owned': 0, 'style-precondition': 0 };
  const reject = reason => { diagnostic.reason = reason; counts[reason] = Math.min(255, counts[reason] + 1); };
  const listener = event => {
    diagnostic.eventCount = Math.min(255, diagnostic.eventCount + 1);
    if (!event.isTrusted || event.touches.length !== 1 || event.changedTouches.length !== 1) { reject('invalid-contact'); return; }
    const contact = event.changedTouches[0];
    const path = event.composedPath();
    const block = path.find(node => node instanceof Element && node.matches('[data-fragment-id]'));
    const owner = document.querySelector('.product-conversation-page__transcript #messages.virtual-transcript');
    if (!owner || !block || !owner.contains(block) || !path.includes(owner)) { reject('target-mismatch'); return; }
    const hit = document.elementFromPoint(contact.clientX, contact.clientY);
    if (!hit || !(hit === block || block.contains(hit))) { reject('hit-mismatch'); return; }
    diagnostic.targetMatched = true;
    diagnostic.ownedStarts = Math.min(255, starts + 1);
    if (++starts !== 2) { reject('not-second-owned'); return; }
    const style = getComputedStyle(block);
    if (style.transform !== 'none' || style.translate !== 'none' || style.transitionDuration.split(',').some(t => parseFloat(t) !== 0)) { reject('style-precondition'); return; }
    const before = block.getBoundingClientRect();
    target = block; prior = block.style.transform;
    const commandedDy = mode === 'touchdown-shift' ? 12 : 0;
    if (commandedDy !== 0) block.style.transform = `translateY(${commandedDy}px)`;
    const after = block.getBoundingClientRect();
    receipt = { kind: 'deliberate-accuracy-control', mode, commandedDy,
      measuredDx: after.left - before.left, measuredDy: after.top - before.top,
      t: performance.now(), connected: block.isConnected };
    diagnostic.receiptPublished = true; diagnostic.reason = 'receipt-published';
  };
  document.addEventListener('touchstart', listener, { capture: true, passive: true });
  diagnostic.installed = true;
  window.__regrabAccuracyControl = {
    receipt: () => receipt,
    diagnostic: () => ({ ...diagnostic, counts: { ...counts } }),
    stop() {
      document.removeEventListener('touchstart', listener, true);
      diagnostic.listenerRemoved = true;
      if (target && prior !== null) {
        diagnostic.restoreAttempted = true;
        try {
          target.style.transform = prior;
          diagnostic.restoreResult = !target.isConnected ? 'target-disconnected'
            : target.style.transform === prior ? 'inline-value-restored' : 'inline-value-mismatch';
        } catch { diagnostic.restoreResult = 'restore-threw'; }
      } else diagnostic.restoreResult = 'no-target';
    },
  };
}

export function inspectRegrabAccuracy(trace, receipt) {
  const failures = [];
  const reject = reason => failures.push(reason);
  if (trace?.status !== 'OBSERVED_UNATTRIBUTED' || trace.overflow) reject('incomplete-observation');
  const events = trace?.events ?? [];
  const pre = events.filter(e => e.kind === 'pre-dispatch');
  const at = events.filter(e => e.kind === 'at-touchstart');
  const first = events.filter(e => e.kind === 'first-raf');
  if (pre.length !== 1 || at.length !== 1 || first.length !== 1) reject('missing-or-duplicate-boundary');
  if (!receipt || receipt.kind !== 'deliberate-accuracy-control' || !receipt.connected) reject('missing-control-receipt');
  if (failures.length) return { status: 'UNOBSERVABLE', failures };
  const timed = events.filter(e => ['pre-dispatch', 'armed-frame', 'at-touchstart', 'first-raf', 'hold-frame', 'at-release', 'release-frame'].includes(e.kind));
  if (!Number.isFinite(receipt.t) || timed.some(e => !Number.isFinite(e.t))
    || timed.some((e, i) => i > 0 && e.t <= timed[i - 1].t)
    || pre[0].t > at[0].t || at[0].t >= first[0].t) reject('invalid-boundary-chronology');
  let released = false;
  for (const e of timed) {
    if (e.kind === 'at-release' || e.kind === 'release-frame') released = true;
    if (e.kind === 'hold-frame' && (released || e.t < first[0].t)) reject('hold-outside-held-phase');
  }
  if (failures.length) return { status: 'UNOBSERVABLE', failures };
  if (!(at[0].t <= receipt.t && receipt.t < first[0].t)) reject('control-outside-touchdown-to-first-raf');
  const expected = receipt.mode === 'touchdown-shift' ? 12 : receipt.mode === 'stable' ? 0 : null;
  if (expected === null || receipt.commandedDy !== expected || receipt.measuredDy !== expected || receipt.measuredDx !== 0)
    reject('control-physical-magnitude-mismatch');
  for (const frame of events.filter(e => ['pre-dispatch', 'at-touchstart', 'first-raf', 'hold-frame', 'release-frame'].includes(e.kind))) {
    if (![frame.x, frame.y, frame.dx, frame.dy].every(Number.isFinite)
      || frame.dx !== frame.x - pre[0].x || frame.dy !== frame.y - pre[0].y) reject('baseline-displacement-inconsistent');
  }
  if (expected === 0 && (at[0].dx !== 0 || at[0].dy !== 0 || first[0].dx !== 0 || first[0].dy !== 0))
    reject('stable-control-has-prestart-or-first-frame-displacement');
  const measuredDy = first[0].dy - at[0].dy;
  const measuredDx = first[0].dx - at[0].dx;
  if (measuredDy !== expected || measuredDx !== 0) reject('oracle-did-not-retain-exact-control-magnitude');
  if (pre[0].dx !== 0 || pre[0].dy !== 0) reject('pre-baseline-not-zero');
  const holds = events.filter(e => e.kind === 'hold-frame');
  if (!holds.some(e => e.t - at[0].t > 2000)) reject('missing-held-frame-beyond-two-seconds');
  if (holds.some(e => e.dx !== first[0].dx || e.dy !== first[0].dy)) reject('held-position-not-stable');
  return { status: failures.length ? 'FAILED' : 'CONTROL_OBSERVED', failures,
    measuredDx, measuredDy, preToAtDx: at[0].dx, preToAtDy: at[0].dy,
    preToFirstDx: first[0].dx, preToFirstDy: first[0].dy,
    productQualification: 'NOT_QUALIFIED', causalAttribution: 'UNKNOWN' };
}
