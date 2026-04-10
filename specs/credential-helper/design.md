# Credential Helper -- Design

## Behavioral Specification

The complete behavioral contract is defined in
`specs/credential-helper/credential-helper.allium`. This document covers
implementation choices; the Allium spec is authoritative for what the system
does.

---

## Backend

### CommandCredential Changes (REQ-CREDHELPER-001)

Current `CommandCredential::get()` captures `stdout.trim()` as the token.
Change to extract only the last non-empty line:

```rust
let token = String::from_utf8_lossy(&out.stdout)
    .lines()
    .filter(|l| !l.trim().is_empty())
    .last()
    .map(str::to_string);
```

This is a safe behavior change: any helper that currently works produces
a single output line, so last-line == full trim. Helpers whose full stdout
blob happened to be used as a token were already broken (no API accepts a
multi-line blob as a key).

### HelperState in AppState (REQ-CREDHELPER-003, -004)

Add `helper_state: Arc<HelperState>` to `AppState`.

```rust
pub struct HelperState {
    inner: TokioMutex<HelperInner>,
}

enum HelperInner {
    Idle,
    Running {
        lines_so_far: Vec<String>,           // replay buffer
        subscribers: Vec<mpsc::Sender<HelperEvent>>,
    },
    Valid {
        credential: String,
        expires_at: Instant,
    },
    Failed {
        exit_code: Option<i32>,
        stderr: String,
    },
}
```

`HelperState` is initialized from `LlmConfig` at startup. If no helper is
configured, the state is permanently `Idle` and `POST /api/credential-helper/run`
returns 404.

The existing `CommandCredential` path (used for lazy LLM requests) is
replaced by querying `HelperState::get_credential()` which reads from the
`Valid` cache — no subprocess spawn in the hot path.

### POST /api/credential-helper/run (REQ-CREDHELPER-003, -004)

Returns an SSE stream. On call:

1. Lock `HelperInner`.
2. If `Running`: subscribe to broadcast, replay buffered lines, unlock, stream.
3. If `Valid` (and not expired): emit one `complete` event, close stream.
4. If `Idle` or `Failed`: transition to `Running { lines_so_far: [], subscribers: [tx] }`,
   unlock, spawn helper task.

Helper task:
- Spawns `sh -c $LLM_API_KEY_HELPER` with `stdout: piped`, `stderr: piped`
- Reads stdout line-by-line via `tokio::io::BufReader::lines()`
- For each line: appends to `lines_so_far`, fans out to all subscriber channels
- On EOF + exit 0: last non-empty line → credential; transition to `Valid`; broadcast `complete`
- On EOF + exit non-zero: transition to `Failed`; broadcast `error`

### GET /api/models Extension (REQ-CREDHELPER-002)

Add `credential_status: CredentialStatusApi` to `ModelsResponse`:

```rust
#[serde(rename_all = "snake_case")]
pub enum CredentialStatusApi {
    NotConfigured,
    Valid,
    Required,
    Running,
    Failed,
}
```

Logic:
- No helper configured + no static/env key → `NotConfigured`
- No helper configured + key present → `Valid`
- Helper configured + `HelperInner::Valid` with unexpired TTL → `Valid`
- Helper configured + `HelperInner::Idle` → `Required`
- Helper configured + `HelperInner::Running` → `Running`
- Helper configured + `HelperInner::Failed` → `Failed`
- Helper configured + `HelperInner::Valid` but expired → `Required` (transition on read)

### Credential Invalidation (REQ-CREDHELPER-007)

`HelperState` implements `CredentialSource`:
- `get()` checks `HelperInner::Valid` and TTL; returns credential if valid, else `None`
- `invalidate()` transitions `Valid → Idle`; returns `true` if a credential was cached

The existing 401-retry path in `LlmService` calls `auth.invalidate()` — this
now transitions `HelperState` to `Idle`, which surfaces as `required` on the
next `/api/models` poll.

---

## Frontend

### Auth Indicator (REQ-CREDHELPER-008)

Add `credentialStatus` to the models API response type in `api.ts`.

In the main layout (sidebar or header), render a credential status chip:

| Status | Display |
|--------|---------|
| `valid` | `AUTH ✓` (green, no click action) |
| `running` | `AUTH ...` (muted, opens panel to watch) |
| `required` / `failed` | `AUTH ✗` (red, click opens auth panel) |
| `not_configured` | hidden (no helper, no key — existing `llm_configured: false` banner handles this) |

The chip polls via the existing models polling interval.

### Auth Panel (REQ-CREDHELPER-008)

Opens as a modal or slide-over when the `AUTH ✗` chip is clicked.

Contents:
- Header: `Authenticate` + close button
- Command display: first word of `LLM_API_KEY_HELPER` only (e.g., `uv`) — rest
  redacted to avoid leaking tokens or internal tool names
- Output box: monospace, scrolling, auto-scrolls to bottom as lines arrive
- Status: spinner while running; green checkmark on `complete`; red error on `error`

On open: immediately calls `POST /api/credential-helper/run` and streams SSE
events into the output box. If the run is already in progress, the user joins
mid-stream (replay + live).

On `complete` event: close button changes to `Done`, output box shows final
success state. The auth chip updates to `AUTH ✓` on the next poll.

---

## Security Notes

- The helper command is configured server-side via env var and never exposed in
  full to the browser. Only the first word (the executable name) is shown.
- Stdout lines are forwarded verbatim to the browser. The helper should not emit
  secrets other than the final token line. The token line itself is NOT forwarded
  to the browser via SSE — only the preceding instruction lines are.
- `HelperInner::Valid.credential` is in process memory only. It is not persisted
  to disk or the database.
