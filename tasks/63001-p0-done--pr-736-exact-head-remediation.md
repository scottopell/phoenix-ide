# Remediate PR #736 and pass the exact-head gate

## Authority and WorkScope

Own the complete remaining remediation and exact-head gate for PR #736 in one isolated managed WorkScope.

- Immutable starting head: `5add86843c82c500fd3a5facf8d910861157eeb5`
- Base authority: `b20ed69ab2ccf84fd27bc63eb46da538c9f96f86`
- Only push: `origin/task-17005-productconversation-bounded-prompt-projection`
- Never merge the PR.
- Keep `tasks/17005-p0-done--productconversation-bounded-prompt-proje.md` done-named unless a genuine material product blocker is established. This remediation task records review closure without reopening the completed feature task.
- Ignore any coordinator dirty draft except as optional read-only evidence; independently implement and verify the candidate.

## Observed journey

PR #736 introduces SQLite-authoritative, generation-fenced provider prompt projection and normalized attachment hydration. Exact-head review found four correctness gaps that can lose sequence-zero history, turn durable corruption into a panic/substitution, let continuation transfer race the live destination sequence allocator, and leave normative migration prose inconsistent with migration 094.

The reproduction authority is the exact immutable starting commit above, not a moving local checkout or an earlier review snapshot.

## Verified findings

1. `prompt_projection::validate_prompt_rows` initializes a full snapshot with `PersistedMessageSequence::default()` (sequence 0) and rejects a real first row at sequence 0. The same scalar cannot represent both an empty snapshot position and `At(0)`. Generation and cursor are also passed as separable values at the tail boundary.
2. `message_attachments::hydrate` uses infallible `SqliteRow::get` for image `message_id`, `media_type`, and `data` (and attachment lookup identity), while file payload decoding already uses `try_get`. Provider-authoritative hydration must return a typed error for malformed SQLite values rather than panic or substitute defaults.
3. Wake delivery materialization reserves one live broadcaster sequence before its DB insert, but `WakeRepository::transfer_once` moves already-materialized wake-result messages to a continuation and resequences them inside the DB transaction without first reserving the destination range through the live destination broadcaster. `SseBroadcaster::reserve_persisted_message_range_after` permits overlapping/nested attempts in release builds and relies on a `debug_assert!`.
4. Migration 094 preserves every row and deterministically repairs each affected partition by `(old sequence_id, created_at, message_id)`, may densely renumber that partition, remaps `clear_watermark` by stable cleared-row identity/order, and bumps transcript generation. REQ-BED-018A and ADR-045 instead describe or imply relocating only duplicate later identities above the old maximum.

## Interaction map

- Committed `messages` rows + normalized attachment children → transactional DB prompt snapshot/tail → executor-owned generation-fenced projection → immutable provider request.
- Empty snapshot position → first durable append at sequence 0 → generation-fenced tail → projection advances to `At(0)`.
- Continuation API or startup recovery → destination live runtime/broadcaster authority → reserved destination persisted-message range above durable floor → wake transfer DB transaction → commit/rollback while reservation guard remains alive → guard release → subsequent live allocation above durable maximum.
- Created and `AlreadyContinued` continuation paths, retry reconciliation, and startup recovery must all use the same transfer boundary; none may perform the DB move first and observe/bump the broadcaster afterward.

## Required implementation

### 1. Generation-fenced prompt position

Replace the defaulted scalar cursor boundary with a type that structurally distinguishes `Empty` from `At(PersistedMessageSequence)`, while keeping sequence 0 valid. Bind the position to its transcript generation at the tail-load boundary so no cursor can be used without its generation fence.

Update DB loaders, runtime traits/testing stores, and executor projection state as needed. The empty-position tail query must include sequence 0; a tail after `At(0)` must include only strictly greater rows. Preserve legal sequence gaps and fail closed for duplicate, regressing, cross-conversation, or generation-invalid rows.

Regression: empty snapshot → append first persisted row at sequence 0 → generation-fenced tail returns that row → applying the tail advances the projection to `At(0)`.

### 2. Strict authoritative attachment decoding

Make every column read in normalized prompt attachment hydration fallible with `try_get` (or an equivalently strict typed query boundary), including image identity, media type, and data. Propagate SQL/type/decode failure through `DbResult`; do not panic and do not replace malformed provider-authoritative data with defaults or empty attachments.

