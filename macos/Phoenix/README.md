# Phoenix for macOS

Phoenix.app is a thin native host for the Phoenix web UI. It keeps Phoenix's HTTP, SSE, WebSocket, persistence, terminal, browser, and update contracts in the Rust server while adding a native macOS window, Dock/Cmd+Tab identity, global shortcut, settings, diagnostics, microphone permission, and `phoenix://` handoff.

## Server modes

### Managed deployment

Connects to a configured canonical HTTP(S) origin. Phoenix.app verifies the public `GET /api/version` identity, loads Phoenix's ordinary login flow in WKWebView, and verifies authenticated `GET /api/deployment` through that same WebKit cookie session.

Only `launchd_managed` grants the expected managed-production classification. Other ownership values are shown but never grant process-management authority. Attached mode has no controls for the server process, database, LLM credentials, logs, or backend updates.

### Bundled Phoenix

Launches only `Phoenix.app/Contents/Helpers/phoenix_ide` (or a clearly marked Debug override) using `Process.executableURL`. The sidecar always receives:

- `PHOENIX_BIND_ADDR=127.0.0.1`
- `PHOENIX_TLS=off`
- a private database under `~/Library/Application Support/Phoenix`
- a fixed configured loopback port

An advisory file lock prevents concurrent Phoenix.app owners from opening that private runtime. A busy port is a bind failure; the app never adopts or kills a listener discovered by port. Quit sends SIGTERM to the exact child, allows 35 seconds for Phoenix's 30-second graceful drain, then escalates to SIGKILL.

Optional Anthropic/OpenAI keys are stored in Keychain and injected only into the app-owned child environment. The legacy plaintext `anthropicApiKey` preference is deleted. Diagnostics are typed and allowlisted; raw environments and secret values are never rendered.

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

The packaging phase copies the executable to `Contents/Helpers/phoenix_ide`, checks architectures, and signs the nested executable when code signing is enabled. The app and sidecar form one release artifact; managed Phoenix's deployment updater does not own this sidecar.

## URL scheme

- `phoenix://open`
- `phoenix://status`
- `phoenix://new?prompt=...&cwd=...`

Conversation creation runs as same-origin JavaScript in WKWebView, so attached deployments reuse the ordinary Phoenix session cookie. `pa://` is intentionally not registered or accepted.

## Trust and origin policy

Phoenix.app uses normal macOS TLS trust. It does not bypass certificate validation. Install the Phoenix-managed local CA through the supported Phoenix deployment flow when needed. Activated links and microphone permission are internal only when scheme, host, and effective port exactly match the configured origin.

## Manual compatibility matrix

Run a signed Finder-launched build, not only Xcode:

- Managed launchd deployment: login, shared Safari/iOS conversation history, reconnect, and quit without process changes.
- Bundled sidecar: private history, verified loopback/non-TLS/non-socket-activated deployment, second-owner refusal, occupied-port failure, intentional restart, graceful quit, and force-stop fallback.
- Web transports: fetch, SSE, terminal WebSocket, browser-view WebSocket, downloads, clipboard, drag/drop, notifications, voice input, and Web Inspector.
- Failures: offline, wrong service, auth required, TLS trust failure, unknown/unmanaged/ambiguous ownership, no models, sidecar crash, missing sidecar, and hotkey conflict.
- Deep links: Unicode, quotes, ampersands, long prompt text, and cwd.
- Security: no key/token/password/custom secret header in preferences, diagnostics, app logs, error text, or process arguments.

The project intentionally retains macOS 15.0 as its deployment target until every used SwiftUI/AppKit/WebKit callback is compatibility-tested on an older target.
