When IndexedDB is blocked by another tab holding the DB open at a different version, `cacheDB.init()` hangs forever silently — no `onblocked` handler, no timeout. This deadlocks both `useAppMachine.init()` (blocks `isReady`) AND `useConversationsRefresh.refreshOnce()` (blocks cache hydration before the network fetch fires). User-visible symptom: empty sidebar, "No conversations yet", no `/api/conversations` request ever fires. Incognito works because it has no other tabs/state.

Repro: open Phoenix in two tabs, force a schema version bump in one (or leave one hung after a crash), open another → silent hang.

Fix:
1. Add `onblocked` handler in `openDB` that logs at warn so operators see "another tab is holding the DB open".
2. Race the `indexedDB.open` request against a 5s timeout. On timeout, reject so callers fall through.
3. Reset `this.initPromise = null` on failure in `init()` so a future tab close can retry instead of staying permanently failed.

Belt-and-suspenders: `refreshOnce` already wraps the cache call in try/catch and falls through to network, so the timeout reject is enough to unblock the sidebar. `useAppMachine` may want similar fall-through behavior — it currently awaits `cacheDB.init()` directly and could swallow the error to advance to ready anyway.

Discovered 2026-05-11 during F+F polish: 30-minute idle in normal browser eventually fixed it after the other tab died, confirming the block-on-other-tab diagnosis.
