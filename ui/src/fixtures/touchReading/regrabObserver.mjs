export function installRegrabObserver() {
  window.__createRegrabObserver = (owner, resolve, publish, now = () => performance.now()) => {
    let phase = 'initial', generation = 0, id = null, down = false, endedAt = null;
    let frozen = null, baseline = null, held = null, frames = 0, stopped = false;
    let heldAt = null, heldFrames = 0, crossedTwoSeconds = false, ordinal = 0;
    const momentum = [];
    let armSampleCount = null;
    const maxPrestartSampleAgeMs = 50;
    const eventTarget = owner.ownerDocument ?? owner;
    const emit = (kind, data = {}) => publish({ kind, phase, generation, ordinal: ++ordinal,
      inputDown: down, t: now(), ...data });
    const fail = reason => { phase = 'unobservable'; stopped = true; emit('failure', { reason }); };
    const point = (contact = null) => {
      let p;
      try { p = resolve(frozen, contact); } catch { fail('semantic-resolution-failed'); return null; }
      if (!p || p.missing || !p.connected || p.occurrences !== 1 || p.fragments !== 1
        || !p.identityMatches || ![p.x, p.y, p.left, p.right, p.top, p.bottom].every(Number.isFinite)
        || p.right <= p.left || p.bottom <= p.top) { fail('semantic-point-unobservable'); return null; }
      return p;
    };
    const snapshot = boundary => {
      if (stopped || !frozen) return;
      if (++frames > 600) { fail('frame-budget'); return; }
      const p = point(); if (!p) return;
      emit(boundary, { x: p.x, y: p.y, dx: p.x - baseline.x, dy: p.y - baseline.y,
        scrollTop: owner.scrollTop, scrollHeight: owner.scrollHeight, clientHeight: owner.clientHeight,
        fingerX: held?.x ?? null, fingerY: held?.y ?? null });
    };
    const event = e => {
      if (stopped) return;
      if (!e.isTrusted || !e.composedPath().includes(owner)) { fail('untrusted-or-foreign-contact'); return; }
      if (e.type === 'touchcancel') { fail('contact-cancelled'); return; }
      if (e.changedTouches.length !== 1) { fail('ambiguous-changed-contact'); return; }
      const c = e.changedTouches[0];
      if (![c.clientX, c.clientY].every(Number.isFinite)) { fail('invalid-contact-coordinate'); return; }
      if (e.type === 'touchstart') {
        if (down || e.touches.length !== 1 || e.touches[0].identifier !== c.identifier) { fail('ambiguous-start'); return; }
        if (!['initial', 'armed'].includes(phase)) { fail('unexpected-start'); return; }
        down = true; id = c.identifier; generation++;
        if (generation === 1) { phase = 'flick'; emit('first-contact'); return; }
        if (generation !== 2 || !frozen) { fail('invalid-generation'); return; }
        emit('actual-second-contact', { contactX: c.clientX, contactY: c.clientY, scrollTop: Number.isFinite(owner.scrollTop) ? owner.scrollTop : null });
        const sampleAgeMs = now() - momentum.at(-1).t;
        emit('prestart-sample-check', { sampleAgeMs, maxPrestartSampleAgeMs,
          armedSamples: momentum.length - armSampleCount });
        if (momentum.length <= armSampleCount || momentum.length < 2
          || momentum.at(-1).top === momentum.at(-2).top) {
          fail('final-prestart-momentum-not-observed'); return;
        }
        if (!Number.isFinite(sampleAgeMs) || sampleAgeMs < 0 || sampleAgeMs > maxPrestartSampleAgeMs) {
          fail('prestart-momentum-sample-stale'); return;
        }
        const p = point({ x: c.clientX, y: c.clientY, path: e.composedPath() }); if (!p) return;
        emit('second-contact-geometry', { contactX: c.clientX, contactY: c.clientY,
          x: p.x, y: p.y, dx: p.x - baseline.x, dy: p.y - baseline.y,
          left: p.left, right: p.right, top: p.top, bottom: p.bottom,
          scrollTop: Number.isFinite(owner.scrollTop) ? owner.scrollTop : null,
          ownedHit: p.ownedHit === true, qualification: 'NOT_ADMITTED', causalAttribution: 'UNKNOWN' });
        if (p.ownedHit !== true) { fail('frozen-fragment-not-hit'); return; }
        if (c.clientX < p.left || c.clientX > p.right || c.clientY < p.top || c.clientY > p.bottom) {
          fail('frozen-point-not-under-contact'); return;
        }
        held = { x: c.clientX, y: c.clientY }; heldAt = now();
        phase = 'held'; snapshot('at-touchstart'); return;
      }
      if (!down || c.identifier !== id) { fail('foreign-contact-identifier'); return; }
      if (e.type === 'touchmove') {
        if (e.touches.length !== 1 || e.touches[0].identifier !== id) { fail('ambiguous-move'); return; }
        if (generation === 2 && (c.clientX !== held.x || c.clientY !== held.y)) fail('held-contact-moved');
        return;
      }
      if (e.type === 'touchend') {
        if (e.touches.length !== 0) { fail('remaining-contact'); return; }
        down = false;
        if (generation === 1) { phase = 'momentum'; endedAt = now(); emit('first-release'); }
        else {
          if (c.clientX !== held.x || c.clientY !== held.y) { fail('release-coordinate-moved'); return; }
          if (!crossedTwoSeconds) { fail('held-window-incomplete'); return; }
          phase = 'released'; snapshot('at-release');
        }
      }
    };
    for (const type of ['touchstart', 'touchmove', 'touchend', 'touchcancel']) eventTarget.addEventListener(type, event, { capture: true, passive: true });
    return {
      frame() {
        if (stopped) return;
        if ((phase === 'momentum' || phase === 'armed') && !down) {
          if (momentum.length >= 60) { fail('momentum-budget'); return; }
          const observation = { t: now(), top: owner.scrollTop };
          if (!Number.isFinite(observation.top)) { fail('invalid-scroll-coordinate'); return; }
          momentum.push(observation); emit('momentum-frame', observation);
          if (phase === 'armed') snapshot('armed-frame');
        }
        if (phase === 'held') {
          const elapsed = now() - heldAt;
          if (elapsed > 2000) crossedTwoSeconds = true;
          snapshot(++heldFrames === 1 ? 'first-raf' : 'hold-frame');
        } else if (phase === 'released') snapshot('release-frame');
      },
      arm(candidate) {
        if (stopped || phase !== 'momentum' || down || generation !== 1 || endedAt === null) throw new Error('Regrab lifecycle not armable');
        if (momentum.length < 2 || !momentum.some(p => p.top !== momentum[0].top)) {
          fail('momentum-not-observed'); return false;
        }
        if (momentum.at(-1).top === momentum.at(-2).top) { fail('final-momentum-not-observed'); return false; }
        frozen = Object.freeze({ ...candidate });
        const p = point(); if (!p) return false;
        baseline = Object.freeze({ x: p.x, y: p.y }); armSampleCount = momentum.length;
        phase = 'armed'; snapshot('pre-dispatch'); return !stopped;
      },
      finish() {
        if (!stopped && (generation !== 2 || down || phase !== 'released')) fail('incomplete-lifecycle');
        return { status: stopped ? 'UNOBSERVABLE' : 'OBSERVED_UNATTRIBUTED', generation, frames,
          heldFrames, crossedTwoSeconds };
      },
      stop() { stopped = true; for (const type of ['touchstart', 'touchmove', 'touchend', 'touchcancel']) eventTarget.removeEventListener(type, event, true); },
    };
  };
}
