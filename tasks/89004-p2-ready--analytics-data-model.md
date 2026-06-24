# Phoenix Analytics Data Model and Trajectory-Compatible Export

## Problem statement

Phoenix already has a useful `/usage` page backed by persisted token usage, but it does not yet have a broader analytics model for understanding how Phoenix conversations behave, where time/tokens are spent, which tools fail or get denied, and which conversations produce outcomes such as commits or PRs.

We initially explored this through the lens of integrating `~/go/src/github.com/DataDog/trajectory`, especially `docs/CLIENT-INSTRUMENTATION.md`. That showed that Trajectory captures agent sessions through client-specific hooks/plugins, normalizes them into local JSONL, indexes them into a local SQLite cache, and can publish metrics/markers to Datadog. However, Phoenix should not make Trajectory the source of truth for Phoenix analytics.

The design direction is now:

> Phoenix owns its local analytics model. Existing Phoenix conversation history and usage data remain the source of truth. Trajectory is an optional downstream export adapter that can be constructed on demand from Phoenix history and Phoenix analytics facts.

This task is to formalize and implement the Phoenix-native analytics data model that covers the existing `/usage` page and creates room for future analytics surfaces. It should make the Trajectory-compatible export path possible without making Phoenix depend on the Trajectory daemon or store a second transcript.

## Non-goals

- Do not implement Trajectory-style hook capture inside Phoenix. Phoenix owns its runtime and can derive analytics from its own persisted data.
- Do not store a second full transcript or JSONL event log solely for analytics.
- Do not make `/usage` depend on Trajectory, Datadog, or `localhost:19222`.
- Do not add generic schemaless analytics blobs for facts that have clear relational shape.
- Do not make full prompt/response/tool-output content first-class analytics rows. That content already lives in Phoenix `messages`.

## Current Phoenix data available

Phoenix already persists most source data needed for analytics.

### Conversations

The `conversations` table provides:

- conversation/session id
- slug/title
- cwd
- parent conversation id
- user-initiated flag
- state and `state_updated_at`
- created/updated timestamps
- archived flag
- model
- project id
- normalized conversation mode fields such as:
  - kind
  - branch name
  - worktree path
  - base branch
  - task id
  - task title
  - next taskmd id hint
- continuation pointer
- chain name
- seed metadata
- spawned-from metadata

This is enough to derive session identity, project/worktree/task attribution, root/sub-agent grouping, model-at-session metadata, rough session start/end, and current/terminal state.

### Messages

The `messages` table provides:

- message id
- conversation id
- sequence id
- message type
- content JSON
- display data
- usage data
- created timestamp

Typed message content includes:

- user text and attachments
- skill invocations
- assistant content blocks
- assistant tool-use blocks with `id`, `name`, and `input`
- tool result messages with `tool_use_id`, `content`, `is_error`, images, and display data
- server-side tool blocks such as web search/fetch/code execution results where providers emit them

This is enough to reconstruct most prompt, response, tool-call, and tool-result relationships.

### Attachments

User/skill attachments are normalized into child tables:

- `message_files`
- `message_images`

Analytics should reference source messages rather than copy attachment contents.

### Token usage

The `turn_usage` table currently provides:

- conversation id
- root conversation id
- model
- input tokens
- output tokens
- cache creation tokens
- cache read tokens
- created timestamp

The existing `/usage` page uses these rows to build:

- total token windows
- daily token series
- by-model totals
- by-provider totals
- by-project totals
- by-conversation totals
- per-conversation turn series
- token-per-turn histogram
- cache hit rate

### Tool duration and deterministic denial information

Tool execution duration is merged into persisted tool-result display data as `duration_ms` when available.

Phoenix's deterministic deny gate returns structured tool-result errors with shape like:

```json
{
  "error": "command_safety_rejected",
  "error_message": "...",
  "reason": "..."
}
```

