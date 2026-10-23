export async function runFullMessageScrollControl() {
  const selector = '.product-conversation-page__transcript #messages.virtual-transcript';
  const owner = document.querySelector(selector), probe = window.__fullMessage;
  const receipt = { kind: 'SCROLL_DISPLACEMENT_DETECTOR_ONLY', status: 'UNOBSERVABLE', before: null, applied: null, restored: null, frames: [], failure: null };
  let wrote = false;
  const sample = () => {
    if (!owner?.isConnected || document.querySelectorAll(selector).length !== 1 || document.querySelector(selector) !== owner
      || window.__fullMessage !== probe || !probe?.admitted() || !probe.heldReady()) throw new Error('Control owner/contact unavailable');
    const point = probe.stream().anchor;
    if (!point || ![owner.scrollTop, point.x, point.y].every(Number.isFinite)) throw new Error('Control geometry unavailable');
    const hit = document.elementFromPoint(point.x, point.y);
    const caret = document.caretRangeFromPoint(point.x, point.y);
    const hitOwned = !!hit && owner.contains(hit);
    const caretOwned = !!caret?.startContainer && owner.contains(caret.startContainer);
    if (!hitOwned || !caretOwned) throw new Error('Moved glyph hit outside owner');
    return { t: performance.now(), scrollTop: owner.scrollTop, x: point.x, y: point.y,
      admitted: probe.admitted(), heldReady: probe.heldReady(), ownerConnected: owner.isConnected,
      hitOwned, caretOwned, contactEvidence: 'ORIGINAL_TRUSTED_TRACE_CONTACT_NO_NEW_HARDWARE_EVENT' };
  };
  try {
    receipt.before = sample();
    if (receipt.before.scrollTop < 12) throw new Error('Insufficient unclamped scroll room');
    receipt.applyRequestedAt = performance.now(); wrote = true;
    owner.scrollTop = receipt.before.scrollTop - 12;
    receipt.applied = sample();
    if (receipt.applied.scrollTop - receipt.before.scrollTop !== -12 || receipt.applied.x !== receipt.before.x
      || receipt.applied.y - receipt.before.y !== 12) throw new Error('Physical control not observed at exact magnitude');
    for (let index = 0; index < 6; index++) {
      await new Promise(resolve => requestAnimationFrame(resolve));
      const frame = sample();
      receipt.frames.push(frame);
      if (frame.scrollTop !== receipt.applied.scrollTop || frame.x !== receipt.applied.x || frame.y !== receipt.applied.y) {
        throw new Error('Intermediate control frame compensated or changed');
      }
    }
    receipt.held = sample();
    if (receipt.held.scrollTop !== receipt.applied.scrollTop || receipt.held.x !== receipt.applied.x || receipt.held.y !== receipt.applied.y) throw new Error('Control coalesced or compensated');
  } catch (e) { receipt.failure = String(e); }
  finally {
    if (wrote) {
      receipt.restoreRequestedAt = performance.now();
      try {
        if (!owner.isConnected || document.querySelector(selector) !== owner) throw new Error('Restore owner lost');
        owner.scrollTop = receipt.before.scrollTop;
        receipt.restoreScrollTop = owner.scrollTop;
        receipt.restored = sample();
        if (receipt.restored.scrollTop !== receipt.before.scrollTop || receipt.restored.x !== receipt.before.x || receipt.restored.y !== receipt.before.y) throw new Error('Physical restoration mismatch');
      } catch (e) { receipt.failure ??= String(e); }
    }
  }
  if (!receipt.failure && receipt.before && receipt.applied && receipt.held && receipt.restored) receipt.status = 'PHYSICAL_ACTION_OBSERVED';
  return receipt;
}

