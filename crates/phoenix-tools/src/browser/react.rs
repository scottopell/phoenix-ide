//! `browser_inject_react_devtools` — install the `window.__phoenix` React helper
//!
//! REQ-BT-017: React Component Access
//!
//! Uses `Page.addScriptToEvaluateOnNewDocument` to inject a lightweight helper
//! into every page *before* React initialises. The helper hooks into React's
//! `__REACT_DEVTOOLS_GLOBAL_HOOK__` — the stable, public interface React exposes
//! for `DevTools` integration — giving structured access to the live fiber tree
//! without DOM traversal or relying on minification-sensitive property names.
//!
//! The injected API (`window.__phoenix`) provides three functions:
//!
//! - `getContext(keys)` — find a context value by duck-typing its shape
//! - `callContext(keys, method, ...args)` — find and call a method on a context value
//! - `getState(componentName)` — get hook state array for a named component
//!
//! See the task spec (tasks/596-p1-ready-browser-react-component-access-tool.md)
//! and spec update (specs/browser-tool/requirements.md REQ-BT-017).

// No imports needed — this module only exports constants (the JS scripts).
// The hook is injected by BrowserSession::launch_and_init() in session.rs.
// React detection is called by BrowserNavigateTool in tools.rs.

// ============================================================================
// The injected JavaScript helper
// ============================================================================

