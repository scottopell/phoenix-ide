# iOS Client — Executive Summary

## Current Status

The native companion implementation is shipped on `main`. The app lives in
`ios/PhoenixMobile/` (SwiftUI,
iOS 17+, no third-party dependencies; Xcode project generated via XcodeGen
from `project.yml` — see `ios/README.md` for build instructions).

Swift is outside `./dev.py check` (Linux lanes); the iOS client has its
own CI job (`.github/workflows/ios.yml`, macOS runner) that generates the
Xcode project and runs the unit-test target on a simulator for any change
under `ios/`.

The connection, foreground/background, delivery-ownership, replay, cache,
and hard-deletion lifecycle is normative in `ios_client_lifecycle.allium`.
It composes with the existing server SSE and user-message queue contracts
rather than duplicating their state machines.

The shipped client still consumes legacy server project/mode vocabulary where the server emits it. The normative grounding surface instead binds files to exact attached WorkScopes and does not expose Project or hidden GitRepository as a mobile grouping or lifecycle object; REQ-IOS-019 through REQ-IOS-021 remain unimplemented.

Unit coverage follows a contract-test pattern (see `ios/README.md`
"Testing"): pure components get one test per rule of the contract they
implement. An opt-in live-server XCUITest covers first-run TLS setup, mock
conversation creation, optimistic-send reconciliation, and cold relaunch;
it is isolated from the ordinary CI scheme. Coverage includes SSE framing and typed
hard-delete decoding, the outbox delivery and real-disk durability rules,
versioned-store downgrade protection, typed conversation states and chat
eligibility, question encoding, image processing, attention diffs, bash
result parsing, certificate-pin decisions, legacy conversation decoding,
and reducer ordering/hard-delete behavior. Remaining verification steps:
on-device build and an airplane-mode queue/drain test pass.

## Requirement Coverage

| Requirement | Surface |
|---|---|
| REQ-IOS-001 offline rendering | `DiskStore`, `ConversationListStore`, `ConversationSession` snapshot load |
| REQ-IOS-002 offline queue | `Outbox` (contract), `ConversationSession.send/drainOutbox` |
| REQ-IOS-003 idempotent delivery | `Outbox.enqueue` (localId = message_id), `ConversationSession.inFlight` |
| REQ-IOS-004 auto drain | drain triggers in `ConversationSession` + `ConnectivityMonitor` observers |
| REQ-IOS-005 SSE + reconnect | `ios_client_lifecycle.allium`, `SSEParser`, `PhoenixEvent`, `ConversationSession.streamLoop` |
| REQ-IOS-006 steering visibility | `Outbox.markAccepted(steering:)`, `OutboxEntryView` |
| REQ-IOS-007 connectivity transparency | `OfflineBanner`, `ConnectionStateBar`, composer send tint |
| REQ-IOS-008 auth/TLS | `PhoenixAPI` (Bearer), `ServerTrustDelegate` + `CertPinStore` (TOFU pinning), `Keychain` |
| REQ-IOS-009 creation flow | `NewConversationView` |
| REQ-IOS-010 rendering | `MessageViews.swift` (generic fallback), `ToolViews.swift` (dispatch + native bash/think renderers), `ConversationSession.toolUseIndex` (result join) |
| REQ-IOS-011 typed state | `ConversationState.swift` (decode + fallback, tested), `StateViews.swift` (detail dispatch) |
| REQ-IOS-012 action policy | `ConversationAction.swift` (policy axis), `ConversationSession.perform`, `AppModel.archive` |
| REQ-IOS-013 task approval | `TaskApprovalCard` (StateViews.swift), approve/reject/feedback actions |
| REQ-IOS-014 versioned persistence | `DiskStore.saveVersioned/loadVersioned` (tested), per-store schema version constants |
| REQ-IOS-015 image attachments | `AttachmentViews.swift`, `ImageProcessing` (tested), composer PhotosPicker, outbox `images` |
| REQ-IOS-016 question answering | `QuestionCard.swift`, `QuestionAnswers` encoder (tested), respond/dismiss actions |
| REQ-IOS-017 coordinator access | `AppModel.openCoordinator`, list globe entry + row badge |
| REQ-IOS-018 advisory nudges | `AttentionMonitor` (diff tested), `BackgroundRefresh`, `NotificationRouter` |
| REQ-IOS-019 grounding/files | Not started; blocked by ProductConversation migration (`tasks/04009-p2-blocked--ios-vnext-grounding-files.md`) |
| REQ-IOS-020 prose reader | Not started; planned under `tasks/04010-p2-blocked--ios-vnext-prose-reader-comments.md` |
| REQ-IOS-021 prose comments | Lifecycle specified in `ios_prose_feedback.allium`; implementation planned under `tasks/04010-p2-blocked--ios-vnext-prose-reader-comments.md` |

