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
  const guardedRoots = new WeakSet();
  function install(root) {
    guardedRoots.add(root);
    for (const type of ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click', 'dblclick', 'auxclick', 'keydown', 'keyup', 'keypress', 'beforeinput', 'input']) {
      root.addEventListener(type, event => {
        if (!active || !event.isTrusted) return;
        if (active.phase === 'activated') return;
        if (active.dispatch && active.dispatch !== event && active.dispatch.eventPhase !== ProbeEvent.NONE) return;
        const keyboard = ['keydown', 'keyup', 'keypress', 'beforeinput', 'input'].includes(event.type);
        const path = event.composedPath();
        const attached = active.node.isConnected && active.node.ownerDocument === document;
        let projected = active.node;
        for (let ancestor = active.node; attached && ancestor.getRootNode() !== document && ancestor.getRootNode() !== root; ancestor = ancestor.getRootNode().host) {
          const shadow = ancestor.getRootNode();
          if (shadow.mode === 'closed') projected = shadow.host;
        }
        if (active.rejected || !attached || !path.includes(projected) || !(keyboard ? hasFocus(active.node) : containsHit(active.node, event.clientX, event.clientY))) {
          if (!active.rejected) receipt(JSON.stringify({token:active.token, outcome:'rejected'}));
          active.rejected = true;
          event.preventDefault();
          event.stopImmediatePropagation();
        } else if (path.includes(active.node)) {
          active.dispatch = event;
          if (event.type === 'click' && active.phase === 'pointer') {
            active.phase = 'activated';
            receipt(JSON.stringify({token:active.token, outcome:'clicked'}));
          }
        }
      }, {capture: true, passive: false});
    }
  }
  install(window);
  const attachShadow = Element.prototype.attachShadow;
  Element.prototype.attachShadow = function(options) {
    const root = attachShadow.call(this, options);
    install(root);
    return root;
  };
  Object.defineProperty(globalThis, '__phoenixReferenceInputGuard', {
    value: Object.freeze({
      arm(node, token) {
        probeAcknowledged = false;
        dispatchProbe(new ProbeEvent(probeType));
        if (!probeAcknowledged) throw Error('Reference input guard unavailable after document reset; navigate to a fresh document and call browser_snapshot again');
        if (active) throw Error('A reference input operation is already active');
        if (!node.isConnected || node.ownerDocument !== document) throw Error('stale ref');
        for (let ancestor = node; ancestor.getRootNode() !== document; ancestor = ancestor.getRootNode().host) {
          const root = ancestor.getRootNode();
          if (root.mode === 'closed' && !guardedRoots.has(root)) throw Error('Reference input guard unavailable for this closed shadow root; navigate to a fresh document and call browser_snapshot again');
        }
        active = {node, token, phase: 'pointer', rejected: false, dispatch: null};
      },
      beginTyping(token) {
        if (!active || active.token !== token || active.phase !== 'activated' || active.rejected) throw Error('Reference input guard unavailable');
        active.phase = 'typing';
      },
      finish(token) {
        if (!active || active.token !== token) throw Error('Reference input guard unavailable');
        const result = {rejected: active.rejected, clicked: active.phase !== 'pointer'};
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
