// Pure UTF-16 identity resolver, also installed verbatim in the browser before the probe.
export function installSemanticResolver() {
  function resolve(blocks, anchor) {
    const exact = blocks.filter(b => b.occurrence === anchor.occurrence && b.fragmentId === anchor.fragmentId);
    if (exact.length !== 1) return { missing: `exact-block-count:${exact.length}` };
    const block = exact[0], text = block.segments.join('');
    if (text !== anchor.blockText || text.slice(anchor.quoteStart, anchor.quoteEnd) !== anchor.quote) return { missing: 'logical-text-offset-quote-lost' };
    if (!Number.isInteger(anchor.logicalOffset) || anchor.logicalOffset < 0 || anchor.logicalOffset >= text.length) return { missing: 'logical-offset-out-of-range' };
    let remaining = anchor.logicalOffset;
    for (let segment = 0; segment < block.segments.length; segment++) {
      if (remaining < block.segments[segment].length) return { missing: null, segment, offset: remaining };
      remaining -= block.segments[segment].length;
    }
    return { missing: 'logical-offset-out-of-range' };
  }
  const resolver = { resolve };
  globalThis.__touchSemanticResolver = resolver;
  return resolver;
}