Those denied calls are reconstructable from persisted tool results, though a typed projection should expose them as denials rather than requiring every analytics consumer to parse arbitrary JSON.

## Trajectory-like data needs

Trajectory's docs point to several classes of analytics data that Phoenix may want to support locally or export downstream.

### Turn-level needs

- turn count
- turn index
- turn status
- model
- token counts
- token provenance/status
- estimated cost
- cost provenance
- completed-turn duration
- first-byte latency
- tool uses per turn
- tool uses by tool name
- failed tool count/category
- denied tool count
- web search request/cost count when available
- permission wait duration when applicable
- duration excluding permission wait when applicable

### Session-level needs

- session duration
- total turns
- total tool uses
- total estimated cost
- running/final last-seen timestamp
- compaction/context-management count where meaningful
- sub-agent count
- lines changed
- commit/yield count
- PR count
- revert count
- task/project/worktree/branch attribution

### Marker/outcome needs

- commits
- PRs/MRs
- pushes and force pushes
- permission denials
- tool errors
- user interruptions/cancellations
- language activity
- CLI tool count
- test-fix cycles
- CI feedback cycles
- cost attribution to commits/PRs/tasks

### Fidelity needs

Trajectory explicitly distinguishes fields such as:

- `cost_source`
- `token_source`
- `tokens_status`

Phoenix analytics should likewise report whether important values are native, derived, estimated, unknown, or unavailable.

## Core design principles

### Phoenix history is the source of truth

Analytics projections are derived from Phoenix conversations, messages, attachments, and typed durable facts. Phoenix should not create a second transcript store.

### `/usage` becomes the first analytics surface

The existing `/usage` page remains in scope. The analytics data model must either preserve its current behavior or provide a clear migration path that improves the implementation without regressing the UI.

### Durable facts only where history is insufficient

If a value can be reconstructed from existing persisted history, do not add another persisted representation. If a value cannot be reconstructed later and matters to analytics, persist a small typed fact.

### Export is a projection, not a capture path

Trajectory-compatible export should be built from Phoenix analytics projections on demand. Export can write JSONL, POST to a local Trajectory daemon, or return debug JSON, but it is not the authoritative data path.

### Fidelity is explicit

Any projection/export field that is estimated, derived, unavailable, or only partially reconstructable must say so structurally. Do not silently fabricate exact-looking data.

## Proposed data model

The v1 model should stay intentionally lean. The latest `/usage` cost work already derives estimated cost from `turn_usage.model` plus token counts at presentation time. That is the right precedent: cost is a projection, not persisted analytics state.

For v1:

- Keep `turn_usage` as the canonical token-bearing LLM turn table.
- Do not add a parallel `conversation_turns` table.
- Do not persist `estimated_cost_usd`, `cost_source`, `token_source`, or `tokens_status`.
- Do persist `first_byte_at` on `turn_usage`, because Phoenix already observes that fact in the runtime and currently only exposes it ephemerally through SSE.
- Build an analytics projection layer over existing `conversations`, `messages`, attachments, `turn_usage`, and derived pricing/tool facts.

The physical table name `turn_usage` becomes slightly historical, but not misleading: it still contains turn usage rows, and `first_byte_at` is timing metadata for those rows.

### Required v1: extend `turn_usage` with `first_byte_at`

Add a nullable `first_byte_at` column:

```sql
ALTER TABLE turn_usage ADD COLUMN first_byte_at TEXT;
```

Semantics:

- `first_byte_at` is the server timestamp at which the first streamed token for the LLM request was observed.
- It should be populated for new streaming turns when Phoenix emits the existing `LlmFirstByte` SSE event.
- It is `NULL` for historical turns, non-streaming paths, and turns where no first byte was observed before failure/cancellation.
- It is analytics metadata for latency calculations; it does not affect conversation correctness.

Do not persist a separate `first_byte_ms` value. Derive latency as needed from `first_byte_at` and an available turn start/anchor timestamp.

