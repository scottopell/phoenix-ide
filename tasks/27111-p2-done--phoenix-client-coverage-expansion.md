# Expand phoenix-client.py API coverage

## Summary

Add four families of missing API surface to `phoenix-client.py`: conversation
 discovery (list + search), conversation introspection (diff, git-status,
 usage, system-prompt, tasks, proposals), agent-in-the-loop interaction
 (user question respond/dismiss, error dismiss, steering cancel), and
 read-only platform/config endpoints (version, deployment, env, mcp status,
 usage analytics, trajectory export).

Excluded per refactor: archive, delete, rename, cancel, continue,
 upgrade-model, regenerate-name, mark-merged, and the "continued conversation"
 concept — all being reworked around transcripts/compaction.

## Plain-English summary of what makes sense to support

The client today can drive one conversation: create it, send a message,
 stream the reply, and exit. It cannot answer back when Phoenix asks a
 question, inspect what changed in the worktree, or find an existing
 conversation by searching its contents. An LLM agent using the client is
 stuck in a send-only loop with no visibility into state or history beyond
 the current reply.

The additions fall into four groups, in rough priority order:

**1. Interaction — let the agent answer back.** When Phoenix pauses for a
 question, the client should be able to resolve it: answer or dismiss a user
 question, dismiss a resumable error, and cancel a queued steering message.
 These are all `POST`/`DELETE` calls gated by conversation state, so the
 client just needs to send the request and report success or the server's
 conflict message. Task and fork-proposal approval flows are deliberately
 left out for now pending the transcript refactor; they can be added once the
 lifecycle model settles.

**2. Introspection — let the agent see what it changed.** Read-only `GET`s
 that surface the worktree diff, git status, token usage, resolved system
 prompt, task files, and fork proposals for the current conversation. An
 agent that just sent a message often wants to check the diff or git status
 before deciding what to do next; today it must shell out to `git` in the
 worktree, which the client never exposes. These endpoints return structured
 JSON the client can print in the same delimited format it already uses for
 messages.

**3. Discovery — let the agent find conversations.** `GET /api/conversations`
 lists all conversations; `GET /api/conversations/search?q=…` searches their
 contents. An agent that wants to continue a prior conversation by topic
 rather than by memorized slug has no way to find it today. Listing is a flat
 dump; search returns hits with slug, snippet, and score, which the client can
 print as a compact table.

**4. Platform & config — let the agent inspect the running system.**
 Read-only `GET`s for version, deployment info, environment, MCP status,
 aggregate usage, per-conversation usage, and trajectory export. These are
 diagnostics: an agent debugging a connection or checking MCP server health
 can use them, and they're cheap to add since they're plain `GET`s with no
 request body.

## Endpoint list

### Interaction (`-c <conv>` required)

| Flag | Method + path | Body |
|---|---|---|
| `--respond QUESTION=ANSWER…` | `POST /api/conversations/:id/respond` | `{answers, annotations?}` |
| `--dismiss-question` | `POST /api/conversations/:id/dismiss-question` | — |
| `--dismiss-error` | `POST /api/conversations/:id/dismiss-error` | — |
| `--cancel-steer MSG_ID` | `DELETE /api/conversations/:id/steering-queue/:message_id` | — |

### Introspection (`-c <conv>` required, read-only)

| Flag | Method + path |
|---|---|
| `--diff` | `GET /api/conversations/:id/diff` |
| `--git-status` | `GET /api/conversations/:id/git-status` |
| `--usage` | `GET /api/conversations/:id/usage` |
| `--system-prompt` | `GET /api/conversations/:id/system-prompt` |
| `--tasks` | `GET /api/conversations/:id/tasks` |
| `--proposals` | `GET /api/conversations/:id/proposals` |

### Discovery (no conversation required)

| Flag | Method + path |
|---|---|
| `--list-conversations` | `GET /api/conversations` |
| `--search-conversations QUERY` | `GET /api/conversations/search?q=…&limit=…` |

### Platform & config (read-only)

| Flag | Method + path |
|---|---|
| `--version` | `GET /api/version` |
| `--deployment` | `GET /api/deployment` |
| `--env` | `GET /api/env` |
| `--mcp-status` | `GET /api/mcp/status` |
| `--usage-overview` | `GET /api/usage` |
| `--trajectory-export` | `GET /api/analytics/conversation/:id/trajectory-export` (requires `-c`) |

## Implementation notes

- All new methods hang off `PhoenixClient` as thin wrappers following the
  existing `get_models` / `get_projects` pattern: build the URL, call
  `self.http`, `raise_for_status`, return JSON.
- New `click` flags are boolean/value options gated on `--conversation`
  where a conversation is required; `--conversation`-less calls raise
  `click.UsageError` early, mirroring the `--wake-status` guard.
- Output uses the existing `===`/`---` delimited style for structured
  payloads (diff, git-status, proposals, tasks) and compact tables for
  lists (conversations, search hits). JSON payloads with nested objects are
  pretty-printed when a human is the audience and compact when piped, but
  that distinction is already handled by the existing `format_response`
  approach — reuse it.
- `--respond` takes `KEY=VALUE` pairs (multiple) like `--respond q1=yes
  --respond q2=no`, collected into a dict; `annotations` is optional and
  omitted for v1.
- No new dependencies: `httpx` + `click` already cover everything.
- Spec impact: `specs/simple_client/requirements.md` gains new REQ-CLI-009
  through REQ-CLI-012 (approval, introspection, discovery, platform); the
  executive table grows accordingly. The refactor exclusion is noted in the
  executive so future readers know lifecycle ops were deliberately deferred.

## Out of scope (refactor in flight)

- archive / delete / rename / cancel / continue / upgrade-model /
  regenerate-name / mark-merged
- the "continued conversation" concept and any endpoint that assumes it
- files/directory, skills, chains, terminal/browser WebSockets, work-scope,
  bash inspect, credential helper, release-updates, share mode, Codex login,
  telemetry, `--list-archived`, `--list-tasks` (project-level), `--projects`
  (already covered)

## Verification

- `uv run phoenix-client.py --list-conversations` prints a table.
- `uv run phoenix-client.py --search-conversations "login bug"` prints hits.
- `uv run phoenix-client.py -c <slug> --diff` prints the worktree diff.
- `uv run phoenix-client.py -c <slug> --respond q1=yes` resolves an
  `AwaitingUserResponse` state.
- `uv run phoenix-client.py --version --deployment --env` prints each.
- `./dev.py check` passes (client is not in the Rust build, but the spec
  changes may touch `specs/simple_client/` which `check` validates).
