export function installFullMessageProbe() {
  const selector = '.product-conversation-page__transcript #messages.virtual-transcript';
  const owner = document.querySelector(selector);
  if (!owner || document.querySelectorAll(selector).length !== 1 || owner.scrollHeight <= owner.clientHeight
    || !['auto', 'scroll'].includes(getComputedStyle(owner).overflowY)) throw new Error('Scroll owner unavailable');
  const samples = [], contacts = [], lifecycle = [], layouts = [], domMutations = [], consumption = [];
  const ids = new WeakMap(); let ordinal = 0, failure = null, baseline = null, anchor = null, contact = null;
  let down = false, ended = false, stopped = false, raf, phase = 'setup';
  const fail = why => { failure ??= why; };
  const push = (list, item, cap = 1024) => { if (list.length >= cap) fail('evidence-budget'); else list.push(item); };
  const id = el => { if (!ids.has(el)) ids.set(el, ++ordinal); return ids.get(el); };
  const rows = root => root instanceof Element ? [...(root.matches('[data-render-unit-key]') ? [root] : []), ...root.querySelectorAll('[data-render-unit-key]')] : [];
  const resize = new ResizeObserver(entries => {
    for (const e of entries) push(layouts, { t: performance.now(), phase, node: id(e.target), key: e.target.dataset.renderUnitKey, height: e.contentRect.height, borderHeight: e.borderBoxSize[0]?.blockSize ?? null });
  });
  for (const row of rows(owner)) { push(lifecycle, { t: performance.now(), phase, kind: 'initial-present', node: id(row), key: row.dataset.renderUnitKey }); resize.observe(row); }
  const mutations = new MutationObserver(records => {
    for (const record of records) {
      const target = record.target instanceof Element ? record.target : record.target.parentElement;
      const row = target?.closest('[data-render-unit-key]');
      push(domMutations, { t: performance.now(), phase, kind: record.type, rowNode: row ? id(row) : null,
        rowKey: row?.dataset.renderUnitKey ?? null, timing: 'MUTATION_OBSERVER_DELIVERY_NOT_REACT_COMMIT' });
      for (const [kind, nodes] of [['dom-removed', record.removedNodes], ['dom-added', record.addedNodes]]) {
        for (const root of nodes) for (const row of rows(root)) {
          push(lifecycle, { t: performance.now(), phase, kind, node: id(row), key: row.dataset.renderUnitKey });
          if (kind === 'dom-added') resize.observe(row); else resize.unobserve(row);
          if (anchor && row === anchor.row && kind === 'dom-removed') fail('anchor-removed-no-reresolution');
        }
      }
    }
  });
  mutations.observe(owner, { childList: true, subtree: true, characterData: true });
  const texts = block => { const out = [], w = document.createTreeWalker(block, NodeFilter.SHOW_TEXT); while (w.nextNode()) { if (out.length >= 1024) throw new Error('Text budget'); out.push(w.currentNode); } return out; };
  const read = () => {
    if (!anchor) throw new Error('Anchor absent');
    const a = anchor;
    if (!owner.isConnected || document.querySelectorAll(selector).length !== 1 || document.querySelector(selector) !== owner
      || !a.node.isConnected || !a.row.isConnected || !owner.contains(a.row) || !a.row.contains(a.message) || !a.message.contains(a.block) || !a.block.contains(a.node)) throw new Error('Anchor disconnected');
    const messages = [...owner.querySelectorAll('.message[data-message-occurrence]')].filter(e => e.dataset.messageOccurrence === a.occurrence);
    const blocks = [...a.message.querySelectorAll('[data-fragment-id]')].filter(e => e.dataset.fragmentId === a.fragmentId);
    const sameRows = [...owner.querySelectorAll('[data-render-unit-key]')].filter(e => e.dataset.renderUnitKey === a.unitKey);
    if (messages.length !== 1 || messages[0] !== a.message || blocks.length !== 1 || blocks[0] !== a.block
      || sameRows.length !== 1 || sameRows[0] !== a.row || texts(a.block).map(n => n.data).join('') !== a.blockText
      || a.node.data !== a.nodeText) throw new Error('Semantic identity changed');
    const r = document.createRange(); r.setStart(a.node, a.offset); r.setEnd(a.node, a.offset + 1);
    const rects = [...r.getClientRects()]; if (rects.length !== 1) throw new Error('Range ambiguity');
    const q = rects[0], x = q.left + q.width / 2, y = q.top + q.height / 2;
    if (![x, y, q.width, q.height, owner.scrollTop].every(Number.isFinite) || q.width <= 0 || q.height <= 0) throw new Error('Invalid geometry');
    const rowRect = a.row.getBoundingClientRect();
    return { t: performance.now(), x, y, left: q.left, right: q.right, top: q.top, bottom: q.bottom,
      rowTop: rowRect.top, rowBottom: rowRect.bottom, scrollTop: owner.scrollTop };
  };
  const onTouch = e => {
    try {
      const c = e.changedTouches[0];
      if (!e.isTrusted || !e.composedPath().includes(owner) || e.changedTouches.length !== 1 || !c || ![c.clientX, c.clientY].every(Number.isFinite)) throw new Error('Untrusted/foreign/multiple contact');
      push(contacts, { t: performance.now(), type: e.type, trusted: e.isTrusted, owned: true, id: c.identifier, x: c.clientX, y: c.clientY, active: e.touches.length }, 64);
      if (e.type === 'touchcancel') throw new Error('Contact cancelled');
      if (e.type === 'touchstart') {
        if (baseline || e.touches.length !== 1 || e.touches[0].identifier !== c.identifier) throw new Error('Duplicate start');
        const p = read(), hit = document.elementFromPoint(c.clientX, c.clientY), caret = document.caretRangeFromPoint(c.clientX, c.clientY);
        if (c.clientX < p.left || c.clientX > p.right || c.clientY < p.top || c.clientY > p.bottom || !hit || !anchor.block.contains(hit)
          || caret?.startContainer !== anchor.node || ![anchor.offset, anchor.offset + 1].includes(caret.startOffset)) throw new Error('Not owned glyph contact');
        contact = { id: c.identifier, x: c.clientX, y: c.clientY }; baseline = p; down = true; phase = 'held';
      } else {
        if (!down || !contact || c.identifier !== contact.id || c.clientX !== contact.x || c.clientY !== contact.y) throw new Error('Stationary contact changed');
        if (e.type === 'touchend') { if (e.touches.length) throw new Error('Remaining contact'); down = false; ended = true; phase = 'released'; }
        else if (e.touches.length !== 1 || e.touches[0].identifier !== contact.id) throw new Error('Active identifier mismatch');
      }
    } catch (e) { fail(String(e)); }
  };
  for (const type of ['touchstart', 'touchmove', 'touchend', 'touchcancel']) document.addEventListener(type, onTouch, { capture: true, passive: true });
  const frame = () => {
    if (stopped) return;
    if (anchor) try { const p = read(); push(samples, { ...p, phase, down, dx: baseline ? p.x - baseline.x : null, dy: baseline ? p.y - baseline.y : null }, 600); } catch (e) { fail(String(e)); }
    raf = requestAnimationFrame(frame);
  }; raf = requestAnimationFrame(frame);
  window.__fullMessage = {
    setupScroll(edge) { if (anchor || baseline) throw new Error('Setup ended'); owner.scrollTop = edge === 'top' ? 0 : owner.scrollHeight; },
    lifecycle: () => lifecycle,
    consume(label, chunks, continuing = false) {
      const bridge = window.__touchFixture, previous = bridge?.status().consumption;
      if (!bridge?.ready()) throw new Error('Bridge unavailable');
      if (continuing && previous?.status !== 'OBSERVED') throw new Error('No consumed prefix');
      const expectedText = (continuing ? previous.text : '') + chunks.join('');
      const t = performance.now();
      if (continuing) bridge.append(label, chunks); else bridge.arm(label, chunks);
      if (bridge.status().consumption.status !== 'UNOBSERVED') throw new Error('Stale arm');
      for (const text of chunks) bridge.emit({ type: 'token', text });
      const receipt = bridge.status().consumption;
      if (window.__touchFixture !== bridge || receipt.status !== 'OBSERVED' || receipt.phase !== label || receipt.liveId !== 'touch-reading-member'
        || receipt.text !== expectedText || !receipt.requestId || !receipt.streamIncarnation || !Number.isSafeInteger(receipt.connectionEpoch)
        || !Number.isSafeInteger(receipt.tokenSequence) || receipt.eventSequence < receipt.tokenSequence
        || (continuing && (receipt.requestId !== previous.requestId || receipt.connectionEpoch !== previous.connectionEpoch
          || receipt.streamIncarnation !== previous.streamIncarnation || receipt.tokenSequence <= previous.tokenSequence
          || receipt.eventSequence <= previous.eventSequence))
        || (!continuing && previous?.status === 'OBSERVED' && receipt.requestId === previous.requestId)) throw new Error('Fresh store consumption absent');
      const observed = { t, observedAt: performance.now(), receipt }; push(consumption, observed, 16); return observed;
    },
    stream() {
      const matches = texts(owner).filter(n => n.data.includes('TOUCH_STREAM_SEED_1729'));
      if (matches.length !== 1) throw new Error('Streaming marker missing/ambiguous');
      const row = matches[0].parentElement.closest('[data-render-unit-key]');
      if (!row || !owner.contains(row)) throw new Error('Streaming row absent');
      const rect = row.getBoundingClientRect();
      const point = anchor ? read() : null;
      return { t: performance.now(), node: id(row), key: row.dataset.renderUnitKey, height: rect.height,
        top: rect.top, bottom: rect.bottom, text: row.textContent, anchor: point,
        relativeToAnchorGlyph: !point ? 'NOT_ACQUIRED' : rect.bottom <= point.top ? 'ABOVE'
          : rect.top >= point.bottom ? 'BELOW' : 'INTERSECTS' };
    },
    point() {
      if (anchor) throw new Error('Cannot replace anchor');
      const point = window.__touchTextContact(owner, 'touch-reading-member');
      const caret = document.caretRangeFromPoint(point.x, point.y), node = caret?.startContainer;
      if (!node || node.nodeType !== Node.TEXT_NODE) throw new Error('Text unavailable');
      const block = node.parentElement.closest('[data-fragment-id]'), message = block?.closest('.message[data-message-id]'), row = message?.closest('[data-render-unit-key]');
      if (!block || !message || !row) throw new Error('Ownership unavailable');
      let offset = null;
      for (const i of [caret.startOffset, caret.startOffset - 1]) {
        if (i < 0 || i >= node.length || /\s|[\uD800-\uDFFF]/u.test(node.data[i])) continue;
        const r = document.createRange(); r.setStart(node, i); r.setEnd(node, i + 1); const q = [...r.getClientRects()];
        if (q.length === 1 && point.x >= q[0].left && point.x <= q[0].right && point.y >= q[0].top && point.y <= q[0].bottom) { if (offset !== null) throw new Error('Ambiguous offset'); offset = i; }
      }
      if (offset === null) throw new Error('Whitespace/padding contact');
      const nodes = texts(block), blockText = nodes.map(n => n.data).join('');
      if (blockText.length > 16384) throw new Error('Block budget');
      anchor = { node, offset, nodeText: node.data, row, message, block, blockText,
        logicalOffset: nodes.slice(0, nodes.indexOf(node)).reduce((n, t) => n + t.length, 0) + offset,
        occurrence: message.dataset.messageOccurrence, fragmentId: block.dataset.fragmentId, unitKey: row.dataset.renderUnitKey };
      return read();
    },
    admitted: () => !failure && !!baseline && down && !ended,
    heldReady: () => !failure && down && samples.some(s => s.down && s.t > baseline.t + 2000),
    result() {
      stopped = true; cancelAnimationFrame(raf); mutations.disconnect(); resize.disconnect();
      for (const type of ['touchstart', 'touchmove', 'touchend', 'touchcancel']) document.removeEventListener(type, onTouch, true);
      if (!ended || down || !baseline || !samples.some(s => s.down && s.t > baseline.t + 2000)) fail('Incomplete held lifecycle');
      return { failure, baseline, anchor: anchor && { occurrence: anchor.occurrence, fragmentId: anchor.fragmentId, unitKey: anchor.unitKey, blockText: anchor.blockText, logicalOffset: anchor.logicalOffset },
        samples, contacts, lifecycle, layouts, domMutations, consumption, remountPolicy: 'UNOBSERVABLE_ON_ANCHOR_REMOVAL_NO_RERESOLUTION', physicalDetector: 'UNAVAILABLE_WRAPPER_OWNS_PAGE_ANCESTOR' };
    },
  };
}
