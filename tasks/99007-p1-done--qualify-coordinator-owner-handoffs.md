# Qualify Coordinator-to-owner handoffs across busy states and continuations

## Commission and boundaries

Deliver the bounded handoff audit and smallest demonstrated correction through independent delta review, exact-source hosted CI, normal merge and postmerge verification. This work is independent of Close and release; coordinate shared files without blocking either. The Ember stack was implemented in Codex Desktop, not by the executive-overview owner.

No production deployment/restart/database operations, real-user fixtures, local native builds, dependency/native-test installs, live-provider spending changes, trust changes, broad prompt/Authority edits, new tracking store/scanner/scheduler/orchestration or dependency-management framework. No unsolicited wake policy. Never auto-rearm paused work or turn evidence into authorization. Stop for a genuinely new trust boundary or broader behavioral-policy decision. Remote semantics must be separately bounded; local tests prove no remote guarantee.

## Observed journey

The commissioning conversation reports `send_conversation_message` returning `invalid_state_for_message` for `AwaitingContinuation`, at Coordinator transcript `eae9f069-cf9b-4aef-a730-01216de67642`, message `2efed8e5-3fd2-40a5-8205-d28e1d8b93ca`; a later reissue queued to a successor. This is supplied incident evidence, not independently reproduced deployed behavior. Rejection is acceptable if explicit and safely recoverable. The obligation must not be silently lost, spuriously completed or multiply accepted across continuation.

## Verified findings from read-only source inspection

Checkout base: `d3b57573c`. Inspected merged source footprints and relevant implementation/contracts of #1091 (`6cebf545f`), #1092 (`3a842c5e7`), #1093 (`5b0e01c09`), #1096 (`e67fb4ec1`) and #1098 (`f4dbefe24`). No local build or test execution is claimed.

- #1091 retires roadmap coordination. Current specs, task/PR state and exact deployed evidence remain authorities.
- `check_user_message_acceptable` rejects `AwaitingContinuation`; `SendChatApplicationService::send_with_admission_guard` returns explicit rejection before new acceptance. `should_enqueue_steering` queues busy/cancellation states, not every rejected state.
- `SendConversationMessage::run` encodes semantic rejection as a successful tool result with rejected outcome/reason. A fixture must inspect the typed result, not merely tool success. Delivered/queued receipts establish acceptance, not recipient model presentation, understanding or completion.
- Stable targets use product admission and durable message-owner lookup before selecting a current transcript. Accepted-identity replay must stay with its original owner. A truly rejected identity has no acceptance receipt; recovery must establish this rather than minting unnecessary duplicate work.
- REQ-TRACK-001/003/004/006 retain captured intent across continuations, make retirement explicit/revision-checked, and prevent observations from granting permission. A failed send does not retire an existing item. It also does not automatically create a durable retry obligation.
- REQ-SCAN-004/006/008 permit source-bound capture/append only. Scanner coverage is not review/completion; background capture is not a retry queue or foreground wake guarantee.
- REQ-CEVT-002/006/007/008 label a completed turn truthfully and freeze occurrence-bound response evidence. They do not automatically resolve broader tracked work or prove sole-cause attribution.
- Existing tests cover slices: `explicit_local_send_preserves_routing_replay_and_provenance`, `fresh_concurrent_requests_dispatch_one_opening_through_exact_turn_settlement`, `tracking_scanner_idle_capture_reaches_authoritative_turn_and_pause_survives_two_continuations`, and `ending_response_threads_through_direct_and_adopted_settlement_without_replay_borrowing`. No composed handoff→continuation→owner-request→response-evidence test was identified.

## Inferences and unknowns

There is a demonstrated composed-coverage gap, not yet a demonstrated runtime data-loss defect. An existing tracking item should remain recoverable after rejected send and Coordinator continuation; prove this using the actual native paths. Do not claim every uncaptured request is already a durable obligation or that the scanner guarantees capture by a deadline. Whether the reported real request was tracked cannot be inferred without exact incident evidence; do not query production or private Desktop stores. If needed, ask Coordinator for only the exact missing source/receipt.

