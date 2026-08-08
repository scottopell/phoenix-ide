# TmuxWindow restart-surviving `wait_until` adapter

PR 3 in the wake experiment stack. Stack directly on the reviewed Bash adapter PR. PR #621 remains the unmerged base of the stack.

## User journey

An agent starts a Phoenix-managed tmux window, explicitly registers `wait_until` for that window, Phoenix restarts, and the surviving window completion produces one durable typed result. Delivery while the conversation is busy does not create a competing turn.

## Concrete scope

- `crates/phoenix-tools/src/tmux/run.rs` and `crates/phoenix-tools/src/tmux/registry.rs`: expose durable server-token/window identity and completion-marker evidence without implicit enrollment.
- Extend the existing tagged `wait_until` handle enum with `TmuxWindow`; do not add another tool.
- Reuse the foundation lifecycle, delivery, cancellation, and runtime-admission paths.

## Acceptance

- Prove condition truth, one delivery, busy-conversation behavior, cancellation, Phoenix restart with surviving completion, and safe runtime admission.
- Test durable result delivery separately from automatic continuation.
- Ordinary `tmux_run` never creates a wake implicitly.
- `./dev.py check` passes and the PR receives exact-head review.

## Stop gate

Pause if tmux requires a second lifecycle authority, a generic handle-provider framework, broad UI, combinators, or ownership transfer unrelated to this journey. Do not start Subagent until this adapter is understandable and green.

## Out of scope

Sub-agent waits, broad public projections/UI, generic providers, arbitrary tmux sessions, combinators, and unrelated ownership transfer.
