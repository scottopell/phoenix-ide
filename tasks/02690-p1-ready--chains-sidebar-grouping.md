---
created: 2026-04-29
priority: p1
status: ready
artifact: ui/src/components/Sidebar.tsx
---

Update ui/src/components/Sidebar.tsx to group conversations into chain blocks. Algorithm: query annotates each conversation with chain_root_conv_id (null if standalone or single-member). UI extracts conversations sharing a root into a single collapsible block, positions block at recency of most-recent member, lists members in chain order (root to latest) inside. Block header shows chain_name (falls back to root.title). Standalone conversations remain interleaved by recency between blocks. Block defaults to expanded; expand/collapse state not persisted across navigations. Tests: grouping logic on mixed lists, chain order independent of updated_at.