## Interaction map

Authentic disposable user commission → existing native tracking → Coordinator native send → stable/exact target resolution → admission rejection OR authoritative direct/steering receipt → native owner continuation/opening → permitted reissue or accepted-identity replay → actual owner model-request content → authoritative response settlement → immutable event-bound evidence → explicit Coordinator reconciliation.

Coordinator continuation separately rotates transcript authority while tracking retains stable aggregate ownership. Queue acceptance, materialization, model-request presentation and terminal settlement are separate observations. Stop/cancel/pause and source authority constrain this journey rather than being overridden by tracking or observations.

## First milestone: composed disposable regression

Use existing in-memory/disposable DB, RuntimeManager, actual native tool registry/send and continuation services with deterministic gated/recording fake provider. No modeled replacement send world, synthetic success receipts, live provider or production fixtures. Reuse existing test seams; prefer one focused integration module rather than exposing new production APIs for tests.

1. Record a synthetic commissioned outcome through trusted pending ToolExecuting context. Include a genuine restriction/pause variant. Assert retained item/evidence identities.
2. Issue a real native send to an owner at `AwaitingContinuation`. Assert exact rejection/code, no accepted turn/steering row/materialized instruction/provider presentation/completion event for that identity, and unchanged unresolved tracking disposition. Include an earlier completed owner turn so old evidence cannot mask this rejection.
3. Complete the owner continuation through the existing reservation/opening service. During pending opening, retain its admission fence. Reissue the still-authorized direction via stable target after resolving the actual current state; exercise queued busy admission. Distinguish original rejection from later acceptance in receipts and evidence.
4. Gate execution to inspect accepted/queued durable identity before presentation. Drain via actual runtime. Capture the exact owner model request containing the materialized instruction and source provenance. This establishes presentation at the provider-request boundary, not comprehension or a live provider guarantee. Verify one accepted instruction despite retries; count instruction-bearing inputs, not all provider requests, since tool loops may repeat context.
5. Produce deterministic owner ending prose and settle through the real authoritative workflow. Assert event binds the accepted input/occurrence and exact response carrier/digest/excerpt, not a preceding/later turn or rejected instruction. Label it Turn finished, not requested outcome complete. Keep the tracking item active until an explicit valid reconciliation/retirement operation.
6. Continue the Coordinator with a summary omitting the request. Assert native tracking detail and admitted successor model context recover the retained obligation and restriction. Predecessor mutation remains disallowed. Do not rearm paused work or emit a new wake merely by observation.

## Bounded companion assertions

Keep only matrix cases needed to protect the composed invariant: stable accepted-ID retry stays with original transcript across owner continuation; fresh identity routes successor; exact predecessor is not silently redirected; changed payload conflicts; canceled accepted identity is not resurrected. Preserve existing Stop semantics—Stop is not queue deletion and source Stop does not revoke an already accepted cross-conversation send. Reuse existing tests where sufficient instead of expanding this into a full cancellation/UI/federation project.

## Minimal corrective implementation

Run the composed test first in hosted validation. If current mechanisms satisfy it, ship regression evidence and concrete operating guidance, not invented runtime machinery. If it falsifies an existing invariant, fix only the owning seam and add the failing case. Do not add schema/foundation work absent a demonstrated necessity. Spec changes, if needed, must preserve normative authority boundaries and use ADRs only for actual policy/architecture decisions; a new boundary requires a user decision.

Guidance must distinguish: rejected (not accepted; diagnose actual state), delivered/queued (accepted only; retain receipt/identity), presented (exact instruction in an owner model request), response evidence (exact settled carrier, not sole-cause/task completion), and resolved outcome (explicit evidence-backed Coordinator reconciliation). Explain existing tracking as intent retention, not guaranteed delivery/retry automation.