### V1 source-of-truth tables

V1 analytics reads from existing source tables:

- `conversations`
- `messages`
- `message_files`
- `message_images`
- `turn_usage` including the new nullable `first_byte_at`

Cost remains derived from:

- `turn_usage.model`
- `turn_usage.input_tokens`
- `turn_usage.output_tokens`
- `turn_usage.cache_creation_tokens`
- `turn_usage.cache_read_tokens`
- the static/current model pricing lookup used by `/api/usage`

Tool facts remain derived from messages:

- Assistant `ToolUse` blocks provide `tool_use_id`, tool name, and input.
- Tool result messages provide result content, error status, images, display data, and `duration_ms` when present.
- Deterministic permission denials are detected from the structured tool-result error/display data (`error = "command_safety_rejected"`).

### Code-level projection records

Add an internal projection module. It does not need to persist rows for v1; it gives `/usage`, future analytics UI, and export adapters a shared interpretation of the source data.

Suggested Rust-level shapes:

```rust
pub struct AnalyticsSession {
    pub session_id: String,
    pub root_session_id: String,
    pub project_id: Option<String>,
    pub cwd: String,
    pub worktree_path: Option<String>,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub branch: Option<String>,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub terminal_status: Option<String>,
    pub turns: Vec<AnalyticsUsageTurn>,
    pub tool_calls: Vec<AnalyticsToolCall>,
    pub fidelity: AnalyticsFidelity,
}
```

```rust
pub struct AnalyticsUsageTurn {
    pub turn_usage_id: i64,
    pub conversation_id: String,
    pub root_conversation_id: String,
    pub model: String,
    pub created_at: DateTime<Utc>,
    pub first_byte_at: Option<DateTime<Utc>>,
    pub tokens: TokenTotals,
    pub cost: TurnCost,
}
```

```rust
pub struct AnalyticsToolCall {
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub tool_result_message_id: Option<String>,
    pub tool_use_id: String,
    pub tool_name: String,
    pub is_error: bool,
    pub denied: bool,
    pub duration_ms: Option<u64>,
    pub normalized_command: Option<String>,
    pub touched_files: Vec<String>,
}
```

Important: `AnalyticsToolCall` references source messages rather than copying full input/output text into analytics storage.

### Deferred durable facts

The following tables are explicitly deferred. Add them only when a concrete analytics surface requires data that cannot be reconstructed from `turn_usage` and `messages`.

#### Deferred: `llm_attempts`

Persist LLM attempt/retry facts only when retry analytics become first-class.

Potential shape:

```sql
CREATE TABLE llm_attempts (
    id TEXT PRIMARY KEY,
    turn_usage_id INTEGER NOT NULL REFERENCES turn_usage(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,

    attempt_number INTEGER NOT NULL,
    max_attempts INTEGER NOT NULL,

    started_at TEXT NOT NULL,
    completed_at TEXT,

    outcome TEXT NOT NULL,
    retryable BOOLEAN NOT NULL DEFAULT 0,
    backing_off_ms INTEGER,
    resets_at TEXT,
    error_kind TEXT,

    CHECK (outcome IN ('success', 'rate_limit', 'server_error', 'network_error', 'cancelled', 'unknown'))
);

CREATE INDEX idx_llm_attempts_turn
    ON llm_attempts(turn_usage_id, attempt_number);
```

Do not add this in v1 unless the task expands to include retry dashboards or export fidelity that requires durable retry details.

#### Deferred: turn lifecycle status

Do not add `status`, `started_at`, `completed_at`, `trigger_message_id`, `assistant_message_id`, or `turn_index` to `turn_usage` in v1. Those fields become useful if Phoenix starts recording failed/cancelled/non-token-bearing turns as rows. The current `/usage` and cost surfaces only require completed token-bearing turns.

#### Deferred: `conversation_interruptions`

Persist user/system interruption facts only if cancellation/interruption analytics become a first-class surface.

