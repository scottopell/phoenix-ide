# Make ProductConversation prompt assembly bounded by construction

## User mandate

This work is mandatory and immediately follows Creation PR #727. It must not be deferred as an optional performance optimization.

Production analysis verified that each ordinary `RequestLlm` reloads the full transcript and hydrates attachments with one `message_files` query per historical user/skill message plus one `message_images` query per historical user message. The common path is `3 + 2U + S` SQLite reads, more than 99.5% of child queries are empty, and approximately 25,000 prompt-assembly reads occurred in six hours. Repeated rounds also reread and parse the historical transcript prefix.

## Required outcome

1. Replace per-message attachment hydration with typed set-based batch hydration over normalized child tables. Transcript plus attachment statement count must be bounded independently of historical message count. Preserve message ordering, file/image ordinals, exact persisted `llm_text`, and provider image/file semantics.
2. Make parent message and normalized attachment persistence atomic so a prompt cannot observe an attachment-incomplete message.
3. Add an executor-owned typed active prompt projection. Initialize it once from a transactionally consistent SQLite snapshot, then refresh each `RequestLlm` from a generation-fenced persisted tail after a known sequence cursor. SQLite remains durable authority.
4. On `transcript_generation` mismatch or invalid/out-of-order tail, fail closed and rebuild once from durable truth; never issue a partial/stale prompt.
5. Freeze the prompt snapshot synchronously after required persistence and before provider task spawn. Never feed reducer intent, eager SSE messages, recovery suffixes, or other unpersisted representations into prompt authority.
6. Preserve continuation/restart/multi-client steering, atomic tool checkpoints, stale-tool clear-watermark behavior, token accounting, expansion replay, and provider capability logging. ProductConversation continuation compaction must not be replaced by naive full-lineage replay.
7. Separate provider-bound prompt projection from UI/API transcript hydration. Do not add a mirrored transcript table, JSON child collections, or parallel semantic authority.
8. Update timeless requirements and behavioral specs, write a new ADR for the projection/authority boundary, and update executive verification.

## Deterministic acceptance evidence

- Query-count tests for 0, 8, and 27 historical user-message profiles prove constant attachment-hydration statement count; no wall-clock correctness thresholds.
- A runtime instance performs one full prompt snapshot load, then one generation-fenced tail read per `RequestLlm`; steady-state rounds perform zero full-history reloads and zero historical child-table hydration.
- Mixed empty/non-empty attachments preserve exact ordering and provider-bound parity.
- Concurrent parent/child persistence cannot expose a partial message.
- Persistence effects settle before tail refresh; provider spawn occurs after an immutable request snapshot is frozen.
- Legal sequence gaps work; duplicate/regressing/out-of-order tails fail closed.
- Existing-row mutation bumps `transcript_generation`, invalidates the projection, prevents provider dispatch, and forces one durable rebuild.
- Steering committed after one request snapshot appears exactly once in the next request; uncommitted tool rounds and eager SSE output never appear.
- Crash recovery and continuation rebuild from durable rows without duplicating continuation summaries or flattening exhausted lineage.
- `./dev.py check --all`, complete-range adversarial review, immutable CI, exact-head Codex, and fully paginated review-thread closure pass.

## Delivery order

Land Creation PR #727 first if it reaches its current correctness gate sooner. Start this task immediately afterward, before History/legacy-deletion completion is declared.
