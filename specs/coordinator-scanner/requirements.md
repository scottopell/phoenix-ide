# Coordinator transcript safety net

Ember retains meaningful user-requested outcomes and unfinished question bookmarks across continuations. A background safety net recovers omissions without turning reports, quoted suggestions or historical intent into fresh commissions. Durable tracking remains the item authority; subscriptions/reminders remain the known-work follow-up mechanisms.

### REQ-SCAN-001: Own-history scope and intent

THE scanner SHALL discover only the singleton Coordinator’s actual continuation members. Workers, inline/coordinator children and remote transcripts SHALL NOT enter discovery by shared WorkScope, product or parent association. It SHALL preserve source origin, prose/quotation and structured-answer boundaries. Assistant/event reports SHALL NOT alone establish a human request. Raw tool dumps and scanner/tracking meta observations SHALL NOT become discovery prose. User-role UnknownHistorical prose SHALL be processed with original provenance intact. Clear apparent intent MAY yield provisional unreviewed capture; it SHALL NOT be silently dropped, relabelled UserApi/human, or granted authority, and ambiguous machine/quoted text SHALL NOT establish a commission.

### REQ-SCAN-002: Honest, resumable coverage

THE SYSTEM SHALL gradually process the whole enrolled history with bounded batches, prioritizing new live prose while preserving historical progress. It SHALL retain separate live and historical positions under a frozen activation boundary; new members start live coverage at their beginning. Historical windows SHALL proceed newest to oldest with chronological prose within each window. An oversized source SHALL retain a generation/digest-bound fragment position or explicit unprocessed status. An unread suffix SHALL NOT be marked covered.

WHEN an enrolled transcript generation changes, THE SYSTEM SHALL invalidate that transcript’s old coverage and incrementally restart in the new generation without deleting captured items, evidence or retirement decisions. Startup and bounded revisits SHALL discover generation-only changes and missed kicks without a new human message.

### REQ-SCAN-003: Closed, source-valid inference

THE scanner SHALL use tool-free inference and a closed result containing new unreviewed candidates, evidence for captured existing candidates, or no action, with explicit complete processing. Every proposed quote SHALL identify a captured fragment/message and match an unambiguous verbatim source span resolved by the host. Fabricated, ambiguous, malformed, truncated or incomplete results SHALL advance neither writes nor coverage. Source-validity accounting SHALL include rejected proposals.

### REQ-SCAN-004: Restricted application authority

THE scanner SHALL use DB-minted batch identity distinct from native ToolExecuting/SourceToolCall authority. It MAY create unreviewed requested-work/bookmark records or append immutable evidence. It SHALL NOT rewrite Ember notes, advance review, retire, dismiss, reopen, grant execution authority or lift a pause. New evidence SHALL invalidate the previously reviewed resumption projection. Exact quote deduplication SHALL bind the same identified item and source locator/content digest independently of generation; unchanged rescan evidence SHALL NOT create review work. Different source wording may still produce semantic duplicates. A later explicit human renewal MAY create a distinct unreviewed foreground-visible request while the old retired record remains closed.

### REQ-SCAN-005: Atomicity and recovery

THE application SHALL compare stable owner/membership, source generation/content, frontier and registry revision before atomically committing candidate/evidence changes with coverage. Conflicts SHALL commit neither. Pending work SHALL remain discoverable through bounded retry and startup/revisit recovery. Durable budget and exact-binding retry-cycle reservation SHALL precede provider dispatch, including reserve-then-crash and restart. The same binding/new nonce SHALL NOT reset its initial-plus-one-retry allowance. Exhaustion SHALL retain a five-minute resume time; a later eligible cycle may retry under global budgets, while real source/frontier/registry/prompt/route binding changes may start a new cycle. Unclassified SQLite write authority SHALL close local runtime authority before ordinary publication.

### REQ-SCAN-006: Bounded resource use

THE scanner SHALL bound source, candidate registry, total inference content, output, time, retry and live/backfill spending. Registry context SHALL expose honest omissions and clipping instead of including unbounded notes. It SHALL use an explicitly eligible cheap model/effort/connection with no foreground/default fallback. Missing routes, exhausted budgets or failures SHALL leave visible pending/degraded scans without blocking normal foreground work. Background scanning SHALL NOT create foreground turns or new review wakeups.

The eligible route SHALL identify both the catalog and actual outbound model as `gpt-6-luna` at Low effort; a request-name override to another model SHALL be ineligible. Before decoding or applying a result, THE SYSTEM SHALL require reported output-token usage of at most 2,048 tokens and at most 16,384 UTF8 text bytes, rejecting unknown usage, nontext, oversized or incomplete output. Official-direct requests SHALL send the generation-token limit. Codex has no provider-enforced generation-token or billing ceiling; its output bound SHALL be represented as received-result acceptance, alongside the independent timeout and durable attempt/input budgets. Every observed usage field SHALL remain distinguishable from an unknown field.


### REQ-SCAN-007: Bounded inspectability

THE current Coordinator SHALL inspect scanner coverage through the existing closed tracking surface. A bounded read-only coverage page SHALL expose captured generations, both lane boundaries/frontiers, pending/stale/incomplete/covered states and omissions. Route unavailable, budget paused and retry/backoff SHALL remain distinguishable from source coverage. A live frontier alone SHALL NOT imply all-history coverage. Compact captured counts SHALL remain historical, within the existing observation budget; coverage-only bookkeeping SHALL NOT append transcript observations.

### REQ-SCAN-008: Truthful historical recovery and qualification

Historical captures SHALL remain unverified until Ember reconciles later/current sources before using an old next step. Newest-first ordering SHALL NOT imply knowledge of cancellation outside the captured window. The scanner SHALL preserve retirement decisions and explicit restrictions. Automatic capture SHALL require demonstrated manual memory usefulness, independently reviewed frozen source/oracle quality gates, and deterministic production-host authority/coverage/recovery tests. Recall, false captures, duplicates, source validity, actual tokens and latency SHALL be reported separately; coverage SHALL NOT be presented as Ember review or completion.
