export function installTextContactSelector() {
  window.__touchTextContact = (owner, transcriptId) => {
    let nodes = 0, units = 0, candidates = 0;
    const fail = reason => { throw new Error(`Text contact unavailable: ${reason}; nodes=${nodes}; units=${units}; candidates=${candidates}`); };
    const walker = document.createTreeWalker(owner, NodeFilter.SHOW_ALL);
    let node;
    while ((node = walker.nextNode())) {
      if (++nodes > 8192) fail('node-budget');
      if (node.nodeType !== Node.TEXT_NODE) continue;
      const block = node.parentElement?.closest('[data-fragment-id]');
      const message = block?.closest('.message[data-message-id]');
      const row = message?.closest('[data-render-unit-key]');
      const identity = () => owner.isConnected && node.isConnected && block?.isConnected && message?.isConnected
        && row?.isConnected && owner.contains(row) && row.contains(message) && message.contains(block) && block.contains(node)
        && Boolean(row.dataset.renderUnitKey) && Boolean(block.dataset.fragmentId)
        && message.dataset.messageOccurrence === `${transcriptId}:${message.dataset.messageId}`
        && Number(message.dataset.messageId?.split('-').at(-1)) > 0;
      for (let offset = 0; offset < node.data.length; offset++) {
        if (++units > 16384) fail('code-unit-budget');
        const value = node.data.charCodeAt(offset);
        if (!identity() || /\s/u.test(node.data[offset]) || (value >= 0xd800 && value <= 0xdfff)) continue;
        if (++candidates > 4096) fail('range-budget');
        const range = document.createRange();
        range.setStart(node, offset); range.setEnd(node, offset + 1);
        const rects = [...range.getClientRects()];
        if (rects.length !== 1) continue;
        const r = rects[0], x = r.left + r.width / 2, y = r.top + r.height / 2;
        if (![x, y, r.width, r.height].every(Number.isFinite) || r.width <= 0 || r.height <= 0) continue;
        const caret = document.caretRangeFromPoint(x, y);
        if (caret?.startContainer !== node || ![offset, offset + 1].includes(caret.startOffset)) continue;
        const hit = document.elementFromPoint(x, y);
        if (!hit || !block.contains(hit) || !message.contains(hit) || !owner.contains(hit)) continue;
        const fresh = [...range.getClientRects()], bounds = owner.getBoundingClientRect();
        if (!identity() || fresh.length !== 1 || fresh[0].left !== r.left || fresh[0].top !== r.top
          || fresh[0].width !== r.width || fresh[0].height !== r.height) continue;
        if (x <= bounds.left || x >= bounds.right || y <= bounds.top || y + 70 >= bounds.bottom) continue;
        return { x, y, nodes, units, candidates, width: r.width, height: r.height,
          ownerWidth: bounds.width, ownerHeight: bounds.height };
      }
    }
    fail('no-eligible-code-unit');
  };
}