Potential shape:

```sql
CREATE TABLE conversation_interruptions (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    turn_usage_id INTEGER REFERENCES turn_usage(id) ON DELETE SET NULL,

    kind TEXT NOT NULL,
    actor TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    completed_at TEXT,
    reason TEXT,

    CHECK (kind IN ('user_cancel', 'hard_delete', 'model_upgrade_abort', 'server_recovery', 'tool_cancel')),
    CHECK (actor IN ('user', 'system', 'runtime'))
);
```

#### Deferred: `conversation_git_yields`

Persist git/PR outcome facts only when outcome attribution becomes a first-class analytics surface.

Potential shape:

```sql
CREATE TABLE conversation_git_yields (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    root_conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    turn_usage_id INTEGER REFERENCES turn_usage(id) ON DELETE SET NULL,

    kind TEXT NOT NULL,
    observed_at TEXT NOT NULL,

    repo_root TEXT NOT NULL,
    branch TEXT,
    commit_sha TEXT,
    parent_sha TEXT,
    pr_number INTEGER,
    pr_url TEXT,

    lines_added INTEGER,
    lines_deleted INTEGER,
    files_changed INTEGER,
    reachable_from_main BOOLEAN,

    source TEXT NOT NULL,

    CHECK (kind IN ('commit', 'push', 'pr', 'revert')),
    CHECK (source IN ('git_scan', 'gh_cli', 'marker_inferred'))
);
```

#### Deferred: `analytics_privacy_events`

Persist export/privacy toggle facts only if Phoenix gains incognito or publish-suppression semantics.

Potential shape:

```sql
CREATE TABLE analytics_privacy_events (
    id TEXT PRIMARY KEY,
    conversation_id TEXT REFERENCES conversations(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL,
    changed_at TEXT NOT NULL,
    source TEXT NOT NULL,
    scope TEXT NOT NULL,

    CHECK (source IN ('user', 'config', 'api')),
    CHECK (scope IN ('conversation', 'workspace', 'global'))
);
```

This is not required for local-only analytics.

## Analytics projection layer

Add an internal projection module that reads source data and produces typed analytics records. The projection is the shared source for `/usage`, future analytics UI, and export adapters.

Suggested Rust-level shapes:

```rust
pub struct AnalyticsSession {
    pub session_id: String,
    pub root_session_id: String,
    pub project_id: Option<String>,
    pub cwd: String,
    pub worktree_path: Option<String>,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub branch: Option<String>,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub terminal_status: Option<String>,
    pub turns: Vec<AnalyticsTurn>,
    pub tool_calls: Vec<AnalyticsToolCall>,
    pub fidelity: AnalyticsFidelity,
}
```

```rust
pub struct AnalyticsTurn {
    pub turn_id: String,
    pub turn_index: i64,
    pub conversation_id: String,
    pub model: String,
    pub status: TurnStatus,
    pub started_at: DateTime<Utc>,
    pub first_byte_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub first_byte_ms: Option<u64>,
    pub tokens: TokenTotals,
    pub estimated_cost_usd: Option<f64>,
    pub token_source: TokenSource,
    pub tokens_status: TokensStatus,
    pub cost_source: CostSource,
    pub tool_count: usize,
    pub failed_tool_count: usize,
    pub denied_tool_count: usize,
}
```

```rust
pub struct AnalyticsToolCall {
    pub turn_id: String,
    pub tool_use_id: String,
    pub tool_name: String,
    pub input_message_id: String,
    pub result_message_id: Option<String>,
    pub is_error: bool,
    pub denied: bool,
    pub duration_ms: Option<u64>,
    pub normalized_command: Option<String>,
    pub touched_files: Vec<String>,
}
```

Important: `AnalyticsToolCall` references source messages rather than copying full input/output text into analytics storage.

## `/usage` migration design

