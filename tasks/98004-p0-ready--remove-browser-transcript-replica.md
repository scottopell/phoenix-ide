# Remove browser transcript and replica persistence

## User journey
A blocked, stale, corrupt, or absent legacy IndexedDB transcript cannot delay or influence online conversation loading. SSE owns the latest tail; older history remains lazy and server-owned.

## Scope
- Remove all IDB transcript message and replicaMeta reads/writes.
- Remove warm cache catch-up loops, cache generation reconciliation, cache write effects/refs/helpers, dead APIs/types/tests.
- Keep in-memory atom history, server transcript generation, REST older-history generation/cursor guards, and SSE reconnect semantics.
- Leave physical legacy IDB stores untouched to avoid mixed-tab upgrade and rollback risk.
- Ensure legacy storage initialization cannot gate route/network/SSE readiness.

## Acceptance
- Blocking IDB indefinitely does not delay route resolution, first network request, SSE construction, or ready.
- Legacy cached transcript rows never render.
- Latest tail and upward-scroll/deep-link history work from server sources.
- Reload intentionally forgets previously expanded history.
- Existing pending operations remain intact pending the journal migration.
