// Serialized into Chromium by Playwright. No imports or DOM-writing baseline behavior.
export function installTouchProbe({ transcriptId }) {
  const candidates = document.querySelectorAll('.product-conversation-page__transcript #messages.virtual-transcript');
  if (candidates.length !== 1) throw new Error('Expected one production virtual transcript scroll owner');
  const scroller = candidates[0];
  const ownerGeometry = {
    overflowY: getComputedStyle(scroller).overflowY,
    scrollTop: scroller.scrollTop, scrollHeight: scroller.scrollHeight, clientHeight: scroller.clientHeight,
    innerHeight: scroller.querySelector(':scope > .virtual-transcript__inner')?.getBoundingClientRect().height ?? null,
  };
  if (!['auto', 'scroll'].includes(ownerGeometry.overflowY)
    || ownerGeometry.clientHeight <= 0 || ownerGeometry.scrollHeight - ownerGeometry.clientHeight < 24
    || ownerGeometry.scrollTop < 24 || ownerGeometry.innerHeight === null
    || ownerGeometry.innerHeight <= ownerGeometry.clientHeight) {
    throw new Error(`Production scroll owner lacks upward-pan geometry: ${JSON.stringify(ownerGeometry)}`);
  }
  const frames = [], events = [];
  const start = performance.now();
  let phase = 'acquisition', anchor = null, finger = null, baseline = null, released = null;
  let inputDown = false, layoutRevision = 0, overflow = false, invalid = false, stopped = false;
  let lastNode = null, remounts = 0, raf = 0;
  const initialScroll = scroller.scrollTop;
  const record = (kind, detail = {}) => {
    if (events.length >= 512) { overflow = true; return; }
    events.push({ t: performance.now() - start, kind, phase, ...detail });
  };
  record('scroll-owner', ownerGeometry);
  const touch = e => {
    const point = e.touches[0] ?? e.changedTouches[0];
    if (point) finger = { x: point.clientX, y: point.clientY };
    inputDown = e.touches.length > 0;
    record(e.type, { trusted: e.isTrusted, down: inputDown, finger });
  };
  for (const name of ['touchstart', 'touchmove', 'touchend', 'touchcancel']) document.addEventListener(name, touch, { capture: true, passive: true });
  const scroll = () => record('scroll', { top: scroller.scrollTop });
  scroller.addEventListener('scroll', scroll, { passive: true });
  const mutations = new MutationObserver(() => { layoutRevision++; });
  mutations.observe(scroller, { subtree: true, childList: true, characterData: true, attributes: true });
  const resize = new ResizeObserver(() => { layoutRevision++; record('resize'); });
  resize.observe(scroller);
  const texts = root => {
    const nodes = [], walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) nodes.push(walker.currentNode);
    return nodes;
  };
  function resolve() {
    if (invalid) return { missing: 'deliberate-oracle-anchor-invalid' };
    if (!anchor) return { missing: 'not-acquired' };
    const candidates = [...scroller.querySelectorAll('.message[data-message-id]')].filter(el =>
      el.dataset.messageId === anchor.messageId && el.dataset.messageOccurrence === anchor.occurrence);
    if (candidates.length !== 1) return { missing: `exact-occurrence-count:${candidates.length}` };
    const message = candidates[0], row = message.closest('[data-render-unit-key]');
    if (row?.dataset.renderUnitKey !== anchor.unitKey) return { missing: 'render-unit-identity-changed' };
    const blocks = [...message.querySelectorAll('[data-fragment-id]')].filter(el => el.dataset.fragmentId === anchor.fragmentId);
    const resolved = window.__touchSemanticResolver.resolve(blocks.map(el => ({
      occurrence: anchor.occurrence, fragmentId: el.dataset.fragmentId, segments: texts(el).map(n => n.data),
    })), anchor);
    if (resolved.missing) return resolved;
    const node = texts(blocks[0])[resolved.segment];
    const range = document.createRange(); range.setStart(node, resolved.offset); range.setEnd(node, resolved.offset + 1);
    // Node segmentation is geometry only, never persisted identity.
    const rects = [...range.getClientRects()];
    if (rects.length !== 1 || rects[0].width <= 0 || rects[0].height <= 0) return { missing: 'glyph-range-unobservable' };
    if (lastNode && lastNode !== node) { remounts++; record('exact-remount-reresolved'); }
    lastNode = node;
    const r = rects[0];
    return { missing: null, x: r.left + r.width / 2, y: r.top + r.height / 2,
      rowTop: row.getBoundingClientRect().top, messageTop: message.getBoundingClientRect().top,
      range: { left: r.left, top: r.top, width: r.width, height: r.height } };
  }
  const sample = () => {
    if (stopped) return;
    if (frames.length >= 2400) { overflow = true; stopped = true; return; }
    const point = resolve();
    const expected = phase === 'release_settle' || phase === 'post_release' ? released
      : finger && baseline ? { x: finger.x + baseline.x, y: finger.y + baseline.y } : null;
    frames.push({ t: performance.now() - start, phase, ...point, expectedX: expected?.x, expectedY: expected?.y,
      finger, inputDown, scrollTop: scroller.scrollTop, scrollHeight: scroller.scrollHeight,
      clientHeight: scroller.clientHeight, layoutRevision,
      mountedUnits: scroller.querySelectorAll('[data-render-unit-key]').length });
    raf = requestAnimationFrame(sample);
  };
  raf = requestAnimationFrame(sample);
  window.__touchProbe = {
    contact() {
      const point = window.__touchTextContact(scroller, transcriptId);
      record('precontact', point);
      return { x: point.x, y: point.y };
    },
    bounds() { const r = scroller.getBoundingClientRect(); return { x: r.left, y: r.top, width: r.width, height: r.height }; },
    acquire(intent) {
      if (!inputDown || !finger || Math.abs(scroller.scrollTop - initialScroll) < 24 || !events.some(e => e.kind === 'touchmove' && e.trusted)) throw new Error('Native pan not acquired (trusted touch + >=24px actual scroll required)');
      const correspondence = window.__touchInputCorrespondence(intent, finger);
      record('input-correspondence', correspondence);
      if (correspondence.status !== 'MATCHED') throw new Error('Input intent/DOM observation mismatch before baseline');
      const caret = document.caretRangeFromPoint(finger.x, finger.y);
      const node = caret?.startContainer;
      const message = node?.parentElement?.closest('.message[data-message-id]');
      if (!caret || node?.nodeType !== Node.TEXT_NODE || !message || !node.data.trim()) throw new Error('No semantic text glyph under acquired finger');
      const occurrence = message.dataset.messageOccurrence;
      if (occurrence !== `${transcriptId}:${message.dataset.messageId}`) throw new Error('Transcript/message occurrence unavailable');
      const offset = Math.min(caret.startOffset, node.length - 1);
      const block = node.parentElement.closest('[data-fragment-id]');
      if (!block || !message.contains(block)) throw new Error('Stable content-block identity unavailable');
      const nodes = texts(block), nodeIndex = nodes.indexOf(node);
      if (nodeIndex < 0) throw new Error('Caret outside content block');
      const logicalOffset = nodes.slice(0, nodeIndex).reduce((n, item) => n + item.length, 0) + offset;
      const blockText = nodes.map(n => n.data).join('');
      anchor = { transcriptId, messageId: message.dataset.messageId, occurrence,
        unitKey: message.closest('[data-render-unit-key]')?.dataset.renderUnitKey,
        fragmentId: block.dataset.fragmentId, blockText, logicalOffset,
        quoteStart: Math.max(0, logicalOffset - 18), quoteEnd: Math.min(blockText.length, logicalOffset + 19) };
      anchor.quote = blockText.slice(anchor.quoteStart, anchor.quoteEnd);
      const point = resolve();
      if (point.missing || Math.hypot(point.x - finger.x, point.y - finger.y) > 36) throw new Error('Acquired range not under finger');
      baseline = { x: point.x - finger.x, y: point.y - finger.y };
      phase = 'drag'; record('acquired', { anchor, nativeScrollDelta: scroller.scrollTop - initialScroll });
      return anchor;
    },
    phase(value) {
      if (value === 'release_settle') {
        if (!finger || !baseline) throw new Error('Release without acquired finger');
        // Do not rebase on observed layout: a prior jump must not disappear at release.
        released = { x: finger.x + baseline.x, y: finger.y + baseline.y };
      }
      phase = value; record('phase', { value });
    },
    note: record,
    mutate(name) {
      record('deliberate-control', { name });
      if (name === 'forced-scroll-bottom') {
        const before = scroller.scrollTop;
        scroller.scrollTop = scroller.scrollHeight; // Deliberate app-scroll mutation, NEVER a gesture proxy.
        record('forced-scroll-write', { before, after: scroller.scrollTop });
      } else if (name === 'uncompensated-above-height') {
        const point = resolve(); if (point.missing) throw new Error(point.missing);
        const node = lastNode;
        const parent = node.parentElement;
        if (!parent || parent.getBoundingClientRect().height < 1) throw new Error('Above-height control target missing');
        scroller.style.overflowAnchor = 'none';
        parent.style.overflowAnchor = 'none';
        const block = document.createElement('span'); block.style.cssText = 'display:block;height:160px;overflow-anchor:none';
        block.dataset.touchDeliberateAboveHeight = '160';
        parent.insertBefore(block, node); // Labelled height-control injection, not streaming content.
        record('above-height-injected', { height: block.getBoundingClientRect().height });
      } else if (name === 'oracle-anchor-invalid') invalid = true;
      else throw new Error('Unknown control');
    },
    async controlWindow(name) {
      const fromFrame = frames.length;
      record('control-injection', { name, fromFrame, maxFrames: 4, maxMs: 200 });
      this.mutate(name);
      // Schedule awaits this acknowledgement; no producer/reflow step may overlap it.
      for (let i = 0; i < 4; i++) await new Promise(resolve => requestAnimationFrame(resolve));
      record('control-ack', { name, fromFrame, toFrame: frames.length });
    },
    snapshot() { return { anchor, frames, events, overflow, remounts, phase }; },
    stop() {
      stopped = true; cancelAnimationFrame(raf); mutations.disconnect(); resize.disconnect();
      scroller.removeEventListener('scroll', scroll);
      for (const name of ['touchstart', 'touchmove', 'touchend', 'touchcancel']) document.removeEventListener(name, touch, true);
      return this.snapshot();
    },
  };
}
