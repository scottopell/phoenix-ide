# Sync macOS Desktop App with Current Production Conventions

Bring `macos/Phoenix` (`Phoenix.app`) and `specs/desktop-release/requirements.md` back into alignment with current Phoenix production and LLM conventions before tackling the Datadog/first-run onboarding UX pass.

## Scope

1. **Fix stale `REQ-LDD-017` spec reference (`specs/desktop-release/requirements.md`)**
   - In `REQ-DESKTOP-REL-010`, remove the reference to the withdrawn "paired launchd consumer under `specs/launchd-deployment/requirements.md` REQ-LDD-017" (withdrawn by ADR-230; macOS `launchd` deployment only accepts local `HEAD` or `--release TAG|latest`).
   - Preserve the receipt schema `1` producer contract emitted by `macos/Phoenix/scripts/package-desktop-release.sh` and `.github/workflows/release.yml`.
   - Check `specs/desktop-release/executive.md` and `specs/desktop-release/desktop-release.allium` for companion drift.

2. **Default attached origin to `http://localhost:8031` (keep `launchd` TLS opt-in)**
   - Change `ConfigurationStore.defaultAttachedOrigin` in `macos/Phoenix/Phoenix/Configuration.swift` from `"https://localhost:8031"` to `"http://localhost:8031"` to match default `launchd` production deployments without requiring manual CA trust for loopback.
   - Keep `dev.py` / `launchd` TLS opt-in unchanged; keep HTTPS mandatory for non-loopback attached origins.

3. **Adopt current logging and LLM/gateway env conventions in bundled mode**
   - In `BundledServerConfiguration` (`macos/Phoenix/Phoenix/Configuration.swift`), add `fatalLogURL` (`phoenix-fatal.log`) alongside `phoenixLogURL` (`phoenix.log`) and `launcherLogURL` (`launcher.log`), and set:
     - `PHOENIX_LOG_FILE=<phoenix.log>`
     - `PHOENIX_FATAL_LOG_FILE=<phoenix-fatal.log>`
     - `PHOENIX_LOG_STDOUT=false`
   - Surface the structured log path alongside the launcher log in `ServerStatusView` / `ServerStatusSnapshot`.
   - In `SidecarLaunchEnvironment`, forward an explicit allowlist of Phoenix LLM/gateway/config env vars from the inherited environment when present and non-empty:
     - `LLM_API_KEY_HELPER`, `LLM_API_KEY_HELPER_TTL_MS`
     - `ANTHROPIC_BASE_URL`, `OPENAI_BASE_URL`, `OPENAI_RESPONSES_BASE_URL`, `OPENAI_CHAT_COMPLETIONS_BASE_URL`
     - `LLM_CUSTOM_HEADERS`, `LLM_REQUEST_TAGS`, `LLM_AUTH_HEADER`
     - `PHOENIX_LLM_MODELS`, `PHOENIX_LLM_REQUEST_MODELS`, `DEFAULT_MODEL`
     - `XDG_CONFIG_HOME`
   - Preserve Keychain secret injection precedence for `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` and redact sensitive header values if ever emitted in launcher diagnostics.
   - Note: Finder-launched configuration (e.g. `.phoenix-ide.env` / `config.toml` / Datadog AI Gateway onboarding UX) is explicitly deferred to the immediate follow-up onboarding pass.

4. **Relax attached ownership check and remove dead `currentMode` wire field**
   - In `ServerManager.swift`, `PhoenixApp.swift`, `Configuration.swift`, and `macos/Phoenix/README.md`:
     - Accept any valid authenticated Phoenix deployment in attached mode (`launchd_managed`, `systemd_managed`, `bare_supervisor_managed`, `development`, `unmanaged`, etc.) as `.ready(version, deployment)` instead of flagging non-`launchd_managed` deployments as `.unsupportedOwnership` with a red error icon. Attached mode never exposes server process-management controls regardless of ownership.
     - Retain `InstallationOwnership` decoding and display in Connection Status, and retain bundled mode's guard rejecting managed-production ownership (`!grantsManagedAuthority`).
     - Remove the unused `DeploymentInfo.currentMode` / `CurrentModeInfo` model and its dead extension helpers (not returned by `GET /api/deployment`).

## Verification

- `swift test --package-path macos/Phoenix`
- `xcodebuild -project macos/Phoenix/Phoenix.xcodeproj -scheme Phoenix -configuration Debug -derivedDataPath "$PWD/.phoenix/phoenix-macos-derived" CODE_SIGNING_ALLOWED=NO build`
- `macos/Phoenix/scripts/test-package-sidecar.sh` and `macos/Phoenix/scripts/test-package-desktop-release.sh`
- `python3 scripts/test-desktop-release-workflow.py` (if applicable) and `allium check specs/desktop-release/desktop-release.allium`
- `./dev.py tasks validate`
