# Command Suggestion — Requirements

`phx` turns a natural-language request typed in a Phoenix terminal into
shell commands rendered as click-to-run links. A stateless one-shot LLM
call backs it; the user reviews and runs each command in their own shell.

Requirement IDs are `REQ-CSUG-*`.

---

### REQ-CSUG-001: One-Shot Suggestion Endpoint

WHEN a client POSTs a natural-language `query` to `/api/suggest`
THE SYSTEM SHALL perform a single non-streaming LLM completion with no tools
AND return the suggested shell commands as an ordered list, one command per
element
AND persist nothing: no conversation, no message, no transcript.

WHEN the model's output contains comment (`#`) lines, blank lines, or stray
markdown code fences
THE SYSTEM SHALL drop them, so every returned element is a runnable command
line.

WHEN the `query` is empty after trimming
THE SYSTEM SHALL reject the request with `400`.

**Rationale:** The endpoint is a suggester, not an agent. Tool-less by
construction, the model can only return text — it cannot execute anything
server-side. Statelessness keeps a throwaway "how do I…" out of conversation
history.

---

### REQ-CSUG-002: Model Selection

WHEN the request omits `model`
THE SYSTEM SHALL use a cheap/fast model (`ModelRegistry::get_cheap_model`).

WHEN the request supplies a `model` id
THE SYSTEM SHALL use that model, or reject with `500` when the id is unknown
to the registry.

**Rationale:** Suggestion is latency-sensitive and low-stakes; the cheap tier
is the right default, mirroring the title generator. An explicit override lets
the caller pin a model (`phx` reads `PHOENIX_SUGGEST_MODEL`).

---

### REQ-CSUG-003: Capability-Token Authorization

WHEN `/api/suggest` is requested
THE SYSTEM SHALL authorize it solely by the suggest capability token presented
in the `X-Phoenix-Suggest-Token` header
AND SHALL NOT require the master password (the endpoint is exempt from the
password middleware, see `specs/auth`).

WHEN the presented token is empty or does not equal the active suggest token
THE SYSTEM SHALL reject the request with `403`.

**Rationale:** The in-terminal `phx` holds the token but not the password. A
scoped capability (it grants only command suggestions) lets `phx` work whether
or not a master password is set, without widening the same-host auth surface:
possession of the token is the authorization, and a remote caller does not
have it.

---

### REQ-CSUG-004: Token Lifecycle

WHEN the server starts
THE SYSTEM SHALL resolve a suggest token that is stable across restarts: it
reuses the token persisted in `app_settings` when that token was minted under
the current password fingerprint, and otherwise mints a fresh token and
persists it together with the current fingerprint.

WHEN `PHOENIX_PASSWORD` changes (a different fingerprint)
THE SYSTEM SHALL mint a new token, invalidating the prior one.

**Rationale:** A terminal opened before a restart keeps the token in its
process environment. Reusing the persisted token keeps that terminal
authorized across a restart, while binding the token to the password
fingerprint — exactly as session tokens bind in `specs/auth` — makes rotating
the password revoke it. The token is stable configuration, not a session
secret that needs periodic rotation.

---

### REQ-CSUG-005: Guaranteed `phx` on the Terminal PATH

WHEN a terminal session is created
THE SYSTEM SHALL make a `phx` executable resolvable on the shell's `PATH`.

WHEN `phx` (or the binary under its `suggest` subcommand) is invoked
THE SYSTEM SHALL run the suggestion client and exit, rather than starting the
server.

**Rationale:** `phx` is a symlink to the already-running server binary,
materialized under the data directory and prepended to the terminal `PATH`.
One artifact that is guaranteed present (it is the binary already running),
with no dependency on an interpreter or package that could be absent — the
strongest reading of "available in every terminal."

---

### REQ-CSUG-006: PTY Environment Injection

WHEN a terminal PTY (or its backing tmux server) is spawned
THE SYSTEM SHALL inject into the child environment: the `phx` bin directory
prepended to `PATH`, `PHOENIX_API_URL` (the server's loopback origin), and
`PHOENIX_SUGGEST_TOKEN` (the active capability token).

THE injection SHALL be an enumerated, deliberate set — never blind inheritance
of the server's environment (see `specs/terminal` REQ-TERM-002).

**Rationale:** `phx` needs to know where to call and how to authorize. A tmux
pane shell inherits the tmux *server's* environment, so the injection is
applied both where the PTY child is the shell and where the PTY child is
`tmux attach` and the shell is a server pane (see `specs/tmux-integration`).

---

### REQ-CSUG-007: Click-to-Run Suggestion Links

WHEN `phx` prints a suggested command
THE SYSTEM SHALL emit it as an OSC 8 hyperlink whose URI is
`phxrun:<base64(command)>` and whose visible text is the command.

WHEN such a link is activated in the terminal UI
THE SYSTEM SHALL decode the command and place it on the shell prompt WITHOUT a
trailing newline — the user reviews it and presses Enter.

WHEN the decoded command contains a CR or LF
THE SYSTEM SHALL cut it at the first such character before placing it, so a
malformed or hostile link cannot submit the line.

THE SYSTEM SHALL NOT auto-execute a suggested command.

**Rationale:** Suggestion, not execution. Placing the command on the prompt
(rather than running it) keeps a human review beat in the loop and runs the
command in the user's own interactive shell. The OSC 8 carrier is rendered by
the terminal UI (see `specs/terminal-panel`); the tmux server forwards it (see
`specs/tmux-integration`).

---

### REQ-CSUG-008: Context Is Graded by Availability

THE suggestion request SHALL be answerable with zero conversation context (a
stateless one-shot completion).

**Rationale:** The endpoint sits at the base of a context spectrum: a stateless
one-shot answers "how do I…" questions with no history. Richer tiers —
retrieval-augmented suggestion scoped to a terminal's conversation lineage, or
a full agent turn — are additive on the same endpoint and `phx` carrier, and
are not part of the base contract. The endpoint shape (an optional model, a
token-gated stateless call) is chosen so those tiers bolt on without reshaping
it.
