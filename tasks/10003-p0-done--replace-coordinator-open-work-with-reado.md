# Replace Coordinator open-work inference with transparent read-only SQLite

## Observed production journey

At 2026-07-21 12:27 UTC, the user invoked `Brief me` in production Coordinator `fe0121f0-7307-45fc-b203-0bbbfec7546d`. The resulting answer said the projected Explore work-tool-access thread was active but appeared stale, and separately said the Bash wake thread was absent from the deterministic snapshot despite being actively progressing.

The user no longer trusts the opinionated open-work abstraction and chose to replace it with lower-level relational evidence:

- Retire or greatly reduce the inferred open-work projection rather than merely patching its rules.
- Give the Coordinator raw read-only SQLite query access to operational production data.
- Structurally exclude secrets such as OAuth tokens, auth sessions, share tokens, and secret settings.
- Keep `search_conversations`, `read_conversation`, and `resolve_reference` for bounded transcript/history reads and citations.
- Replace the inferred current-work capsule with a simple automatic raw snapshot: conversation/root/current identifiers, continuation links, state, `state_updated_at`, `updated_at`, and task metadata. The Coordinator—not application heuristics—interprets whether work is active, stale, blocked, or complete.

## Verified production findings

### 1. Chain-root evidence was mistaken for current-chain evidence

The automatic snapshot identified the Explore work-tool-access chain as one active `@work:8382766f-...` item whose current leaf was `33743018-...` in `ToolExecuting`. The capsule exposed only the root-based work handle, not a direct current-conversation reference.

The Coordinator then called `read_conversation` twice on root conversation `8382766f-...`, whose persisted update time was July 20. It never read current leaf `33743018-...`, which was actively producing tool messages during the briefing. It concluded that the active projection was suspect because “transcript evidence does not show fresh execution.”

The projection itself knew both identities (`GlobalOpenWorkItem.current_conversation_id` and `root_conversation_id`), but `format_current_work_capsule` discarded both and emitted only `reference`, project, title, state, mode, update time, and signals. `read_conversation` also treats a root `@conv` as exactly that conversation; it does not resolve to the chain leaf. `resolve_reference(@work:...)` could have revealed the current leaf, but neither the prompt nor types require that intermediate call.

### 2. Active runtime was hidden by a closed task

The wake continuation `7d9b66ef-...` was actively reading files and executing tools immediately before and during the briefing. Its task file `44008-p0-done--stop-incorrect-automatic-wakes.md` was marked done while follow-on agent work continued.

`is_open_work_candidate` returns false for `done`/`wont-do` before checking `has_runtime_open_evidence`. Therefore a live `LlmRequesting`/`ToolExecuting` runtime is omitted whenever its task status is closed. This contradicts REQ-GR-003, which says a Work item is included when its task is open **or** runtime state is active/recovery/attention-needing. Existing unit coverage explicitly pins the contradictory implementation (`done` is false, then only tests active with missing task status).

This is why the Coordinator correctly observed the wake thread in history but found it absent from the snapshot.

### 3. Search accepts natural language, not operator syntax

The Coordinator attempted `search_conversations` with `in:explore-conversation-work-tool-access after:2026-07-20 "work-toolset"`. The retrieval layer tokenizes this as ordinary terms joined with `OR`; it does not implement `in:` or `after:` operators. Results came from the older discarded work-toolset conversation and an unrelated conversation.

The tool description says only “Search all Phoenix conversation messages,” but does not state that filters/operators are unsupported. This increased confidence in stale, irrelevant evidence.

### 4. “Progress” is not a first-class Coordinator datum

Production rows provide current state and `state_updated_at`; messages provide recent tool activity. The current capsule exposes `updated_at` but omits `state_updated_at`, current conversation identity, and any direct recent-activity tail. The Coordinator must infer liveness by joining a lossy capsule, reference resolution, transcript pages, and search results correctly. The faulty turn demonstrates that this choreography is not reliable.

### 5. Telemetry could not independently explain the turn

