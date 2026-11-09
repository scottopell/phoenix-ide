# Native Task Approval Tools — Requirements

## Purpose and terms

Native Coordinator tools inspect and approve blocking task proposals without weakening [bedrock](../bedrock/requirements.md) lifecycle/authority boundaries (REQ-PROJ-003, REQ-BED-028/046), [compatibility](../compatibility/requirements.md) (REQ-COMP-001/002), or [durable outcomes](../durable-workflows/requirements.md) (REQ-DWF-043).

A **source call** is a tool-use occurrence proven by validating the full transcript/message/tool-use identity and canonical content against the server-owned persisted `ToolExecuting` replay input/state using typed Rust; its assistant message may not yet have a finalized messages row while the tool is running. A minimum immutable source-call occurrence row identifies that proven call and is referenced by the receipt. A **proposal occurrence** identifies one canonical persisted `propose_task` call, with full message/owner relationships enforced relationally. A **reviewed snapshot** is the single typed aggregate in `AwaitingTaskApproval`: the exact captured task-file Markdown, which is both the reviewed body and approved plan, repository-relative path, task kind, digest of those exact body bytes and canonical occurrence reference. There is no separate plan-text value. An **explicit target** resolves to both stable owner and concrete `CURRENT` transcript; approval freezes both, never a successor. **Committed** denotes a durable approval fact, not dispatch or provisioning.

## REQ-TAT-001 — Current Coordinator and proven source

WHEN native inspection or approval is invoked
THE SYSTEM SHALL admit only the current Coordinator source, proving its occurrence against server-owned canonical content
AND SHALL reject stale, historical, foreign, missing or mismatched source provenance before consequential work or receipt lookup.

THE SYSTEM SHALL require a typed explicit target, return both owner and concrete `CURRENT` transcript identities on inspection, and freeze both for approval without successor retargeting
AND SHALL NOT derive authority from supplied paths, display text, mode or a free-floating `CURRENT` selector.

## REQ-TAT-002 — Read-only canonical inspection

WHEN a pending proposal is inspected
THE SYSTEM SHALL return its canonical occurrence and the exact reviewed body, path, kind and digest from the single reviewed snapshot; the reviewed body is the captured task-file Markdown and approved plan, and its digest covers the exact body bytes
AND SHALL distinguish absence from an inspectable but approval-ineligible proposal, including ineligibility reasons and approval consequences, including when canonical proposal occurrence is missing and native approval is unavailable.

A stopped target's pending proposal SHALL remain inspectable; Stop is not a refusal of read-only inspection. Inspection SHALL NOT approve, write, resume, or change task/Goal lifecycle or grant mutation authority.

## REQ-TAT-003 — Explicit approval and admission

WHEN new native approval effects are requested
THE SYSTEM SHALL require explicit consequential approval, explicit `ContinueHere` placement, and the exact inspected target, occurrence and snapshot identity
AND SHALL return a typed unsupported refusal for `StartFresh`, without substitution; HTTP fresh-start behavior SHALL remain unchanged.

THE SYSTEM SHALL preserve existing mutation, owner, WorkScope, task lifecycle, Close and history admission
AND SHALL re-read only the reviewed path for the reviewed kind and compare body and digest before effects
AND SHALL reject changed reviewed body, path, kind or occurrence, including identical-path/body re-proposal, requiring inspection and approval of the replacement.

THE SYSTEM SHALL order new native approval admission against the explicit target conversation's Stop/pause latch at the owning pre-Git admission boundary; if Stop/pause is admitted first, approval SHALL return a typed refusal with no new approval effects
AND SHALL NOT clear, initialize, release or bypass that latch, force admission, or start/stop a Goal.

A later Stop SHALL remain governed by existing dispatch and cancellation ownership; this requirement makes no global cancellation or rollback promise. The ordering boundary SHALL be proven before the native mutator is enabled. Suspension of a saved Goal alone SHALL NOT block approval. This native Stop policy SHALL NOT change HTTP approval behavior.

## REQ-TAT-004 — Single typed pending snapshot

