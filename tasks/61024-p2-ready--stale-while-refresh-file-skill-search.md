Remove flicker and duplicate requests from file and skill discovery surfaces.

Retain previous file-search results while a debounced request is pending, share identical in-flight searches by resolution root and query, and share skill discovery requests/cache by resolution root across composer/reference consumers. Preserve stale-result suppression when roots change and do not cache failures permanently. Add focused UI tests for in-flight deduplication, root changes, stale-while-refresh presentation, errors, and cache invalidation.

No backend architecture, durable workflow, or endpoint contract changes. Prefer a small client-side cache with explicit bounds/TTL over a new global state system.
