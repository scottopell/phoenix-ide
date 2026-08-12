# Phoenix for macOS

Phoenix.app is a thin native host for the Phoenix web UI. It keeps Phoenix's HTTP, SSE, WebSocket, persistence, terminal, browser, and update contracts in the Rust server while adding a native macOS window, Dock/Cmd+Tab identity, global shortcut, settings, diagnostics, microphone permission, and `phoenix://` handoff.

The app icon is derived from the canonical `ui/public/phoenix.svg` bird/flame mark, adapted to a padded dark macOS application tile for legibility from 16px through 1024px.

## Server modes

On first launch Phoenix.app stays disconnected until the user explicitly chooses either **Managed deployment** or **Bundled Phoenix** in Settings and clicks **Apply and Connect**. Draft edits in Settings remain local to that window until apply; simply closing Settings never changes the next launch target.

### Managed deployment

Connects to a configured canonical HTTP(S) origin. Phoenix.app verifies the public `GET /api/version` identity, loads Phoenix's ordinary login flow in WKWebView, and verifies authenticated `GET /api/deployment` through that same WebKit cookie session. HTTP attached origins are accepted only for localhost or loopback hosts; remote attached deployments must use HTTPS.

Only `launchd_managed` grants the expected managed-production classification. Other ownership values are shown but never grant process-management authority. Attached mode has no controls for the server process, database, LLM credentials, logs, or backend updates.

### Bundled Phoenix

Launches only `Phoenix.app/Contents/Helpers/phoenix_ide` (or a clearly marked Debug override) using `Process.executableURL`. The sidecar always receives:

- `PHOENIX_BIND_ADDR=127.0.0.1`
- `PHOENIX_TLS=off`
- a private runtime home under `~/Library/Application Support/Phoenix/sidecar-home`
- a private `PHOENIX_DATA_DIR` and database beneath that runtime home
- a fixed configured loopback port

An advisory file lock prevents concurrent Phoenix.app owners from opening that private runtime. A busy port is a bind failure; the app never adopts or kills a listener discovered by port. Readiness accepts only a deployment whose echoed `instance_id` matches the exact child Phoenix.app launched. Quit sends SIGTERM to the exact child, allows 35 seconds for Phoenix's 30-second graceful drain, then escalates to SIGKILL.

Optional Anthropic/OpenAI keys are stored in Keychain and injected only into the app-owned child environment. Phoenix.app writes or deletes those Keychain entries only during **Apply and Connect**; clearing a field in Settings does not touch Keychain until that apply step. The legacy plaintext `anthropicApiKey` preference is deleted from both the current preference domain and the legacy `com.scottopell.pa` domain. Diagnostics are typed and allowlisted; raw environments and secret values are never rendered.

## Build and test

```bash
swift test --package-path macos/Phoenix
xcodebuild \
  -project macos/Phoenix/Phoenix.xcodeproj \
  -scheme Phoenix \
  -configuration Debug \
  -derivedDataPath "$PWD/.phoenix/phoenix-macos-derived" \
  CODE_SIGNING_ALLOWED=NO build
```

Debug builds may omit the sidecar and use attached mode. Release builds fail unless `PHOENIX_SIDECAR_PATH` names an executable with every requested architecture:

```bash
PHOENIX_SIDECAR_PATH=/absolute/path/to/phoenix_ide \
  xcodebuild -project macos/Phoenix/Phoenix.xcodeproj \
  -scheme Phoenix -configuration Release build
```

The packaging phase copies the executable to `Contents/Helpers/phoenix_ide`, removes any stale helper during Debug no-sidecar builds, checks the packaged helper's architectures, and verifies nested signing when code signing is enabled. The app and sidecar form one release artifact; managed Phoenix's deployment updater does not own this sidecar.

Signed hardened-runtime builds also require the `com.apple.security.device.audio-input` entitlement for WebKit microphone capture; Phoenix.app includes it and still denies camera-only or combined camera/microphone requests.

## URL scheme

- `phoenix://open`
- `phoenix://status`
- `phoenix://conversation/<uuid>`

Conversation links activate Phoenix.app and navigate the existing authenticated WebView to the UUID route. Deep links never create, mutate, or seed conversations. Invalid UUIDs, unsupported actions such as `phoenix://new`, and `pa://` links are rejected.

## Trust and origin policy

Phoenix.app uses normal macOS TLS trust. It does not bypass certificate validation. Install the Phoenix-managed local CA through the supported Phoenix deployment flow when needed. Identity verification rejects cross-origin redirects instead of silently rebinding the configured origin. Activated links, popup `window.open` requests, microphone permission, and notification permission are internal only when scheme, host, and effective port exactly match the configured origin.

Same-origin `window.open` authentication uses managed child WebKit windows with WebKit's supplied opener configuration. Auth popups may navigate through HTTP(S) OAuth pages but cannot request microphone/notification permission, download files, or mutate the primary deployment-verification state.

## Manual compatibility matrix

Run a signed Finder-launched build, not only Xcode:

- Managed launchd deployment: login, shared Safari/iOS conversation history, reconnect, and quit without process changes.
- Bundled sidecar: private history, verified loopback/non-TLS/non-socket-activated deployment, second-owner refusal, occupied-port failure, intentional restart, graceful quit, and force-stop fallback.
- Web transports: fetch, SSE, terminal WebSocket, browser-view WebSocket, downloads, clipboard, drag/drop, notifications, voice input, popup authentication windows, and Web Inspector.
- Failures: offline, wrong service, auth required, TLS trust failure, unknown/unmanaged/ambiguous ownership, no models, sidecar crash, missing sidecar, and hotkey conflict.
- Deep links: an existing conversation UUID opens in the authenticated app; malformed and unknown UUIDs do not navigate or mutate server state. `phoenix://new` remains unsupported.
- Security: no key/token/password/custom secret header in preferences, diagnostics, app logs, error text, or process arguments.

The project intentionally retains macOS 15.0 as its deployment target until every used SwiftUI/AppKit/WebKit callback is compatibility-tested on an older target.
