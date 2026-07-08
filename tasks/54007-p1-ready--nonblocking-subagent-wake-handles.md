# Add non-blocking sub-agent wake handles

Design and implement a parent-facing path for non-blocking sub-agent spawning so a parent can receive stable child conversation / agent ids, return to Idle, and explicitly register `wait_until` contracts for those children.

Context: PR #405 keeps existing `spawn_agents` blocking fan-in as compatibility sugar for wake-contract v1. That is acceptable only as a scoped v1 bridge; explicit sub-agent terminal waits need a first-class non-blocking handle surface in a follow-up.

Acceptance sketch:
- Define the LLM-facing shape for non-blocking sub-agent spawn or equivalent child-handle exposure.
- Ensure the parent can become Idle and user-interruptible after spawning.
- Ensure returned child ids are valid inputs for `wait_until` sub-agent terminal waits.
- Preserve compatibility behavior for existing `spawn_agents` users or deliberately specify a migration.
