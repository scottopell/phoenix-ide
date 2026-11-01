function() {
  if (!this.isConnected || this.ownerDocument !== document) throw Error('stale ref');
  let node = this;
  while (node) {
    const root = node.getRootNode();
    if (root.activeElement !== node) throw Error('Referenced element lost focus');
    if (root === document) return true;
    node = root.host;
  }
  throw Error('Referenced element is outside the main document');
}
