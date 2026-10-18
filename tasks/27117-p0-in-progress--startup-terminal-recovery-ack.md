# Acknowledge legitimate terminal runtime startup settlement

Approved narrow startup repair; no production operations or schema/persistence changes. Parent owns authorized source commit/push, PR and hosted qualification.

## Causal evidence

Parent reports exact candidate `23badb8be` startup failure: conversation prefix `42e` was durably `ContextExhausted` with one retained steering row. The startup scanner unions queued-steering conversation IDs into recovery candidates, so this terminal row is selected. Host logs showed normal runtime cleanup, not a panic. In `ConversationRuntime::run_inner`, the `StepResult::Terminal` branch emits lifecycle notification and returns before sending `startup_llm_recovery_ack`; dropping its sender makes `RuntimeManager::settle_persisted_llm_request` fail with `runtime exited before acknowledging startup LLM recovery`. This aborts the manager's recovery pass despite a legitimate terminal outcome. Evidence is parent-supplied host diagnosis; this task performs no production inspection or mutation.

## Acceptance

- Explicitly send `Ok(())` before the legitimate startup terminal return. This acknowledges startup settlement, not provider completion. Interrupted/fatal/error paths retain failure semantics; no blanket drop/RAII success acknowledgement.
- ContextExhausted + retained queue executor regression: terminal exit, installed oneshot receives success, exact persisted state/queue preserved, no LLM request.
- Extend existing expired-subagent-overload terminal-startup regression to assert success acknowledgement.
- Database-backed RuntimeManager regression uses canonical migrated in-memory database fixture (schema 128), actual terminal row plus steering, successful bounded startup settlement, unchanged state/queue and zero provider requests; another recovering row is processed after it.
- Bounded normative startup contract clarification. Only lightweight formatting/offline task checks locally; hosted Rust before/after and independent review belong to parent. No local Rust build.

## Red-before reasoning (not an executed test result)

On the unmodified early-return path, the executor exits Terminal but the installed oneshot resolves to RecvError instead of Ok(Ok(())). The manager regression selects the terminal row first by sorted recovery ID, then returns Err from that dropped receiver, never reaching the second recovering row. The expired-subagent-overload regression reaches the same terminal branch after durable expiry settlement and likewise drops its acknowledgement.


## Initial validation and ownership

Independent static review found no actionable defect. Initial formatting check failed before rustfmt correction; final focused cargo-fmt/task/spec-anchor checks pass and diff whitespace is clean. A broader spec-shape invocation entered full devpy discovery and was stopped; it is not claimed passed. No local Rust build/test. Registry migrations.rs unchanged against23bad; SHA256 a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6, ordered target128. Installer owner7ad retains exact candidate-only forward replacement protocol; host owns prepared arm64 artifact dispatch unless explicitly reassigned. No production restart/rollback/DB queue edits.
