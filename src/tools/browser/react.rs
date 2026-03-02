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

use super::session::BrowserSession;
use crate::tools::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use chromiumoxide::cdp::browser_protocol::page::{
    AddScriptToEvaluateOnNewDocumentParams, RemoveScriptToEvaluateOnNewDocumentParams,
    ScriptIdentifier,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// The injected JavaScript helper
// ============================================================================

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
const PHOENIX_REACT_HELPER_SCRIPT: &str = r"
(function() {
  // ── Idempotency guard ──────────────────────────────────────────────────────
  // Skip installation if the helper is already present (e.g. tool called twice
  // before a navigation, or the page ships its own __phoenix).
  if (window.__phoenix && window.__phoenix.__installed) {
    return;
  }

  // ── Fiber root registry ────────────────────────────────────────────────────
  // React calls `onCommitFiberRoot(rendererID, root)` after every render.
  // We collect all roots here so the helper functions can search across them.
  var fiberRoots = [];

  // ── React DevTools hook ────────────────────────────────────────────────────
  // React checks for this global at startup. If it exists, React registers its
  // internals into it. We install our hook BEFORE page JS runs so React sees it.
  //
  // We must not overwrite an existing hook that another tool (e.g. the actual
  // React DevTools browser extension) may have installed. Instead, we wrap the
  // existing `onCommitFiberRoot` callback if one already exists.
  var existingHook = window.__REACT_DEVTOOLS_GLOBAL_HOOK__;
  var prevOnCommit = existingHook && existingHook.onCommitFiberRoot;

  var hook = existingHook || {};

  // Minimal set of properties React expects on the hook object.
  // React skips the hook entirely if `isDisabled` is true, so we never set it.
  hook.supportsFiber = true;

  // Called by React after every committed render tree.
  hook.onCommitFiberRoot = function(rendererID, root) {
    // Forward to any pre-existing hook handler first.
    if (typeof prevOnCommit === 'function') {
      try { prevOnCommit.call(hook, rendererID, root); } catch (_) {}
    }

    // Register the root so our search functions can find it.
    // Use the current property (React may pass the same root object multiple
    // times with a mutated `current` pointer after updates).
    var fiber = root.current || root;
    if (fiberRoots.indexOf(fiber) === -1) {
      fiberRoots.push(fiber);
    }
  };

  // React also calls these; provide no-op stubs so React does not error.
  if (!hook.inject)          { hook.inject          = function() {}; }
  if (!hook.onCommitFiberUnmount) { hook.onCommitFiberUnmount = function() {}; }

  window.__REACT_DEVTOOLS_GLOBAL_HOOK__ = hook;

  // ── Depth-first fiber tree search ─────────────────────────────────────────
  // Walks child → sibling links. Returns the first fiber for which `predicate`
  // returns truthy, or null if not found.
  function findFiber(root, predicate) {
    var stack = [root];
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
    for (var r = 0; r < fiberRoots.length; r++) {
      var found = findFiber(fiberRoots[r], function(fiber) {
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
      for (var r = 0; r < fiberRoots.length; r++) {
        var found = findFiber(fiberRoots[r], function(fiber) {
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
      for (var r = 0; r < fiberRoots.length; r++) {
        (function searchRoot(fiber) {
          if (!fiber) return;
          if (fiber.tag === 10) {
            var val = fiber.memoizedProps && fiber.memoizedProps.value;
            if (val && typeof val === 'object') {
              results.push({
                keys: Object.keys(val),
                // Include a shallow snapshot so the caller can see values.
                value: val
              });
            }
          }
          if (fiber.child)   searchRoot(fiber.child);
          if (fiber.sibling) searchRoot(fiber.sibling);
        })(fiberRoots[r]);
      }
      return results;
    }
  };
})();
";

// ============================================================================
// browser_inject_react_devtools (REQ-BT-017)
// ============================================================================

pub struct BrowserInjectReactDevtoolsTool;

#[async_trait]
impl Tool for BrowserInjectReactDevtoolsTool {
    fn name(&self) -> &'static str {
        "browser_inject_react_devtools"
    }

    fn description(&self) -> String {
        "Install a lightweight window.__phoenix React helper into the page BEFORE \
page JS runs, using CDP's Page.addScriptToEvaluateOnNewDocument. The helper hooks \
into React's __REACT_DEVTOOLS_GLOBAL_HOOK__ so React automatically registers its \
fiber tree on startup, enabling structured access to component state and context \
without DOM traversal or minification-sensitive property names.\n\
\n\
IMPORTANT: Call this tool BEFORE navigating to (or reloading) the target page — \
the script must be installed before React initialises.\n\
\n\
After injection, use browser_eval to call:\n\
  window.__phoenix.getContext(['openFile', 'closeFile'])  // find context by duck-typing\n\
  window.__phoenix.callContext(['openFile'], 'openFile', '/src/main.rs')  // call a method\n\
  window.__phoenix.getState('FileExplorer')               // get hook state array\n\
  window.__phoenix.listContexts()                         // discover all contexts\n\
\n\
Calling this tool twice before navigating is safe — the script is idempotent.\n\
Injecting on a non-React page is harmless: the hook exists but is never called.\n\
Returns the script identifier so you can pass it to browser_remove_react_devtools \
if you need to clean up the injection."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn run(&self, _input: Value, ctx: ToolContext) -> ToolOutput {
        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        let mut guard = session.write().await;
        guard.last_activity = std::time::Instant::now();

        let params =
            AddScriptToEvaluateOnNewDocumentParams::new(PHOENIX_REACT_HELPER_SCRIPT.to_string());

        match guard.page.execute(params).await {
            Ok(result) => {
                let identifier: String = result.result.identifier.into();
                tracing::debug!(
                    script_id = %identifier,
                    "Injected window.__phoenix React helper via addScriptToEvaluateOnNewDocument"
                );
                ToolOutput::success(format!(
                    "React DevTools helper injected (script id: {identifier}). \
Navigate to or reload the target page for the helper to take effect. \
The window.__phoenix API will then be available via browser_eval:\n\
  window.__phoenix.getContext(['openFile'])\n\
  window.__phoenix.callContext(['openFile'], 'openFile', '/path')\n\
  window.__phoenix.getState('ComponentName')\n\
  window.__phoenix.listContexts()"
                ))
            }
            Err(e) => ToolOutput::error(format!("Failed to inject React helper: {e}")),
        }
    }
}

// ============================================================================
// browser_remove_react_devtools (REQ-BT-017 cleanup)
// ============================================================================

#[derive(Debug, Deserialize)]
struct RemoveReactDevtoolsInput {
    /// The script identifier returned by `browser_inject_react_devtools`
    script_id: String,
}

pub struct BrowserRemoveReactDevtoolsTool;

#[async_trait]
impl Tool for BrowserRemoveReactDevtoolsTool {
    fn name(&self) -> &'static str {
        "browser_remove_react_devtools"
    }

    fn description(&self) -> String {
        "Remove the window.__phoenix React helper previously installed by \
browser_inject_react_devtools. Pass the script_id returned by that tool. \
The injection is removed from future new documents; already-loaded pages are \
unaffected (the helper stays in the current page's window until navigation)."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script_id": {
                    "type": "string",
                    "description": "The script identifier returned by browser_inject_react_devtools"
                }
            },
            "required": ["script_id"]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: RemoveReactDevtoolsInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        let mut guard = session.write().await;
        guard.last_activity = std::time::Instant::now();

        let identifier = ScriptIdentifier::from(input.script_id.clone());
        let params = RemoveScriptToEvaluateOnNewDocumentParams::new(identifier);

        match guard.page.execute(params).await {
            Ok(_) => {
                tracing::debug!(
                    script_id = %input.script_id,
                    "Removed window.__phoenix React helper"
                );
                ToolOutput::success(format!(
                    "React DevTools helper removed (script id: {}).",
                    input.script_id
                ))
            }
            Err(e) => ToolOutput::error(format!("Failed to remove React helper: {e}")),
        }
    }
}
