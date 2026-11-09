# Native Task Approval Tools — Executive

## Draft status and authority

**Draft only; owner review blocked.** Requirements and Proposed ADR-244 are a review packet, not shipped behavior. Slice A has added core identity vocabulary only; it does not capture canonical proposals or enable/register native approval. Focused tests are written but NOT RUN; all requirements remain NOT DONE and runtime verification is NOT DONE. No schema, migration, native tool, generated type or wire change is delivered.

The native Stop/pause policy is resolved: the Coordinator, acting within saved standing authority / the commissioned Goal, selected refusal of new approval effects while the explicit target conversation latch is set, conveyed through owner `a52`'s question response. This is not new human approval or authority to resume. Existing human resume is required; a suspended saved Goal alone does not block approval. Inspection remains read-only and may expose stopped pending proposals with ineligibility and consequences. Exact historical receipt replay after current-source admission returns `Committed` without redispatch even after target Stop.

This revision is the owner `a52` minimization direction, superseding the full normalized-snapshot-row draft. Task 53019's body correction is explicitly authorized; its filename/status is unchanged. Policy selection is resolved; owner review, not another policy decision, remains the gate.

## Bounded supplied facts; no new audit

- HTTP approval follows `mutation_admission` → reload `AwaitingTaskApproval` → `GitBackedActiveWorkScope` → task lifecycle admission → unbound event → `RequestLlm`.
- Lifecycle admission rejects SubAgents, Close/history violations, and states other than `Accepted` / `StandaloneAvailable`. `TaskLifecycleAction` does not exist.
- HTTP approval has no blanket GoalStop/pause predicate. Its unbound event does not prove a native source call.
- No independent SQL body/path lookup or lifetime need has been established. Preserve the single typed pending aggregate and existing HTTP/wire projection; normalize only canonical proposal occurrence/full-message ownership and immutable scoped native receipts for joins, FKs and deduplication. The native source call is proven by validating full transcript/message/tool-use identity against server-owned persisted `ToolExecuting` replay input/state using typed Rust; the assistant message is not yet a finalized messages row while its tool runs. Retain only a minimum immutable source-call occurrence row for receipt identity, without a source-message FK.

## Minimized proposed impact

- Keep one reviewed-body/path/kind/digest plus canonical occurrence reference in the single typed `AwaitingTaskApproval` snapshot, an earned whole-read aggregate. The exact captured task-file Markdown is both the reviewed body and approved plan; there is no separate plan-text value, and the digest covers exact body bytes. Atomically checkpoint occurrence/ownership and whole state. No normalized snapshot row, duplicate receipt snapshot or hydration refactor. Existing approved-task normalization consumes pending authority rather than maintaining a parallel copy.
- Add only necessary typed core identity/provenance enrichment; preserve HTTP/wire projection. Pending state without a canonical proposal occurrence captured atomically with the reviewed snapshot remains inspectable but native approval unavailable; preserve history and provide actionable re-proposal. No heuristic reconstruction, enrichment or backfill.
- Initial native approval requires explicit consequential intent and `ContinueHere`. `StartFresh` is a typed unsupported refusal, never substituted; HTTP fresh-start remains untouched.
- Keep lifecycle and authority publication boundaries. At the owning pre-Git boundary, Stop admitted before native approval admission yields typed refusal and no new approval effects. Never clear/bypass Stop. A later Stop remains under existing dispatch/cancellation ownership; no global rollback promise. Prove this ordering before enabling the native mutator. Goal suspension alone is not Stop; HTTP policy is unchanged.
- Commit immutable source-call-scoped receipt in the existing approval transaction, referencing canonical occurrence with full owner/message relationships. Current-source policy precedes lookup; exact replay precedes pending/new-effect admission and never redispatches. Changed payload conflicts.
- Fatal/unclassified and post-Git/pre-DB failures use owning fail-stop, not `NotApplied`. Projection failure after commit retains the durable fact and uses owning fail-stop, not a false no-effect result.

No REQ-BED-028/REQ-PROJ-003 amendment is required by this minimized aggregate model absent a concrete conflict; none is identified in the supplied facts. REQ-BED-046, REQ-COMP-001/002 and REQ-DWF-043 retain authority. The prior assertion that bedrock amendment and projection refactoring were prerequisites is withdrawn.

## Coverage and proposed verification

Every status below is **NOT DONE**; scenarios are a plan, not evidence.

| Requirement | Implementation | Tests | Runtime | Focused verification to add |
|---|---|---|---|---|
| REQ-TAT-001 | NOT DONE | NOT DONE | NOT DONE | Current Coordinator; full transcript/message/tool-use identity matched against persisted typed `ToolExecuting` replay input/state; forged/mismatched state rejected; source assistant messages row absent while tool runs; explicit owner and concrete CURRENT returned/frozen; no successor retargeting |
| REQ-TAT-002 | NOT DONE | NOT DONE | NOT DONE | Exact aggregate inspection; stopped pending and missing-occurrence proposals remain inspectable with ineligibility/consequences; missing occurrence reports native approval unavailable; no mutation |
| REQ-TAT-003 | NOT DONE | NOT DONE | NOT DONE | Explicit ContinueHere; typed StartFresh refusal/no substitution; HTTP unchanged; stale identical-body re-proposal; disk drift; lifecycle/Close/history; prove pre-Git Stop-first ordering returns typed refusal with no new effects before mutator enablement; later Stop follows existing cancellation ownership; Goal suspension alone is not Stop |
| REQ-TAT-004 | NOT DONE | NOT DONE | NOT DONE | Atomic canonical occurrence/whole-state checkpoint; full-message ownership FKs; one typed aggregate; unchanged HTTP/wire projection; approved normalization consumes pending authority |
| REQ-TAT-005 | NOT DONE | NOT DONE | NOT DONE | Receipt/approval atomicity; scoped target-proposal full-message FKs and minimum source-call occurrence reference (no finalized source-message FK); source-call identity validated against typed persisted replay input/state; source gate before lookup before pending/Stop; exact historical replay even stopped, no redispatch; payload conflict |
| REQ-TAT-006 | NOT DONE | NOT DONE | NOT DONE | Committed/requested vs dispatched/response/provisioned; fatal/unclassified and post-Git/pre-DB fail-stop; post-commit projection failure retains committed fact; no NotApplied or second continuation obligation |
| REQ-TAT-007 | NOT DONE | NOT DONE | NOT DONE | Missing atomically captured canonical occurrence keeps proposal inspectable but native approval unavailable; preserve history/actionable re-proposal; no heuristic reconstruction, enrichment, backfill, fabricated provenance/receipts or hydration refactor |
| REQ-TAT-008 | NOT DONE | NOT DONE | NOT DONE | Authority publication before work; owning projection fail-stop; no Goal start/stop or pause clear |

## Review gates and exclusions

1. Owner review of the minimized contract and Proposed ADR-244; implementation remains blocked.
2. Follow-on implementation and focused evidence for all eight requirements only after authorization; update NOT DONE with checkable evidence.

No audit, build, tests, native execution, runtime verification, commit, push, registration or RC2 work is part of this revision. Only static `git diff --check` is permitted. ADR allocation was locally checked in the earlier draft (244 selected; known unmerged 243 collision avoided), not remotely reserved. The README index labels ADR-244 Proposed pending owner draft review; the Stop/pause policy is resolved.
