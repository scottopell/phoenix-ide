<!--
ID 27104 chosen above 27102 (27103 reserved for the cross-process flock
follow-up flagged in the same conversation). Created without `taskmd new`
since the binary isn't installed in this env; run `./dev.py tasks fix`
if reallocation needed.
-->

# Native first-party `ChatGPT`/`Codex` OAuth login (browser + device code)

## Problem

Phoenix's codex auth bridge currently borrows `~/.codex/auth.json` written
by the official Codex CLI. This works for users who already have Codex CLI
installed and have run `codex login`, but it has three concrete failure
modes:

1. **No Codex CLI installed.** A fresh dev machine, a CI runner, a
   container — none of these have `~/.codex/auth.json` and Phoenix can't
   help the user create one beyond surfacing the actionable hint we
   already wired up ("run `codex login`"). The user has to install Codex
   CLI to use the bridge, even though Phoenix itself has nothing else to
   do with that tool.
2. **No browser available.** SSH sessions, headless servers, dev
   containers without port forwarding — the PKCE + loopback HTTP server
   pattern Codex CLI uses requires the user's machine to receive a
   browser callback at `127.0.0.1:1455`. Doesn't work over SSH without
   port forwarding gymnastics, doesn't work in CI.
3. **File-format coupling.** If Codex CLI changes its `auth.json` schema
   or rotates its client_id, the bridge breaks until we follow.

A first-party login flow inside Phoenix removes all three.

## Goal

Phoenix can produce its own `CodexCredential` without requiring Codex CLI
to be installed, supporting both:

- **Browser/PKCE flow** for the standard local dev case (matches what Pi
  and Codex CLI do today)
- **Device code flow** for headless / no-browser cases (matches Codex
  CLI's `request_device_code` path, which Pi notably does NOT support)

Both paths produce the same token shape and reuse the existing refresh /
mtime-cache / atomic-write machinery in `src/llm/codex_credential.rs`.

## Reference implementations

### Pi (`badlogic/pi-mono`) — what we learned

Pi added first-party ChatGPT login in `pi-mono/packages/ai/src/utils/oauth/openai-codex.ts`
(~450 lines). They implemented:

- **PKCE-based authorization_code grant** to `https://auth.openai.com/oauth/authorize`
- **Local loopback callback server** on `127.0.0.1:1455/auth/callback`
  (port chosen to match Codex CLI's expectation, since the OAuth
  redirect_uri is registered against the shared `app_EMoamEEZ73f0CkXaXp7hrann`
  client ID)
- **Manual paste fallback** when the loopback server can't bind (SSH'd
  in, port already in use, etc.) — they race the browser callback
  against an `onManualCodeInput` promise and take whichever lands first
- **OAuth params** `id_token_add_organizations=true`, `codex_cli_simplified_flow=true`,
  `originator=pi` — the `codex_cli_simplified_flow` param is what gets
  the same simplified flow Codex CLI uses
- **Account ID extraction** from JWT claim `["https://api.openai.com/auth"].chatgpt_account_id`
  — every request, since Pi doesn't trust the file's `account_id` field
- **Storage** via Pi's own `~/.pi/agent/auth.json` (different file from
  Codex CLI's), with `proper-lockfile` for cross-process safety
- **Refresh** via form-encoded `refresh_token` grant to the same
  `oauth/token` endpoint (note: form-encoded — Codex CLI's refresh body
  is JSON; OpenAI accepts both)

What Pi did NOT add: device code flow. That's the gap this task closes
on top of Pi's pattern.

### Codex CLI — device code support

Codex CLI's `codex_login` crate has `request_device_code` and
`complete_device_code_login` (`codex-rs/login/src/device_code_auth.rs`).
The wire flow is **NOT RFC 8628** — it's a custom OpenAI flow under
`/api/accounts/deviceauth/`:

1. `POST {issuer}/api/accounts/deviceauth/usercode`
   - body: `{"client_id": "app_EMoamEEZ73f0CkXaXp7hrann"}`
   - response: `{user_code, device_auth_id, interval, ...}`
2. Print verification URL `{issuer}/codex/device` + `user_code` to the
   user; tell them to visit on any device, sign in, enter the code
3. Poll `POST {issuer}/api/accounts/deviceauth/token`
   - body: `{"device_auth_id", "user_code"}`
   - 403 / 404 = still pending; sleep `interval` seconds, retry
   - 200 = success; body shape matches what the PKCE flow returns
     (`{access_token, refresh_token, id_token, expires_in}`)
   - 15-minute hard timeout
4. Same token storage / refresh / JWT account-id-extraction as the PKCE
   path

