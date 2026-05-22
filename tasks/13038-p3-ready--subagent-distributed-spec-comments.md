`tools/subagent.rs` carries three comments describing executor /
state-machine behaviour from inside the tool implementation. None
contradict the code today, but all match the "Move to spec, then
delete" example in AGENTS.md: a design fact about a remote module that
will silently become wrong if the remote module changes.

## Verified locations (after the section-divider deletions in commit 1ad67c4)

- `crates/phoenix-ide/src/tools/subagent.rs:47-49`
  ```rust
  // The actual state transition is handled by the transition function,
  // not here. This tool just validates and returns the result.
  // The executor will detect this is submit_result and handle specially.
  ```
- `crates/phoenix-ide/src/tools/subagent.rs:91`
  ```rust
  // Same as submit_result - actual transition handled by state machine
  ```
- `crates/phoenix-ide/src/tools/subagent.rs:181-183`
  ```rust
  // The actual spawning is handled by the executor when it receives
  // the SpawnAgentsComplete event. Here we just validate and return
  // a description of what will be spawned.
  ```

## Why this matters

AGENTS.md "Comments are local facts" test: "If this comment becomes
false, will anything fail?" Answer here: no -- which is precisely
*why* the comment is dangerous. If `submit_result` interception moves
from the transition fn to the executor (or vice versa), the comment
becomes wrong and the next reader trusts it instead of grepping.

The substrate already exists for the correct disposition: the
`specs/subagents/` spec set (`requirements.md`, `design.md`,
`subagents.allium`) is the right home for the submit_result /
spawn_agents lifecycle prose. AGENTS.md: "When an Allium spec exists
for a module, the spec is the authoritative source for design
rationale, invariants, and operation sequences."

## Fix direction

For each comment block:

1. Verify the claim against `specs/subagents/design.md` and
   `specs/subagents/subagents.allium`. If the spec already captures
   the rationale, delete the comment outright.
2. If the spec doesn't capture it, add a brief @guidance block or
   design-md paragraph that does, then delete the comment.

Filed as p3 rather than handled in the audit PR because resolution
requires reading the subagents spec and judging whether each claim is
already covered there -- not a mechanical edit.

## Related
- 13010 (distill-subagents-allium-spec, done -- created the spec
  surface this content should live on)
