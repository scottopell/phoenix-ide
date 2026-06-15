# Auth & Share -- Design

## Design Goals

The auth and share system prioritizes simplicity over sophistication.
This is a single-user developer tool, not a multi-tenant service.
Every design decision optimizes for the fewest moving parts that
achieve the security and sharing requirements.

### Session-Token Auth (REQ-AUTH-001, REQ-AUTH-002)

A successful login mints a random opaque session token, persisted
server-side in the `auth_sessions` table (token, created_at, expires_at)
via `SessionStore`, and sets it in the `phoenix-auth` cookie; the password
itself never travels in a cookie. Each request is authenticated by token
membership (an unexpired row in `auth_sessions`). API clients that do not
run the cookie login flow may instead present the password directly via
`Authorization: Bearer <password>`. The password is compared with a
constant-time check (at login and on Bearer requests) to prevent timing
attacks. Tokens survive a server restart, so a redeploy does not log
browsers out; a token is valid until its `expires_at` (matching the
cookie's `Max-Age`), after which it is rejected and swept.

When `PHOENIX_PASSWORD` is unset, the auth middleware is a no-op. No
conditional compilation, no feature flags -- the middleware checks the env
var and short-circuits.

### Login Throttle Identity

`POST /api/auth/login` is rate-limited per client: after a threshold of
consecutive failures a client is locked out for a back-off window, with the
counter cleared on success. The throttle bucket is keyed on the connection's
**real peer IP**, taken from the serve loop's `ConnectInfo<SocketAddr>` (wired
through both the plain and TLS listeners). A client cannot spoof its peer
address on a direct TCP/TLS connection, so it cannot mint a fresh bucket per
attempt to escape the lockout.

Forwarded client-IP headers (`X-Forwarded-For`, `X-Real-IP`) are **client-
controlled on a directly-reachable deployment** and so are ignored by default.
They are honored only when the operator sets `PHOENIX_TRUST_PROXY` to a truthy
value (`1`/`true`/`on`/`yes`), which asserts that a trusted reverse proxy sets
or overwrites them. In that mode the first `X-Forwarded-For` hop, then
`X-Real-IP`, take precedence over the peer (which is then the proxy's address).
Without `ConnectInfo` (peer unavailable) and without the opt-in, requests share
a single `"direct"` bucket — still bounding brute force.

### Share Token Lifecycle (REQ-AUTH-004, REQ-AUTH-006, REQ-AUTH-008)

Share tokens are random strings generated per-conversation on demand.
The creation trigger is navigating to `/share/c/{slug}`, which is a
GET that creates-if-not-exists and redirects. This makes sharing a
URL manipulation gesture, not a settings dialog.

Tokens are persisted to a `share_tokens` table (conversation_id, token,
created_at). The table is small (one row per shared conversation) and
queried by token on every `/s/{token}` request.

### Read-Only Surface (REQ-AUTH-005, REQ-AUTH-007)

The share view reuses existing conversation data endpoints (messages,
state, SSE stream) but serves them through a separate route prefix
(`/s/{token}/...`) that validates the token instead of the password.
The frontend renders a stripped-down view: message list and StateBar
only, no InputArea, no WorkActions, no file explorer, no settings.

SSE fan-out for multiple viewers is already supported by the broadcast
channel architecture. Each viewer subscribes to the same channel.

### Behavioral Specification

The complete behavioral contract (actors, surfaces, invariants, rules)
is defined in `specs/auth/auth.allium`. This design document describes
the implementation approach; the Allium spec is authoritative for
what the system does.
