# Credential Helper -- Requirements

## Background

`LLM_API_KEY_HELPER` lets operators configure a shell command that emits an API
key or token to stdout. The existing `CommandCredential` runs the command silently,
captures all stdout trimmed as the token, and caches it for a configurable TTL.

This works for simple helpers (`echo $TOKEN`) but breaks for interactive auth
flows (OIDC device flow, SSO) where the helper:
1. Emits a URL and a device code to stdout and blocks
2. Waits for the user to complete auth in a browser
3. Emits the token on the final line and exits

The user cannot complete the flow because Phoenix never surfaces the instructions.

---

### REQ-CREDHELPER-001: Last-Line Credential Extraction

WHERE `LLM_API_KEY_HELPER` is configured
THE SYSTEM SHALL treat the last non-empty line of the helper's stdout as the
credential
AND treat all preceding lines as instruction/progress output

**Rationale:** Interactive helpers emit URLs, device codes, and progress messages
before the final token. Capturing all stdout as a blob fails as an API key.
Simple one-line helpers (`echo $TOKEN`) are unaffected since their sole output
line is also the last.

---

### REQ-CREDHELPER-002: Credential Status in Models Response

THE SYSTEM SHALL include a `credential_status` field in `GET /api/models`:

- `not_configured`: no helper configured, no static key, no env key
- `valid`: a credential is available (static, env, or cached helper result)
- `required`: helper configured but no valid cached credential
- `running`: helper is currently executing
- `failed`: last helper run exited non-zero or produced no output

**Rationale:** The UI already polls `/api/models` for gateway and model
availability. Credential status belongs in the same response so the UI can
present a unified "LLM inaccessible" state with a contextual CTA — Authenticate
for `required`/`failed`, spinner for `running`, vs. check config for
`not_configured`.

---

### REQ-CREDHELPER-003: Auth Execution Endpoint

THE SYSTEM SHALL expose `POST /api/credential-helper/run` that:
- Starts the helper subprocess if status is `required` or `failed`
- Joins the existing run if status is `running` (no duplicate spawn)
- Returns an SSE stream of events until the helper exits:
  - `{ "type": "line", "text": "..." }` — each stdout line as it arrives
  - `{ "type": "complete" }` — helper exited 0, credential cached
  - `{ "type": "error", "exit_code": N, "stderr": "..." }` — non-zero exit

**Rationale:** The user clicks Authenticate in the Phoenix UI. Phoenix runs the
helper and streams its output in real-time so the user can see the URL and code,
open the browser link, and complete OIDC auth — all without leaving Phoenix.

---

### REQ-CREDHELPER-004: Single Concurrent Execution

THE SYSTEM SHALL run at most one helper process at a time.

WHEN `POST /api/credential-helper/run` is called while a run is already in
progress, the new connection SHALL receive all output lines emitted since the
run started (replay) followed by live lines, with no new subprocess spawned.

**Rationale:** Multiple browser tabs or concurrent conversation starts should
not spawn multiple auth processes. The first caller drives the auth; others
observe.

---

### REQ-CREDHELPER-005: Failure Surfacing

WHEN the helper exits non-zero OR emits no non-empty stdout lines
THE SYSTEM SHALL:
- Emit a `{ "type": "error", "exit_code": N, "stderr": "..." }` SSE event
- Set credential status to `failed`
- NOT cache any credential

**Rationale:** Silent failure is indistinguishable from a hang. The user needs
to see the error to know whether to retry or fix the command.

---

### REQ-CREDHELPER-006: TTL-Based Caching

THE SYSTEM SHALL cache the obtained credential for `LLM_API_KEY_HELPER_TTL_MS`
milliseconds (default: 7 200 000 ms / 2 hours).

WHEN the TTL elapses, credential status SHALL return to `required` on the next
`/api/models` poll.

**Rationale:** Tokens from OIDC device flows typically expire in hours, not
minutes. The existing TTL mechanism is the right knob; the default should match
common token lifetimes (~2 hours for short-lived JWTs, configurable up to 24h
for longer ones).

---

### REQ-CREDHELPER-007: Cache Invalidation on 401

WHEN the LLM provider returns HTTP 401 or 403 on any request
THE SYSTEM SHALL immediately invalidate the cached credential
AND set credential status to `required`

**Rationale:** Tokens may expire before the configured TTL (server-side
revocation, clock skew). The existing invalidate-on-401 mechanism in
`LlmAuth` already handles this; it should continue to work and reflect
correctly in the status API.

---

### REQ-CREDHELPER-008: UI Auth Panel

THE SYSTEM SHALL display an auth status indicator in the global header:
- `AUTH ✓` (green) when credential status is `valid`
- `AUTH ...` (muted) when status is `running`
- `AUTH ✗` (red) when status is `required`, `failed`, or `not_configured`
  with a helper configured

WHEN the user clicks the auth indicator with status `required` or `failed`
THE SYSTEM SHALL open an auth panel showing:
- The helper command (redacted to first word for security)
- A scrolling output box streaming lines from `POST /api/credential-helper/run`
- A success or error state when the run completes

**Rationale:** The user should never need to fall back to the CLI to complete
auth. The auth panel brings the terminal-style OIDC flow into the browser.
