# Recover failed context compaction exactly once

Investigate the P0 conversation `investigate-slow-conversation-connect-latency`, where context exhaustion triggered compaction but provider-capacity failure during summary generation left the conversation stranded without a continuation. Preserve the conversation as the primary evidence target and do not drive or continue it until the evidence needed to reconstruct the failure has been collected.

## Objective

Make context compaction a durable, retryable state-machine operation that either completes exactly once and resumes the intended continuation, or remains visibly recoverable without losing, duplicating, or reordering user/assistant work.

## Investigation

1. Identify the target conversation and establish a narrow incident timeline from persisted conversation/message/effect state, runtime logs, and local production traces. Prefer TraceQL against VictoriaTraces (`phoenix-ide` at `127.0.0.1:10428`), using bounded time ranges and result limits; fetch full traces only after identifying relevant trace IDs. Use Jaeger only as fallback.
2. Before continuing or mutating the target, capture the evidence required to explain:
   - the transition that detected context exhaustion;
   - the compaction/summary request and provider-capacity response;
   - every state transition, persisted record, emitted event, retry decision, and queued continuation around the failure;
   - process restarts/recovery attempts and collector/runtime warnings;
   - the exact persisted state that makes the conversation unable to proceed.
3. Trace the complete implementation path across runtime/state-machine effects, provider error classification, persistence/recovery, SSE/API behavior, and UI controls. Read and honor all relevant normative specs before changing behavior.
4. Reproduce the failure deterministically with a provider-capacity error injected at summary generation, including crash/restart boundaries where relevant. Distinguish deployed production behavior from local source behavior using traces and version/commit evidence.

## Design constraints

- Model compaction as an explicit durable operation with structurally distinct pending, in-flight/recoverable, and completed outcomes; invalid combinations must be unrepresentable.
- The continuation intent and all data needed to retry must be persisted before the summary side effect can run.
- Provider-capacity/transient failures must retain a retryable operation and continuation. Terminal failures must remain visible and actionable rather than silently stranding the conversation.
- Recovery after restart must deterministically resume or expose the same operation.
- Exactly-once means one logical committed compaction and one logical continuation despite retries, duplicate deliveries, late responses, reconnects, or crashes. External provider calls may be at-least-once; durable operation identity and idempotent commit/transition guards must prevent duplicate summaries, messages, or continuations.
- A stale or duplicate compaction result must not overwrite newer conversation state.
- Persisted structure belongs in relational schema where fields are addressed independently; do not introduce child collections or migration-time field extraction inside JSON blobs.
- Capability gaps and dropped/ignored duplicate results must be logged at debug level or above.

## Implementation plan

1. Update the applicable spEARS requirements/executive documentation and, because this is a multi-step crash-recoverable lifecycle, add or amend the relevant Allium behavior specification. Record durable-operation/idempotency rationale in a project ADR if it is a new architectural decision. Keep timeless artifacts free of incident/task references.
2. Introduce the minimum typed durable state and schema/migration needed for compaction operation identity, retry state, continuation intent, attempt/error metadata, and committed result ownership. Avoid parallel authoritative representations.
3. Refactor state-machine transitions and effects so intent persistence precedes provider execution; classify provider capacity as retryable; make result commit compare-and-transition/idempotent; and schedule exactly one continuation only after successful compaction commit.
4. Add startup/conversation recovery and an explicit user-visible retry path where automatic retry cannot safely proceed. Ensure SSE/UI state accurately reflects recovery without inventing a second source of truth.
5. Repair the target conversation only after evidence capture and after the recovery implementation is validated. Use the same supported recovery path users will rely on; do not apply an opaque one-off database edit. Confirm with the user before sending/continuing any conversational content if recovery would trigger an LLM turn.

## Verification

- State-machine/unit tests cover success, capacity failure then retry, repeated retry, terminal error, duplicate/late provider result, stale operation result, and context exhaustion during continuation.
- Persistence/integration tests cover crashes immediately before and after intent persistence, provider dispatch, result persistence, compaction commit, and continuation scheduling.
- Restart tests prove a pending operation is recoverable and a committed operation cannot commit or continue twice.
- SSE/UI tests prove recoverable status and retry controls survive reconnect/reload.
- A deterministic end-to-end reproduction first demonstrates the original stranded state, then demonstrates recovery without duplicate summary/message/continuation.
- Run focused tests and `./dev.py check`; validate Allium and perform the spec-authoring pre-flight for touched specs.
- Reinspect production/local traces and logs during controlled recovery of `investigate-slow-conversation-connect-latency`, documenting trace IDs, timeline, root cause, and proof that it resumed exactly once.

## Deliverables

- Evidence-backed root-cause report with incident timeline and the precise failed invariant.
- Normative lifecycle/recovery specification and architectural rationale where required.
- Migration, implementation, and regression tests for durable retryable exactly-once compaction recovery.
- Safely recovered target conversation, only after evidence is preserved and continuation is explicitly authorized.