#[allow(clippy::doc_markdown, clippy::doc_overindented_list_items)]
/// The JavaScript installed into every new document via `addScriptToEvaluateOnNewDocument`.
///
/// Design notes:
/// - Installs before any page JS runs, so React finds the hook at startup and
///   registers its fiber roots into it automatically.
/// - Idempotent: guards against double-registration by checking `window.__phoenix`
///   and the `__REACT_DEVTOOLS_GLOBAL_HOOK__` sentinel before writing anything.
/// - Works with both development and minified production builds — no display name
///   lookups in hot paths; `getContext` uses duck-typing, `getState` falls back
///   to fiber order when names are stripped.
/// - Harmless on non-React pages: the hook exists but `onCommitFiberRoot` is never
///   called, so `__phoenix` helpers simply return `null` / `[]`.
///
/// ## Hook implementation: vendored from React DevTools
///
/// The `__REACT_DEVTOOLS_GLOBAL_HOOK__` object installed here is derived from
/// React's official `installHook()` function in:
///   https://github.com/facebook/react/blob/main/packages/react-devtools-shared/src/hook.js
///
/// We vendor the **minimal viable subset** rather than the full 700-line function.
/// The full function includes console patching for StrictMode, profiler module range
/// tracking, and DCE detection — none of which we need. What we keep:
///
/// - `renderers: Map`         — React Fast Refresh (`@react-refresh`) iterates this
///                              via `hook.renderers.forEach()`. Missing it crashes
///                              Vite dev server page loads (see task 599).
/// - `rendererInterfaces: Map` — DevTools backend populates this; we keep it as an
///                              empty Map so code that checks it doesn't crash.
/// - `backends: Map`          — Same reason.
/// - `listeners: {}`          — Event emitter storage for `on`/`off`/`emit`/`sub`.
/// - `inject(renderer)`       — React calls this to register renderers. Must return
///                              a numeric ID and populate `renderers` Map.
/// - `on`/`off`/`emit`/`sub`  — Event emitter methods. React and third-party tools
///                              (Fast Refresh, DevTools backend) use these.
/// - `getFiberRoots(id)`      — Returns a Set of fiber roots per renderer ID.
/// - `onCommitFiberRoot`      — Called after every React render commit.
/// - `onCommitFiberUnmount`   — Called when a fiber is unmounted.
/// - `onPostCommitFiberRoot`  — Called after passive effects (React 18+).
/// - `setStrictMode`          — Called during StrictMode double-renders.
/// - `supportsFiber: true`    — React 16+ checks this flag.
/// - `supportsFlight: true`   — React Flight (Server Components) checks this.
/// - `checkDCE`               — React production builds call this.
///
/// ## How to update
///
/// If React changes the hook interface (rare — it's a de facto public API):
/// 1. Read the latest hook.js at the URL above
/// 2. Compare the returned object shape with what we construct below
/// 3. Add any new properties React expects
/// 4. Test against both Vite dev server (Fast Refresh) and production builds
///
/// ## Alternatives considered
///
/// - **Full `react-devtools-inline/backend`**: 206 KB minified. Includes the complete
///   DevTools inspection protocol, profiler, etc. Overkill for our getContext/callContext
///   use case. See task 596 design notes.
/// - **Our original minimal stub**: Missing `renderers`, `inject`, event emitter.
///   Crashed Vite's `@react-refresh` preamble (task 599).
/// - **Runtime npm dependency**: Would require a build step to bundle JS into the
///   Rust binary. The vendored approach keeps us as a single static binary.
pub const PHOENIX_REACT_HELPER_SCRIPT: &str = r"
(function() {
  // ── Idempotency guard ──────────────────────────────────────────────────────
  // Skip installation if the helper is already present (e.g. tool called twice
  // before a navigation, or the page ships its own __phoenix). Guard also on
  // __perfRead so a re-injection cannot double-install the longtask observer.
  if (window.__phoenix && window.__phoenix.__installed && window.__phoenix.__perfRead) {
    return;
  }

  // ── Commit metrics ring buffer (REQ-BT-019.4 / .8) ──────────────────────────
  // Hoisted to the IIFE scope (NOT the hook-install block) so it exists
  // whether we install our own hook OR wrap a pre-existing one, and so the
  // __phoenix API methods below can read it. Bounded so a long-lived page
  // can't grow this unboundedly. The run_scenario harness brackets a run
  // with __resetCommits()/__getCommits().
  var __commits = [];
  var __COMMITS_CAP = 500;
  var __TOP_N = 20;
  // REQ-BT-019.13: set true the first time any commit carries a numeric
  // fiber.actualDuration. Distinguishes a profiling-capable React build
  // (measured) from a production build that never exposes actualDuration
  // (no_profiling_build). Reset by __resetCommits() so a per-run bracket
  // re-derives it from that run's commits.
  var __sawActualDuration = false;

  // ── Page-anchored measurement window (REQ-BT-019.20) ────────────────────────
  // The measured window is defined IN THE PAGE, not inferred from two host-side
  // Performance.getMetrics round-trips (F5: those collapse to ~0 for real
  // in-window work). A longtask PerformanceObserver installed here (at
  // document-start, BEFORE page scripts) accumulates blocking-task duration
  // from the very start. __perfReset opens a window (t0 = performance.now(),
  // accumulators zeroed, React commit buffer cleared); __perfRead closes it
  // and returns the in-page accumulators in one call.
  var __lt_ms = 0, __lt_n = 0, __win_t0 = null;
  try {
    var __po = new PerformanceObserver(function (l) {
      l.getEntries().forEach(function (e) {
        __lt_ms += e.duration;
        __lt_n += 1;
      });
    });
    __po.observe({ entryTypes: ['longtask'] });
  } catch (e) {}

  // Walk the fiber tree from a root collecting per-component render cost
  // plus a best-effort why-did-render attribution. Defensive throughout:
  // must be harmless and never throw on non-React or exotic pages.
  function __collectCommit(root) {
    try {
      if (!root || !root.current) return null;
      var components = [];
      var stack = [root.current];
      while (stack.length > 0) {
        var fiber = stack.pop();
        if (!fiber) continue;
        if (fiber.sibling) stack.push(fiber.sibling);
        if (fiber.child) stack.push(fiber.child);
        var dur = fiber.actualDuration;
        if (typeof dur !== 'number') continue;
        __sawActualDuration = true;
        var name = 'Unknown';
        try {
          var t = fiber.type;
          if (typeof t === 'string') name = t;
          else if (t && (t.displayName || t.name)) name = t.displayName || t.name;
        } catch (e) {}
        var isMount = fiber.alternate == null;
        var rec = {
          name: name,
          actualDuration: dur,
          phase: isMount ? 'mount' : 'update',
          changedProps: [],
          changedHooks: []
        };
        // why-did-render: only meaningful for update-phase fibers.
        if (!isMount) {
          try {
            var cur = fiber.memoizedProps || {};
            var prev = (fiber.alternate && fiber.alternate.memoizedProps) || {};
            var keys = {};
            var k;
            for (k in cur) keys[k] = true;
            for (k in prev) keys[k] = true;
            for (k in keys) {
              if (cur[k] !== prev[k]) {
                // REQ-BT-019.17: label, do not diagnose. A bare !== flags
                // an inline object/array/fn prop (new reference every
                // render) the same as a real value change; an agent reads
                // the bare key as a root cause. Classify cheaply so the
                // #1 false positive is labelled, not stated as fact.
                var cv = cur[k];
                var pv = prev[k];
                var ct = typeof cv;
                var pt = typeof pv;
                var curRef = (cv !== null && (ct === 'object' || ct === 'function'));
                var prevRef = (pv !== null && (pt === 'object' || pt === 'function'));
                var kind;
                if (curRef || prevRef) {
                  kind = 'reference_changed';
                } else if (ct !== pt) {
                  kind = 'value_changed';
                } else if (ct === 'undefined' || cv === null || pv === null) {
                  kind = 'value_changed';
                } else {
                  kind = 'value_changed';
                }
                rec.changedProps.push({ key: k, kind: kind });
              }
            }
          } catch (e) {}
          try {
            var a = fiber.memoizedState;
            var b = fiber.alternate ? fiber.alternate.memoizedState : null;
            var idx = 0;
            while (a && b) {
              if (a.memoizedState !== b.memoizedState) rec.changedHooks.push(idx);
              a = a.next;
              b = b.next;
              idx++;
              if (idx > 256) break;
            }
          } catch (e) {}
        }
        components.push(rec);
      }
      // REQ-BT-019.4: the commit's cost is the ROOT fiber's
      // actualDuration — React already accumulates the whole committed
      // subtree there. Summing every fiber double-counts: a parent's
      // actualDuration already includes its children's, so a naive sum
      // inflates roughly by tree depth/fan-out. Fall back to the
      // largest single subtree (never the sum) if the root frame did
      // not carry a number.
      var rootDur = (root.current && typeof root.current.actualDuration === 'number')
        ? root.current.actualDuration : null;
      var total;
      if (rootDur !== null) {
        total = rootDur;
      } else {
        total = 0;
        for (var i = 0; i < components.length; i++) {
          if (components[i].actualDuration > total) total = components[i].actualDuration;
        }
      }
      components.sort(function(x, y) { return y.actualDuration - x.actualDuration; });
      var mountCount = 0;
      var updateCount = 0;
      for (var j = 0; j < components.length; j++) {
        if (components[j].phase === 'mount') mountCount++; else updateCount++;
      }
      return {
        ts: (typeof performance !== 'undefined' && performance.now) ? performance.now() : Date.now(),
        count: components.length,
        mountCount: mountCount,
        updateCount: updateCount,
        totalActualDuration: total,
        components: components.slice(0, __TOP_N)
      };
    } catch (e) {
      return null;
    }
  }

  function __recordCommit(root) {
    try {
      var rec = __collectCommit(root);
      if (rec) {
        __commits.push(rec);
        if (__commits.length > __COMMITS_CAP) __commits.shift();
      }
    } catch (e) {}
  }

  // ── React DevTools hook (vendored from react-devtools-shared/src/hook.js) ──
  //
  // If a hook already exists (e.g. React DevTools extension installed it),
  // we DON'T replace it — but we DO wrap its onCommitFiberRoot so commit
  // metrics are still recorded (REQ-BT-017: wrap, don't replace).
  // If no hook exists, we install one with the shape React expects.
  if (!window.__REACT_DEVTOOLS_GLOBAL_HOOK__) {
    // --- Event emitter ---
    var listeners = {};
    function on(event, fn) {
      if (!listeners[event]) listeners[event] = [];
      listeners[event].push(fn);
    }
    function off(event, fn) {
      if (!listeners[event]) return;
      var idx = listeners[event].indexOf(fn);
      if (idx !== -1) listeners[event].splice(idx, 1);
      if (!listeners[event].length) delete listeners[event];
    }
    function emit(event, data) {
      if (listeners[event]) listeners[event].forEach(function(fn) { fn(data); });
    }
    function sub(event, fn) {
      on(event, fn);
      return function() { off(event, fn); };
    }

    // --- Renderer tracking ---
    // React calls inject(renderer) at startup to register itself.
    // The returned ID is passed to all subsequent onCommitFiberRoot calls.
    var renderers = new Map();        // ID -> renderer object
    var rendererInterfaces = new Map(); // ID -> renderer interface (populated by DevTools backend)
    var backends = new Map();
    var fiberRoots = {};              // ID -> Set of fiber roots
    var uidCounter = 0;

    function inject(renderer) {
      var id = ++uidCounter;
      renderers.set(id, renderer);
      emit('renderer', { id: id, renderer: renderer });
      return id;
    }

    function getFiberRoots(rendererID) {
      if (!fiberRoots[rendererID]) {
        fiberRoots[rendererID] = new Set();
      }
      return fiberRoots[rendererID];
    }

    // --- Lifecycle hooks called by React ---
    function onCommitFiberRoot(rendererID, root, priorityLevel) {
      var mountedRoots = getFiberRoots(rendererID);
      var current = root.current;
      var isKnownRoot = mountedRoots.has(root);
      var isUnmounting = current.memoizedState == null ||
                         current.memoizedState.element == null;
      if (!isKnownRoot && !isUnmounting) {
        mountedRoots.add(root);
      } else if (isKnownRoot && isUnmounting) {
        mountedRoots.delete(root);
      }
      // Record per-commit render metrics (REQ-BT-019.4 / .8) into the
      // bounded ring buffer. Best-effort; never blocks React's commit.
      __recordCommit(root);
      var iface = rendererInterfaces.get(rendererID);
      if (iface != null && iface.handleCommitFiberRoot) {
        iface.handleCommitFiberRoot(root, priorityLevel);
      }
    }

    function onCommitFiberUnmount(rendererID, fiber) {
      var iface = rendererInterfaces.get(rendererID);
      if (iface != null && iface.handleCommitFiberUnmount) {
        iface.handleCommitFiberUnmount(fiber);
      }
    }

    function onPostCommitFiberRoot(rendererID, root) {
      var iface = rendererInterfaces.get(rendererID);
      if (iface != null && iface.handlePostCommitFiberRoot) {
        iface.handlePostCommitFiberRoot(root);
      }
    }

    // --- Assemble the hook object ---
    // This shape matches what React's installHook() returns.
    // See: https://github.com/facebook/react/blob/main/packages/react-devtools-shared/src/hook.js
    var hook = {
      rendererInterfaces: rendererInterfaces,
      listeners: listeners,
      backends: backends,
      renderers: renderers,            // Critical: @react-refresh calls renderers.forEach()
      hasUnsupportedRendererAttached: false,
      supportsFiber: true,             // React 16+ checks this
      supportsFlight: true,            // React Flight (Server Components) checks this
      emit: emit,
      getFiberRoots: getFiberRoots,
      inject: inject,
      on: on,
      off: off,
      sub: sub,
      checkDCE: function() {},         // React production builds call this
      onCommitFiberUnmount: onCommitFiberUnmount,
      onCommitFiberRoot: onCommitFiberRoot,
      onPostCommitFiberRoot: onPostCommitFiberRoot,  // React 18+
      setStrictMode: function() {},    // Called during StrictMode; we don't patch console
      getInternalModuleRanges: function() { return []; },
      registerInternalModuleStart: function() {},
      registerInternalModuleStop: function() {}
    };

    // Use Object.defineProperty like the real DevTools hook does.
    // configurable: true so tests can delete and recreate.
    Object.defineProperty(window, '__REACT_DEVTOOLS_GLOBAL_HOOK__', {
      configurable: true,
      enumerable: false,
      get: function() { return hook; }
    });
  } else {
    // A hook already exists (DevTools extension or the app's own). Per
    // REQ-BT-017 we wrap rather than replace: chain our commit recorder
    // onto the existing onCommitFiberRoot so metrics still flow without
    // breaking the incumbent hook.
    var existing = window.__REACT_DEVTOOLS_GLOBAL_HOOK__;
    if (existing && !existing.__phoenixWrapped) {
      var prevOCFR = existing.onCommitFiberRoot;
      existing.onCommitFiberRoot = function(rendererID, root, priorityLevel) {
        __recordCommit(root);
        if (typeof prevOCFR === 'function') {
          return prevOCFR.call(existing, rendererID, root, priorityLevel);
        }
      };
      existing.__phoenixWrapped = true;
    }
  }

  // ── Reference to the hook (ours or pre-existing) ───────────────────────────
  var hook = window.__REACT_DEVTOOLS_GLOBAL_HOOK__;

  // ── Collect all fiber roots from the hook ─────────────────────────────────
  // The hook stores fiber roots per renderer ID via getFiberRoots(id) -> Set.
  // We collect all roots across all renderers for searching.
  function getAllFiberRoots() {
    var roots = [];
    if (hook.getFiberRoots && hook.renderers) {
      hook.renderers.forEach(function(renderer, id) {
        var set = hook.getFiberRoots(id);
        if (set) set.forEach(function(root) { roots.push(root); });
      });
    }
    return roots;
  }

  // ── Depth-first fiber tree search ─────────────────────────────────────────
  // Walks child → sibling links. Returns the first fiber for which `predicate`
  // returns truthy, or null if not found.
  function findFiber(root, predicate) {
    // Start from root.current if available (FiberRootNode → FiberNode)
    var start = root.current || root;
    var stack = [start];
    while (stack.length > 0) {
      var fiber = stack.pop();
      if (!fiber) continue;
      if (predicate(fiber)) return fiber;
      // Push sibling before child so child is explored first (DFS pre-order).
      if (fiber.sibling) stack.push(fiber.sibling);
      if (fiber.child)   stack.push(fiber.child);
    }
    return null;
  }

  // ── Duck-typing context search ─────────────────────────────────────────────
  // A context value matches if ALL supplied keys are present on the object
  // (using `in` operator — presence only, no value check).
  function matchesKeys(value, keys) {
    if (!value || typeof value !== 'object') return false;
    for (var i = 0; i < keys.length; i++) {
      if (!(keys[i] in value)) return false;
    }
    return true;
  }

  // Scan every registered fiber root for a ContextProvider (tag === 10) whose
  // `memoizedProps.value` duck-types to the requested shape.
  function findContext(keys) {
    var roots = getAllFiberRoots();
    for (var r = 0; r < roots.length; r++) {
      var found = findFiber(roots[r], function(fiber) {
        // tag 10 = ContextProvider in React source (stable across React 16–18)
        if (fiber.tag !== 10) return false;
        var val = fiber.memoizedProps && fiber.memoizedProps.value;
        return matchesKeys(val, keys);
      });
      if (found) return found.memoizedProps.value;
    }
    return null;
  }

  // ── Public API ─────────────────────────────────────────────────────────────
  window.__phoenix = {
    // Sentinel so the idempotency guard works across multiple injections.
    __installed: true,

    /**
     * getContext(keys) → context value | null
     *
     * Find a React context value by duck-typing: returns the first context
     * whose value has all `keys` as own or inherited properties.
     *
     * Example:
     *   window.__phoenix.getContext(['openFile', 'closeFile'])
     */
    getContext: function(keys) {
      return findContext(keys);
    },

    /**
     * callContext(keys, method, ...args) → return value | null
     *
     * Find a context value (same duck-typing as getContext) and call `method`
     * on it, forwarding additional arguments. Returns the method's return value,
     * or null if the context or method was not found.
     *
     * Example:
     *   window.__phoenix.callContext(['openFile'], 'openFile', '/src/main.rs')
     */
    callContext: function(keys, method) {
      var ctx = findContext(keys);
      if (!ctx) return null;
      if (typeof ctx[method] !== 'function') return null;
      var args = Array.prototype.slice.call(arguments, 2);
      return ctx[method].apply(ctx, args);
    },

    /**
     * getState(componentName) → array of hook state values | []
     *
     * Find the first fiber whose `type.name` or `type.displayName` matches
     * `componentName` and return its memoized hook state as an ordered array.
     * The caller must know the hook order (same limitation as React DevTools).
     *
     * Note: In minified production builds, display names are stripped and this
     * function will return []. Prefer getContext / callContext for production use.
     *
     * Example:
     *   window.__phoenix.getState('FileExplorer')
     */
    getState: function(componentName) {
      var states = [];
      var roots = getAllFiberRoots();
      for (var r = 0; r < roots.length; r++) {
        var found = findFiber(roots[r], function(fiber) {
          var type = fiber.type;
          if (!type) return false;
          return type.name === componentName || type.displayName === componentName;
        });
        if (found) {
          // Unwind the memoizedState linked list into an array.
          var node = found.memoizedState;
          while (node) {
            states.push(node.memoizedState);
            node = node.next;
          }
          return states;
        }
      }
      return states;
    },

    /**
     * listContexts() → array of partial context snapshots (debug aid)
     *
     * Returns a list of objects describing each ContextProvider found in the
     * fiber tree: { keys: string[], value: any }. Useful for discovering what
     * context shapes are available without knowing the component names upfront.
     *
     * Large context values are truncated to prevent JSON serialisation issues.
     */
    listContexts: function() {
      var results = [];
      var roots = getAllFiberRoots();
      for (var r = 0; r < roots.length; r++) {
        (function searchRoot(fiber) {
          if (!fiber) return;
          // Start from .current if this is a FiberRootNode
          var node = fiber.current || fiber;
          (function walk(f) {
            if (!f) return;
            if (f.tag === 10) {
              var val = f.memoizedProps && f.memoizedProps.value;
              if (val && typeof val === 'object') {
                results.push({
                  keys: Object.keys(val),
                  value: val
                });
              }
            }
            if (f.child)   walk(f.child);
            if (f.sibling) walk(f.sibling);
          })(node);
        })(roots[r]);
      }
      return results;
    },

    /**
     * __getCommits() → array of recorded commit records (copy)
     *
     * Each record: { ts, count, mountCount, updateCount,
     * totalActualDuration, components: [{name, actualDuration, phase,
     * changedProps: [{key, kind}], changedHooks}, ...] }. kind is
     * reference_changed | value_changed | unknown (REQ-BT-019.17). Used
     * by the run_scenario harness (REQ-BT-019.4) per run.
     */
    __getCommits: function() {
      return __commits.slice();
    },

    /**
     * __resetCommits() → void
     *
     * Clears the commit ring buffer. The run_scenario harness calls this
     * at the start of each run so commits are attributed to that run only.
     */
    __resetCommits: function() {
      __commits.length = 0;
      __sawActualDuration = false;
    },

    /**
     * __reactStatus() returns 'measured' | 'absent' | 'no_profiling_build'
     *
     * REQ-BT-019.13 React-capability probe. The harness reads this so a
     * not-taken React measurement is never a numeric 0:
     *   - absent              no renderers / no fiber roots (no React).
     *   - no_profiling_build  React present but no commit ever carried a
     *                         numeric actualDuration (production build).
     *   - measured            React present and actualDuration seen.
     * Defensive: any throw returns absent (the safe no-data default).
     */
    __reactStatus: function() {
      try {
        var roots = getAllFiberRoots();
        if (!roots || roots.length === 0) return 'absent';
        return __sawActualDuration ? 'measured' : 'no_profiling_build';
      } catch (e) {
        return 'absent';
      }
    },

    /**
     * __getWhyRender() → { note, components: [{name, changedProps,
     *                       changedHooks}] }
     *
     * Best-effort why-did-render attribution (REQ-BT-019.8) derived from
     * the recorded commits: every update-phase component that re-rendered
     * with at least one changed prop or hook, with the changed keys merged
     * across all commits in the buffer. Each changedProps entry is
     * {key, kind} where kind is reference_changed | value_changed |
     * unknown (REQ-BT-019.17); reference_changed wins on conflict. `note`
     * states the comparison is a shallow reference compare so an agent
     * does not read a new-reference-every-render prop as a root cause.
     */
    __getWhyRender: function() {
      var byName = {};
      for (var i = 0; i < __commits.length; i++) {
        var comps = __commits[i].components || [];
        for (var j = 0; j < comps.length; j++) {
          var c = comps[j];
          if (c.phase !== 'update') continue;
          var cp = c.changedProps || [];
          var ch = c.changedHooks || [];
          if (cp.length === 0 && ch.length === 0) continue;
          if (!byName[c.name]) {
            byName[c.name] = { name: c.name, changedProps: {}, changedHooks: {} };
          }
          var k;
          // REQ-BT-019.17: each changedProps entry is {key, kind}.
          // reference_changed wins over value_changed when the same prop
          // appears with both kinds across commits (it is the louder
          // false-positive signal an agent must see).
          for (k = 0; k < cp.length; k++) {
            var e = cp[k];
            var pk = (e && e.key != null) ? e.key : e;
            var pkind = (e && e.kind) ? e.kind : 'unknown';
            var existing = byName[c.name].changedProps[pk];
            if (!existing || existing !== 'reference_changed') {
              byName[c.name].changedProps[pk] = pkind;
            }
          }
          for (k = 0; k < ch.length; k++) byName[c.name].changedHooks[ch[k]] = true;
        }
      }
      var out = [];
      for (var name in byName) {
        var cpMap = byName[name].changedProps;
        var props = [];
        for (var pk2 in cpMap) {
          props.push({ key: pk2, kind: cpMap[pk2] });
        }
        out.push({
          name: name,
          changedProps: props,
          changedHooks: Object.keys(byName[name].changedHooks).map(Number)
        });
      }
      return {
        note: 'shallow reference compare; inline object/array/fn props change reference every render (reference_changed), which is not necessarily a real value change',
        components: out
      };
    },

    /**
     * __perfReset() → 'ok'
     *
     * REQ-BT-019.20: OPEN the page-anchored measurement window. Records
     * t0 = performance.now(), zeroes the longtask accumulators, and
     * clears the __phoenix React commit buffer so the React commit
     * measurement is window-scoped too. The run_scenario harness calls
     * this immediately after the first readiness step satisfies.
     */
    __perfReset: function () {
      __lt_ms = 0;
      __lt_n = 0;
      __win_t0 = (typeof performance !== 'undefined' && performance.now)
        ? performance.now() : Date.now();
      if (window.__phoenix.__resetCommits) window.__phoenix.__resetCommits();
      return 'ok';
    },

    /**
     * __perfRead() → JSON string of the windowed accumulators
     *
     * REQ-BT-019.20: CLOSE the page-anchored window and read the
     * accumulators in one in-page call. Returns a JSON string:
     *   { script_ms, long_tasks, wall_ms, dom_nodes,
     *     react_status, react_commits, react_actual_ms }
     * script_ms/long_tasks are the longtask sum/count since
     * __perfReset; wall_ms is the performance.now() span; React fields
     * mirror __reactStatus()/__getCommits() over the same window.
     */
    __perfRead: function () {
      var t0 = __win_t0;
      var wall = (t0 != null)
        ? (((typeof performance !== 'undefined' && performance.now)
            ? performance.now() : Date.now()) - t0)
        : null;
      var dom = 0;
      try { dom = document.getElementsByTagName('*').length; } catch (e) {}
      var status = 'absent', commits = null, ams = null;
      try {
        if (window.__phoenix.__reactStatus) {
          status = window.__phoenix.__reactStatus();
          var c = window.__phoenix.__getCommits ? window.__phoenix.__getCommits() : [];
          var n = c.length, s = 0;
          for (var i = 0; i < c.length; i++) s += (c[i].totalActualDuration || 0);
          if (status !== 'absent') commits = n;
          if (status === 'measured') ams = s;
        }
      } catch (e) {}
      return JSON.stringify({
        script_ms: __lt_ms,
        long_tasks: __lt_n,
        wall_ms: wall,
        dom_nodes: dom,
        react_status: status,
        react_commits: commits,
        react_actual_ms: ams
      });
    }
  };
})();
";

// ============================================================================
// React detection — called after browser_navigate to enrich the result
// ============================================================================

/// JavaScript snippet that detects React on the current page and returns a
/// summary string for the navigate tool output. Returns empty string if no
/// React detected.
pub const REACT_DETECT_SCRIPT: &str = r#"
(function() {
  var hook = window.__REACT_DEVTOOLS_GLOBAL_HOOK__;
  if (!hook || !hook.renderers || hook.renderers.size === 0) return '';
  var contextCount = 0;
  try {
    if (window.__phoenix && window.__phoenix.listContexts) {
      contextCount = window.__phoenix.listContexts().length;
    }
  } catch(e) {}
  var msg = 'React detected (' + hook.renderers.size + ' renderer(s)';
  if (contextCount > 0) msg += ', ' + contextCount + ' context(s)';
  msg += '). window.__phoenix is available for inspection:\n';
  msg += '  __phoenix.listContexts()              — discover all context providers\n';
  msg += '  __phoenix.getContext([key1, key2])     — find context by duck-typing its keys\n';
  msg += '  __phoenix.callContext([key], method)   — find context and call a method on it\n';
  msg += '  __phoenix.getState("ComponentName")    — get hook state array by component name';
  return msg;
})()
"#;
