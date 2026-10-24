# Simple Client

## User Story

As an LLM agent or developer, I need a simple command-line client to interact with the Phoenix API so that I can test and use the system without requiring the full React web UI.

## Requirements

### REQ-CLI-001: Single-Shot Execution

WHEN user runs the client with a message
THE SYSTEM SHALL send the message to the conversation
AND poll for completion
AND print the agent's response
AND exit

WHEN agent response includes tool use and results
THE SYSTEM SHALL display tool names, inputs, and outputs in readable format

**Rationale:** LLM agents work best with discrete command invocations rather than interactive sessions. Single-shot execution fits naturally into agent tool use patterns.

---

### REQ-CLI-002: Conversation Management

WHEN user specifies conversation ID or slug
THE SYSTEM SHALL continue that existing conversation

WHEN user specifies working directory without conversation
THE SYSTEM SHALL create a new conversation in that directory

WHEN neither is specified
THE SYSTEM SHALL use current working directory for new conversation

**Rationale:** Agents need to both start new conversations and continue existing ones.

---

### REQ-CLI-003: Image Support

WHEN user provides image file paths
THE SYSTEM SHALL read and base64-encode the images
AND include them in the message payload

WHEN image file cannot be read
THE SYSTEM SHALL exit with error before sending message

**Rationale:** Agents need to share screenshots and diagrams with Phoenix.

---

### REQ-CLI-004: Output Format

WHEN displaying agent response
THE SYSTEM SHALL format output for LLM comprehension:
- Clear section delimiters for message boundaries
- Tool use blocks with name and input
- Tool result blocks with output
- Final text response clearly marked

WHEN displaying errors
THE SYSTEM SHALL print to stderr with clear error indication

**Rationale:** Output must be easily parsed by LLM agents reading the response.

---

### REQ-CLI-005: Polling Behavior

WHEN waiting for agent completion
THE SYSTEM SHALL poll the conversation endpoint at reasonable interval
AND continue until conversation state is idle or error

WHEN conversation enters error state
THE SYSTEM SHALL display error message and exit with non-zero code

WHEN polling times out (configurable, default 10 minutes)
THE SYSTEM SHALL exit with timeout error

**Rationale:** Simple polling avoids SSE complexity while still providing completion detection.

---

### REQ-CLI-006: Configuration

WHEN API endpoint is needed
THE SYSTEM SHALL check in order:
1. `--api-url` command-line flag
2. `PHOENIX_API_URL` environment variable
3. Default to `http://localhost:8000`

WHEN conversation is specified
THE SYSTEM SHALL accept either:
- `--conversation` or `-c` flag with ID or slug
- `PHOENIX_CONVERSATION` environment variable

**Rationale:** Environment variables enable persistent configuration; flags enable per-invocation override.

---

### REQ-CLI-008: Model Selection

WHEN user specifies `--model` flag
THE SYSTEM SHALL pass the model ID to the create conversation request
AND the conversation SHALL use that model instead of the server default

WHEN user runs `--list-models`
THE SYSTEM SHALL query the API for available models
AND print each model's ID, provider, and description
AND exit without sending a message

WHEN `--model` is used with `--conversation` (continuing existing conversation)
THE SYSTEM SHALL ignore the `--model` flag (model is fixed at creation time)

**Rationale:** Agents and developers need to select specific models without using the web UI. Listing models enables discovery of what's available through the current LLM configuration.

---

### REQ-CLI-007: Single File Distribution

WHEN client is distributed
THE SYSTEM SHALL be a single Python file
AND use PEP 723 inline script metadata for dependencies
AND be runnable via `uv run client.py`

**Rationale:** Single file with inline deps maximizes portability and simplifies distribution.

---

### REQ-CLI-009: Interaction

WHEN a conversation is paused awaiting a user response
THE SYSTEM SHALL let the agent answer via `--respond KEY=VALUE` (repeatable)
AND let the agent dismiss the question via `--dismiss-question`

WHEN a conversation is in a user-resumable error state
THE SYSTEM SHALL let the agent dismiss the error via `--dismiss-error`

WHEN a steering message is queued
THE SYSTEM SHALL let the agent cancel it via `--cancel-steer MSG_ID`

WHEN a conversation is in a context-exhausted state
THE SYSTEM SHALL let the agent continue it via `--continue` with a handoff message
AND print the successor conversation's id and status

WHEN any interaction flag is used without `--conversation`
THE SYSTEM SHALL exit with a usage error before contacting the server

WHEN the conversation is not in the required state
THE SYSTEM SHALL surface the server's conflict message and exit non-zero

**Rationale:** An agent driving Phoenix needs to resolve the states that pause a conversation — questions, resumable errors, queued steering messages, and context exhaustion — not just send messages. Task and fork-proposal approval flows are out of scope for this requirement (see `executive.md` for the deferral note).

---

### REQ-CLI-010: Introspection

WHEN the agent passes `--conversation <conv>` with a read-only introspection flag
THE SYSTEM SHALL fetch and print the corresponding server projection:
- `--diff`: worktree diff against the base branch
- `--git-status`: git status snapshot
- `--usage`: token usage totals
- `--system-prompt`: resolved system prompt
- `--tasks`: task files in the conversation's working directory
- `--proposals`: fork proposals for the conversation

WHEN an introspection flag is used without `--conversation`
THE SYSTEM SHALL exit with a usage error before contacting the server

**Rationale:** An agent that just sent a message often needs to inspect what changed (diff, git status) or the conversation's configuration (system prompt, model usage) before deciding what to do next. Surfacing these as structured output avoids shelling out to `git` in the worktree, which the client never exposes.

