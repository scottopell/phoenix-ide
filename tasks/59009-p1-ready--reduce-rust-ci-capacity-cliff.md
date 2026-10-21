# Reduce Rust CI capacity cliff without weakening coverage

PR #902 exact-head CI demonstrates that the 5,439-test Rust suite is now close to the existing 1,500-second test-subprocess bound. Run 39520418629 attempt 2 emitted 5,096 passes, zero ordinary FAIL records, then SIGTERM after 1,500 seconds; three interrupted tests had run for only 0.3–7 seconds and 340 tests were not started. The byte-identical Rust parent passed run 39515495117, which classifies this as aggregate suite cost plus runner variance rather than a blocked federation fixture.

Evidence from the retained artifact shows broad phoenix-db stretches at roughly 1.7–2.4 seconds per test, consistent with repeated full current-schema/migration fixture setup. Diagnose and reduce aggregate wall time or introduce an existing-authority incremental qualification composition while preserving every validation contract.

Constraints:
- Do not merely increase timeout, workers, test threads, or CI machine size.
- Do not remove coverage, weaken assertions, or skip required lanes.
- Measure compile, codegen, nextest setup, and execution separately.
- Start with repeated phoenix-db current-schema initialization/migration cost and prove any fixture reuse remains isolated and correct.
- Preserve a full-suite authority boundary; parent+exact-delta evidence is acceptable only when ancestry and byte-identical unchanged surfaces are explicit.

Acceptance:
- Rust CI completes reliably inside the existing bound across repeated hosted samples, or the authorized incremental gate composes a fully qualified parent with all exact-delta lanes without omitted coverage.
- Retained artifacts still include final failure bodies and bounded diagnostic evidence.

## Diagnosed cause

Retained attempt-2 evidence from PR #902 run 39520418629 shows a suite-capacity cliff rather than one blocking test:

- 5,439 total statuses; 5,096 passed; zero ordinary `FAIL` records.
- The 1,500-second test subprocess interrupted three newly running tests after only 0.3–7 seconds and left 340 unstarted.
- In the retained tail, 302 visible `phoenix-db` tests consumed 724.679 worker-seconds; median duration was 2.2395 seconds. The common floor corresponds to each ordinary fixture replaying the base schema plus the full numbered migration registry.
- The byte-identical Rust tree passed run 39515495117. Later head 04fc17d3 changes exactly five UI files and no Rust/Cargo/toolchain/nextest/workflow/dev.py input.

Do not optimize the three interrupted tests: they were the scheduling frontier, not long-running blockers.

## Minimal release-unblocking remedy (recommended)

Split the qualified source boundary from its five-file UI delta; do not change CI policy.

1. Preserve `04fc17d33d68b1aa12aa78187e9aea23c2ce53da` on a new child branch.
2. With explicit PR-owner approval, move PR #902's branch to its fully qualified parent `6f390fe0cb613d54adf6e5581de7ac37129a5b44` using `--force-with-lease`.
3. Merge PR #902 only through the normal protected process; no override.
4. Open the child PR from `04fc17d3` after the parent lands (or as a stacked PR whose base is the parent branch).
5. The repository's existing planner then sees exactly five UI files and selects `tsc,ui-lint,vitest,ast-grep,spec-anchors,pkglock,task`, structurally disabling unchanged Rust/Clippy/E2E.

Exact delta:

- `ui/src/components/ConversationEventCard.tsx`
- `ui/src/components/ConversationEventCard.test.tsx`
- `ui/src/components/SettingsDropdown.tsx`
- `ui/src/components/SettingsDropdown.test.tsx`
- `ui/src/components/settingsEvents.ts`

Coverage accounting:

- Parent: run 39515495117 passed full Rust, Clippy, UI/specs, E2E, task, and finalization lanes.
- Child: exact parent-baseline `./dev.py check` passed all 16 selected checks in 45.2 seconds, including TypeScript, ESLint, Stylelint, full Vitest, task validation, structural/SQL/timing checks, spec anchors, and lock cleanliness.
- The ordinary child PR additionally runs the existing finalization geometry and platform workflows selected by current policy.
- No test is removed, skipped outside the repository's existing path authority, or represented by a different source tree.

Expected cost:

- One protected parent merge plus one small UI PR.
- Child core validation is approximately 45 seconds locally plus normal hosted queue/setup and UI geometry; it avoids the repeated 25-minute unchanged Rust subprocess.
- Review cost is bounded to the five files above; the parent review target becomes the exact fully green SHA.

Approval boundary:

- Requires PR-owner approval to rewrite/split the active PR and preserve review continuity.
- Does not require CI-policy, branch-protection, worker, timeout, machine, coverage, deployment, or merge override approval.
- No force-push or PR retarget should occur before that explicit decision.

## Structural throughput correction (separate follow-up)

The measured cost is repeated fresh-schema construction. Ordinary `Database::open_in_memory()` tests execute base DDL and every numbered migration; there are hundreds of call sites.

Exact initial correction candidates:

1. `crates/phoenix-db/src/lib.rs::Database::open_in_memory`
2. `crates/phoenix-db/src/migrations.rs::test_pool`

Make the private one-connection in-memory pool non-reaping with `.idle_timeout(None).max_lifetime(None)`. Reaping the sole `:memory:` connection would destroy the schema; disabling it is a correctness-preserving fixture invariant and removes per-pool maintenance tasks. Measure before claiming meaningful wall-time savings.

The larger remedy, only after compatibility review and benchmark proof, is a once-migrated SQLite image cloned into a fresh isolated in-memory database per ordinary test. Requirements:

- The template must be produced by the exact production migration runner, never by ledger stamping or parallel hand-maintained DDL.
- Every test retains its own writable database, pool, telemetry, and transaction boundaries.
- Historical migration/startup tests continue using uncached construction.
- Add parity checks for ordered `sqlite_schema`, migration `(version,name)` rows, required PRAGMAs, triggers/indexes, and baseline contents.
- Resolve whether cloned `_migrations.applied_at` timestamps are contractually acceptable before implementation.
- Benchmark repeated hosted samples inside existing worker/thread/time budgets.

Affected files for a bounded experiment:

- `crates/phoenix-db/src/lib.rs`
- `crates/phoenix-db/src/migrations.rs`
- workspace SQLx features / `Cargo.lock` only if SQLite serialize/deserialize is not already enabled
- focused parity and fixture-isolation tests beside the owning helpers

This throughput work is not required to split and land the already qualified federation stack.
