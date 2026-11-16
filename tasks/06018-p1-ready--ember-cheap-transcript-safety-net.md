# Ember phase 7: cheap transcript-scanning safety net

Parent: [06013](./06013-p1-ready--ember-consistency-and-durable-tracking.md). Depends on the relevant terminology/baseline from [phase 1](./06014-p1-in-progress--ember-tool-contract-and-consistency-base.md), [phase 5](./06016-p1-ready--ember-durable-tracking-store-and-tools.md) and [phase 6](./06017-p1-ready--ember-tracking-continuity-and-inspection.md). This task owns frontier/invalidation/application/model-budget contracts before scanner activation; those decisions do not gate event or direct-tracking delivery. User authorizes automatic capture; Ember bears stale/duplicate retirement.

## Outcome

A bounded auxiliary scanner recovers omitted things from persisted Ember history, automatically adds records/evidence, and exposes honest coverage without scheduling work or reversing Ember decisions.

Activation follows the demonstrated phase 5→6 manual memory journey, not just an available database schema. Offline source-label/corpus and budget preparation may proceed in parallel with that lane; it does not commission a separate checker service. The same memory owner can then implement this safety net against the proven application service. Context/retrieval burden and useful recovery remain acceptance metrics alongside capture/cost.

## Source and model contract

User-selected discovery scope: ONLY the ONE Ember continuation chain. Focus on user-requested work and meaningful unfinished conversational threads, using persisted user/assistant prose with exact typed origin. Assistant responses supply context/stopping points; agent promises are not the primary capture target. Exclude raw tool dumps, worker and inline-child transcripts from scanner discovery. Ember may separately inspect linked workers through existing tools; that does not enroll them in scanning. See Issue #1075: preserve a curiosity as a bookmark without creating a work commitment. User selected the whole Ember history gradually in bounded batches, prioritizing new messages. No initial date cutoff or age-based item expiry. Saved items are reviewed during normal turns and existing watch/reminder wakeups, without new periodic review wakeups. Use a frozen activation high-water boundary with separate bounded live/backfill frontiers so new tail progress never falsely covers an older gap. New continuation members start their live frontier at the beginning. Resolve exact fragment/frontier encoding, generation invalidation and fair lane selection in this slice before activation. Historical suggestions/roadmap quotes never become fresh commissions. Temporary stale historical captures are permitted when a later correction is outside the scanned chunk, but historical provenance and unverified freshness must remain visible. Ember checks later/current evidence before acting and retires stale items. Test an old request and much later cancellation outside the inference window.

Use generation-fenced snapshots and a durable covered position. Append does not change transcript_generation, so generation alone misses new messages; sequence alone misses edits/recovery. Oversized messages need a source-digest/generation-bound fragment cursor or explicit unprocessed status, never silently consume a prefix as the whole message.

Preserve origin, quotation and structured-answer boundaries in scanner inputs. The measured recent window had 7,985 event-origin rows versus 674 user_api rows; the latter also include quotes, review templates, button messages and answers. Neither the user role nor user_api origin alone establishes a new commission. Enriched completion excerpts are worker evidence: they may reconcile an existing record, but cannot establish a fresh human request. Explicit user pauses/corrections remain restrictions until lifted; do not convert errors, event prose or agent suggestions into requested work.

Use tool-free LlmService completion, an explicit eligible cheap route and hard input/output/time/retry/backfill limits. Current get_cheap_model_with_id falls back to default; do not adopt that silent expensive behavior. No eligible route means visible pending/degraded scans, not foreground model substitution or blocked normal Ember work.

## Generation changes after completed coverage

Before activation, specify and implement a durable invalidation transition for committed coverage. Discover enrolled transcript-generation changes even when no message is appended. Startup discovery and bounded revisits by the existing background runner recover missed mutation kicks; no new foreground Ember wakeup or second scheduler is needed.

Use the conservative initial policy: a changed transcript generation makes that transcript's prior coverage stale and resets its scan positions to the beginning in the new generation, with an explicit pending/unprocessed domain and newly captured live boundary. Preserve already committed item/evidence records and retirement decisions; coverage reset is not deletion or reenrollment of all transcripts. Do not copy an old frontier into a new generation or leave the old generation conflicting forever. Process the reset transcript incrementally under existing live/backfill fairness and resource limits.