---

### REQ-CLI-011: Discovery

WHEN the agent runs `--list-conversations`
THE SYSTEM SHALL list all non-archived conversations and exit

WHEN the agent runs `--search-conversations QUERY`
THE SYSTEM SHALL search conversation contents and print matching hits
AND accept `--search-limit N` to cap the number of hits

**Rationale:** An agent that wants to continue a prior conversation by topic rather than by memorized slug needs a way to find it by content. Listing and search let the agent discover conversations by content.

---

### REQ-CLI-012: Platform & Config

WHEN the agent runs a platform/config flag
THE SYSTEM SHALL fetch and print the corresponding read-only server state and exit:
- `--version`: server build version and git SHA
- `--deployment`: deployment info (build, network, TLS, resources, disk)
- `--env`: server environment info
- `--mcp-status`: status of all connected MCP servers
- `--usage-overview`: aggregate token usage across all conversations
- `--trajectory-export` (requires `--conversation`): full trajectory export

**Rationale:** An agent debugging a connection, checking MCP server health, or inspecting aggregate usage needs read-only visibility into the running system without the web UI. These diagnostics let the agent observe build identity, deployment posture, environment, MCP health, and token usage.


---

### REQ-CLI-013: Owner-mediated peer operations

WHEN an agent selects a saved peer using `--instance` or a typed remote client method
THE SYSTEM SHALL require a nonempty UUID or unambiguous saved display name
AND invoke the authenticated local owner API, never the destination's peer-authenticated API directly
AND leave destination bearer credentials and private-CA material on the owner server
AND fail on an unknown, ambiguous, unavailable, or malformed destination without local fallback.

THE SYSTEM SHALL expose typed methods `list_peer_connections`, `remote_query_database`,
`remote_search_messages`, `remote_read_conversation`, `remote_resolve_reference`, and
`remote_send_conversation_message`, with matching CLI actions `--list-peer-connections`,
`--query-database SQL`, `--search-messages QUERY`, `--read-conversation TARGET_JSON`,
`--resolve-reference REFERENCE`, and `--send-conversation-message TARGET_JSON`.

WHEN a remote action returns data
THE SYSTEM SHALL validate response discriminants, required fields, qualified instance identities,
and echoed request identities before returning or emitting JSON
AND preserve typed query cells and qualified conversation references without flattening them into prose.

WHEN sending to a peer
THE SYSTEM SHALL require a caller-supplied send UUID (`--message-id` on the CLI)
AND preserve that UUID in the request
AND make only one send attempt
AND on an uncertain outcome instruct the caller to reuse the same UUID and payload, never generate a replacement UUID or fall back to local send.

THE SYSTEM SHALL keep local `--search-conversations` and the local single-shot send/wait flow distinct from peer message search and peer send receipts.

WHEN `--instance` accompanies an unsupported local action, or conflicting actions/options are supplied
THE SYSTEM SHALL fail before dispatch rather than silently ignoring either selection.

WHEN accessing authentication or reporting federation errors
THE SYSTEM SHALL accept the owner password only through `PHOENIX_PASSWORD` or a hidden prompt
AND expose no destination-token or private-CA command-line options
AND report static error diagnostics without echoing server error bodies, request URLs, or transport exception text.

WHEN a peer response includes a server filesystem location
THE SYSTEM SHALL preserve it as an opaque server-owned handle
AND SHALL NOT test its local existence, expand `~`, open it, or reveal it on the client's host.

---

### REQ-CLI-014: Safe peer notes

THE SYSTEM SHALL expose `get_peer_notes` and `set_peer_notes`, with `--get-peer-notes` and
`--set-peer-notes TEXT` requiring `--instance`.

WHEN replacing notes
THE SYSTEM SHALL send only the `notes` text field to the local owner's notes route
AND preserve whitespace, Unicode, newlines, and empty text without trimming or truncation
AND enforce the 32768-byte UTF-8 bound
AND SHALL NOT treat notes as authentication, connection configuration, or sender authority.

---

### REQ-CLI-015: Remote subscription management

THE SYSTEM SHALL expose `remote_watch_conversation(product_id, instance)`,
`list_remote_subscriptions()`, `sync_remote_subscription(id)`, and
`remote_unwatch_subscription(id)`, with matching `--watch-conversation PRODUCT_ID`,
`--list-remote-subscriptions`, `--sync-remote-subscription UUID`, and
`--unwatch-remote-subscription UUID` CLI actions.

WHEN creating a watch
THE SYSTEM SHALL require an explicit peer and stable ProductConversation ID
AND reuse an existing active watch's subscription UUID
AND reject an unconfirmed unsubscribe rather than creating a competing watch.

WHEN syncing or stopping a subscription by UUID
THE SYSTEM SHALL derive its peer and ProductConversation identity from the owner's validated saved subscription list
AND reject a missing or ambiguous UUID before mutation
AND never infer a local conversation from a remote subscription ID.

WHEN reporting unsubscribe results
THE SYSTEM SHALL retain the distinction between local delivery stopped and destination unsubscribe confirmed.

---

### REQ-CLI-016: Federation API reference

THE SYSTEM SHALL expose `get_openapi(instance=None)` and `--openapi` for the local federation API reference.

WHEN `--openapi --instance INSTANCE` is selected
THE SYSTEM SHALL fetch the selected peer's reference through the local owner's OpenAPI facade
AND verify both the destination envelope identity and the document's `x-phoenix-instance-id`
AND SHALL NOT fetch arbitrary peer URLs directly or substitute the local document on failure.

THE SYSTEM SHALL identify this reference as the federation API subset, not the complete Phoenix API.
