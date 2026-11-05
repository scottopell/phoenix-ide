# Fix native Goal preparation for in-flight Coordinator tool calls

Native Coordinator prepare_goal must admit a live server-authenticated ToolUse before the assistant message reaches the later checkpoint, while retaining current Coordinator authority, live source-turn ownership, stable target/current-transcript checks, expected-version conflicts, immutable preparation receipts, and idempotent apply. Add the combined continued-Coordinator plus continued-ordinary-owner public-path regression and keep all negative fences.
