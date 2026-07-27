# Reuse Explore-sandboxed Bash for Coordinator

Keep Coordinator repository investigation on the existing fail-closed Explore `nono` sandbox while making each Bash process solely owned and stored by its selected WorkScope. Coordinator selects an active `work_scope_id`; Phoenix resolves its canonical execution directory and builds a validated spawn request without mutating `ToolContext`.

Use one globally unique opaque handle ID across tool operations, lifecycle events, WorkScope inventory and teardown, process inspection, health attribution, UI links, logs, and wake targets. Coordinator continuation control is authorization metadata rather than a storage namespace. WorkScope teardown must fence new spawns before removing and terminating its handles.

Preserve ephemeral restart semantics and keep lifecycle events narrow. Do not introduce an actor platform, durable Bash ledger, event sourcing, broad command framework, or new specification family.
