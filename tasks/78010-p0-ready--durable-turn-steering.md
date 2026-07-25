# Cut over durable steering ownership and queueing

Depends on acceptance and authoritative repository phases.

Scope:
- bounded atomic enqueue
- ordered durable queue membership and typed batch claims
- cancellation tombstones and materialization
- deletion of mutable runtime steering truth

Acceptance:
- Capacity check plus enqueue is atomic.
- Drain/cancel/restart/concurrent-enqueue matrix passes.
- Runtime does not own a mutable shadow steering queue.

Out of scope: terminal release and UI/SSE projection cleanup.
