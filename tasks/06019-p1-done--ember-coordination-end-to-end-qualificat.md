# Ember phase 8: end-to-end qualification and cutover

Parent: [06013](./06013-p1-done--ember-consistency-and-durable-tracking.md). Final combined qualification depends on delivered phases 1 and 3–7; [phase 2](./06015-p1-done--retire-stale-delivery-roadmap.md) retirement is independently verifiable. Each slice qualifies before integration: the [phase 4 event comparison](./06021-p1-done--enrich-ember-event-context-and-measure-f.md) must not wait for scanner delivery.

## Outcome

Demonstrate the complete user journey through the real assembled Coordinator surface, and prove which portions are shipped rather than confusing branch completion, notification acceptance and useful follow-through.

Qualification is incremental, not a final big-bang harness gate. First accept phase 5→6's manual record → detour → continuation → return journey with no scanner. Independently qualify phase 3→4's event retrieval improvement and phase 2's roadmap cutoff. Then qualify scanner auto-add against those proven contracts and run the combined journey. The latest product-direction review requests no new implementation slice or executive-oversight subsystem.

## Validation journeys

Before scanner activation: manually record a user request and conversational bookmark, detour/continue, and answer “where were we / what am I forgetting?” through bounded inspection with an honest stopping point/source. Verify historical note versus new evidence versus reconciled next step in both receipts and compact context. Source gaps/overflow remain explicit. Also verify that nothing useful due produces no inventory recital, worker nudge, alternating unchanged-item check or new wakeup. Existing Goal/turn/PR completion cannot resolve the broader item automatically, nor can the open item diagnose worker stall.

1. User requests two meaningful pieces of work in Ember, then detours into a separate exploratory question. Direct tracking records one request, scanner captures the omitted request and a useful conversational bookmark with exact source evidence. The curiosity remains a bookmark, not another commissioned task. Worker-only promises are outside discovery scope. Returning to the question supplies a short refresher and resumes from the stopping point, as in Issue #1075.
2. Continue Ember; fresh tracker index retains identity and owed step despite a deliberately terse handoff. Exact source evidence remains historical; current worker target follows continuation.
3. Dismiss a stale record while inference is in flight. Stale apply changes neither registry nor cursor; next scan sees retirement. Rediscovering old source cannot reopen that existing retired record; a semantic duplicate under a new identity is measured separately, not incorrectly claimed impossible. A later explicit user renewal may create a distinct provisional unreviewed record; the old retired identity stays closed.
4. Interrupt before/after capture/cursor commit; reopen the same supported DB. No partial committed capture/coverage or duplicate exact application. Model-call repetition is not represented as exactly-once billing.
5. Deliver new tail while the same transcript's backfill is incomplete; verify the gap stays explicit. Include an old request canceled far beyond its historical chunk and verify no action follows unverified historical capture. Make source too large, edit old source, disable the chosen cheap route, and return truncated/invalid responses. Coverage stays honest, limits hold and normal input admission continues.
6. Message accepted/queued, watch event received, reminder due and Goal applied remain distinct from verified completed follow-through. Tracking does not wake/rearm stopped work or bypass user pauses/Authority.
7. Test local discovery/action/observation, selected-peer query/read/watch, and exact continuation using installed definitions and real DB admission. Preserve no-local-fallback and explicitly local-only operations.
8. Retire roadmap automation with workflow/runs/variable/Issue readback, while retained historical evidence can still be retrieved. Reconcile only confirmed live obligations; no blanket import.
9. A finishes, a different message starts B, and B responds before A's delayed event reaches Ember. Inspect A's exact settled input, sender and bounded ending response without borrowing B's text. Test tool-only/empty endings, continuation, steering settlement races, unavailable source, edits/deletion, historical absence, unsupported remote evidence and private/shared projections.
10. Event-heavy history reaches the scanner. Copied worker excerpts can support existing records but cannot create human commissions; pauses and intentional drops survive reconciliation. A completed execution/milestone does not resolve an unfinished user outcome.
11. Initial discovery enrolls the whole Ember history without a date cutoff, incrementally and behind new-message work. An idle scan can commit captures without starting a foreground turn; the next normal turn or existing watch/reminder wakeup receives the fresh index. No new periodic review wakeup is created.
12. Oversized event response: edit only its uncaptured conclusion before follow-up, then separately edit it between read pages. The pinned read path returns explicit changed/historical-unavailable evidence; separately labelled current text cannot be represented as occurrence-time content.
13. Store an actionable next step, miss a user pause, then attach that pause through scanner evidence. After continuation, list/detail/overflow exposes review-required and cannot expose the stale step as ready. Only trusted revision-checked reconciliation clears review state, retaining the pause; concurrent append conflicts.
14. Complete transcript coverage through sequence 100 at G, edit message 5 to G+1, append nothing, miss the kick and restart. The existing runner discovers and durably invalidates/reset-rescans the transcript incrementally. Existing item/retirement history survives, unchanged exact evidence is not duplicated and changed content is not suppressed as an old application. Bound conflict retries and prove progress after contention stops.
15. Update the registry/page between existing turns and inspect assembled provider requests: changed tracking context appends at the tail while the system block and earlier tracker-independent prefix remain stable. No-change loops/retries do not manufacture new snapshots; an idle scan starts no foreground turn. Measure accumulated context and provider behavior separately.
16. User commissions work, detours and continues Ember; an existing watch/reminder or normal turn recovers the owed next step without the user naming the item. Qualify phase 6's chosen attention policy against pagination starvation, relevance noise, repeated healthy-worker nudges and ignored saved requests. Capture success and a visible list alone do not pass.
17. Two recently reviewed unchanged requested-work items and many turns inside the review interval do not alternate into repeated queries or worker messages. At most one eligible item is surfaced, often none. New evidence/due existing follow-up may change eligibility; clock expiry does not start a turn or lift a pause.
18. Recover a user request from two continuations ago, absent from the current handoff, because it connects to today's topic; separately recover quiet unrelated work through occasional review. Both paths share one cue/cooldown, deduplicate the same item and avoid starving quiet requests. Existing known-worker completion subscriptions alone do not satisfy this forgotten-intent test and acquire no second checkpoint lifecycle.

