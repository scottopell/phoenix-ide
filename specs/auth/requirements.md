# Auth & Share -- Requirements

## User Stories

### Story 1: Protect My Instance

As a developer running Phoenix IDE in a shared workspace (DD workspaces,
exe.dev), I need to prevent coworkers from accidentally or intentionally
sending messages, approving tasks, or abandoning conversations on my
instance, so that my agent sessions are not disrupted by unauthorized
actions through auto-forwarded ports.

### Story 2: Share My Session

As a developer working with an agent on a complex task, I want to share
a live view of my conversation with coworkers so they can follow along
in real-time for pair programming, demos, or review -- without giving
them the ability to interfere with the agent's work.

### Story 3: Demo to a Group

As a developer demoing Phoenix IDE to my team, I need multiple coworkers
to view the same conversation simultaneously so the whole team can watch
the agent work in real-time.

---

### REQ-AUTH-001: Password-Gated Access

WHERE the `PHOENIX_PASSWORD` environment variable is set
THE SYSTEM SHALL require a matching password for all API endpoints
AND reject requests without a valid password with HTTP 401

WHERE `PHOENIX_PASSWORD` is not set
THE SYSTEM SHALL allow all requests without authentication (current behavior)

**Rationale:** Developers in shared workspaces need a simple way to lock
their instance. When not in a shared environment, auth adds no value and
should not impose friction.

---

### REQ-AUTH-002: Credential Verification

WHEN a request presents a session-token cookie
THE SYSTEM SHALL grant access if the token is a currently-valid
server-issued session token

WHEN a request presents a password via the `Authorization: Bearer` header
THE SYSTEM SHALL compare it against `PHOENIX_PASSWORD` using constant-time
string comparison
AND grant access if the values match

THE SYSTEM SHALL rate-limit password comparisons per client peer address,
locking out a peer after repeated failures within a window; session-token
validation SHALL NOT be rate-limited

**Rationale:** Browsers hold an opaque server-issued session token, so the
password never rides in a cookie; non-browser API clients present the
password directly via Bearer. Throttling password comparisons (but not
token checks) per real peer address blocks brute-force guessing through any
password-checking endpoint without locking out legitimate token-bearing
clients.

---

### REQ-AUTH-003: Login Flow

WHEN an unauthenticated user navigates to any page
THE SYSTEM SHALL display a single-field password prompt instead of the app

WHEN the user submits the correct password
THE SYSTEM SHALL mint a session token, store it server-side, set it in a
persistent cookie, AND redirect to the originally requested page

WHEN the user submits an incorrect password
THE SYSTEM SHALL display an error message and remain on the login page

**Rationale:** The cookie carries a session token rather than the password,
and persists across page refreshes so the user enters the password once per
browser session, not on every page load. Tokens are cleared on restart;
browsers re-login transparently.

---

### REQ-AUTH-004: Share Token Creation

WHEN an authenticated owner navigates to `/share/c/{slug}`
THE SYSTEM SHALL look up the conversation by slug
AND if no share token exists, generate a random unguessable token and persist it
AND redirect to `/s/{token}`

WHEN a share token already exists for that conversation
THE SYSTEM SHALL redirect to `/s/{existing-token}`

**Rationale:** The owner's workflow is: copy the conversation URL, change
`/c/` to `/share/c/`, and send the resulting URL to coworkers. The redirect
produces a canonical share URL with a random token that is safe to share
publicly.

---

### REQ-AUTH-005: Read-Only Share View

WHEN a viewer navigates to `/s/{token}` with a valid share token
THE SYSTEM SHALL display the full conversation history
AND stream live updates via SSE (new messages, state changes)
AND NOT display any input controls, mutation buttons, or settings

WHEN a viewer navigates to `/s/{token}` with an invalid or revoked token
THE SYSTEM SHALL return HTTP 404

**Rationale:** Viewers get full context (all messages from the start) plus
live updates, making the share link useful for pair programming and demos.
No input controls means no ambiguity about what viewers can do.

---

### REQ-AUTH-006: Share Token Exemption from Auth

WHERE `PHOENIX_PASSWORD` is set
THE SYSTEM SHALL exempt `/s/{token}` routes from password authentication
AND validate only the share token itself

**Rationale:** The whole point of sharing is that coworkers access without
the password. The share token is the authorization -- it grants read-only
access to one specific conversation.

---

### REQ-AUTH-007: Multiple Simultaneous Viewers

WHEN multiple viewers connect to the same share URL simultaneously
THE SYSTEM SHALL serve each viewer independently
AND stream SSE events to all connected viewers

**Rationale:** Demos and team reviews involve multiple viewers watching
the same conversation. The SSE broadcast channel already supports multiple
subscribers.

---

### REQ-AUTH-008: Share Token Persistence

THE SYSTEM SHALL persist share tokens to the database
AND restore them on server restart

**Rationale:** Share tokens represent a user decision ("I want to share
this conversation"). They should survive server restarts so shared links
don't break unexpectedly.