WHEN a blocking canonical proposal checkpoint is persisted
THE SYSTEM SHALL atomically capture canonical occurrence/full-message ownership and the whole state containing the single typed reviewed snapshot in `AwaitingTaskApproval`
AND SHALL enforce relational proposal-occurrence/full-message/owner identity for SQL joins and foreign keys; target proposal occurrence and approval relationships retain their full-message foreign keys. The source-call occurrence is distinct: it SHALL identify the source proven from persisted `ToolExecuting` replay input/state and need not reference a finalized assistant messages row before tool completion.

The snapshot SHALL remain an earned whole-read aggregate: exact reviewed body/path/kind/digest plus canonical occurrence reference, where the captured task-file Markdown is both reviewed body and approved plan and the digest covers exact body bytes; it SHALL NOT be a second normalized snapshot row or independently editable projection. The existing HTTP/wire projection SHALL remain unchanged; core enrichment SHALL be limited to the typed identity needed for this contract. Approval's existing approved-task normalization SHALL consume pending authority, not create a parallel pending snapshot authority.

## REQ-TAT-005 — Scoped immutable native receipt

WHEN approval commits
THE SYSTEM SHALL persist an immutable native source-call receipt in the existing approval transaction with approved objective, WorkScope authority, post-approval state, approved-plan context and any owed continuation obligation
AND SHALL scope deduplication to the proven source call through its minimum immutable source-call occurrence row, with full owner/target-proposal-message/occurrence foreign-key relationships; the source-call occurrence itself SHALL NOT require a finalized assistant messages-row foreign key while the tool is running
AND SHALL reference the canonical proposal occurrence rather than copy the reviewed snapshot into the receipt.

WHEN a source call is retried
THE SYSTEM SHALL apply current-source policy before receipt lookup, then consult the receipt before requiring pending state or new-effect Stop admission
AND SHALL return historical `Committed` for an exact payload replay without redispatching approval or continuation, even if the target has since stopped
AND SHALL reject changed payload under that scoped source-call identity as conflict.

A receipt SHALL identify the historical fact and owner, not assert that the owner remains pending, current, writable or freshly provisioned. Stop blocks new effects, not exact replay after current-source admission.

## REQ-TAT-006 — Honest outcomes and owning fail-stop

THE SYSTEM SHALL distinguish committed approval/continuation requested from dispatch, model response and fresh provisioning, using REQ-DWF-043's closed command-scoped outcomes.

WHEN failure is fatal, unclassified, or follows Git mutation before approval database commit
THE SYSTEM SHALL preserve actionable evidence and fail-stop at the owning execution boundary, not report `NotApplied`, ordinary success or presumed safe automatic retry.

WHEN authority projection fails after commit
THE SYSTEM SHALL retain the durable committed fact and follow the owning fail-stop contract, not report `NotApplied` or pretend projection succeeded. Receipt replay SHALL create no second continuation obligation; fulfilment remains owned by REQ-BED-046.

## REQ-TAT-007 — Evidence-bounded state compatibility

WHEN pending state lacks a canonical proposal occurrence captured atomically with its reviewed snapshot
THE SYSTEM SHALL preserve history, keep the proposal inspectable, report native approval unavailable and return an actionable request to re-propose for native approval
AND SHALL NOT infer, reconstruct, enrich or backfill occurrence identity from a latest matching message, equal plan, mutable disk or other heuristic evidence, synthesize provenance, or invent receipts.

This contract SHALL NOT add generic exactly-once external execution, downgrade, automatic Git rollback or a recovery framework beyond the owning contracts.

## REQ-TAT-008 — Preserve authority publication

WHEN approval establishes Work authority
THE SYSTEM SHALL use REQ-BED-046's single authority publication boundary before accepting post-approval work
AND SHALL NOT equate a committed receipt or requested continuation with a successfully projected runtime.

Native approval SHALL NOT introduce implicit Goal start/stop or pause clearing. Existing lifecycle admission and durable continuation ownership SHALL remain authoritative; the native Stop rule SHALL NOT change HTTP approval policy.
