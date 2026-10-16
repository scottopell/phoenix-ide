(() => {
  if (globalThis.__phoenixReferenceInputGuard) return true;
  let active = null;
  let probeAcknowledged = false;
  const probeType = '__phoenixReferenceProbe_' + Math.random().toString(36).slice(2);
  const ProbeEvent = Event;
  const dispatchProbe = EventTarget.prototype.dispatchEvent.bind(window);
  window.addEventListener(probeType, () => { probeAcknowledged = true; }, {capture: true});
  const receipt = globalThis.__phoenixReferenceInputReceipt;
  if (typeof receipt !== 'function') throw Error('Reference receipt binding unavailable');
  function containsHit(target, x, y) {
    if (!target.isConnected || target.ownerDocument !== document) return false;
    let node = target;
    while (node) {
      const root = node.getRootNode();
      let hit = root.elementFromPoint(x, y);
      while (hit && hit !== node) hit = hit.parentNode || hit.host;
      if (hit !== node) return false;
      if (root === document) return true;
      node = root.host;
    }
    return false;
  }
  function hasFocus(target) {
    if (!target.isConnected || target.ownerDocument !== document) return false;
    let node = target;
    while (node) {
      const root = node.getRootNode();
      if (root.activeElement !== node) return false;
      if (root === document) return true;
      node = root.host;
    }
    return false;
  }
  for (const type of ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click', 'dblclick', 'auxclick', 'keydown', 'keyup', 'keypress', 'beforeinput', 'input']) {
    window.addEventListener(type, event => {
      if (!active) return;
      const keyboard = ['keydown', 'keyup', 'keypress', 'beforeinput', 'input'].includes(event.type);
      if (active.rejected || !(keyboard ? hasFocus(active.node) : containsHit(active.node, event.clientX, event.clientY))) {
        if (!active.rejected) receipt(JSON.stringify({token:active.token, outcome:'rejected'}));
        active.rejected = true;
        event.preventDefault();
        event.stopImmediatePropagation();
      } else if (event.type === 'click') {
        active.clicked = true;
        receipt(JSON.stringify({token:active.token, outcome:'clicked'}));
      }
    }, {capture: true, passive: false});
  }
  Object.defineProperty(globalThis, '__phoenixReferenceInputGuard', {
    value: Object.freeze({
      arm(node, token) {
        probeAcknowledged = false;
        dispatchProbe(new ProbeEvent(probeType));
        if (!probeAcknowledged) throw Error('Reference input guard unavailable after document reset; navigate to a fresh document and call browser_snapshot again');
        if (active) throw Error('A reference input operation is already active');
        if (!node.isConnected || node.ownerDocument !== document) throw Error('stale ref');
        active = {node, token, rejected: false, clicked: false};
      },
      finish(token) {
        if (!active || active.token !== token) throw Error('Reference input guard unavailable');
        const result = {rejected: active.rejected, clicked: active.clicked};
        active = null;
        return result;
      },
      cancel(token) {
        if (active && active.token === token) active = null;
      }
    }),
    configurable: false,
    writable: false,
    enumerable: false
  });
  return true;
})()
