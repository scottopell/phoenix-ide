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
export function observePhysicalHistoricalRow({ known, expected = null, diagnostics = false }) {
  const counts = {};
  const reject = predicate => { counts[predicate] = Math.min(512, (counts[predicate] ?? 0) + 1); };
  const finish = observation => diagnostics ? { observation, predicates: counts } : observation;
  const scroller = document.querySelector('#messages');
  if (!scroller) { reject('missing_scroller'); return finish(null); }
  const bounds = el => {
    const b = el.getBoundingClientRect();
    return { top: b.top, bottom: b.bottom, left: b.left, right: b.right, height: b.height, width: b.width };
  };
  const viewport = bounds(scroller);
  const clip = { top: Math.max(0, viewport.top), bottom: Math.min(innerHeight, viewport.bottom),
    left: Math.max(0, viewport.left), right: Math.min(innerWidth, viewport.right) };
  const visibilityFailure = (el, b) => {
    const style = getComputedStyle(el);
    if (!Object.values(b).every(Number.isFinite)) return 'nonfinite_geometry';
    if (!(b.height > 0 && b.width > 0)) return 'nonpositive_geometry';
    if (!(el.getClientRects().length > 0)) return 'no_client_rect';
    if (style.display === 'none' || style.visibility !== 'visible') return 'hidden';
    if (!(b.top >= clip.top && b.bottom <= clip.bottom && b.left >= clip.left && b.right <= clip.right)) return 'clipped';
    return null;
  };
  const identities = new Map(known.map(k => [k.persistedMessageID, k.sourceRow]));
  const allRows = [...document.querySelectorAll('[data-virtual-key]')];
  const rowKeys = new Map();
  for (const row of allRows) {
    const key = row.dataset.virtualKey;
    if (!rowKeys.has(key)) rowKeys.set(key, new Set());
    rowKeys.get(key).add(row);
  }
  const identityElements = new Map();
  for (const el of document.querySelectorAll('[id], [data-message-id]')) {
    const id = el.getAttribute('id');
    const wrappedID = id?.startsWith('message-') ? id.slice('message-'.length) : null;
    // Count physical elements, including hidden/zero-size instances and conflicting aliases.
    for (const identity of [wrappedID, el.getAttribute('data-message-id')]) {
      if (!identity) continue;
      if (!identityElements.has(identity)) identityElements.set(identity, new Set());
      identityElements.get(identity).add(el);
    }
  }
  for (const row of scroller.querySelectorAll('[data-virtual-key]')) {
    const key = row.dataset.virtualKey, rowBounds = bounds(row);
    if (diagnostics) {
      for (const child of row.querySelectorAll('[id], [data-message-id]')) {
        if (child.closest('[data-virtual-key]') !== row) continue;
        const id = child.getAttribute('id'), data = child.getAttribute('data-message-id');
        const wrapped = id?.startsWith('message-') ? id.slice('message-'.length) : null;
        if (!identities.has(data) && !identities.has(wrapped)) continue;
        reject('known_target_seen');
        if (rowBounds.height > clip.bottom - clip.top) reject('known_target_row_taller_than_clip');
        const rowFailure = visibilityFailure(row, rowBounds);
        if (rowFailure) reject('known_target_row_' + rowFailure);
        const childFailure = visibilityFailure(child, bounds(child));
        if (childFailure) reject('known_target_message_' + childFailure);
        else reject('known_target_message_fully_visible');
      }
    }
    if (!key) { reject('missing_row_key'); continue; }
    if (rowKeys.get(key)?.size !== 1) { reject('duplicate_row_key'); continue; }
    if (expected && key !== expected.key) { reject('unexpected_row_key'); continue; }
    const rowFailure = visibilityFailure(row, rowBounds);
    if (rowFailure) { reject('row_' + rowFailure); continue; }
    for (const child of row.querySelectorAll('[id], [data-message-id]')) {
      if (child.closest('[data-virtual-key]') !== row) { reject('foreign_row_child'); continue; }
      // MessageList/MessageComponents wrap persisted IDs with exactly one `message-` for DOM IDs.
      const elementID = child.getAttribute('id'), dataMessageID = child.getAttribute('data-message-id');
      const wrappedID = elementID?.startsWith('message-') ? elementID.slice('message-'.length) : null;
      if (wrappedID && dataMessageID && wrappedID !== dataMessageID) { reject('conflicting_identity'); continue; }
      const persistedMessageID = dataMessageID || wrappedID;
      const sourceRow = identities.get(persistedMessageID);
      if (!sourceRow || (expected && (persistedMessageID !== expected.persistedMessageID || sourceRow !== expected.sourceRow))) { reject('unknown_or_unexpected_identity'); continue; }
      const matches = identityElements.get(persistedMessageID);
      if (matches?.size !== 1 || !matches.has(child)) { reject('duplicate_physical_identity'); continue; }
      const messageBounds = bounds(child);
      const messageFailure = visibilityFailure(child, messageBounds);
      if (messageFailure) { reject('message_' + messageFailure); continue; }
      return finish({ key, offset: rowBounds.top - viewport.top, persistedMessageID, sourceRow,
        messageOffset: messageBounds.top - viewport.top, elementID, dataMessageID,
        DOMbounds: { viewport, row: rowBounds, message: messageBounds },
        currentURL: location.href, sourcepin: new URL(location.href).searchParams.get('source_transcript'),
        layoutdimensions: { scrollTop: scroller.scrollTop, scrollHeight: scroller.scrollHeight,
          clientHeight: scroller.clientHeight, innerHeight, innerWidth } });
    }
  }
  return finish(null);
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