The existing `/usage` endpoints are already analytics queries over `turn_usage`, and the recent cost work extends them with derived estimated cost. V1 should preserve that direction rather than replacing it with a new table.

- `GET /api/usage` remains an aggregate query over `turn_usage`, enriched by shared analytics/pricing projection helpers.
- `GET /api/usage/conversation/:id` remains a per-root-session turn timeline over `turn_usage`, enriched with derived cost and `first_byte_at` when available.
- Current response fields should remain stable unless intentionally updated with generated TypeScript schema changes.

Current `/usage` behavior that must be preserved:

- all-time/rolling-window token totals
- daily token breakdown
- model/provider breakdowns
- project breakdowns
- conversation breakdowns
- per-conversation cumulative token series
- token-per-turn histogram
- cache hit rate
- estimated cost and unknown-pricing warnings from the current cost implementation

New fields can be added later:

- first-byte latency
- failed/denied tool counts
- retry counts
- duration percentiles
- outcome attribution

Do not add persisted cost/provenance columns for v1. Cost fidelity is already represented in the API as `pricing_known` and `unknown_turns`; export adapters can derive `token_derived` vs `pricing_unknown` from the same information.

## Trajectory-compatible export design

Add a Trajectory-compatible export adapter as part of v1. This consumer should drive acceptance testing for the analytics projection: if the projection cannot produce a useful Trajectory-compatible session export from Phoenix history, the analytics model is not concrete enough.

The export adapter is still a projection, not the source of truth. It must not require Trajectory to be installed for Phoenix analytics or `/usage` to work.

Inputs:

- conversations
- messages
- message attachment tables
- `turn_usage` including `first_byte_at`
- optional future retry/yield/interruption/privacy facts

Outputs:

- debug JSON for tests/development
- canonical JSONL-compatible export payload for one conversation/root session
- optional POST to local Trajectory capture server if configured and available

The export must include fidelity metadata for fields that are reconstructed, estimated, or unavailable. Example:

```json
{
  "client": "phoenix",
  "session_id": "...",
  "source": "phoenix_conversation_history",
  "fidelity": {
    "tokens": "native_for_token_bearing_turn_usage_rows",
    "cost": "token_derived_or_pricing_unknown",
    "tool_calls": "derived_from_messages",
    "first_byte": "native_for_new_streaming_rows_unavailable_for_historical_rows",
    "retries": "unavailable_until_retry_facts_exist"
  }
}
```

## Requirements to add to `specs/analytics/requirements.md`

### REQ-AN-001: Local Analytics Independence

Phoenix SHALL provide local analytics without requiring Trajectory, Datadog, or any external daemon.

### REQ-AN-002: Usage Page Preservation

Phoenix SHALL preserve the existing `/usage` page semantics while migrating it onto the analytics projection model.

### REQ-AN-003: Conversation History Source of Truth

Phoenix SHALL derive analytics from persisted Phoenix conversation history and typed durable facts.

### REQ-AN-004: No Duplicate Transcript Store

Phoenix SHALL NOT persist a second full transcript or full tool I/O copy solely for analytics.

### REQ-AN-005: First-Byte Durability

Phoenix SHALL persist the server timestamp of the first streamed LLM token for token-bearing turns when that timestamp is observed.

### REQ-AN-006: Tool Parentage Projection

Phoenix SHALL project tool calls by pairing assistant tool-use blocks with tool-result messages through `tool_use_id` and turn membership.

### REQ-AN-007: Explicit Fidelity

Phoenix SHALL mark analytics/export fields as native, derived, estimated, unknown, or unavailable where exactness matters. V1 cost fidelity is represented by `pricing_known` and `unknown_turns`; persisted cost/source columns are not required.

### REQ-AN-008: Retry Facts

Phoenix SHOULD persist LLM attempt/retry facts when retry behavior is used in analytics or export.

### REQ-AN-009: Outcome Facts

Phoenix SHOULD persist commit/PR/yield facts when outcome attribution becomes a first-class analytics surface.

