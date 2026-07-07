# iOS Client — Executive Summary

## Current Status

Initial implementation. The app lives in `ios/PhoenixMobile/` (SwiftUI,
iOS 17+, no third-party dependencies; Xcode project generated via XcodeGen
from `project.yml` — see `ios/README.md` for build instructions).

The codebase is Swift and cannot be compiled or tested by this repo's CI
(`./dev.py check` does not cover `ios/`). Verification so far is
design-level review against the server contracts; first device build and
an on-device offline test pass (airplane-mode queue/drain cycle) are the
next verification steps.

## Requirement Coverage

| Requirement | Surface |
|---|---|
| REQ-IOS-001 offline rendering | `DiskStore`, `ConversationListStore`, `ConversationSession` snapshot load |
| REQ-IOS-002 offline queue | `Outbox` (contract), `ConversationSession.send/drainOutbox` |
| REQ-IOS-003 idempotent delivery | `Outbox.enqueue` (localId = message_id), `ConversationSession.inFlight` |
| REQ-IOS-004 auto drain | drain triggers in `ConversationSession` + `ConnectivityMonitor` observers |
| REQ-IOS-005 SSE + reconnect | `SSEParser`, `PhoenixEvent`, `ConversationSession.streamLoop` |
| REQ-IOS-006 steering visibility | `Outbox.markAccepted(steering:)`, `OutboxEntryView` |
| REQ-IOS-007 connectivity transparency | `OfflineBanner`, `ConnectionStateBar`, composer send tint |
| REQ-IOS-008 auth/TLS | `PhoenixAPI` (Bearer), `ServerTrustDelegate`, `Keychain` |
| REQ-IOS-009 creation flow | `NewConversationView` |
| REQ-IOS-010 rendering | `MessageViews.swift` (generic fallback), `ToolViews.swift` (dispatch + native bash/think renderers), `ConversationSession.toolUseIndex` (result join) |

## Known Gaps / Future Work

- No image attachments in the composer (wire types support them; UI does not).
- No push notifications; updates arrive only while the app is foregrounded
  with a stream open.
- Steering-queue entries are not reorderable/deletable server-side from the app.
- No archived-conversations view, rename, or delete.
- Markdown rendering is inline-only (no fenced code blocks or tables).
- Native tool renderers cover `bash` and `think` only; all other tools
  (patch, browser, keyword_search, tmux, …) hit the generic JSON cards.
- The `recoverable_inconsistency` trigger is time-based rather than
  causally proven (deviation recorded in REQ-IOS-002).