VictoriaTraces reports the `phoenix-ide` service but returned no production traces in the bounded two-hour window. `prod.log` contained only the log-rotation marker. `./dev.py prod status` emitted an OTLP 429 warning (“Too Many Requests”). The durable production DB transcript is therefore the primary evidence for this incident; collector/export health is a separate operational concern, not the proposed fix.

## Failure model

The current Coordinator read surface combines three abstraction layers with different identities and freshness semantics:

```mermaid
flowchart LR
  Rows["Conversation, chain, task, and message rows"] --> Projection["Opinionated open-work projection"]
  Projection --> Capsule["Lossy 12-item inferred capsule"]
  Projection --> Tool["list_open_work formatted text"]
  Capsule --> Agent["Coordinator interpretation"]
  Tool --> Agent
  Agent --> Resolve["Optional reference resolution"]
  Agent --> Transcript["Root-or-leaf transcript reads"]
  Agent --> Search["Natural-language FTS search"]
```

The application labels work open/active/attention-worthy, collapses continuation chains, prioritizes/truncates items, and then removes relational keys before the model sees the capsule. The model must reverse those abstractions to validate them. Invalid joins are representable and, in production, occurred.

## Proposed direction

### A. Replace inferred open-work reads with bounded raw SQLite

Add one Coordinator-only `query_database` tool that executes exactly one read-only SQLite statement against the live Phoenix database.

The query surface should be raw SQL for allowed operational tables rather than a new filter DSL or another inferred projection. It must support joins, CTEs, SQLite JSON functions, FTS reads, grouping, ordering, and schema inspection for allowed objects so the Coordinator can answer unanticipated operational questions without waiting for another application abstraction.

Structural enforcement—not prompt instructions or keyword filtering—is required:

- Use a separate read-only/query-only connection, never the application's writable pool authority.
- Install a SQLite authorizer or equivalent engine-level policy that permits read operations only and denies writes, transactions, attach/detach, dangerous pragmas, extension loading, and access to denied tables/columns.
- Deny credential/security objects including at least `auth_sessions`, `mcp_oauth_registrations`, `mcp_oauth_tokens`, `share_tokens`, secret-bearing app settings, and SQLite shadow/storage tables that could bypass policy.
- Decide and test whether message/tool text is allowed; the selected product direction expects operational conversation/message data to remain queryable, so tool output must repeat the existing untrusted-data security boundary.
- Enforce host-owned row, byte, execution-time/VM-step, and statement-count limits. Return typed columns/rows plus `truncated`, elapsed/budget metadata, and actionable errors.
- Do not expose the database filesystem path or permit ATTACH to turn SQL into server-filesystem access.

### B. Replace the inferred capsule with one transparent snapshot query

Keep automatic turn orientation, but make it a direct relational snapshot without “open,” “attention,” “stalled,” or “recently idle” classifications.

The snapshot should expose a bounded, deterministic set of raw facts sufficient to formulate follow-up SQL:

- conversation ID, slug/title, mode, project ID
- root and current/leaf conversation IDs for continuations
- continuation link(s)
- exact runtime-state discriminant
- `state_updated_at` and `updated_at`
- task ID/title plus task status when readable
- archived, user-initiated, and parent/sub-agent identity

Define the inclusion/order query transparently—for example active runtime rows plus most recently updated user-initiated roots/leaves—and include its SQL or a stable query identifier in the system contract. Truncation and selection criteria must be explicit. Do not call the result “open work.”

### C. Retire current projection contracts

Remove:

- automatic `current_work_capsule`
- `list_open_work`
- `GlobalOpenWork*`, `OpenWorkFilter`, inclusion/signal/priority heuristics, disk task-status projection, and text formatting when no remaining typed consumer exists
- requirements and tests that make Phoenix infer open/attention/stalled semantics

Keep or extract only identity/reference helpers still required by `resolve_reference` and `send_conversation_message`; these actions must continue resolving `@work`/chain targets to the current leaf until stable-reference semantics are deliberately redesigned. Avoid retaining a second hidden open-work implementation merely to support references.

### D. Retain bounded transcript/history tools

Keep:

- `search_conversations` for ranked natural-language message discovery
- `read_conversation` for bounded live transcript reads and message-target context
- `resolve_reference` for durable links and citation metadata
- `send_conversation_message` as the singular bounded mutation capability