### REQ-AN-010: Trajectory-Compatible Export

Phoenix SHALL provide a Trajectory-compatible export adapter that projects Phoenix analytics sessions into an external session format without making Trajectory the source of truth.

## Implementation plan

### Step 1: Author analytics spec

Create `specs/analytics/requirements.md`, `design.md`, and `executive.md` covering:

- existing `/usage`
- derived cost as part of the existing usage surface
- analytics projection model
- `turn_usage.first_byte_at`
- deferred retry/outcome/privacy facts
- Trajectory-compatible export adapter
- fidelity/provenance
- non-goals around duplicate transcript storage

### Step 2: Persist first-byte timestamps on `turn_usage`

Add a nullable `first_byte_at` column to `turn_usage` and DB helper(s) to set it for the current token-bearing turn.

Populate it from the existing runtime path that emits `SseEvent::LlmFirstByte`. This should make an already-observed fact durable without introducing a second turn table.

### Step 3: Build analytics projection module

Create a typed projection module that assembles `AnalyticsSession`, `AnalyticsUsageTurn`, and `AnalyticsToolCall` from existing source data.

The projection should:

- use `turn_usage` as the token-bearing turn source;
- attach derived cost using the existing pricing logic;
- expose `first_byte_at` when present;
- derive tool call/result parentage from assistant/tool messages;
- detect deterministic deny-gate results from structured tool-result errors;
- report fidelity for fields that are derived or unavailable.

### Step 4: Refactor `/usage` to share projection/pricing helpers and expose first-byte latency

Keep `/api/usage` and `/api/usage/conversation/:id` behavior stable, but route cost and turn-shaping through reusable analytics/pricing helpers where practical. Do not add persisted cost columns.

Expose first-byte latency in the v1 `/usage` conversation drilldown. Historical rows with NULL `first_byte_at` should render as unavailable rather than zero latency.

### Step 5: Add Trajectory-compatible export adapter

Add an API or internal service that exports one conversation/root session through the analytics projection into a Trajectory-compatible payload.

Acceptance expectations:

- Export reconstructs session metadata from `conversations`.
- Export reconstructs turns from `turn_usage`.
- Export includes derived cost and unknown-pricing fidelity using the current `/usage` cost semantics.
- Export includes first-byte data when `turn_usage.first_byte_at` is present.
- Export reconstructs tool calls/results from messages without duplicating transcript storage.
- Export marks retries, outcome attribution, and lifecycle fields as unavailable/deferred until follow-up facts exist.
- Export can be exercised in tests without a running Trajectory daemon.

### Step 6: Add tests

Cover:

- migration adds nullable `turn_usage.first_byte_at`
- new streaming turns persist `first_byte_at` when `LlmFirstByte` is observed
- historical rows leave `first_byte_at` NULL
- `/usage` parity for existing token/cost behavior
- projection-level derived cost uses current `pricing_known` / `unknown_turns` semantics
- tool call/result parentage from existing messages
- deterministic denied command projection
- `/usage` exposes first-byte latency for rows with `first_byte_at`
- `/usage` renders historical NULL `first_byte_at` as unavailable
- Trajectory-compatible export reconstructs session/turn/tool data from projection
- export fidelity marks retries/outcome/lifecycle fields as unavailable/deferred
- export tests run without a Trajectory daemon

### Step 7: File explicit analytics v1 deferrals

Create a follow-up task for analytics v1 deferrals, including retry facts, outcome attribution, turn lifecycle rows, and privacy/export suppression facts.

## Resolved decisions

1. Trajectory-compatible export is in scope for this task and should drive acceptance testing.
2. `/usage` should expose first-byte latency in v1, not merely persist it.
3. Retry analytics (`llm_attempts`) remain deferred and should be tracked in the deferrals follow-up task.
4. Outcome attribution (`conversation_git_yields`) remains deferred and should be tracked in the deferrals follow-up task.
