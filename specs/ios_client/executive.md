# iOS Client — Executive Summary

## Current Status

Initial implementation. The app lives in `ios/PhoenixMobile/` (SwiftUI,
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

Unit coverage follows a contract-test pattern (see `ios/README.md`
"Testing"): pure components get one test per rule of the contract they
implement; views stay untested. Covered so far: `SSEParser` (SSE wire
format) and `Outbox` (the user_message_queue delivery rules, exercised
against real disk persistence via a test seam). Remaining verification
steps: on-device build and an airplane-mode queue/drain test pass;
next unit-test candidates are `PhoenixEvent` decoding, `BashResult`
parsing, and the `ConversationSession` reducer (needs dependency
injection first).

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

## Known Gaps / Future Work

- No push notifications; updates arrive only while the app is foregrounded
  with a stream open.
- Steering-queue entries are not reorderable/deletable server-side from the app.
- No archived-conversations view, rename, or delete (archive itself is
  wired via swipe).
- Task approval is answerable in-app; the remaining needs-action states
  (respond-to-question, commission review approval) render as cards but
  resolve from the web UI — natural next ConversationAction cases.
- Markdown rendering is inline-only (no fenced code blocks or tables).
- Native tool renderers cover `bash` and `think` only; all other tools
  (patch, browser, keyword_search, tmux, …) hit the generic JSON cards.
- The `recoverable_inconsistency` trigger is time-based rather than
  causally proven (deviation recorded in REQ-IOS-002).