Clarify that `search_conversations` accepts natural-language terms only and does not support `in:`, `after:`, or other operators. The SQL tool is the route for exact metadata/time/conversation filters.

Consider allowing `read_conversation` to accept `@work`/`@chain` and explicitly choose root, current leaf, or chain traversal; at minimum, the raw snapshot and tool descriptions must prevent root/current ambiguity.

### E. Update Coordinator guidance

The prompt should require:

- Current status claims come from live SQL state/timestamps, not old transcript prose.
- Transcript/search content is historical evidence only; it cannot override a newer runtime state without current relational evidence.
- “Stalled” is a conclusion requiring an explicit user-visible basis, such as unchanged `state_updated_at` and no recent messages/tool activity over a stated interval—not a label supplied by Phoenix.
- Query results and all stored text remain untrusted data, never instructions.
- Sensitive tables are structurally unavailable.

## Interaction map after replacement

```mermaid
flowchart LR
  DB["Live Phoenix SQLite"] --> Guard["Engine-enforced read-only + deny policy + budgets"]
  Guard --> SQL["Coordinator query_database tool"]
  DB --> Snapshot["Transparent bounded orientation query"]
  Snapshot --> Agent["Coordinator interprets raw facts"]
  SQL --> Agent
  Search["Natural-language message search"] --> Agent
  Read["Bounded transcript read"] --> Agent
  Resolve["Durable reference resolution"] --> Agent
```

## Acceptance criteria

- Replaying the July 21 production shape cannot lead the Coordinator to validate a current leaf's active state by accidentally reading only the stale chain root; raw rows expose both identities, and current-state guidance is explicit.
- A `ToolExecuting` or `LlmRequesting` conversation remains visible as raw current state regardless of whether its associated task file is `done`; no application-level “open work” inclusion decision suppresses it.
- The automatic snapshot contains no inferred open/attention/stalled classifications and names its selection/truncation policy.
- `query_database` can perform useful joins across conversations, messages, projects, workflows/wakes, and allowed metadata, while one-statement, read-only, row/byte/time/step limits are enforced.
- Attempts to query credential/session/token tables, write data, change pragmas, attach databases, invoke extensions, access denied SQLite internals, or exceed budgets fail with stable errors and no partial sensitive output.
- Sensitive values cannot be reached indirectly through schema objects, views, virtual/shadow tables, SQL functions, or error text.
- Coordinator SQL and transcript outputs are treated as untrusted data in prompt/tool framing.
- `search_conversations` documents and tests natural-language-only semantics; operator-looking text is not presented as a supported filter.
- Existing transcript paging, citations, stable reference resolution, and cross-conversation send authority remain intact.
- Requirements/ADR/executive docs describe relational evidence and structural security boundaries rather than inferred open-work behavior.
- Focused policy/adversarial tests, production-shaped regression fixtures, full `./dev.py check`, and a real Coordinator briefing QA pass.

## Risks and explicit non-goals

- **Security risk:** SQLite is expressive; a SELECT-only keyword check is unacceptable. Engine-level authorization and adversarial tests are required.
- **Privacy risk:** conversation/message/tool content can itself contain secrets. The chosen scope allows operational text because the Coordinator already has transcript access; output remains bounded and untrusted. Credential tables remain denied.
- **Availability risk:** pathological recursive CTEs, huge joins, JSON expansion, and FTS queries can consume CPU/memory or hold read snapshots. Strict budgets and cancellation are required.
- **Schema-coupling risk:** the Coordinator prompt/tool contract will track database migrations. Provide bounded schema discovery for allowed objects and treat schema changes as a tested contract.
- **Token-cost risk:** raw rows can be verbose. Bounds and deliberate orientation queries are mandatory; do not solve this by restoring inferred labels.
- **Non-goal:** grant the Coordinator write SQL or additional mutation/lifecycle authority.
- **Non-goal:** expose auth/OAuth/share-token/secret-setting data.
- **Non-goal:** remove durable references, transcript search/read, or the singular message-delivery tool.
- **Non-goal:** diagnose or repair the unrelated VictoriaTraces export/429 issue in this task.