## Evidence and harness

Use production constructors and sanitized disposable runtimes. Extend/add a purpose-built isolated QA harness; the existing coordinator-drive-eval uses four fake tools, canned SQL and archived prompts and cannot prove full tool or live continuation behavior.

Record commit, configured model/effort/endpoint, catalog/language, prompt/schema hashes, failed-call/identity-error counts, context bytes or actual tokens with named measurement, scanner coverage/latency/usage and capture-quality review. Synthetic tests prove lifecycle invariants; real-model trials assess behavior separately; user acceptance proves utility. Do not substitute source assertions or server health for those later layers.

Compare native-first/clearer-target guidance to baseline before promoting new discovery helpers or wrappers. Do not require a universal API wrapper to finish.

Carry forward phase 4's isolated A/B/C event results: current packet; exact evidence/excerpt/trigger; then compact labels. Compare correct next decisions, request counts, total packet/tool-result context, follow-up source reads and latency with model, effort, endpoint, prompts, tools, corpus and budgets held constant. Report tokenizer/cache/replay assumptions. Production temporal associations and wording-only token estimates are baselines, not realized savings. Do not activate filtering/coalescing merely because packets became shorter.

## Repository and delivery checks

Use ./dev.py check-plan for changed lanes, then required task/spec/allium/Rust/fmt/clippy and presentation checks. Honor existing host execution/resource owners; no unsolicited production restart/deploy. Ensure generated wire files, if changed, are regenerated via dev.py. Independently review concurrency/retention and provider-boundary changes. PR descriptions expose phase dependencies and outstanding mandatory tasks.

Done when each phase has checkable evidence, live deployed revision/controlled acceptance is distinguished from merge, user can recover an owed next step after continuation, and roadmap updater/posting instructions are retired. If a layer is not exercised, retain the explicit gate rather than marking the whole program complete.

Out of scope: inventing new wake or suppression policy, expanding remote event protocol, a broad search rewrite, production mutations outside the delivered slice and substituting planning checks for runtime qualification.


## Current qualification map

