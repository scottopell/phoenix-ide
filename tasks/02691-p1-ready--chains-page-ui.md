---
created: 2026-04-29
priority: p1
status: ready
artifact: ui/src/pages/ChainPage.tsx
---

Create ui/src/pages/ChainPage.tsx for route /chains/:rootConvId. Wire route in App.tsx. Two-column layout. Left: member cards in chain order (root, continuation, latest position labels) with latest member visually emphasized; clicking navigates to that conversation detail page. Right: Q&A panel with bottom-anchored input, chronological history above (most recent immediately above input), streamed answers via SSE subscription with skeleton pre-token and incremental render once tokens arrive. Each Q&A entry as self-contained card (no chat ligatures), with inline staleness tag computed from snapshot_member_count and snapshot_total_messages vs current. Status-specific rendering for in_flight/completed/failed/abandoned. Page header shows chain_name as click-to-edit inline (Enter/blur commits via PATCH; Esc cancels). Browser-test the page end-to-end before declaring done.