Add a focused malformed image-column/type regression where SQLite permits constructing the corrupt row. Keep absent child collections semantically empty; the target is malformed persisted values/types, not inventing a parallel attachment-count authority.

### 3. Live-authority continuation transfer reservation

Refactor continuation wake transfer so the destination live broadcaster reserves the complete sequence range for messages that the transaction will move before any DB mutation. Determine the destination durable floor and transfer cardinality under a retry-safe protocol, assign the reserved sequences to the DB transfer, and retain an affine reservation guard through commit or rollback. Do not commit and then call `observe_seq` or otherwise repair the broadcaster afterward.

The reservation API must make overlapping or nested persisted-message reservations structurally unavailable in production, rather than relying on `debug_assert!` or caller discipline. A reservation guard must not expose another reservation operation; all competing checkpoint/append paths must coordinate through the same live broadcaster authority.

Apply the same boundary to:

- newly created continuations;
- `AlreadyContinued` idempotent/retry paths;
- wake transfer retries after version/set conflicts;
- startup continuation-transfer recovery.

Preserve ProductConversation/work-scope lineage and existing owner/idempotence rules.

Add a deterministic no-sleep race test proving:

1. the live destination broadcaster starts from its durable floor;
2. transfer reserves a range of `N` persisted-message sequences;
3. a competing checkpoint/append reservation cannot overlap, collide, or pass that range;
4. transfer commits, then releases its guard;
5. destination DB rows are unique and strictly increasing; and
6. the next broadcaster sequence is greater than the destination durable maximum.

Also retain focused DB tests for successful transfer, owner mismatch, version/set retry, rollback/failpoint, and startup recovery. No timing sleeps or probabilistic race assertions.

### 4. Normative migration alignment

Update REQ-BED-018A and ADR-045 to state migration 094's actual contract:

- preserve every row;
- use deterministic durable order `(old sequence_id, created_at, message_id)`;
- densely renumber an affected conversation partition when required;
- remap `clear_watermark` to preserve the stable identity/order of rows that were cleared; and
- bump transcript generation for affected conversations.

Do not change migration 094 back to a move-only-duplicates-above-old-max algorithm. Keep timeless requirements free of task/PR/status language and run the spec-authoring pre-flight before push.

## Standing invariants and non-goals

- SQLite committed parent rows and normalized attachment children remain the sole provider prompt authority.
- `Effect::RequestLlm` remains payload-free; `ConvState` remains transcript-free.
- Steady-state full transcript reload remains eliminated.
- Sequence 0 is valid and durable corruption fails closed.
- Unsupported nested/overlapping persisted-message reservations are impossible by construction.
- ProductConversation lineage and continuation compaction semantics remain intact.
- Do not introduce a mirrored transcript, JSON attachment authority, process-local prompt authority, or unrelated architecture expansion.

## Validation and exact-head review gate

Follow this order:

1. Confirm exact start/base/branch authority and verify task 17005 remains done-named.
2. Implement focused `phoenix-db` prompt/attachment/wake tests and `phoenix-ide` wake/continuation/race tests.
3. Run formatting and clippy for all targets of `phoenix-db` and `phoenix-ide`.
4. Run disk-aware tests with `CARGO_INCREMENTAL=0 cargo test -p phoenix-db` and `CARGO_INCREMENTAL=0 cargo test -p phoenix-ide`.
5. Run serialized `./dev.py check --all` when required by the complete change surface.
6. Proactively run `phoenix-adversarial-review` against the exact complete range `b20ed69ab2ccf84fd27bc63eb46da538c9f96f86..candidate`, plus a fresh independent reviewer, before push. Remediate every valid finding and repeat validation.
7. Commit logical remediation units, push only the authorized branch, and verify local HEAD, remote branch HEAD, and PR head are byte-identical with a clean worktree.
8. Wait for all CI on that immutable candidate. Request `@codex review` exactly once for each immutable candidate head.
9. Inspect all paginated GraphQL review threads, evidence-reply and resolve addressed threads, and iterate with a new immutable candidate only when remediation is required.
10. Finish only when exact-head review is clean, unresolved review-thread count is zero, all CI passes, and GitHub reports mergeable `CLEAN`. Never merge.

Preserve credential secrecy in commands, logs, comments, and reports. Report a blocker only after classifying it against the accepted product invariants above.
