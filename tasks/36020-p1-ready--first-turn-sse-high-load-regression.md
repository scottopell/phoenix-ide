# Fix recurring first-turn SSE timeout under build contention

After PR #498 replaced the keepalive timing race with concurrent creation/stream attachment, `./dev.py check` still reproduced `text_streaming` timing out after 45 seconds under build contention on 2026-07-16. The server persisted no actionable error and the harness reported `first-turn SSE did not reach terminal in 45s`; all other check lanes passed. The run had cargo build lock contention and took 106 seconds to build before the scenario.

Reproduce under concurrent cargo/clippy/test load, capture the exact creation POST, stream attach/retry, init payload, terminal events, and persisted message identity. Determine whether the remaining gap is stream attachment during shell visibility, event loss, or Python scheduling. Replace the race with an explicit server acknowledgement or replay-safe identity-bound witness; do not increase the timeout or restore a mock stall.
