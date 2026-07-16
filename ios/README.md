# Phoenix iOS Client

A native SwiftUI client for the Phoenix API, focused on **robustness to
network gaps**: everything you've seen stays readable offline, and messages
you write while disconnected queue locally and deliver themselves when
connectivity returns. Think "usable on the subway."

It is a deliberately simplified companion to the web UI — conversations,
messages, tool activity, and sending — not a replacement (no terminal,
diff viewer, chains, or file browser).

Spec: [`specs/ios_client/`](../specs/ios_client/requirements.md).

## Building

Requires a Mac with Xcode 15+ (iOS 17 SDK). The Xcode project is generated
from `PhoenixMobile/project.yml` with [XcodeGen](https://github.com/yonaskolb/XcodeGen):

```bash
brew install xcodegen
cd ios/PhoenixMobile
xcodegen generate
open PhoenixMobile.xcodeproj
```

Select your device or a simulator and run. There are no third-party
package dependencies — first build works offline.

To install on a physical device, set your development team in
Signing & Capabilities (or add `DEVELOPMENT_TEAM` under
`settings.base` in `project.yml` and regenerate).

## Connecting to a server

On first launch, enter the server URL and password:

- **Dev server:** `./dev.py status` prints the URL (e.g.
  `https://localhost:8034`). From a device, replace `localhost` with the
  Mac's LAN name (e.g. `https://your-mac.local:8034`).
- **Prod deploy:** `https://<host>:8031` by default.
- **Password:** the same password the web login uses. The app sends it as
  `Authorization: Bearer <password>` (same scheme as `phoenix-client.py`);
  it is stored in the iOS Keychain. Leave blank if the server runs with
  auth disabled.

Phoenix servers usually serve TLS with a self-signed certificate — leave
**Trust self-signed certificate** on for those. Trust is **pinned on first
use**: the first successful connection records the certificate's SHA-256
fingerprint, and a later mismatch fails closed until you explicitly forget
the pin in Settings → Security (e.g. after reinstalling the server). The
bundled ATS exception (`NSAllowsArbitraryLoads`) exists for this
personal-tool posture; the pin governs what is actually trusted.

For reaching your server away from home (the actual subway case), put the
server on a tailnet/VPN and use its tailnet hostname.

## Testing

```bash
cd ios/PhoenixMobile
xcodegen generate
xcodebuild test -project PhoenixMobile.xcodeproj -scheme PhoenixMobile \
  -destination 'platform=iOS Simulator,name=iPhone 16'
```

CI runs the same thing on a macOS runner for any change under `ios/`
(`.github/workflows/ios.yml`) — the Linux-based `./dev.py check` lanes
cannot cover Swift.

**The testing pattern:** pure components get *contract tests* — one test
per rule of the contract they implement, named after that rule — and
views stay untested. The two exemplars to mimic:

- `Tests/SSEParserTests.swift` — contract: the SSE wire format.
- `Tests/OutboxTests.swift` — contract: the delivery rules in
  `specs/user_message_queue/user_message_queue.allium` (+ REQ-IOS-002
  deviations). Persistence runs for real against a per-test temp
  directory via the `DiskStore.baseDirectory` seam.

When adding logic, keep it out of views and in a pure type so it can be
tested this way. Next candidates, in value order: `PhoenixEvent` decoding
(init snapshot + pending-events replay shapes), `BashResult` envelope
parsing, and the `ConversationSession` reducer (sequence guard, init
replay floor, dedup-by-message_id) — the reducer needs its API/SSE
dependencies made injectable first.

## How offline works

| Concern | Mechanism |
|---|---|
| Navigate & read offline | Conversation list + per-conversation snapshots cached as JSON under Application Support; rendered before any network I/O |
| Queue messages offline | Outbox per conversation, persisted to disk *before* the send is attempted; survives app restarts |
| No duplicate sends | The queue entry's local id **is** the `message_id` the server deduplicates on, so retries/resends are idempotent |
| Auto-delivery | Outbox drains on connectivity restore (`NWPathMonitor`), SSE reconnect, app foreground, and turn completion |
| Spotty streaming | SSE reconnect with exponential backoff + jitter; every reconnect gets a fresh `init` snapshot including the server's replay ring (`pending_events`), so a mid-turn reconnect resumes the in-flight view instead of blanking |
| Sending mid-turn | Server accepts it as a steering message; the entry shows "queued for after current turn" until it lands in history |
| Hard failures | A definitive server rejection marks the entry failed with Retry/Discard; transport failures never fail an entry, they just wait |

The outbox implements the same delivery contract as the web UI
(`specs/user_message_queue/user_message_queue.allium`); iOS-specific
deviations are recorded in `specs/ios_client/requirements.md`.

## Code map

```
PhoenixMobile/Sources/
  PhoenixMobileApp.swift    App entry; scene-phase resync hooks
  AppModel.swift            Settings (Keychain/UserDefaults), API + session ownership
  Support/
    JSONValue.swift         Generic JSON tree for polymorphic wire payloads
    Keychain.swift          Password storage
    CertPinStore.swift      Trust-on-first-use certificate pin (self-signed TLS)
    ConnectivityMonitor.swift  NWPathMonitor -> offline banner + drain triggers
    DiskStore.swift         Versioned atomic JSON persistence — read the
                            versioning rule in its header before changing
                            any persisted struct
    ImageProcessing.swift   Picked photos -> bounded JPEG ImagePayload
    AttentionMonitor.swift  STOPGAP nudge tier: BGAppRefresh list diff ->
                            local notifications. Deleted (not extended)
                            when server-side APNs lands (tasks/58046)
    BackgroundRefresh.swift BGTaskScheduler plumbing for the stopgap tier
  API/
    Models.swift            Wire types (Conversation, Message, envelopes)
    PhoenixAPI.swift        REST client, Bearer auth, self-signed trust delegate
    SSE.swift               Byte-level SSE parser + PhoenixEvent decoding
    ConversationState.swift Typed state decode with .other/.unknown fallback
  Store/
    Outbox.swift            Persistent offline message queue (the contract)
    ConversationAction.swift  Action seam: online-only vs outboxed policy axis
    ConversationListStore.swift  Cached conversation list
    ConversationSession.swift    Per-conversation reducer + SSE loop + drains
  Views/                    SwiftUI screens (list, conversation, composer, setup…)
    ToolViews.swift         Per-tool native renderers (bash, think) + dispatch;
                            unknown tools fall back to the generic JSON cards
    StateViews.swift        Typed-state detail dispatch (working detail,
                            needs-action cards, error card + dismiss)
    QuestionCard.swift      In-app question answering (single/multi select,
                            Other text, dismiss) — TaskApprovalCard's sibling
    AttachmentViews.swift   Base64 image rendering with visible decode
                            fallback; strips for messages + tool results
```

