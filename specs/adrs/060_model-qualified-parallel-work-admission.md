# ADR-060: Parallel Work admission is explicitly model-qualified

- **Status:** Accepted
- **Date:** 2026-09-20
- **Affects:** REQ-SA-001, REQ-SA-003 through REQ-SA-005, REQ-PROJ-008, REQ-BED-008, REQ-BED-018, REQ-LLM-003

## Context

Phoenix historically admitted at most one Work child per parent. That universal rule prevented qualified orchestration models from partitioning independent implementation work, even though Work children already share the parent's environment. Removing the rule without another boundary would let unknown models, child-model overrides, or catalog version assumptions silently expand write concurrency.

Parallel admission also exposes lifecycle races that a process-local active-child count cannot decide durably: partial batches, cancellation before initial work, concurrent child materialization, reconstructed result delivery, and duplicate parent acceptance. The parent must conserve fan-in without making a live runtime reference the source of lifecycle truth.

At this decision point Phoenix's supported catalog contains no Opus 5+ identifier. The exact parent identifiers manually qualified for parallel Work admission are `gpt-5.6-sol`, `gpt-5.6-terra`, and `gpt-6-astra`; `gpt-5.6-luna` is not qualified.

## Options considered

1. Keep one Work child for every parent: simple, but prevents capable parents from coordinating independent writers.
2. Qualify by provider family or minimum model version: easy to extend, but silently grants concurrency to untested or newly discovered models.
3. Isolate every Work child in a separate worktree and merge automatically: structurally avoids shared writes, but creates a second integration and lifecycle framework that conflicts with exact shared `WorkScope` attachment.
4. Use an explicit resolved-parent allowlist, atomic durable admission, and trusted shared-environment collaboration: bounded, fail-closed, and compatible with existing parent fan-in and WorkScope ownership.

## Decision

Parallel Work admission is decided only from the parent's actual resolved model identifier. The qualified set is exactly `gpt-5.6-sol`, `gpt-5.6-terra`, and `gpt-6-astra`. Luna, custom routes, other Anthropic models, unknown identifiers, and newly introduced identifiers are unqualified. No Opus identifier is listed because no Opus 5+ identifier exists in the supported catalog at this decision point. Future qualification requires another explicit decision and exact catalog entry. Reasoning effort, service tier, configuration tier, named worker, child model, child persona, provider family, and version ordering do not affect qualification.

Qualified parents may atomically admit multiple Work children in a bounded batch and across calls. Unqualified parents remain sequential: at most one Work child may be pending, and a multi-Work batch is rejected. A changed parent model affects only later admission; admitted children and results retain their identities.

The complete validated batch is durably admitted before any child starts, or none of it is admitted. Each admitted child has one durable parent identity, cancellation fact, initial-start fact, terminal evidence, and parent-acceptance fact. Cancellation before start prevents initial work. Concurrent creation or recovery joins one materialization per child identity. Terminal evidence precedes delivery; delivery resolves the current parent by durable identity. Repeated terminal delivery is an idempotent success and cannot append or resume twice.

Parallel Work children intentionally attach to the parent's exact `WorkScope` and share its working environment as trusted collaborators. Parent instructions require partitioning and integration; child instructions require preserving unrelated edits and reporting overlap, conflicts, or uncertainty. Phoenix does not promise path locking, automatic merge reconciliation, or atomic arbitrary writes.

Addressed current-runtime resolution is a narrow delivery primitive extracted from direct-turn delivery, not a general event bus. Durable workflow facts decide whether cancellation or terminal delivery is owed; current-runtime resolution only finds the present recipient.

## Consequences

Qualified parents gain bounded parallel implementation while unqualified and future models fail closed. Shared-worktree throughput depends on parent partitioning and honest conflict reporting rather than structural write isolation.

Admission, cancellation, start, terminal evidence, and acceptance require normalized durable authority. Parent fan-in removes the exact accepted child from pending, conserves admitted identities, ignores accepted duplicates, and cannot ordinarily settle while a child remains pending. These rules tighten existing sub-agent and bedrock behavior without introducing a new ProductConversation Close lifecycle.

The LLM catalog retains Luna, Sol, Terra, and Astra and excludes the retired 5.4-mini, 5.4, and 5.5 built-ins. Existing live pins use explicit compatibility mappings, while historical and already-resolved request/runtime identity remains unchanged.
