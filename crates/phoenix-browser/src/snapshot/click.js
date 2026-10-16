function() {
  if (!this.isConnected || this.ownerDocument !== document) throw Error('stale ref');
  const r = this.getBoundingClientRect();
  if (!r.width || !r.height || this.disabled) throw Error('Referenced element is not actionable');
  const left = Math.max(0, r.left), right = Math.min(innerWidth, r.right);
  const top = Math.max(0, r.top), bottom = Math.min(innerHeight, r.bottom);
  if (right <= left || bottom <= top) throw Error('Referenced element is outside viewport');
  const x = (left + right) / 2, y = (top + bottom) / 2;
  let node = this;
  while (node) {
    const root = node.getRootNode();
    let hit = root.elementFromPoint(x, y);
    while (hit && hit !== node) hit = hit.parentNode || hit.host;
    if (hit !== node) throw Error('Referenced element is obscured');
    if (root === document) return {x, y};
    node = root.host;
  }
  throw Error('Referenced element is outside the main document');
}
