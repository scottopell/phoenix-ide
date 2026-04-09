# Auth & Share -- Design

## Design Goals

The auth and share system prioritizes simplicity over sophistication.
This is a single-user developer tool, not a multi-tenant service.
Every design decision optimizes for the fewest moving parts that
achieve the security and sharing requirements.

### Stateless Auth (REQ-AUTH-001, REQ-AUTH-002)

The password is the token. No server-side session state, no token
generation, no expiry, no refresh. The client stores the password
in a persistent cookie; the server compares it on every request.
Constant-time comparison prevents timing attacks.

When `PHOENIX_PASSWORD` is unset, the auth middleware is a no-op.
No conditional compilation, no feature flags -- the middleware
checks the env var and short-circuits.

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