If `usercode` returns 404, the message Codex CLI shows is "device code
login is not enabled for this Codex server" — so device flow is gated
by the issuer. The hosted ChatGPT issuer at `auth.openai.com` enables
it; self-hosted forks may not.

Useful clone commands while implementing:

```bash
git clone --depth=1 https://github.com/openai/codex.git /tmp/codex-cli
git clone --depth=1 https://github.com/badlogic/pi-mono.git /tmp/pi-mono

# Key reference files:
#   /tmp/codex-cli/codex-rs/login/src/device_code_auth.rs
#   /tmp/codex-cli/codex-rs/login/src/server.rs                  (PKCE)
#   /tmp/codex-cli/codex-rs/login/src/pkce.rs                    (PKCE helper)
#   /tmp/pi-mono/packages/ai/src/utils/oauth/openai-codex.ts     (Pi's PKCE)
#   /tmp/pi-mono/packages/ai/src/utils/oauth/pkce.ts             (Pi's PKCE helper)
```

## Design sketch

### New module: `src/llm/codex_login.rs`

Two public functions producing the same `(access_token, refresh_token,
id_token, expires_at, account_id)` tuple that the existing
`apply_refresh_response` already knows how to persist:

```rust
pub async fn login_pkce(
    auth_path: &Path,
    on_open_url: impl Fn(&str),       // UI hook: open browser, show URL
    on_manual_paste_fallback: impl FnOnce() -> Pin<Box<dyn Future<Output = String>>>,
) -> Result<CodexCredentialFile, CodexAuthError>;

pub async fn login_device_code(
    auth_path: &Path,
    on_prompt: impl Fn(&DeviceCodePrompt),  // UI hook: show URL + user_code
) -> Result<CodexCredentialFile, CodexAuthError>;

pub struct DeviceCodePrompt {
    pub verification_url: String,  // "{issuer}/codex/device"
    pub user_code: String,
    pub interval_secs: u64,
    pub expires_at: Instant,
}
```

Each function returns an `AuthFile` ready to be written via the existing
`write_auth_file` (already 0600-safe, atomic). On success, the next
`CodexCredential::get()` call's mtime check picks the new file up
without restart.

### Wire constants (shared with `codex_credential.rs`)

```rust
const ISSUER_BASE: &str = "https://auth.openai.com";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPE: &str = "openid profile email offline_access";
const JWT_ACCOUNT_CLAIM_PATH: &str = "https://api.openai.com/auth";
// CLIENT_ID already defined in codex_credential.rs as
// "app_EMoamEEZ73f0CkXaXp7hrann"
```

### PKCE flow (`login_pkce`)

1. Generate verifier (32 random bytes, URL-safe base64) + S256 challenge
2. Generate random `state` (16 bytes hex)
3. Construct authorize URL with params:
   `response_type=code`, `client_id=...`, `redirect_uri=...`,
   `scope=openid profile email offline_access`,
   `code_challenge=<S256>`, `code_challenge_method=S256`,
   `state=<random>`, `id_token_add_organizations=true`,
   `codex_cli_simplified_flow=true`, `originator=phoenix-ide`
4. Start a tiny `hyper`/`axum` server on `127.0.0.1:1455` that handles
   `/auth/callback?code=...&state=...`. Validate state matches.
