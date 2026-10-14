import assert from 'node:assert/strict';

export function historicalMessageIdentities(a) {
  assert.equal(a.kind, 'global');
  assert.ok(Array.isArray(a.members) && new Set(a.members).size === a.members.length);
  const seen = new Set();
  return a.occurrences.map(([ordinal, sequence, persistedMessageID]) => {
    const sourceRow = a.members[ordinal];
    assert.ok(Number.isSafeInteger(ordinal) && ordinal >= 0 && sourceRow, 'known historical member');
    assert.ok(Number.isSafeInteger(sequence) && sequence >= 0);
    assert.ok(typeof persistedMessageID === 'string' && persistedMessageID.length > 0, 'exact fixture message ID');
    // Source comes from the fixture ordinal, never inferred from an ID prefix.
    assert.ok(!seen.has(persistedMessageID), 'unambiguous fixture occurrence');
    seen.add(persistedMessageID);
    return { persistedMessageID, sourceRow };
  });
}

// Self-contained: this exact function is serialized by Playwright into the real document.
export function observePhysicalHistoricalRow({ known, expected = null }) {
  const scroller = document.querySelector('#messages');
  if (!scroller) return null;
  const bounds = el => {
    const b = el.getBoundingClientRect();
    return { top: b.top, bottom: b.bottom, left: b.left, right: b.right, height: b.height, width: b.width };
  };
  const viewport = bounds(scroller);
  const clip = { top: Math.max(0, viewport.top), bottom: Math.min(innerHeight, viewport.bottom),
    left: Math.max(0, viewport.left), right: Math.min(innerWidth, viewport.right) };
  const fullyVisible = (el, b) => {
    const style = getComputedStyle(el);
    return Object.values(b).every(Number.isFinite) && b.height > 0 && b.width > 0
      && el.getClientRects().length > 0 && style.display !== 'none' && style.visibility === 'visible'
      && b.top >= clip.top && b.bottom <= clip.bottom && b.left >= clip.left && b.right <= clip.right;
  };
  const identities = new Map(known.map(k => [k.persistedMessageID, k.sourceRow]));
  for (const row of scroller.querySelectorAll('[data-virtual-key]')) {
    const key = row.dataset.virtualKey, rowBounds = bounds(row);
    if (!key || (expected && key !== expected.key) || !fullyVisible(row, rowBounds)) continue;
    for (const child of row.querySelectorAll('[id], [data-message-id]')) {
      if (child.closest('[data-virtual-key]') !== row) continue;
      // MessageList/MessageComponents wrap persisted IDs with exactly one `message-` for DOM IDs.
      const elementID = child.getAttribute('id'), dataMessageID = child.getAttribute('data-message-id');
      const wrappedID = elementID?.startsWith('message-') ? elementID.slice('message-'.length) : null;
      if (wrappedID && dataMessageID && wrappedID !== dataMessageID) continue;
      const persistedMessageID = dataMessageID || wrappedID;
      const sourceRow = identities.get(persistedMessageID);
      if (!sourceRow || (expected && (persistedMessageID !== expected.persistedMessageID || sourceRow !== expected.sourceRow))) continue;
      const messageBounds = bounds(child);
      if (!fullyVisible(child, messageBounds)) continue;
      return { key, offset: rowBounds.top - viewport.top, persistedMessageID, sourceRow,
        messageOffset: messageBounds.top - viewport.top, elementID, dataMessageID,
        DOMbounds: { viewport, row: rowBounds, message: messageBounds },
        currentURL: location.href, sourcepin: new URL(location.href).searchParams.get('source_transcript'),
        layoutdimensions: { scrollTop: scroller.scrollTop, scrollHeight: scroller.scrollHeight,
          clientHeight: scroller.clientHeight, innerHeight, innerWidth } };
    }
  }
  return null;
}

export function physicalBaselineMatches(before, after) {
  return !!before && !!after && before.key === after.key
    && before.persistedMessageID === after.persistedMessageID && before.sourceRow === after.sourceRow
    && before.sourcepin === after.sourcepin && before.currentURL === after.currentURL
    && Math.abs(before.offset - after.offset) <= 2 && Math.abs(before.messageOffset - after.messageOffset) <= 2;
}

export async function holdPostFaultBaseline(first, read, nextFrame) {
  assert.ok(first, 'visible known persisted historical row required; no generic row fallback');
  const frames = [];
  for (let i = 0; i < 4; i++) {
    await nextFrame();
    const frame = await read(first);
    assert.ok(physicalBaselineMatches(first, frame), 'declared physical historical identity must hold four frames within 2px');
    frames.push(frame);
  }
  return { ...first, stability_frames: frames, tolerance_css_px: 2,
    observation_scope: 'settled postfault physical historical identity; not prefault viewport or original middle/hash reconfirmation' };
}
