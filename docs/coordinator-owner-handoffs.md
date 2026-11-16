# Coordinator-to-owner handoffs: receipts, recovery and evidence

This guide describes the existing local handoff contract. Tracking retains intent; it is not an outbox, retry scheduler or grant of authority. A rejected send can be correct. The operational obligation is to distinguish it from acceptance and keep any unresolved commission visible.

## Five distinct observations

| Observation | What it establishes | What it does not establish |
| --- | --- | --- |
| **Rejected** receipt | This attempt was not accepted; inspect `reason_code` and diagnostic | Queueing, presentation, understanding or completion |
| **Delivered / queued as steering** receipt | Phoenix accepted the identity on the reported receiving transcript | A materialized transcript message, an owner model request, understanding or completion |
| **Presented** instruction | The exact materialized input and source framing appeared in an owner model request | The model understood, obeyed or completed it; a provider request is not a provider acknowledgement |
| **Turn finished / response evidence** | An exact settled occurrence has an input-bound, immutable response reference or honest absence | Sole-cause attribution, success of the commissioned outcome, or permission to resume paused work |
| **Reconciled outcome** | The Coordinator explicitly reviewed relevant evidence against the requested outcome and reconciled its native tracking item | Permission beyond the user's existing scope |

A semantic rejection is encoded in a successful `send_conversation_message` tool output. Check its `outcome`; tool transport success alone is not acceptance. The UI's accepted-state disclaimer is intentional.

## Recovering a handoff at a continuation boundary

1. **Retain the commission and restrictions.** Use existing native tracking for important unresolved outcomes. Link the source request and relevant evidence. Keep pauses and approval conditions in the retained item. A send failure neither retires an existing item nor automatically creates one. Scanner coverage is not proof every request has been captured.
2. **Read the actual receipt.** Keep `target`, `message_id`, receiving transcript, outcome and rejection reason together. Do not credit an earlier owner's finished turn to a later rejected direction.
3. **Diagnose before reissuing.** `AwaitingContinuation` rejects ordinary input rather than queueing it. A pending continuation opening has its own admission fence. Use existing state/read and continuation mechanisms under their authority rules; do not force a send through the fence or schedule a new wake as a workaround. Continuing a transcript is not permission to rearm paused work.
4. **Use the right target and identity.** `@conv:<product>` selects the current writable transcript for a new acceptance. `@transcript:<id>` remains exact; it must not silently follow a successor. Retry an unchanged accepted request with the same UUID: durable replay preserves its original owner across continuations. A changed payload with the same UUID conflicts. After an explicit rejection, the unchanged UUID can be reissued once the target is admissible; the regression exercises that path. Never create multiple fresh identities merely because an accepted receipt has not yet produced a response. If acceptance is uncertain, retry the original identity rather than guessing it failed.
5. **Treat queued as pending, not finished.** Busy/cancellation states can accept steering. Observe its materialization/presentation and eventual exact response when needed. Do not mistake queue length, a changed state badge or a different turn's response for delivery of this instruction.
6. **Reconcile only with attributable evidence.** Ending evidence binds its saved response to its actual settled input/occurrence. Read the response and assess the commission; a finished turn may report incomplete work. Append relevant evidence and explicitly reconcile the existing tracking item. Observation does not lift pauses, move review state or grant authorization.

Stop is not queue deletion. Stopping the source does not revoke an already accepted target send. Target Stop and removal of a specific accepted steering entry are different operations. A canceled accepted identity must not be resurrected by a retry. Use those existing boundaries rather than promising blanket revocation.

## Disposable qualification

The focused composed fixture is:

`continuation_service::native::tests::owner_handoff::native_owner_rejection_continuation_busy_recovery_and_coordinator_retention`

It uses an in-memory database, actual native tools and continuation/admission/runtime paths, and a gated recording fake provider. It does not call a real provider. The gates make acceptance, provider-request presentation and response settlement separately observable; runtime settlement events, not sleeps, delimit response-evidence assertions.

The journey seeds an earlier real response, records a paused commission through trusted native tracking, rejects two instructions at `AwaitingContinuation`, reserves the owner continuation, checks the opening fence, opens through the native tool, and reissues one rejected identity into busy steering. Retry/conflicting-payload assertions protect one acceptance. The recovered input reaches the provider with original conversation provenance. Its exact saved ending response is distinguished from the older response and opening. Tracking remains unresolved. The Coordinator then continues with a summary omitting the obligation and recovers it, including the restriction and source evidence, through native tracking context/detail.

The fixture deliberately seeds the exhausted-summary boundary; it does not exercise a live summarizer or reproduce production timing. Its explicit pause is an execution prohibition while evidence inspection remains authorized, not an automatic retry permission. Existing companion tests cover original-owner replay, queue cancellation, Stop, exact-target restrictions and response immutability; this fixture is not a replacement for them.

### Limits

- The reported incident's deployment and whether that exact commission was already tracked are not established by source inspection or synthetic fixtures.
- A fake-provider capture proves content at the local provider-request boundary, not comprehension or live-provider receipt.
- No every-request capture deadline, automatic retry, spontaneous model follow-through or guaranteed completion is claimed.
- These are local-service semantics. No remote federation, network outage, remote continuation, cross-instance idempotency or remote delivery guarantee follows from this test.
- Qualification does not deploy or restart Phoenix. Hosted check evidence and source/merge identities belong in the PR/task, not in an inferred production claim.

## Contract anchors

- `specs/coordinator-tracking/requirements.md`: retained intent, explicit reconciliation and no permission expansion.
- `specs/coordinator-scanner/requirements.md`: capture/provenance boundaries; coverage is not review or completion.
- `specs/conversation-events/requirements.md`: truthful turn status and exact ending-response evidence.
- `specs/global-recall/requirements.md` and `specs/steering-messages/requirements.md`: typed targets, send acceptance and durable steering.
- `SendChatApplicationService`, `SendConversationMessage::run` and native `ContinuationApplicationService`: local admission, replay and continuation ownership.
