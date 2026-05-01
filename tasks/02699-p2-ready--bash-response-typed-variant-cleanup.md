---
created: 2026-05-01
priority: p2
status: ready
artifact: src/api/wire.rs
---

Collapse the BashResponse tagged-enum field-presence scrubs by splitting kill_pending_kernel-on-passive-wait into a dedicated wire variant.

The 02697 wire-type migration kept byte-for-byte compatibility with 02694's hand-built JSON by post-serialization scrubbing of `display: ""` and `signal_sent: ""` placeholders on the kill-pending-kernel-on-passive-wait branch. This is cosmetic only — the wire bytes are clean — but the typed enum has implicit-default fields that get scrubbed away at the boundary.

Cleanup direction: introduce a dedicated `BashKillPendingKernelOnPassiveWait` (or similar) variant whose Rust-side fields exactly match the wire shape, so no post-serialization mutation is needed. This requires a small wire-shape evolution.

See agent commentary in the 02697 closeout for the exact serialization sites involved (operations.rs `shape_handle_response`, `still_running_response`, `terminal_or_panic_response`).

Out of scope of 02697; YAGNI until someone touches that branch again.