## Validation and delivery

- Use existing hosted CI and repository dev.py workflow; no local native build/install commands, even to fix a test. Prefer existing CI lanes; avoid a new validation framework or repeated unchanged-source design review.
- Record exact commit SHA, commands/test filters, hosted run/check URLs, test names and evidence assertions. A compiler/test/fixture failure remains an in-scope fix; continue without repeated approval. If infrastructure blocks execution, report the concrete blocker rather than bypassing the local hold.
- Obtain focused independent delta review of affected implementation/tests and final integration. Resolve in-scope findings, pass exact-source required hosted checks, normal merge, then verify merged source and hosted outcomes. No deployment or restart.
- Final report: proven usable local behavior, operating recovery steps, checkable evidence, and explicit residual limits (no understanding proof, no every-request scanner guarantee, no remote/deployed guarantee). Do not treat a progress report as a pause.

## Acceptance

A reproducible hosted disposable handoff→continuation→presentation→response-evidence journey demonstrates no silent loss of captured intent, no duplicate accepted instruction and no spurious completion attribution. Any runtime correction is justified by a failing composed case and independently reviewed. If no runtime correction is necessary, evidence plus operating guidance is the valid outcome.

## Implementation and qualification evidence

- Outcome: composed coverage gap closed with a disposable native regression and `docs/coordinator-owner-handoffs.md`; no production runtime or schema change was needed. Captured intent survives rejection and both continuations without duplicate acceptance or borrowing an unrelated response. Tracking stays unresolved and its pause is retained.
- Hosted CI [run 43914880276](https://github.com/scottopell/phoenix-ide/actions/runs/43914880276) passed all planned checks: early exact-source gate, Clippy, Rust build/codegen/four test shards, UI/specs, E2E, assistant finalization geometry, musl, dev.py tests and task validation. The optional instrumented non-shipping witness was skipped.
- PR head for that run: `6f38f01c8ab4edf80c02bd9ad2e07bcba6f34f65`. Native build/test checkout was synthetic merge `83e078afe8847616fa91fb3d56635203c4969e1e` on `aea30bfab7934707cab16f4ffea9566e8cb9c91a`, **not** that exact head. [Artifact 14253080161](https://github.com/scottopell/phoenix-ide/actions/runs/43914880276/artifacts/14253080161), `rust-evidence-43914880276-1-test-2`, records the exact fixture invocation with return code 0 and `1 passed; 0 failed; 0 ignored` (1.37s). Test: `continuation_service::native::tests::owner_handoff::native_owner_rejection_continuation_busy_recovery_and_coordinator_retention`.
- Independent source reviews covered the full fixture and each corrective delta, including public APIs, typed tracking reference, product/input provenance, event settlement, and addressed standalone dispatch. Latest semantic head reviewed: `6f38f01c8ab4edf80c02bd9ad2e07bcba6f34f65`, no actionable remaining findings. Rebase onto `7bd5b438d` replayed all eight commits unchanged (`git range-diff` all `=`); final integration/exact-head hosted qualification remains a PR gate.
- Fixture failures corrected during hosted qualification: private/wrong APIs and lints; malformed tracking-reference discriminator; unscoped provider-request ordinal assumption with background notification discovery. The final fixture uses existing addressed production dispatch and asserts each capture's conversation identity. It covers ending-evidence creation, not watch-notification scheduling/delivery.
- No local native build/test/install, production operation, real-user fixture or live-provider call was used. Fake-provider presentation is not comprehension, sole-cause attribution, completed work, an automatic retry guarantee, or remote/deployed qualification.

Implementation evidence is complete; final exact-head hosted checks, normal PR merge and postmerge source/check verification are recorded on [PR #1104](https://github.com/scottopell/phoenix-ide/pull/1104), not inferred from the earlier merge-ref run.