This table records exercised boundaries; it does not replace the eighteen
obligations above or claim that synthetic lifecycle success proves model usefulness.
Merged components are #1091 retirement, #1096 tool audit, #1093 manual tracking
(`5b0e01c09`) and #1092 event evidence/context (`3a842c5`). Delivery of the
qualified scanner implementation is tracked in PR #1098. The third frozen scanner
trial at `e978b7686` was independently accepted after typed host chronology,
review of all 37 prepared inputs, twenty-two DB tests, owning all-target Clippy,
actual host fixtures and fresh source reviews. The later topology repair passed
twenty-five DB tests at `6c11894f7`, followed by Clippy, host fixtures and offline
input equivalence at `74881f401`. Both earlier semantic trials remain rejected.
This completes implementation and qualification; exact review, CI and main
integration are tracked by GitHub PR #1098.

| Journey | Concrete qualified evidence | Qualification scope |
|---|---|---|
| 1: Omitted work and bookmark after detour | Manual real-model request/bookmark recovery; actual-input scanner capture/index composition; actual 37-case prepared host fixtures | Accepted e978 scanner gates; B04 bookmark miss remains within 4/5 threshold |
| 2: Continuation identity and stopping point | Four runtime continuity/authority regressions and actual scanner/pause composition through two continuations | Controlled production-runtime lifecycle evidence; PR #1098 delivery state |
| 3: Retirement during inference and renewal | DB retirement/CAS race, closed candidate readback and distinct unreviewed renewal with old record closed | Third trial passed all visible-retired negatives with typed host chronology; earlier failures retained |
| 4: Atomic capture/coverage and restart | Classified transaction, exact application replay, durable reservation/crash and file-reopen host tests | Transaction/restart evidence; no physical power-fault claim |
| 5: Live gap, incomplete/unavailable source and failures | DB partial Unicode, malformed/oversized source, live gaps/fairness; runner route/output/timeout bounds | Accepted frozen semantic gates; outside-window H01 remains historical/unverified |
| 6: Receipts, pauses and authority | Typed send/event/Goal distinctions, scanner no-dispatch authority, actual pause/quiet foreground and no-wake composition | No second worker-checkpoint lifecycle or new authority |
| 7: Local/peer/exact routing | Merged native federation/no-fallback and local-only schema regressions; accepted assembled inventory | Existing routing contracts preserved; no remote enrichment expansion |
| 8: Roadmap cutoff | #1091 independent workflow/variable/Issue readback and historical retention | Complete |
| 9: Exact ending versus newer response | Event capture/settlement/read/privacy regressions and accepted complete 39-trial C2 comparison | Component merged; deployment unclaimed |
| 10: Event reports, corrections and broader outcome | Host machine-source restrictions and append-only scanner authority; actual origin-preserving corpus inputs | Third trial: all restrictions/negatives and source-authority gates passed; first two failures retained |
| 11: Whole-history progress and no wakeup | Own-chain/member enrollment, historical/live fairness/gaps and file restart; repaired discovery-page cursor covers fourteen transcripts/thirteen actual continuations and revisits middle generations; idle scanner → next normal admitted input fresh index | Host lifecycle and separate semantic gates passed; deployed backfill unclaimed |
| 12: Edited uncaptured tail and paging | Whole typed-response digest, explicit changed/unavailable reads, between-page edits and C2 controls | Component merged; no mixed-version quotation claim |
| 13: Missed pause and stale resumption | Storage CAS/overflow/review-required tests plus actual scanner append → two-continuation invalidation | Host review-state/CAS proof plus third trial 13/13 restriction attachments |
| 14: Old source edit without append/kick | DB30 includes the `89ff24d7b` case: sequence-5/covered-through-100 metadata restart/dedup, then external valid-prose/new-generation rewrite, reopen/rescan/new evidence with old history retained; contention recovery | Externally changed source fixture; no generic user-edit API claimed |
| 15: Immutable prefix and bounded context | Assembled request prefix/no-change snapshot regressions; manual/foreground actual request and token accounting | No provider cache/billing savings inferred |
| 16: Useful owed next step | Accepted manual `aca480f4a` and foreground `32ada836a` due-item → specific question → real user pause → revision-checked update | Controlled synthetic usefulness; general utility/deployment unclaimed |
| 17: Two recently reviewed items stay quiet | Accepted foreground arithmetic returns only its answer with zero tools/cue after actual pause reconciliation; durable cooldown tests | No wall-clock wakeup or healthy-worker polling introduced |
| 18: Contextual hidden versus quiet neglected request | Accepted `32ada836a`: beyond-eight-entry/two-handoff Harbor recovery via bounded reads/original source and owed question; separate Cobalt due recovery, shared cue/cooldown | Fixed scenarios, not a general relevance reliability claim |