export function inspectFullMessageScrollControl(trace, receipt) {
  const fail = reason => ({ status: 'UNOBSERVABLE', reason });
  if (!trace || trace.failure || !trace.baseline || !receipt || receipt.kind !== 'SCROLL_DISPLACEMENT_DETECTOR_ONLY'
    || receipt.status !== 'PHYSICAL_ACTION_OBSERVED' || receipt.failure) return fail('Incomplete observation/action');
  const contacts = trace.contacts ?? [];
  const starts = contacts.filter(c => c.type === 'touchstart'), ends = contacts.filter(c => c.type === 'touchend');
  if (!trace.anchor?.occurrence || !trace.anchor.fragmentId || !trace.anchor.unitKey || starts.length !== 1 || ends.length !== 1
    || contacts.some((c, i) => !Number.isFinite(c.t) || (i > 0 && c.t < contacts[i - 1].t) || !c.trusted || !c.owned || c.id !== starts[0].id || c.x !== starts[0].x || c.y !== starts[0].y
      || !['touchstart', 'touchmove', 'touchend'].includes(c.type) || c.active !== (c.type === 'touchend' ? 0 : 1))) return fail('Invalid identity/contact lifecycle');
  const { before, applied, held, restored } = receipt;
  if (![before, applied, held, restored].every(p => p && [p.t, p.x, p.y, p.scrollTop].every(Number.isFinite))) return fail('Missing independent receipts');
  if (!(starts[0].t <= trace.baseline.t && ends[0].t > restored.t && trace.baseline.t + 2000 < before.t && before.t <= receipt.applyRequestedAt && receipt.applyRequestedAt <= applied.t
    && applied.t < held.t && held.t <= receipt.restoreRequestedAt && receipt.restoreRequestedAt <= restored.t)) return fail('Invalid ordering');
  if (before.x !== trace.baseline.x || before.y !== trace.baseline.y || applied.scrollTop - before.scrollTop !== -12
    || applied.y - before.y !== 12 || applied.x !== before.x || held.scrollTop !== applied.scrollTop || held.x !== applied.x || held.y !== applied.y
    || restored.scrollTop !== before.scrollTop || restored.x !== before.x || restored.y !== before.y) return fail('Independent physical magnitude/restoration mismatch');
  const frames = receipt.frames;
  if (!Array.isArray(frames) || frames.length !== 6 || frames.some((p, i) => !p
    || ![p.t, p.x, p.y, p.scrollTop].every(Number.isFinite)
    || p.t < (i === 0 ? applied.t : frames[i - 1].t) || p.t > held.t
    || p.admitted !== true || p.heldReady !== true || p.ownerConnected !== true || p.hitOwned !== true || p.caretOwned !== true
    || p.contactEvidence !== 'ORIGINAL_TRUSTED_TRACE_CONTACT_NO_NEW_HARDWARE_EVENT'
    || p.scrollTop !== applied.scrollTop || p.x !== applied.x || p.y !== applied.y)) return fail('Invalid six-frame physical receipts');
  if (!Array.isArray(trace.samples) || trace.samples.some((p, i) => !p || !Number.isFinite(p.t)
    || (i > 0 && p.t < trace.samples[i - 1].t))) return fail('Nonfinite or reversed trajectory ordering');
  const samples = trace.samples?.filter(p => p.down) ?? [];
  if (!samples.length || samples.some(p => ![p.t, p.x, p.y, p.dx, p.dy].every(Number.isFinite)
    || p.dx !== p.x - trace.baseline.x || p.dy !== p.y - trace.baseline.y || p.dx !== 0 || ![0, 12].includes(p.dy))) return fail('Invalid trajectory');
  const during = samples.filter(p => p.t > applied.t && p.t < receipt.restoreRequestedAt);
  const after = samples.filter(p => p.t > restored.t);
  if (during.length < 2 || during.some(p => p.dy !== 12) || !after.length || after.some(p => p.dy !== 0)
    || samples.filter(p => p.t < receipt.applyRequestedAt).some(p => p.dy !== 0)) return fail('Raw trajectory did not detect and restore physical step');
  return { status: 'SCROLL_DISPLACEMENT_DETECTOR_OBSERVED', expectedDy: 12, measuredDuring: during.length,
    limit: 'NOT_NATURAL_PRODUCT_REPRO_OR_SPECIFIC_COMPENSATION_BRANCH_QUALIFICATION' };
}
