# ADR-263: Scan only Coordinator history through bounded host-owned batches

- **Status:** Accepted
- **Date:** 2026-11-15
- **Affects:** REQ-SCAN-001–008, REQ-TRACK-004–007

## Context

A durable manual list survives continuation but can still omit user intent that Ember never saved. The user authorized automatic capture across all Ember history gradually, focused on meaningful requested work and unfinished questions. They rejected a second worker-checkpoint system and required immutable conversation prefixes, bounded context and no new foreground review wakeups.

## Options considered

1. Rely on manual capture alone. It avoids auxiliary inference but leaves the original omission risk unaddressed.
2. Scan all workers or use the foreground/default model for broad relevance/status ranking. This adds scope, cost and execution overlap, and risks treating reports or quoted promises as commissions.
3. Add one bounded own-history safety net with exact source validation, host-owned application and honest coverage. It recovers omissions while leaving action/review/retirement to Ember and known execution follow-up to existing subscriptions/reminders.

## Decision

Choose the third option. Enroll only actual singleton Coordinator continuation members using existing validated topology, excluding workers and inline/coordinator children even when they share a product or WorkScope. Preserve original input origin, quotation and structured-answer boundaries. Raw tool dumps and scanner/tracking meta observations do not enter discovery. Preserve and process user-role UnknownHistorical prose as Unknown: clear apparent intent can be captured provisionally, but never silently relabelled as verified human/UserApi or granted authority. Ambiguous machine/event/quoted content is not a commission. Assistant/events may support an existing item; they cannot alone establish a new human commission.

Use separate live and historical frontiers with a frozen activation boundary. New members start live-from-beginning. Process historical windows newest to oldest, chronological within each window. Persist generation/digest-bound UTF8 fragment positions for oversized prose or explicit unprocessed status; never consume an unread suffix. A generation change resets only that member's coverage under a newly captured boundary and preserves item/evidence/retirement history. Startup plus bounded cyclic revisits recover missed kicks and generation-only changes.

A private DB-minted batch identity binds owner, actual member, generation, lane/frontier, exact source digests and registry revision. It is distinct from native SourceToolCall/ToolExecuting authority. Validate a closed tool-free result, captured candidate IDs and every exact quote. The model returns captured fragment/message ID and verbatim quotation; the host resolves an unambiguous UTF8 span instead of asking the model to count bytes. Ambiguity, forged/missing source, malformed/truncated output or incomplete processing leaves coverage pending.

Atomically compare owner/membership, source generation/content, frontier and registry revision before committing item/evidence changes with coverage. Conflict commits neither. New captures are unreviewed; appends invalidate ready resumption. Exact same-item locator/quote digest dedup ignores generation when text is unchanged. The scanner cannot overwrite notes, review, retire, dismiss, reopen, lift a pause or grant authority. Later explicit human renewal can be a distinct unreviewed foreground-visible request while the previous retired record stays closed. Ordering alone cannot discover cancellation outside a captured window; historical captures remain unverified pending later/current-source reconciliation. Semantic duplicate matching is bounded and measured, not perfect.

Bound encoded source and registry context to8KiB each and complete inference content to24KiB. Include at most16 compact candidates, preferring source overlap (including retired) then recent active/retired items. Title/context fields each use at most256 encoded bytes with clipping flags; disclose omissions. Do not include16 full notes. Use gpt-6-luna/Low only through its validated configured codex or official-direct connection, with no generic cheap-model/default/foreground fallback.

Initial hard limits are2048 output tokens,30-second timeout, one initial plus one retry per persisted exact-binding cycle,128 charged attempts/3MiB inference content perUTCday,16 attempts/hour and historical at most4/hour and64/day. Reserve durable daily/hourly attempt/input budgets before dispatch, including retries and restart. Charge the retry-cycle count in reservation itself. Same-binding recapture/new nonce/restart cannot reset it. After the second reservation, record a five-minute resume time; later cycles can retry under global caps so outages do not permanently poison a source. Real source/frontier/registry/prompt/route changes may start a new binding cycle. Do not promise exact billing or exactly-once provider calls. Select3live:1historical when both pending and limits permit, with fair bounded member revisits. Coalesced post-commit kicks reduce latency; startup/revisit discovery remains the authority. No inference blocks user admission or wakes foreground work.

Expose bounded read-only coverage through an additional nested operation on the existing tracking tool, at most8 members/page. Return captured generation, both boundaries/frontiers, omissions and pending/stale/incomplete/covered-through-tail states. Distinguish route unavailable, budget paused and retry/backoff. A live frontier alone is not all-history coverage. Existing lists/observations may capture compact counts; coverage bookkeeping alone never appends transcript observations, avoiding a self-feeding meta loop.

Before automatic capture, require the demonstrated manual journey, independently frozen synthetic corpus oracles and actual host lifecycle tests. Source-validity metrics include rejected proposals. Gates are100% exact valid evidence, zero false new commissions/restriction loss/reopen/authority violations, requested-work recall at least90%, bookmark capture at least80% and visible-candidate semantic duplication at most10%. Separately report omitted-candidate duplication, actual provider tokens and latency; byte caps are not token savings.

## Consequences

Capture can run while Ember is idle without scheduling work. Progress, coverage, explicit review and actual completion remain different facts. Old history may produce stale or duplicate unverified captures that Ember retires; there is no global cancellation registry, extra ranking service or new workstream dashboard. Eligibility, coverage and omissions remain inspectable rather than silently invoking an expensive model or claiming complete history from a live tail.
