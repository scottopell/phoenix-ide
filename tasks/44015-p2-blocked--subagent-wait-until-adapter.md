# Subagent typed `wait_until` adapter

PR 4 in the wake experiment stack. Stack directly on the reviewed TmuxWindow adapter PR. PR #621 remains the unmerged base.

## User journey

A parent agent explicitly registers `wait_until` for one spawned child. The child reaches one typed terminal outcome and the parent receives one durable correlated result without polling or creating a second sub-agent lifecycle authority. Busy parent conversations store the result without a competing turn.

## Concrete scope

- `crates/phoenix-tools/src/subagent.rs`: expose typed child identity and terminal result evidence.
- `crates/phoenix-state-machine/src/transition.rs`: keep the existing sub-agent lifecycle authoritative; wake only observes its terminal fact.
- Extend the existing tagged `wait_until` handle enum with `Subagent`; do not add another tool.
- Reuse foundation delivery, cancellation, recovery, and runtime admission.

## Acceptance

- Prove condition truth, one typed delivery, busy-conversation behavior, cancellation, crash recovery, and safe runtime admission.
- Test durable result delivery separately from automatic continuation.
- No polling and no duplicate child lifecycle state.
- `./dev.py check` passes and the PR receives exact-head review.

## Stop gate

Pause if this requires a second sub-agent lifecycle authority, generic provider framework, broad UI, combinators, or unrelated ownership transfer.

## Out of scope

Multi-child combinators, generic providers, broad public projections/UI, and changes to spawn policy unrelated to observing one terminal child.
