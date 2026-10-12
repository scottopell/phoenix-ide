# Remove unused durable wake contracts

## Ownership and approved scope

This conversation is the sole implementing owner; Global shepherds coordination. Implement locally on essmbp in the isolated owned worktree created by task approval, not the shared project coordinator checkout. Before edits, inspect current source/branch and coordinate migration allocation with the project coordinator, including federation860 and resource/UI landings. Do not overwrite concurrent work.

The user decided to remove durable wake contracts, explicitly retire residual obligations during migration, and preserve historical data. This is simplification of unused functionality, not delivery of a unified Promise-like tool surface.

## Implementation milestones

1. Establish isolated source ownership; refresh the dependency map against current main, inspect roadmap context if accessible, and coordinate migration numbering before allocating a migration.
2. Define and test a one-time transactional retirement migration with synthetic persisted states. Preserve bindings, receipts, payloads, message links and audit messages. Explicitly record feature retirement; disable remaining wake-owned observation, delivery and resumption obligations without manufacturing completion or executing work. Cover unresolved/nonterminal states, pending and materialized deliveries, accepted deliveries with runtime acceptance owed, and wake execution occurrences. Leave already-admitted Direct Turn execution on its ordinary lifecycle. If an actual accepted-delivery state conflicts with that policy, surface concrete evidence and a safe migration plan before implementing it.
3. Remove wake admission/registrar types and injection, runtime wake worker/startup/supervision, automatic adoption/dispatch, wake status/cancel APIs and UI, wake-only profile/repository code and tests, and wake lifecycle coupling in Close, continuation and deletion guards. No replacement settlement framework.
4. Prune shared-looking helpers by actual active consumers. Preserve shared workflow infrastructure used by Direct Turn; preserve creation protocol/worker and worker kicks. Preserve explicit runtime activation and deferred steering; names containing wake do not alone establish wake-contract ownership. Preserve historical schema/migrations necessary for forward upgrades without retaining operational wake codecs or workers solely for hypothetical use.
5. Update normative requirements and executive documentation to the approved retirement/history policy, recording the decision in a new ADR without rewriting historical ADRs. Run bounded checks, independent adversarial review and regression validation, then commit logical slices.

## Regression acceptance

- Bash run/peek/wait/kill and tmux readiness/handles work unchanged, with no wake admission.
- Sub-agent admission/completion and MCP requests/results work unchanged; no unified future handle abstraction is claimed.
- Conversation creation, Direct Turn, explicit runtime activation, continuation and Close retain their non-wake behavior.
- No wake worker starts, no registrar is injected, no wake adoption event causes automatic execution, and retired obligations cannot resume a conversation.
- Migration preserves historical data and leaves non-wake workflows and already-admitted Direct Turns unchanged; synthetic tests cover restart and atomicity as appropriate.
- Shared workflow consumers remain tested. Wake-only tests are removed or rewritten as preservation/retirement tests, not silently dropped with their relevant regression coverage.

## Evidence starting points

Read-only inspection at c5c33a53c found no non-test registration caller; api.rs retires bindings then starts the wake worker. RuntimeManager::settle_close_obligation and close_foundation::require_close_participants_quiescent_tx include wake coupling. continuation_service transfers/checks wake ownership. Executor process_adopted_wake_batch starts LLM execution. Direct Turn uses shared commit_transition_plan, effects and deliveries. Existing retire_all_registrations covers active/no-receipt and pending cases but is not sufficient evidence for all retirement states. Revalidate against current source. Prior local binding counts are reported, not independently verified or a migration precondition.

## Non-goals and authorization limits

No Promise-handle redesign, replacement workflow framework, multi-user draining/upgrade refusal, production DB copies/repairs/mutation, production process changes, merge, release or deployment. Git history preserves retired code; it does not justify purging persisted history. Separate performance/replay design work must not be blocked or steered by this removal.

## Local implementation evidence and integration boundary

The candidate removes wake registration/injection, startup worker and adoption, API/UI/client commands, and wake lifecycle gates. Additive migration 126 records retirement without rewriting historical payloads/statuses or cancelling admitted Direct Turns. Independent full-range source review found no actionable defect after migration/client fixes.

Qualification: eight retirement tests pass; complete migration module passes 92; runtime selection passes 802 with one existing ignored test; historical pointer/Direct Turn and startup-no-resumption tests pass. The initial DB run passed 892 with nine partial-schema migration-fixture failures; the fixtures were corrected and their entire migration module rerun. Python client passes 14; UI TypeScript/lint/Vitest and changed Allium models pass. Workspace all-target Clippy finishes exit 0 with warnings denied; its guard recorded descendant cleanup at exit, and actual child/group absence was independently verified before slot release. Full dev.py check is not claimed. Tools suite passes 533 with an unchanged macOS registry-test failure captured separately in task 33002.

Integration gate: federation PR #860 reserves migrations 124/125 and ADRs 092–094. Wake reserves migration 126 and ADR-095; shared ADR sequence preflight requires those predecessors and a rebase/recheck before merge. No merge, production mutation or deployment performed. Existing Bash/tmux handles are preserved, not delivery of a unified Promise interface.
