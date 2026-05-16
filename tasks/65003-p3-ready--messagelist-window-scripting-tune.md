Reduce the scroll-compensation / window-recompute scripting introduced by
the bottom-anchored MessageList virtualization (db record
MessageList-bottom-anchored-window).

Accepted optimization cut react_commit_ms -46.6% and js_heap -21%, but
script_ms regressed +12% (384->430, p=1.4e-6) and long_tasks went 2->4 on
the conversation-load scenario — the per-render derived window + scroll
listener + layout-effect scrollTop compensation add main-thread scripting,
so end-to-end wall only improved -10.6% instead of tracking the -46% commit
drop. Tune: throttle/raf-coalesce the scroll listener, memoize the window
derivation, avoid redundant compensation when delta==0 (already partly),
consider passive batching. Re-measure via the conversation-load scenario
through the phoenix-perf suite; goal: long_tasks back to <=2 and script_ms
not regressed, without losing the react_commit_ms/heap wins.
