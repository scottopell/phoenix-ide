# Native Task Approval Tools — Executive

## Status and authority

**Bounded Part A underway; NOT verified.** Owner authorized canonical proposal capture using the existing conversation/assistant canonical receipt as the ownership witness. The code captures typed provenance in the pending aggregate and preserves it on approval retry, but no native approval tool is enabled or registered. Focused tests are written but NOT RUN locally; hosted CI is pending. Source-live proof, native approval, Close, and runtime verification remain NOT DONE. No schema, migration, generated type or wire change is delivered.

The native Stop/pause policy is resolved: the Coordinator, acting within saved standing authority / the commissioned Goal, selected refusal of new approval effects while the explicit target conversation latch is set, conveyed through owner `a52`'s question response. This is not new human approval or authority to resume. Existing human resume is required; a suspended saved Goal alone does not block approval. Inspection remains read-only and may expose stopped pending proposals with ineligibility and consequences. Exact historical receipt replay after current-source admission returns `Committed` without redispatch even after target Stop.

Owner's minimized single-aggregate direction governs this bounded implementation. The supported task remains in progress; the native source policy and mutator are separate follow-on gates.

## Bounded supplied facts; no new audit

- HTTP approval follows `mutation_admission` → reload `AwaitingTaskApproval` → `GitBackedActiveWorkScope` → task lifecycle admission → unbound event → `RequestLlm`.
- Lifecycle admission rejects SubAgents, Close/history violations, and states other than `Accepted` / `StandaloneAvailable`. `TaskLifecycleAction` does not exist.
- HTTP approval has no blanket GoalStop/pause predicate. Its unbound event does not prove a native source call.
- No independent SQL body/path lookup or lifetime need has been established. Preserve the single typed pending aggregate and existing HTTP/wire projection; reuse the existing canonical receipt for proposal ownership, and reserve immutable scoped native approval receipts for the later mutator. The native source call is proven by validating full transcript/message/tool-use identity against server-owned persisted `ToolExecuting` replay input/state using typed Rust; the assistant message is not yet a finalized messages row while its tool runs. The future source-call receipt must not require a source-message FK before tool completion.

## Minimized proposed impact

- Keep one reviewed-body/path/kind/digest plus canonical occurrence reference in the single typed `AwaitingTaskApproval` snapshot, an earned whole-read aggregate. The exact captured task-file Markdown is both the reviewed body and approved plan; there is no separate plan-text value, and the digest covers exact body bytes. Atomically checkpoint occurrence/ownership and whole state. No normalized snapshot row, duplicate receipt snapshot or hydration refactor. Existing approved-task normalization consumes pending authority rather than maintaining a parallel copy.
- Add only necessary typed core identity/provenance enrichment; preserve HTTP/wire projection. Pending state without a canonical proposal occurrence captured atomically with the reviewed snapshot remains inspectable but native approval unavailable; preserve history and provide actionable re-proposal. No heuristic reconstruction, enrichment or backfill.
- Initial native approval requires explicit consequential intent and `ContinueHere`. `StartFresh` is a typed unsupported refusal, never substituted; HTTP fresh-start remains untouched.
- Keep lifecycle and authority publication boundaries. At the owning pre-Git boundary, Stop admitted before native approval admission yields typed refusal and no new approval effects. Never clear/bypass Stop. A later Stop remains under existing dispatch/cancellation ownership; no global rollback promise. Prove this ordering before enabling the native mutator. Goal suspension alone is not Stop; HTTP policy is unchanged.
- Commit immutable source-call-scoped receipt in the existing approval transaction, referencing canonical occurrence with full owner/message relationships. Current-source policy precedes lookup; exact replay precedes pending/new-effect admission and never redispatches. Changed payload conflicts.
- Fatal/unclassified and post-Git/pre-DB failures use owning fail-stop, not `NotApplied`. Projection failure after commit retains the durable fact and uses owning fail-stop, not a false no-effect result.

No REQ-BED-028/REQ-PROJ-003 amendment is required by this minimized aggregate model absent a concrete conflict; none is identified in the supplied facts. REQ-BED-046, REQ-COMP-001/002 and REQ-DWF-043 retain authority. The prior assertion that bedrock amendment and projection refactoring were prerequisites is withdrawn.

## Coverage and proposed verification

Part A (canonical pending-state capture) is in progress. Focused tests are written but have NOT RUN locally; hosted CI is required. Parts B (source-live proof), C (native approval), and Close remain deferred.

| Requirement | Implementation | Tests | Runtime | Focused verification to add |
|---|---|---|---|---|
| REQ-TAT-001 | NOT DONE | NOT DONE | NOT DONE | Current Coordinator; full transcript/message/tool-use identity matched against persisted typed `ToolExecuting` replay input/state; forged/mismatched state rejected; source assistant messages row absent while tool runs; explicit owner and concrete CURRENT returned/frozen; no successor retargeting |
| REQ-TAT-002 | NOT DONE | NOT DONE | NOT DONE | Exact aggregate inspection; stopped pending and missing-occurrence proposals remain inspectable with ineligibility/consequences; missing occurrence reports native approval unavailable; no mutation |
| REQ-TAT-003 | NOT DONE | NOT DONE | NOT DONE | Explicit ContinueHere; typed StartFresh refusal/no substitution; HTTP unchanged; stale identical-body re-proposal; disk drift; lifecycle/Close/history; prove pre-Git Stop-first ordering returns typed refusal with no new effects before mutator enablement; later Stop follows existing cancellation ownership; Goal suspension alone is not Stop |
| REQ-TAT-004 | PARTIAL A: typed occurrence/digest/kind captured before canonical checkpoint, validated against same-round assistant/tool-use and receipt; no separate occurrence table | WRITTEN, NOT RUN | NOT VERIFIED | Atomic canonical receipt/whole-state checkpoint and rollback; foreign occurrence rejection; same-body/new-assistant identity; unchanged HTTP/wire projection; approved normalization consumes pending authority |
| REQ-TAT-005 | NOT DONE | NOT DONE | NOT DONE | Receipt/approval atomicity; scoped target-proposal full-message FKs and minimum source-call occurrence reference (no finalized source-message FK); source-call identity validated against typed persisted replay input/state; source gate before lookup before pending/Stop; exact historical replay even stopped, no redispatch; payload conflict |
| REQ-TAT-006 | NOT DONE | NOT DONE | NOT DONE | Committed/requested vs dispatched/response/provisioned; fatal/unclassified and post-Git/pre-DB fail-stop; post-commit projection failure retains committed fact; no NotApplied or second continuation obligation |
| REQ-TAT-007 | NOT DONE | NOT DONE | NOT DONE | Missing atomically captured canonical occurrence keeps proposal inspectable but native approval unavailable; preserve history/actionable re-proposal; no heuristic reconstruction, enrichment, backfill, fabricated provenance/receipts or hydration refactor |
| REQ-TAT-008 | NOT DONE | NOT DONE | NOT DONE | Authority publication before work; owning projection fail-stop; no Goal start/stop or pause clear |

## Review gates and exclusions

1. Owner authorized bounded Part A; proposed ADR-244 reflects the existing canonical receipt as its ownership witness.
2. Complete hosted CI verification of Part A, then separately prove source-live identity before enabling any native mutator. Remaining requirements are not implemented.

No native execution, local build/tests/codegen, registration, RC2, release or production work is included in Part A. A draft PR triggers standard hosted CI; runtime behavior remains unverified pending those results. The README index labels ADR-244 Proposed; Stop/pause policy is resolved but its implementation is deferred.
