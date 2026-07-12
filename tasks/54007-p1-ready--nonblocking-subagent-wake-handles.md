# Add non-blocking sub-agent wake handles

Follow-up to `tasks/47002-p1-in-progress--implement-wake-plane-core-bash-tmux.md`.

Design and implement a parent-facing path for non-blocking sub-agent spawning so a parent can receive stable child conversation / agent ids, return to Idle, and explicitly register `wait_until` contracts for those children.

Context: task 47002 deliberately keeps wake-plane core scope to bash + tmux and preserves sub-agent wake integration as follow-up work. Existing `spawn_agents` blocking fan-in remains compatibility sugar in that core slice; explicit sub-agent terminal waits need a first-class non-blocking handle surface here.

Acceptance sketch:
- Define the LLM-facing shape for non-blocking sub-agent spawn or equivalent child-handle exposure.
- Ensure the parent can become Idle and user-interruptible after spawning.
- Ensure returned child ids are valid inputs for `wait_until` sub-agent terminal waits.
- Preserve compatibility behavior for existing `spawn_agents` users or deliberately specify a migration.

## Authority notes

- Delivery-protocol semantics for `wait_until` are governed by ADR-008, not the older delayed-tool-result framing.
- This task should integrate with the durable wake observation / continuation-transfer model established for the bash/tmux core.
- Do not reopen whether sub-agent wake integration belongs in task 47002; that boundary is already settled.
