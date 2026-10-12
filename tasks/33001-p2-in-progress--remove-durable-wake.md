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

No Promise-handle redesign, replacement workflow framework, multi-user draining/upgrade refusal, production DB copies/repairs/mutation, or dedicated urgent release/restart. The user subsequently authorized: “Qualify and merge, it will deploy out organically in next batch, deployment is authorized but not urgent for this work item”. Global remains the sole merger; deployment routes through the sole host in a normal qualified batch without bypassing the production search hold. This supersedes the opening merge/deployment restriction, not qualification gates. Git history preserves retired code; it does not justify purging persisted history. Separate performance/replay design work must not be blocked or steered by this removal.

## Local implementation evidence and integration boundary

The candidate removes wake registration/injection, startup worker and adoption, API/UI/client commands, and wake lifecycle gates. Additive migration 126 records retirement without rewriting historical payloads/statuses or cancelling admitted Direct Turns. Independent full-range source review found no actionable defect after migration/client fixes.

Qualification: eight retirement tests pass; complete migration module passes 92; runtime selection passes 802 with one existing ignored test; historical pointer/Direct Turn and startup-no-resumption tests pass. The initial DB run passed 892 with nine partial-schema migration-fixture failures; the fixtures were corrected and their entire migration module rerun. Python client passes 14; UI TypeScript/lint/Vitest and changed Allium models pass. Workspace all-target Clippy finishes exit 0 with warnings denied; its guard recorded descendant cleanup at exit, and actual child/group absence was independently verified before slot release. Full dev.py check is not claimed. Tools suite passes 533 with an unchanged macOS registry-test failure captured separately in task 33002.

Integration gate: federation merged as 1839a37f67c27d13f479068717d59a5aa413c450; this candidate is rebased onto that exact base, preserving ledger 124/125. Resource #888 reserves ADRs 095/096. Wake reserves migration 126 and ADR-098; shared ADR sequence preflight requires resource predecessors and rebase/recheck before merge. No merge, production mutation or deployment performed. Existing Bash/tmux handles are preserved, not delivery of a unified Promise interface.

Post-federation qualification at source 2ef580110: eight retirement tests and complete 94-test migration module pass with guard exit 0, stop_reason null, exact test-name/count oracles. Both claims released after verified child/group absence. Incremental target growth 0.224 GiB; total 2.414 GiB. Earlier runtime/UI/Clippy receipts remain pre-rebase evidence. Minimal guard fix refreshes process snapshot after parent poll; exact original guard and failure receipts are preserved, and the data-only proof does not classify the historical residual.

Actual resource merge eec3fae65f113705733cbd8c5f98b97a03fdcf81 is now the candidate base. ADRs095/096 and collector code are preserved; final097/126 ADR/spec shape passes. Final cumulative qualification and hosted/independent review are required before Global merger receipt.