The accepted [foreground report](../scripts/qa/ember-foreground-memory/foreground-32ada836a.md)
preserves its earlier failed/harness-rejected runs and presentation limits. The
[event report](../scripts/qa/ember-event-eval/results-cfda0b0.md) preserves the first
capped experiment. Scanner [first](../scripts/qa/coordinator-scanner-quality/results-ec570d0f3.md)
and [paired](../scripts/qa/coordinator-scanner-quality/results-db6f2b44f.md) reports
retain both complete rejected trials, raw/host effect distinctions and exact hashes.
The [accepted third trial](../scripts/qa/coordinator-scanner-quality/results-e978b7686.md)
records 11/11 requested units, 4/5 bookmarks, 13/13 restrictions and 29/29 valid
quotes; zero false commissions/21, all seventeen negatives correct, zero visible
duplicates/11 and no authority/reopen violations. B04 is the allowed bookmark
miss. L01/L02 pass, H01 remains historical, and I01's one genuine invisible
duplicate is reported separately. Root and fresh independent review inspected all
raw results, record hashes and host effects before acceptance.

The final assembled inventory matrix and export passed at `89ff24d7b`: eight
language/catalog/spawn variants, 28/29 tools without/with authenticated skills,
and tracking's eight operations including coverage. Export SHA256:
`d4a4f557a1eeb059c52dafd4b69fd9718d4f3a598492e88ae8e0eef29d2fad7f`.
This is configuration-scoped evidence, not production inventory.

Implementation and qualification for the existing eight phases and eighteen
journeys are complete within these stated boundaries. No further feature slice or
acceptance campaign is introduced. Delivery PR #1098 owns exact review, CI and
main integration. Local
model/host evidence, merged source and deployed/physical-device behavior remain
explicitly distinct.


Final host-only follow-up passed twenty-five DB tests at `6c11894f7`, then
owning DB/IDE all-target Clippy and all 37 host fixtures at the subsequent
binding-name-only head `74881f401`. Discovery-cursor advancement
uses only the eight-member discovery page before priority-head inclusion; scalar
actual-head lineage, exact membership and eight-plus-lookahead coverage avoid
runtime/handoff aggregate decoding. Metadata returned to scanner/Rust is bounded;
SQLite internal working data and recursive traversal/count work can scale with
lineage; constant CPU is not claimed. Long-chain, malformed irrelevant
metadata/genuine pre-corruption handoff and explicitly detached-source regressions
passed. Offline preparation made zero provider calls and matches all 37 accepted
inputs, prepared records, prior states, input hashes and bindings byte for byte.
The [accepted report](../scripts/qa/coordinator-scanner-quality/results-e978b7686.md#host-only-topology-follow-up)
retains the original `e978b7686` model trial and appends the later host receipts.


Later host-resource/fairness qualification at `1824f48d3` passed thirty DB tests,
owning DB/IDE all-target Clippy, scoped Allium and all 37 host fixtures. One shared
32-fetched-source-row allowance covers the complete preparation across members
and lanes, charged before errors/backoff with known spending checked before
prose. Blocked exhaustion persists opposite lane preference even without a route;
committed non-prose Progress uses ordinary preference without an inference charge
or blocker clearing, and Ready keeps existing reserve behavior. Restart in both
lane directions, route absence, continuous non-prose progress and actual hourly
caps are covered. All 111 offline artifacts/37 hashes/bindings match the accepted
preparation with zero provider calls; the [resource follow-up](../scripts/qa/coordinator-scanner-quality/results-e978b7686.md#later-host-resource-and-fairness-follow-up)
records exact provenance separately from paid model and topology results.
