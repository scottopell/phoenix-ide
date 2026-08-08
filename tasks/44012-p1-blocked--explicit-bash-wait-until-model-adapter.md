# Explicit Bash `wait_until` adapter

PR 2 in the wake experiment stack. Stack directly on PR #621; do not wait for the foundation to merge, and do not merge this adapter independently of the reviewed stack decision.

## User journey

An agent starts a long Bash command, explicitly registers one tagged `wait_until` for that handle, returns the conversation to Idle, and later receives one durable typed terminal result without polling. A busy conversation stores the result without starting a competing turn. Automatic continuation is a separate acceptance path and proof.

## Concrete scope

- `crates/phoenix-tools/src/bash.rs` and `crates/phoenix-tools/src/bash/`: expose Bash handle identity and terminal evidence without implicit enrollment.
- `crates/phoenix-tools/src/lib.rs`: register one narrow tagged `wait_until` surface with only the Bash variant enabled in this PR.
- `crates/phoenix-ide/src/runtime/executor.rs`: mint registration and observation capabilities at the existing checked tool/runtime boundaries.
- `crates/phoenix-ide/src/runtime/`: admit owed durable results only when the conversation can safely accept work.
- `crates/phoenix-db/src/workflow/wake_contract.rs`: use the foundation repository; do not add another lifecycle authority.

## Acceptance

- Explicit registration only; ordinary Bash run/wait/peek paths never create a wake.
- Prove condition truth, exactly one durable result delivery, busy-conversation behavior, cancellation, typed Phoenix-restart loss, and safe runtime admission.
- Test durable result delivery independently from automatic continuation.
- One end-to-end Bash journey passes through production capability minting and runtime admission.
- `./dev.py check` passes and the PR receives exact-head review.

## Stop gate

**Triggered during implementation investigation.** The authority foundation has no production observation/delivery/admission executor. Current production execution still uses the legacy `workflow::wake::WakeRepository`, `wake_profile`, and `runtime::wake::WakeWorker`. A correct Bash adapter would therefore require a coordinated replacement across `phoenix-tools`, `phoenix-db`, runtime execution, and conversation admission before one journey can run. Building that generic runtime here would violate this task's complexity budget and create the second-authority risk the foundation forbids.

No adapter code was added. Resume only with a separately reviewed, bounded plan that shows how the existing durable-workflow executor can run the foundation effects without introducing a new wake worker or generic provider framework.

Pause this adapter if it requires a second wake authority, a generic provider framework, broad UI, combinators, or unrelated ownership-transfer work. Do not start TmuxWindow until this PR is understandable and green.

## Out of scope

TmuxWindow, sub-agent waits, broad public projections/UI, multi-condition combinators, generic providers, and ownership transfer unrelated to the Bash journey.