Distinguish an exact batch retry from a new source-content revision. Durable application identity binds generation and exact source range/fragment; retained evidence identity also binds message/span and canonical quoted-content digest for its identified item. A bounded rescan does not append the same exact evidence to that item again or generate a spurious review-needed transition. Changed content remains a new source observation, preserves prior evidence and triggers reconciliation when attached to an item. This is exact evidence deduplication, not a promise of semantic matching or a global one-item-per-message restriction. Never treat a generation number alone as evidence that text changed or as item identity.

## Processing

- One supervised low-priority in-process runner; durable cursor plus startup discovery; coalesced post-commit kicks reduce latency but are not the authority.
- Never await inference inside direct_turn_worker::discover_inputs or before durable user/assistant admission completes.
- Read bounded source and registry revision; include active and relevant retired records.
- Closed result supports existing match/new candidate/no candidate with exact validated source quotes and explicit completion. Reject fabricated evidence, tool calls, malformed/truncated output and oversized incomplete coverage.
- Atomically apply capture/evidence plus cursor only if source generation, cursor and registry revision still match. Conflict commits neither writes nor coverage. Respect existing unclassified SQLite commit authority failure.
- Scanner can auto-add or append evidence. New captures are unreviewed; a new evidence append atomically advances phase 5's evidence watermark and invalidates the prior reviewed next step. Scanner cannot advance the reviewed watermark, overwrite Ember prose, grant authority, resolve/dismiss, or reopen a retired thing. Phase 6's compact index/overflow must expose review-needed before the old note can be used.
- Bound contention retries within the existing retry/time budgets. Re-read source/cursor/registry state after conflicts and keep pending work discoverable; once contention stops, the existing runner must make progress without another user message. No finer-grained concurrency system is required.
- Safe repetition of inference is allowed; do not promise exactly-once calls/billing or perfect semantic dedup. Existing cursor prevents exact application replay; changed wording may still produce duplicates Ember dismisses.

## Starting points

Database singleton/topology helpers; prompt_projection.rs generation/sequence types; load_hydrated_prompt_snapshot/tail; get_bounded_read_messages_after_limited; title_generator.rs auxiliary call; ModelRegistry execution-route validation; RuntimeExecutor authoritative persistence hooks; reminder_delivery background lifecycle pattern.

## Acceptance and gates

Deterministic fake-service tests cover new tail arriving during incomplete same-transcript backfill, two continuations, partial/oversized source, edited old message, concurrent manual create/dismissal, replay/interrupted apply, empty complete output, failed/truncated output, restart, model unavailable and retry limits. Two commitments in one message remain representable. Replayed committed ranges cannot repeat application and the scanner cannot reopen an existing retired record. Define host-owned batch/application identity; do not rely on model-generated extraction keys. Different source spans/wording can still yield a new duplicate; semantic duplicate and retired-meaning rediscovery rates are measured rather than asserted zero.

Before enabling auto-add, run a small isolated real-model corpus with pinned route/prompt hashes, source-validity/recall/false-positive/duplicate/retirement metrics, latency and actual usage. Include explicit user requests, an unanswered user question, topic detour and return, an exploratory idea preserved as a bookmark, a worker promise quoted in Ember history, later correction/completion, dismissed duplicates and PR-complete-but-follow-up-owed. Measure requested-work recall separately from bookmark usefulness; an agent promise must not become a fabricated user request. Shadow evaluation is qualification only; product behavior after activation remains automatic capture. Define accepted quality/cost bounds before claiming success.

Use independently reviewed expected captures and restrictions as the reference corpus. The scanner model's own assessment of its recall or usefulness is not sufficient evidence.

Add event-heavy cases with bounded worker excerpts, quoted corrections, structured answers, explicit pauses, intentional abandonment and a completed milestone with a still-owed user outcome. Pauses must remain attached to the saved resumption condition through scanning and continuation.

Required post-coverage edit case: fully cover through sequence 100 at generation G, edit message 5 to create G+1, append nothing, miss the kick and restart. Discovery must durably invalidate/reset that transcript, expose incomplete coverage and finish bounded rescanning. Unchanged source evidence is not appended twice; changed content is not suppressed as an old exact application. Include continued contention followed by quiescence and prove eventual bounded-attempt progress.

Required missed-pause case: an existing actionable note receives previously missed user-pause evidence, then Ember continues. Index/detail/overflow report reconciliation required, trusted source reconciliation retains the pause, and scanning cannot clear it. No new wakeup or permission is manufactured.

Out of scope: worker/remote corpus discovery, scanner retirement/reopening, foreground inference, expensive default-route fallback, scheduling saved work and perfect semantic deduplication.
