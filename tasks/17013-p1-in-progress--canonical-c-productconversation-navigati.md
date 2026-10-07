# /c canonical ProductConversation navigation with exact-evidence compatibility

## Queue scope and retained owner

Implementation explicitly approved by the user through Global after the read-only route assessment. This is part of the current ProductConversation milestone, not federation implementation. The same retained routing/provenance owner implements it alongside the separately admitted mobile Actions-menu change in bounded commits/PRs; native and federation work continue without a blanket coordination hold. Merge/deployment remain Global-owned.

Retained routing/provenance owner: ProductConversation `58cff20d-81c3-4655-b9d8-61fe615bda28`; writable transcript verified at queue admission: `b9cf8836-32bf-40b5-b092-1da2ba52fb24`. Use the stable owner reference and re-resolve before execution; do not create a duplicate writer.

User decision evidence: `@transcript:0e1f9e79-827a-4875-99d1-d83d47b28e90#message-0e1f9e79-827a-4875-99d1-d83d47b28e90:a37304c5-c004-4e25-88a3-c7f54f201d54` (2026-10-06T20:17:45Z). Federation response `164fcc7e-620c-466a-966c-f703f67636af` confirms destination-generated routes rather than hardcoded route spelling or a competing migration.

## First action / decision boundary

Read-only inventory of routes and link producers on actual current main (queue-time main `41a5bad0b9952c6423e11fd4c467e2923057e7ef`). Map server `canonical_route`, UI navigation/copy links, resolve/search citations, legacy alias resolution, Coordinator routes, and macOS `phoenix://conversation/<uuid>` handoff.

Before code edits, define explicit stable-current versus legacy/exact resolution against the existing authoritative ProductConversation/transcript binding. Do not assume UUID appearance, transcript root equality, or blind string replacement determines intent. If an old exact route and proposed stable route are genuinely ambiguous, return a small route-shape proposal for user decision; do not silently repin historical evidence.

## Read-only owner assessment and decision gate

Retained owner acknowledged queue-only ownership and inspected exact main `41a5bad0b9952c6423e11fd4c467e2923057e7ef`, not its older checkout HEAD. Inventory: backend `product_conversations.rs` canonical/list routes, creation handlers, `global_read.rs` search/read/resolve, `App.tsx` Global-first alias routing, `DesktopLayout` aggregate-versus-row store selection, sidebar/list/palette/create consumers, Coordinator/message link producers, and macOS AppDelegate handoff.

The authoritative ordinary resolver matches product identity then transcript identity then slug, with domain/membership checks; UUID shape is not authority. Browser `/c` aliases already resolve ordinary aggregates, while `global_read::resolve_reference_impl` interprets `/c` as transcript. These semantics need deliberate alignment.

**User-approved route distinction:** bare stable ProductConversation links follow the latest continuation; explicit transcript/message references remain historically pinned. Apply this distinction consistently to browser/tool resolution and server-produced canonical links, preserving legacy link compatibility, message anchors, query/fragments and reload/back/copy behavior. `/global` remains separate and desktop `phoenix://` handoff must keep correct ownership; no historical DB rewrite or new URI grammar.

An unqualified root URL that is also the authoritative ProductConversation identity takes the stable interpretation. Do not infer exact intent from UUID shape or silently substitute latest for explicit transcript/message/source-tool evidence. The original ambiguity is adjudicated; no repeated user decision is needed for this same boundary.

## Finish line / acceptance

- [ ] Stable ProductConversation `/c` navigation follows its current continuation.
- [ ] Exact transcript/message evidence remains historically pinned, including source-transcript/tool query data and message fragments.
- [ ] Existing `/product-conversations/...` and `/c/<slug-or-transcript>` links keep working or redirect compatibly without losing query/fragment or changing exact-vs-current intent.
- [ ] Direct load, reload, back/history, copy-link, and continuation transitions have focused regression coverage.
- [ ] Server canonical route and UI/resolve/search link producers agree on typed target meaning.
- [ ] Existing Global/Coordinator routing and desktop `phoenix://` handoff are not misrouted.
- [ ] Actual rendered stable-current and historical-evidence journeys verify the accepted route distinction, not DOM-only assertions.

## Explicit exclusions

No federation implementation or design steering; no new URI grammar, database/historical rewrite, universal resolver, redirect service, broad navigation redesign or host operations. Focused implementation and validation are approved; publication follows normal qualification and final merge/deployment remains Global-owned. Federation consumes destination-authoritative routes. Existing completed provenance/UI acceptance is not reopened wholesale.

## Tracking

Roadmap #806 records the explicit implementation decision and same-owner admission. First implementation gate: inspect actual current main and authoritative route bindings, then implement focused stable-versus-pinned and legacy/Global/desktop regressions. Primary heavy builds remain constrained; use hosted/coordinated remote validation. Important-message marking is explicitly deferred and outside this task.
