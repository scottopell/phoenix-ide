# Ember phase 5: cheap transcript-scanning safety net

Parent: [06013](./06013-p1-ready--ember-consistency-and-durable-tracking.md). Depends on phases 1, 3 and 4. User authorizes automatic capture; Ember bears stale/duplicate retirement.

## Outcome

A bounded auxiliary scanner recovers omitted things from persisted Ember history, automatically adds records/evidence, and exposes honest coverage without scheduling work or reversing Ember decisions.

## Source and model contract

Recommended discovery scope: the ONE Ember continuation chain; persisted user/assistant prose with exact typed origin, excluding raw tool dumps/inline children and blanket worker/remote discovery. Linked workers can support reconciliation. Use a frozen activation high-water boundary with separate bounded live/backfill frontiers so new tail progress never falsely covers an older gap. New continuation members start their live frontier at the beginning. Resolve exact fragment/frontier encoding and fair lane selection in phase 1. Historical suggestions/roadmap quotes never become fresh commissions. Temporary stale historical captures are permitted when a later correction is outside the scanned chunk, but historical provenance and unverified freshness must remain visible. Ember checks later/current evidence before acting and retires stale items. Test an old request and much later cancellation outside the inference window.

Use generation-fenced snapshots and a durable covered position. Append does not change transcript_generation, so generation alone misses new messages; sequence alone misses edits/recovery. Oversized messages need a source-digest/generation-bound fragment cursor or explicit unprocessed status, never silently consume a prefix as the whole message.

Use tool-free LlmService completion, an explicit eligible cheap route and hard input/output/time/retry/backfill limits. Current get_cheap_model_with_id falls back to default; do not adopt that silent expensive behavior. No eligible route means visible pending/degraded scans, not foreground model substitution or blocked normal Ember work.

## Processing

- One supervised low-priority in-process runner; durable cursor plus startup discovery; coalesced post-commit kicks reduce latency but are not the authority.
- Never await inference inside direct_turn_worker::discover_inputs or before durable user/assistant admission completes.
- Read bounded source and registry revision; include active and relevant retired records.
- Closed result supports existing match/new candidate/no candidate with exact validated source quotes and explicit completion. Reject fabricated evidence, tool calls, malformed/truncated output and oversized incomplete coverage.
- Atomically apply capture/evidence plus cursor only if source generation, cursor and registry revision still match. Conflict commits neither writes nor coverage. Respect existing unclassified SQLite commit authority failure.
- Scanner can auto-add or append evidence. It cannot overwrite Ember prose, grant authority, resolve/dismiss, or reopen a retired thing.
- Safe repetition of inference is allowed; do not promise exactly-once calls/billing or perfect semantic dedup. Existing cursor prevents exact application replay; changed wording may still produce duplicates Ember dismisses.

## Starting points

Database singleton/topology helpers; prompt_projection.rs generation/sequence types; load_hydrated_prompt_snapshot/tail; get_bounded_read_messages_after_limited; title_generator.rs auxiliary call; ModelRegistry execution-route validation; RuntimeExecutor authoritative persistence hooks; reminder_delivery background lifecycle pattern.

## Acceptance and gates

Deterministic fake-service tests cover new tail arriving during incomplete same-transcript backfill, two continuations, partial/oversized source, edited old message, concurrent manual create/dismissal, replay/interrupted apply, empty complete output, failed/truncated output, restart, model unavailable and retry limits. Two commitments in one message remain representable. Replayed committed ranges cannot repeat application and the scanner cannot reopen an existing retired record. Define host-owned batch/application identity; do not rely on model-generated extraction keys. Different source spans/wording can still yield a new duplicate; semantic duplicate and retired-meaning rediscovery rates are measured rather than asserted zero.

Before enabling auto-add, run a small isolated real-model corpus with pinned route/prompt hashes, source-validity/recall/false-positive/duplicate/retirement metrics, latency and actual usage. Include speculation, implicit promise, later correction/completion, dismissed duplicates, quotes/peer reports and PR-complete-but-follow-up-owed. Shadow evaluation is qualification only; product behavior after activation remains automatic capture. Define accepted quality/cost bounds before claiming success.
