# Add a minimal display-only sidebar startup snapshot

## User journey
A returning user sees provisional conversation labels immediately instead of a blank sidebar, then the authoritative server list replaces them.

## Scope
- Store one bounded, versioned localStorage envelope of display-only rows.
- Use an impoverished type with identity/display/order/group hints only; exclude phase, PR state, transcript state, action eligibility, and full Conversation objects.
- Hydrate provisionally without delaying active/archived list requests.
- Replace the snapshot wholesale after authoritative list success.
- Ignore corruption, unsupported versions, quota, and disabled storage.
- Snapshot route mappings are hints only and never select stream authority.

## Acceptance
- Snapshot renders synchronously before delayed network list response.
- Authoritative refresh replaces renamed, archived, added, and deleted rows.
- Storage failure cannot affect app readiness.
- Snapshot stays under explicit row/byte limits and contains no volatile operational state.

## Dependency
Blocked on task 92013's unified conversation presentation. Snapshot identity, ordering, and Open/History grouping must follow that product model rather than preserve the legacy active/archived sidebar shape. After 92013 lands, this task must land before task 98004 retires the legacy conversation-metadata sidebar cache.