## Planned iOS vNext Program

iOS vNext first establishes an independent deterministic native fixture
harness, then migrates the client to the shipped ProductConversation REST
aggregate while preserving ordinary transcript-row sessions as the sole owners
of live SSE, cache, reducer, and outbox behavior. Core migration may begin after
the fixture contract seam is committed and reviewed. Permanent delete,
follow-up/source retrieval, grounding, and comments are later capabilities and
do not block that migration.

| Section | Status | Task |
|---|---|---|
| Program completion owner | Blocked | `tasks/04011-p1-blocked--ios-vnext-productconversation-expansion.md` |
| ProductConversation migration | Blocked | `tasks/04004-p1-blocked--ios-vnext-productconversation-migration.md` |
| Deterministic rendering fixture harness | In progress | `tasks/04005-p1-in-progress--ios-vnext-rendering-fixture-harness.md` |
| Conversation status and actions | Blocked | `tasks/04006-p2-blocked--ios-vnext-conversation-status-actions.md` |
| Tool-output rendering | Blocked | `tasks/04007-p2-blocked--ios-vnext-tool-output-rendering.md` |
| Markdown rendering | Blocked | `tasks/04008-p2-blocked--ios-vnext-markdown-rendering.md` |
| Grounding and file browsing | Blocked | `tasks/04009-p2-blocked--ios-vnext-grounding-files.md` |
| Prose reader and comments | Blocked | `tasks/04010-p2-blocked--ios-vnext-prose-reader-comments.md` |

The fixture harness is one bounded implementation task. The remaining rows are
umbrella section owners, not broad implementation instructions; they are split
only when separate source ownership and scheduled concurrent work justify the
coordination cost. ProductConversation migration remains blocked until the
fixture contract seam is committed and reviewed. Later capability umbrellas
remain blocked until independently scheduled.

## Known Gaps / Future Work

- Real-time push does not exist yet. The nudge tier (REQ-IOS-018) is
  best-effort BGAppRefresh polling with local
  notifications, bounded by iOS's ≥15-minute opportunistic cadence. The
  intended end state is server-side APNs on durable inbox observations —
  tracked as `tasks/20004` (blocked on the durable-workflows stack) — at
  which point the stopgap is deleted, not extended.
- Steering-queue entries are not reorderable/deletable server-side from the app.
- No archived-conversations view, rename, or delete (archive itself is
  wired via swipe).
- Task approval and question answering are resolvable in-app; commission
  review approval renders as a generic needs-action card and resolves from
  the web UI. Complete state/action coverage belongs to the blocked iOS vNext
  status/actions section.
- Question responses omit option previews and per-answer notes
  (annotations); the server accepts answers without them.
- Markdown rendering is inline-only (no fenced code blocks or tables); the
  blocked iOS vNext Markdown section owns the comprehensive pass.
- Native tool renderers cover `bash` and `think` only; all other tools
  (patch, browser, keyword_search, tmux, …) hit the generic JSON cards. The
  blocked iOS vNext tool-output section owns the renderer catalog and queue.
- Grounding/file browsing and the prose reader/commenting interface
  (`REQ-IOS-019` through `REQ-IOS-021`) are not implemented in the native
  client. Their blocked iOS vNext sections are later capabilities and do not
  block core ProductConversation migration.
- The `recoverable_inconsistency` trigger is time-based rather than
  causally proven (deviation recorded in REQ-IOS-002).