5. Call `on_open_url(&authorize_url)` so the UI can launch the browser
6. `select!` between the callback and the manual-paste fallback (mirror
   Pi's race pattern — whichever lands first wins)
7. Exchange `code + verifier` at `POST {ISSUER_BASE}/oauth/token`
   (form-encoded body: `grant_type=authorization_code, code, code_verifier,
   client_id, redirect_uri`)
8. Decode JWT `access_token`, extract `chatgpt_account_id` from claim
9. Build `AuthFile { auth_mode: "chatgpt", tokens: { access_token,
   refresh_token, id_token, account_id }, last_refresh: now }`,
   write via `write_auth_file`

### Device code flow (`login_device_code`)

1. `POST {ISSUER_BASE}/api/accounts/deviceauth/usercode` with
   `{client_id}` → parse `{user_code, device_auth_id, interval}`
2. Call `on_prompt(&DeviceCodePrompt { verification_url:
   format!("{ISSUER_BASE}/codex/device"), user_code, interval_secs,
   expires_at: now + 15min })` so the UI can render
3. Loop with `tokio::time::sleep(interval)`:
   `POST {ISSUER_BASE}/api/accounts/deviceauth/token` with
   `{device_auth_id, user_code}` →
   - 403/404: continue polling (not yet authorized)
   - 200: parse `{access_token, refresh_token, id_token, expires_in}`,
     break loop
   - other: error
4. Same JWT decode + AuthFile write as PKCE path
5. Hard timeout at `now + 15min`

### Storage location

Two options, picked at integration time:

- **(a) Write to `~/.codex/auth.json`** (same file Codex CLI uses).
  Pros: zero new state, one source of truth, Codex CLI also benefits
  from any rotation we do. Cons: requires we be confident in the
  schema match, file-format change in Codex CLI breaks both sides.
- **(b) Write to `~/.phoenix-ide/codex-auth.json`** (private to
  Phoenix). Pros: schema independence; no risk of corrupting Codex CLI
  state. Cons: two source-of-truth problem if user also uses Codex CLI.

Recommend (a) initially — Pi went with their own file because they
don't want a Codex CLI dependency, but Phoenix isn't trying to be
fully independent. A user who also runs Codex CLI gets one shared
session. If we need to diverge later, switching to (b) is a one-line
change in the `default_auth_path()` we pass to login.

### UI integration (out of scope for this task)

- TUI prompt for device-code path (URL + code in big text)
- Browser launcher for PKCE path (`open` on macOS, `xdg-open` on
  Linux, `start` on Windows) — or just print the URL and let the user
  click it
- Manual paste fallback input field
- `/login` slash-command in the chat or settings panel

These belong in a follow-up UI task once the core login is in.

### Env-var integration with the bridge gate (depends on the gate-restoration follow-up)

The PR-merge state has the bridge auto-enabling on file presence,
which is being walked back behind an env var (separate follow-up
flagged in the same conversation). Native login interacts with that
gate as follows:

- The env-var gates whether the bridge is _used_, not whether login
  is _available_. A user can run `codex login` (via Codex CLI or via
  Phoenix's new `/login`) without the gate set, and Phoenix won't
  route OpenAI traffic through it.
- The login command should optionally set the env var in the
  user's standard phoenix env file as a convenience ("Login successful.
  Add `OPENAI_USE_CODEX_AUTH=1` to your env file? [y/N]").

## Acceptance criteria

- [ ] `src/llm/codex_login.rs` (new) implements `login_pkce` and
      `login_device_code`, both producing an `AuthFile` ready for
      `write_auth_file`
- [ ] Constants (issuer, redirect URI, scope, client ID, JWT claim
      path) live in one place — extract from `codex_credential.rs` if
      they need to be shared
- [ ] Both flows: state validation (PKCE), 15-minute timeout (device),
      JWT account-id extraction, atomic 0600 write
- [ ] `codex_credential::CodexCredential::load()` continues to work
      against files written by either login path
- [ ] Unit tests: PKCE state-mismatch rejection, JWT account-id
      extraction with the `https://api.openai.com/auth` claim, device
      code timeout after 15 minutes, device code 200-on-pending success
- [ ] Doc comment on `codex_login.rs` explains the trade-offs
      (browser-required for PKCE, no-browser-needed for device code,
      shared client_id)
- [ ] No new dependencies if avoidable: PKCE base64 helpers can use
      `base64` (already in deps), random bytes via `getrandom` or `uuid`
      (already in deps for v4 UUIDs); `axum`/`hyper` already in deps
      for the existing API surface

## Out of scope

- UI/TUI integration (browser launch, code-display widget, manual
  paste field, `/login` command) — file separately
- Logout flow (clear in-memory cache + delete file) — trivial follow-up
- Switching the default storage path away from `~/.codex/auth.json`
- Cross-process file locking on the auth file (separate task 27103)
- Migration from any other auth file format

## Open questions to resolve before implementation

1. **Storage path** — share `~/.codex/auth.json` with Codex CLI (recommended)
   or use a Phoenix-private file?
2. **Browser launcher** — bundle a portable opener (`open`/`xdg-open`/`start`)
   or print the URL and let the user click? (Recommend: print URL, optionally
   try to launch.)
3. **Should the login flow itself live behind the same env-var gate?** Probably
   no — login is harmless without the bridge being active; the gate controls
   request routing, not credential acquisition.
4. **Device code: poll endpoint shape** — Codex CLI's body is
   `{device_auth_id, user_code}` (not the RFC 8628 `device_code` field). Match
   Codex's shape exactly since we're targeting the same issuer.

## Notes

- Pi's `pi-mono` plus Codex CLI's `codex_login` crate together cover
  every wire detail needed; this task is mostly translation work, not
  protocol research.
- Both Pi and Codex CLI use the same client_id (`app_EMoamEEZ73f0CkXaXp7hrann`)
  and Phoenix already does too. There's no per-app registration step.
- Headless / device-code is the differentiator vs Pi. Codex CLI added
  it in `device_code_auth.rs:159`; copying that flow into Phoenix gives
  us SSH/CI/container support that Pi lacks.
- This task is independent of 27103 (cross-process file locking) and
  27105 (other missing wire fields), but lands cleanly on top of either.
