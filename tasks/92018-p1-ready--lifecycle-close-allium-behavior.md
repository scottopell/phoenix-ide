Child of task 92010.

Concern:
- Define the lifecycle and Close behavior precisely in Allium, including state transitions, preconditions, postconditions, and invariants that the requirements baseline leaves abstract.

Settled terminology guardrails:
- Reuse the settled lifecycle terms from the baseline task without aliasing them.
- Use “Close” for the operation and “Closed” only for the resulting lifecycle condition if the spec requires that distinction.
- Use “legacy mapping” or “legacy representation” for historical shapes rather than treating them as live states.
- Keep lifecycle authority singular: Allium describes behavior of the authoritative model; it does not create a second competing terminology set.

Done evidence:
- The affected Allium spec validates with allium check.
- Close-related states, transitions, invariants, and evidence rules are explicit and unambiguous.
- Any required cross-spec imports or helper declarations are present and valid.
- Notes identify how the Allium behavior traces back to the settled requirements terminology instead of introducing drift.
