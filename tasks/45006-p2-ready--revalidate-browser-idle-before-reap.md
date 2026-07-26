# Revalidate browser idleness at kill acquisition

`BrowserSessionManager::cleanup_idle_sessions` identifies idle candidates and evaluates scope liveness from a snapshot, then later requests teardown by key. Browser activity or scope liveness can change between candidate selection and `kill_requested` acquisition, so the reaper can act on stale age/liveness evidence.

Investigate a structural revalidation at the kill boundary that preserves the existing fail-closed DB policy and does not block unrelated scopes. Add a deterministic concurrency regression using lifecycle signals rather than sleeps.
