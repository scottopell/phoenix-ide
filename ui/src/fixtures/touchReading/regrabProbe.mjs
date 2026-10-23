export function installRegrabProbe({ transcriptId }) {
  const owners = document.querySelectorAll('.product-conversation-page__transcript #messages.virtual-transcript');
  if (owners.length !== 1) throw new Error('Regrab owner ambiguous');
  const owner = owners[0];
  if (!['auto', 'scroll'].includes(getComputedStyle(owner).overflowY)
    || owner.clientHeight < 300 || owner.scrollTop < 160) throw new Error('Regrab owner geometry invalid');
  const events = [];
  let overflow = false, raf = 0, stopped = false, prepared = false;
  const nodesOf = block => {
    const nodes = [], walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT);
    let units = 0;
    while (walker.nextNode()) {
      units += walker.currentNode.length;
      if (nodes.length >= 8192 || units > 16384) throw new Error('Semantic scan budget');
      nodes.push(walker.currentNode);
    }
    return nodes;
  };
  function resolve(a, contact = null) {
    if (!a || !owner.isConnected) return { missing: 'owner-or-anchor-lost' };
    const matches = [...owner.querySelectorAll('.message[data-message-id]')].filter(m =>
      m.dataset.messageId === a.messageId && m.dataset.messageOccurrence === a.occurrence);
    if (matches.length !== 1) return { missing: 'occurrence-not-unique' };
    const message = matches[0], row = message.closest('[data-render-unit-key]');
    if (!message.isConnected || !row || row.dataset.renderUnitKey !== a.unitKey) return { missing: 'unit-lost' };
    const blocks = [...message.querySelectorAll('[data-fragment-id]')].filter(b => b.dataset.fragmentId === a.fragmentId);
    if (blocks.length !== 1 || !blocks[0].isConnected) return { missing: 'fragment-not-unique' };
    const nodes = nodesOf(blocks[0]);
    const location = window.__touchSemanticResolver.resolve([{
      occurrence: a.occurrence, fragmentId: a.fragmentId, segments: nodes.map(n => n.data),
    }], a);
    if (location.missing) return location;
    const node = nodes[location.segment];
    if (!node?.isConnected) return { missing: 'text-node-lost' };
    const range = document.createRange();
    range.setStart(node, location.offset); range.setEnd(node, location.offset + 1);
    const rects = [...range.getClientRects()];
    if (rects.length !== 1) return { missing: 'range-not-unique' };
    const r = rects[0];
    const hit = contact ? document.elementFromPoint(contact.x, contact.y) : null;
    const ownedHit = contact !== null && contact.path.includes(blocks[0])
      && hit !== null && (hit === blocks[0] || blocks[0].contains(hit));
    return { connected: true, identityMatches: true, ownedHit, occurrences: matches.length, fragments: blocks.length,
      x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2,
      left: r.left, right: r.right, top: r.top, bottom: r.bottom };
  }
  const observer = window.__createRegrabObserver(owner, resolve, event => {
    if (events.length >= 768) { overflow = true; return; }
    events.push(event);
  });
  const sample = () => {
    if (stopped) return;
    observer.frame(); raf = requestAnimationFrame(sample);
  };
  raf = requestAnimationFrame(sample);
  window.__regrabProbe = {
    firstContact() {
      const p = window.__touchTextContact(owner, transcriptId), bounds = owner.getBoundingClientRect();
      if (p.y + 160 >= bounds.bottom) throw new Error('Fixed flick lacks 160px room');
      return { x: p.x, y: p.y };
    },
    prepare() {
      if (prepared) throw new Error('Regrab candidate already selected');
      prepared = true;
      const p = window.__touchTextContact(owner, transcriptId);
      const caret = document.caretRangeFromPoint(p.x, p.y), node = caret?.startContainer;
      if (!caret || node?.nodeType !== Node.TEXT_NODE || !node.isConnected
        || caret.startOffset >= node.length || !node.data[caret.startOffset]?.trim()) throw new Error('Exact caret unavailable');
      const message = node.parentElement.closest('.message[data-message-id]');
      const block = node.parentElement.closest('[data-fragment-id]');
      const row = message?.closest('[data-render-unit-key]');
      if (!message || !block || !row || !owner.contains(message) || !message.contains(block)
        || message.dataset.messageOccurrence !== `${transcriptId}:${message.dataset.messageId}`
        || !row.dataset.renderUnitKey || !block.dataset.fragmentId) throw new Error('Exact semantic identity unavailable');
      const nodes = nodesOf(block), index = nodes.indexOf(node);
      if (index < 0) throw new Error('Caret not in semantic block');
      const logicalOffset = nodes.slice(0, index).reduce((sum, n) => sum + n.length, 0) + caret.startOffset;
      const blockText = nodes.map(n => n.data).join('');
      const quoteStart = Math.max(0, logicalOffset - 18), quoteEnd = Math.min(blockText.length, logicalOffset + 19);
      const candidate = Object.freeze({ transcriptId, messageId: message.dataset.messageId,
        occurrence: message.dataset.messageOccurrence, unitKey: row.dataset.renderUnitKey,
        fragmentId: block.dataset.fragmentId, logicalOffset, blockText, quoteStart, quoteEnd,
        quote: blockText.slice(quoteStart, quoteEnd) });
      const exact = resolve(candidate);
      if (exact.missing || p.x < exact.left || p.x > exact.right || p.y < exact.top || p.y > exact.bottom)
        throw new Error('Selected coordinate not inside exact glyph');
      if (!observer.arm(candidate)) throw new Error('Regrab PRE observation failed');
      return { x: exact.x, y: exact.y };
    },
    stop() {
      const result = observer.finish();
      stopped = true; cancelAnimationFrame(raf); observer.stop();
      return { ...result, status: overflow ? 'UNOBSERVABLE' : result.status, overflow, events };
    },
  };
}
