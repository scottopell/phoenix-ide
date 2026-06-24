# Analytics v1 Deferrals

## Problem statement

The Phoenix analytics v1 task intentionally stays lean:

- keep `turn_usage` as the canonical token-bearing turn table;
- add only `turn_usage.first_byte_at` as new durable timing metadata;
- expose first-byte latency in `/usage`;
- build a projection layer over existing Phoenix history;
- implement Trajectory-compatible export from that projection.

Several valuable analytics concepts are deliberately out of v1 because they require new durable facts that cannot be reliably reconstructed from existing `messages` and `turn_usage`. This task tracks those deferred follow-ups so the v1 scope remains clear without losing the design work.

## Deferred areas

### 1. Retry and LLM attempt analytics

Phoenix currently exposes retry/attempt information through runtime state and SSE, but full retry history is not durable.

Future durable shape:

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

Questions this would answer:

- How often do retries happen?
- Which models/providers hit rate limits?
- How much latency is retry/backoff?
- Which conversations are dominated by retry behavior?

### 2. Turn lifecycle rows for failed/cancelled/non-token turns

V1 keeps `turn_usage` limited to token-bearing LLM turns. Failed or cancelled requests that never produce usage remain outside that table.

Future options:

- Expand `turn_usage` with status/lifecycle columns.
- Add a sibling lifecycle table keyed to `turn_usage` where present and nullable otherwise.

Potential future fields:

```text
status
started_at
completed_at
trigger_message_id
assistant_message_id
turn_index
```

Questions this would answer:

- How many turns fail before usage exists?
- How often are turns cancelled/interrupted?
- What is true turn duration, independent of token-row creation time?
- Which user message or assistant message belongs to a turn without relying on heuristics?

### 3. User/system interruption facts

Phoenix may need durable interruption records if cancellation/interruption analytics become a first-class surface.

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

Questions this would answer:

- How often do users interrupt the agent?
- Which cancellations are user-driven vs runtime/system-driven?
- Which sessions ended by interruption rather than successful completion?

### 4. Git/PR outcome attribution

Outcome attribution is valuable but separate from v1 usage/latency/export foundations.

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

Questions this would answer:

- Which sessions produced commits or PRs?
- How many turns/tokens/cost were attributed to a commit?
- How much of a session contributed to PR creation?
- How many lines/files changed during a session?
- How many yielded commits later became reverts?

### 5. Privacy/export suppression facts

If Phoenix gains incognito or publish-suppression semantics, it needs a durable record of those toggles.

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

Questions this would answer:

- Was export suppression enabled during a session?
- Which conversations are safe to export or publish?
- Did suppression apply globally, per workspace, or per conversation?

## Acceptance criteria for this deferral task

When picked up later, this task should be split into one implementation task per analytics surface. Do not implement all deferred facts at once unless a concrete product surface needs them together.

Each follow-up implementation should:

- identify the user-facing analytics question it answers;
- add only durable facts that cannot be reconstructed from existing history;
- avoid full transcript/tool I/O duplication;
- update the analytics projection and Trajectory-compatible export fidelity;
- include backfill behavior for historical conversations when possible;
- leave unavailable historical data explicitly unavailable rather than fabricated.
