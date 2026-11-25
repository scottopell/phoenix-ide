# macOS Desktop Onboarding & TOML `[llm]` Configuration

Make downloading `Phoenix.app` and configuring a provider (auth helper + gateway, Codex/ChatGPT sign-in, or API keys) a smooth first-run experience:

1. Support an optional `[llm]` section in `$XDG_CONFIG_HOME/phoenix-ide/config.toml` (default `~/.config/phoenix-ide/config.toml`) for auth helper, gateway base URLs, auth header style, custom headers, request tags, default model, and request model overrides (env vars override per field; static API keys rejected in TOML).
2. Capture login-shell `PATH` (bounded timeout, sentinel-framed, with fallback) for the bundled macOS sidecar and auth helper tests.
3. Replace the blank first-run window + floating Settings sheet in `Phoenix.app` with an inline, local-first onboarding flow supporting Auth Helper (with Slack-friendly TOML paste, live validation/summary, and live "Test Helper" verification), Codex/ChatGPT sign-in, and Keychain API keys, plus a subtle link to connect to an existing server.
4. Expose the same TOML config + Test Helper controls in Settings (`Cmd+,`), update `LlmStatusBanner.tsx`, and update specs/docs.
