# Standalone Rust deployment controller and thin Python delegation

## Explicit user commission

“Get this going as priority, current deploy system is brittle.” Implementation and qualification of an independent controller, not docs-only. No production replacement, merge, deployment, fourth18b attempt or reservation waiver is authorized.

## Scope and acceptance

- Standalone Rust controller outside running Phoenix; durable typed attempt/phase/recovery state and meaningful secret-safe diagnostics.
- Allium first/in lockstep with code: exact state transitions, effect ownership, crash reconciliation, migration-invoked forward recovery, no accidental downgrade guarantees. Read compatibility/launchd specs and ADRs before changing contracts; resolve real ambiguities rather than prose open questions.
- Resource admission from measured sustained contention and capacity, not process-name blacklist or machine-wide metadata invariance. Separate stable process identity, verified ownership, and volatile telemetry types.
- Verified owned-tree cleanup handles child exit, PID reuse, reparenting, interruption and permission uncertainty without blanket ESRCH/no-FD waivers. One absolute operation deadline and accountable descendant closure.
- Fake OS deterministic regressions and real harmless macOS lifecycle tests with isolated labels/processes/paths; no production launchd/database/cache mutation.
- Thin dev.py delegates to one Rust policy authority. No competing Python/Rust policy copies. Stage adoption with explicit opt-in/integration and proof before replacing existing deployed behavior.
- Focused PR, exact review/tests, adoption plan and honest remaining coverage. No unsupported recovery/compatibility expansion.

## Ownership and immediate slice

Dedicated isolated source repository, base d69cde67597605dffdb5739647220820cc8f3aa8. Existing installer owner7ad380c9/current1acc998c owns conversation-creation repair API/UI; sole host5d57fcc6 owns current guard/18b attempt3 reconciliation. Initial scope NEW controller crate/spec/fakeOS tests only; serialize dev.py/helper changes with installer/host before touching them. No edits to their worktrees/guards/evidence.

First concrete deliverable: normative Allium phase model plus matching Rust typed transitions/OS boundary and deterministic fake OS regression for the observed transient-child identity race, not a prose-only plan. Commit bounded source units; register workspace narrowly. Heavy compiler/local lifecycle test resources require current host slot; no inherited resource clearance. Production search recovery is separate and must not wait for whole rewrite.

## Incident evidence

Third authorized18b: root5161, nextest5188, newly observed child5189/PPID5188/PGID5188 yielded ESRCH before stable birth identity; guard stopped before activation, reservation retained because cleanup absence unproven. Prior false-positive RSS/stat baseline mismatch was separately corrected. Do not encode either broad weakening or perpetual-stop-on-any-transient-exit; model acquired evidence and actual ownership uncertainty explicitly.

## Staged delivery

1. Allium + pure Rust phase/identity/ownership model and fake OS tests.
2. Durable attempt storage/reconciliation and observed resource admission, typed command protocol.
3. Harmless macOS process/launchd integration tests, independently reviewed boundaries.
4. Thin dev.py integration and staged adoption PR, no activation in this commission.
