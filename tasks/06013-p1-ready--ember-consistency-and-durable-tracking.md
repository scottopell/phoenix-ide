# Production-informed plan: useful Ember events and durable tracking

## Outcome and user decisions

Make Ember's existing coordination surface easier to use, preserve the user's requested work and conversational stopping points across continuations, and retire the stale roadmap. Improve the event-driven reading journey first; deliver tracking and scanning as a separate dependent path. This task owns the full program; planning completion is not runtime implementation.

Accepted user decisions:
- "things ember is keeping track of is right"
- "the scanner auto adds, put the burden on ember to remove if it’s truly stale, this would then be visible to next scan"
- "deprecate roadmap as part of this, it’s so stale and infrequently used"
- "Wait there is only one ember global coordinator though"
- "1) use recommendation": one item per independently meaningful concern/commitment, with its next resumption point; no parent/child task hierarchy initially.
- "2) ember only, focus is less on promises the agents made and more on user requested work, see adhd related prompt suggestion in gh issues"
- "Agree good suggestion, finish revisiting and refining plan based on this and any other production analysis you want to do": approval of enriching the existing completion event with triggering context, exact ending-response reference and bounded excerpt, following the observed repeated latest-message lookups.
- "Normal turns and existing wakeups (Recommended)": review saved items during normal Ember turns and existing watch/reminder wakeups; no new periodic review wakeups.
- "All history, gradually (Recommended)": scan the whole Ember continuation history in bounded batches, prioritizing new messages; no initial date cutoff.

There is ONE existing Ember singleton and ONE tracking list attached to its durable product identity. Remote instances are places referenced work lives, not additional owners of this list.

[Issue #1075](https://github.com/scottopell/phoenix-ide/issues/1075) supplies thread-keeping behavior: preserve user requests, meaningful unanswered questions, decisions and where exploration stopped; a curiosity is a bookmark, not a new execution commission. Answer the current question first; avoid routine inventories and topic-switch permission requests; provide a short refresher when returning. Topic change alone neither abandons prior work nor makes it highest priority.

## Verified baseline and limits

Initial source review used e74290876. The production study inspected RC5 at 4da5f81c60d65d548254264003540bc7d462d2b0; the planning branch was then rebased onto main at 6316b31a0. Re-ground implementation against the then-current source and installed definitions.

Production contains one singleton chain with 48 transcripts, 63,657 messages and 23,721 tool calls through 2026-11-14 21:10:36 UTC. The descriptive recent window is Oct 15 21:10:36 through that cutoff. Queries used the production database read-only with bounded execution/results and no database copy or service mutation. Per-transcript generation checks observed no changes during reads; this is not a database-wide atomic snapshot.

Publish sanitized aggregates and synthetic scenarios only. Raw messages, production identifiers, private excerpts, extraction artifacts and credentials remain local. Aggregate methodology/limitations are recorded below; private local evidence is under .phoenix/ember-analysis and is not a hidden implementation dependency.

### What the production evidence changes

| Observation | Planning consequence |
| --- | --- |
| 6,849/7,985 event-to-next-input windows first used SQL; 6,978 included message-reading SQL | Enrich the existing event before adding another discovery tool. |
| Of 120 evenly-spaced message-reading queries, 118 requested latest messages, 102 literally matched the event's source, 106 filtered assistant messages; only 1 requested origin/trigger metadata | Prioritize the exact ending response; sender/trigger is useful context, not a substitute for the response. |
| 7,985 subscription events versus 674 user_api rows in the recent window | Preserve origin and quotation structure; user-role text is not equivalent to new human requests. |
| About 95% of recent event messages report turn completion; traced rapid repeats had distinct incoming messages from other conversations | A completed execution is not a finished user outcome. Do not infer duplicate delivery or invent a queue-delivery event. |
| Current deployed create/rename use nested requests; create/rename/continue and Goal operations have peer support | Remove stale local-only/schema-repair assumptions. Preserve delivered fixes and test current contracts. |
| Historical errors include an organic local-watch routing incident and intentional negative acceptance tests | Evaluate recovery from deterministic errors; don't use pooled error flags as current tool-quality scores. |
| SQL is fast (recent median 15 ms) and widely used for narrow latest-message reads | Keep SQL. Measure the observed retrieval journey rather than replace it wholesale. |
| Recorded search samples were slow and already had an investigation owner | Reconcile that owner before commissioning another search initiative. |

Temporal windows are associations, not complete execution-causality attribution; the 120-call sample is descriptive rather than a statistical estimate. Tool success does not establish recipient execution or user-outcome completion. Current post-deploy calls are a small mixed usage/qualification cohort, not a general reliability verdict.

## Existing tool surface and targeted changes

The measured installed registry has 27 tools including the optional authenticated skill catalog. This is an observed inventory, not a second tool authority: regenerate through RuntimeManager::coordinator_tool_registry before changing contracts.

| Family | Existing tools | Problem and planned treatment |
| --- | --- | --- |
| Reasoning/delegation | think, spawn_agents, optional skill | Keep their separate roles; inventory configured spawn options and actual expanded guidance, not only the base registry fixture. |
| Evidence/navigation | search_conversations, read_conversation, query_database, resolve_reference | Latest-response retrieval dominates measured follow-up. Put exact ending evidence in the event; retain narrow SQL and exact historical reads. Existing search-latency owner handles search. |
| Execution/presentation | bash, work_scope_load, present_svg | Preserve explicit work scope and presentation capabilities. Bash/API remains an escape hatch; no wrapper per endpoint or universal consolidation. |
| Coordination | send_conversation_message, ask_user_question | An accepted/queued send is a receipt, not execution or user-outcome completion. Preserve trusted sender/origin facts and question-answer boundaries; no extra send alert. |
| Events/time | watch_conversation, unwatch_conversation, list_watched_conversations, reminders | Completion packets lack useful ending text. Capture exact evidence and compact projection; describe supported waits accurately. Watches remain future facts; reminders remain one-shot input. |
| Conversation lifecycle | create_conversation, rename_conversation, continue_conversation, read_automatic_continuation, set_automatic_continuation, retry_close_safely | Preserve delivered nested schemas and qualified peer operations, plus operation-specific receipt/retry rules. Reconcile existing Close owner. |
| Goal metadata | read_goal, prepare_goal, apply_goal | Worker objective/disposition is distinct from requested work spanning conversations. Preserve current peer capabilities and busy/changed-intent refusals. |
| Standing scope | read_authority, set_authority | Authority remains standing scope/permission. Stored tracking prose, scanner interpretations and quoted worker reports cannot grant it. |

Installed descriptions/schemas totaled 41,923 characters; this is not a token count or full standing-prompt measurement. Recent SQL+Bash accounted for about 75% of calls and 83% of result characters; those totals do not demonstrate waste. Repeated SQL input within five minutes occurred 3,100 times, but only 59 had unchanged meaningful returned rows/columns/truncation. Favor measured journeys over a blanket polling/consolidation project.

Routing remains operation-specific: search is local; selected-peer read/query/resolve/send/watch and lifecycle/Goal capabilities retain their actual contracts and unsupported cases. Never reinterpret an unknown peer as local. Current-target ProductConversation, exact transcript/message, generated summary, run ordinal, revision and prepared reference retain distinct meanings.

## Keep the domain responsibilities distinct

| Concept | Responsibility |
| --- | --- |
| Completion event | A recorded execution/wait/failure fact with occurrence-bound evidence |
| Tracking item | User-requested work or meaningful bookmark, purpose and where to resume |
| ProductConversation / transcript | Stable execution target / exact historical evidence |
| Goal | One worker conversation's objective/disposition |
| Authority | Standing instructions and delegated permission |
| Watch / reminder | Future factual input / one-shot timed input |
| Task / PR / deployment | Repository work status / review state / shipped evidence |

Tracking does not mirror PR/runtime state, create Goals, issue commands or grant authority. Event acceptance, message delivery, Goal application and execution completion cannot independently resolve a tracked user outcome.

## First behavior slice: useful existing completion events

### Bind the right evidence

Reuse DirectTurnTerminalObligation.response_message_id, selected by RuntimeExecutor from the actual terminal PersistMessage effect. record_terminal_event_tx already reads the canonical input and runs before deleting the terminal obligation. The outer terminalize_authoritative_turn_with_occurrence_at_cut must capture one typed immutable ending binding before terminalize_authoritative_turn_in_tx deletes that obligation, then thread it to both direct and applicable captured-steering event writers. AlreadyTerminal/TerminalReplay must use original occurrence-bound evidence or explicit absence, never combine a new adopted input with an older completion. Capture exact non-direct evidence through its owning settlement boundary; never search for the latest assistant message during delivery or prompt rendering.

Persist a typed completion-evidence record atomically with the existing outbox occurrence:
- Exact settled-input locator and typed origin/sender facts where known. Do not claim one input was the sole cause if others were co-consumed.
- Exact ending-response locator, with captured prose versus tool-only/empty/no-carrier/historical-unavailable cases.
- A bounded verbatim occurrence-time excerpt with explicit truncation and source-version provenance.
- A versioned full-response canonical-content digest for guarded follow-up reads, not a hash of only the captured prefix.
- Normalized fields/ordered child rows for addressable evidence, not a JSON collection or a second full transcript.

source_generation is an execution-occurrence generation, not a transcript-edit fence. Bind content/version provenance explicitly. A later edit, deletion, continuation or newer response must never replace the captured evidence with different text. Define the bounded historical-quotation retention contract, including recipient history and source deletion, in requirements/ADR; do not introduce a generic source-retention guarantee.

Old events receive an honest historical-not-captured state, not a guessed backfill. Preserve original persisted diagnostic/provider text and already accepted identities. No new send/queued-delivery notification, model-written summary, work scheduler, suppression or wake-timing change.

Exact message identity does not freeze its content. Emit an occurrence-bound evidence reference that preserves the expected full-response digest through the existing read surface and every page. Validate and read from one consistent snapshot; a changed/deleted source yields explicit changed/historical-unavailable evidence rather than an edited occurrence-time tail. Current-source navigation/text remains separately labelled. This narrow composition needs neither full historical-response storage nor a universal retrieval wrapper.

Tool-only endings have an exact history pointer and honest lack of prose. Do not build outgoing-report fragment extraction by default; consider it only if evaluation demonstrates need and exact execution attribution is available.

### Render and qualify

Use the same typed captured evidence for model-private context and private UI details. Model context keeps its factual-origin marker once and avoids duplicated identifiers/verbose labels. Copied worker prose remains evidence, not user authority. Preserve the original diagnostic text; shared/redacted reads must not acquire private excerpts or provenance through typed fields or raw content.

Start controlled evaluation with both capture and render caps at 1,024 UTF-8 bytes of response prose, Unicode-safe clipping and explicit omitted-content indication. Persist capture-limit/version and omission provenance; rendering cannot supply more than was captured. Tuning a higher capture cap applies to new occurrences only; old excerpts remain explicitly truncated and cannot be rehydrated from edited source. Freeze these bounds in phase 1 and tune from truncation and total-context results before activation. Include trigger sender/identity context, not a copied triggering-message body by default.

V1 is local-first. Remote terminal-fact v1 uses a closed wire contract with no enrichment fields. Preserve existing remote facts and an explicit unsupported/not-captured enrichment state, with a typed/logged capability boundary. Remote propagation requires its own producer/receiver contract and qualification before claiming parity; no compatibility defaults or local fallback are implied.

Compare A current packet, B occurrence-bound pointer/excerpt/trigger, and C B plus compact labels under the same model/effort/endpoint/tools/prompts/corpus/budgets. Measure correct next decision, request/tool count, total model-visible packet and tool-result context, follow-up retrieval, latency, source attribution and pauses/authority. Declare promotion thresholds before trials: no source/authority failures in required cases, reduced targeted latest-message retrieval, and explicit limits on total-context/latency regressions in control cases. If gates fail, revise rather than activate or claim improvement. Packet length alone is not success. Fresh live-state reads may remain necessary.

Offline wording-only compaction estimated 33.72% fewer event-text tokens using o200k_base while retaining parsed fields and the factual marker. That is not an exact endpoint-tokenizer, replay/cache/billing or enriched-packet improvement claim. The enriched packet can be larger yet avoid repeated queries and model round trips.

## Durable tracking and reliable return

Use a small record: stable ID, title, purpose/current-understanding note, resumption/follow-through, tracking/resolved/dismissed disposition, nonblank retirement reason, revision and checked integer-microsecond timestamps. Last reviewed means an explicit review, not a scan. Preserve the difference between a bookmark and requested work without creating a second task hierarchy or permission system.

Typed references/evidence are ordered child rows. Distinguish instance-qualified current ProductConversation targets, exact transcript/message evidence, and repository/PR identity. Do not cache authoritative PR/worker status. A missing source remains unavailable evidence, not completion. Freeze feature-scoped retention and exact retry/revision receipts before implementation.

Expose one compact bounded tracking capability for list/read/record/update/retire/reopen through one application service. Trusted invocation identity and revision checks apply; scanner uses the same service. Retained retirement reasons remain visible to later scans. Explicit Ember decisions may reopen on new evidence; the scanner cannot reopen/retire, overwrite Ember's prose, or turn its interpretation into authority.

New evidence since explicit review invalidates the saved resumption step. Derive review-needed from ordered evidence and a trusted reviewed watermark, without a second mutable flag or model restriction classifier. Scanner-created items start unreviewed. List/detail/context returns a review-required variant instead of presenting stale prose as a ready next step; overflow exposes omitted/review-needed counts and read pointers. Trusted Ember reconciliation reads new source evidence and clears the signal only under matching revisions, preserving pauses. Scanner cannot clear it or lift a restriction; reviewed never implies permission.

At every Coordinator dispatch, including after continuation, provide a fresh bounded active index independent of the generated summary: registry revision, count, entries/pointers and explicit overflow/pagination. Full notes/evidence use bounded reads. Keep /global chat-first; use existing conversation/tool surfaces, with any added bounded inspector's web/native scope explicit.

Paused work stays visible with its restriction and resumption condition; completed milestones must not hide unresolved user outcomes. Preserve user corrections and dropped investigative threads so scanning does not revive them.

## Scanner safety net

Discovery is ONLY Ember's continuation chain. Focus on user requests and useful unfinished questions; assistant prose supplies context/stopping points. Worker/inline-child transcripts and raw tool dumps are excluded from discovery. Ember may inspect a linked worker to follow up; that does not enroll it in scanning.

Pass explicit origin, quotation and question-answer structure. Subscription events and enriched worker excerpts can support reconciliation of an existing item, not establish a new human commission. Even user_api includes quoted replies, button templates and structured answers; parse intent in context rather than treating each row as independent requested work.

Use one tool-free, bounded, low-priority inference runner. A durable checkpoint plus startup discovery recovers missed post-commit kicks; do not await inference on foreground input admission. Use an explicit eligible cheap route, input/output/time/retry/backfill budgets and visible coverage/failure. The existing cheap selector's default-model fallback cannot silently choose expensive work.

Snapshots bind transcript generation, message/fragment position and registry revision. Append does not bump transcript_generation; edits/recovery do. Oversized messages need a generation/digest-bound fragment position or explicit unprocessed state. A complete valid empty result may advance its exact range; failed/partial/truncated/invalid output may not.

Committed coverage is also generation-bound. The same runner discovers generation changes without a new tail, including after restart or a missed kick. Conservatively mark that transcript's old coverage stale and reset its new-generation positions to the beginning with a new frozen live boundary, retaining all saved items/evidence/retirements. Rescan incrementally; do not silently carry forward an old completed frontier. Generation/range identifies batch application; unchanged exact source evidence for the same identified item is not appended again, while changed content is a new observation requiring reconciliation. No second scheduler or foreground review wakeup is added.

Apply captures/evidence and coverage atomically under source/cursor/registry revision checks. Dismissal/edit conflicts commit neither stale additions nor coverage. Model calls may repeat after interruption; committed range application must not repeat. Host-owned batch/application identity is separate from model semantic matching. Two commitments in one message stay representable; changed wording may still produce duplicates for Ember to dismiss.

Freeze the activation boundary with separate live/backfill frontiers so new messages do not cover historical gaps. Retain historical provenance and uncertain freshness; later cancellation/completion may lie outside an old chunk. Ember checks current/later evidence before acting on historical capture.

## Settled attention and historical coverage

- Saving a bookmark/request retains memory. Ember reviews saved items during normal turns and existing watch/reminder wakeups. Saving or scanning does not create periodic supervision, dispatch work or change existing wake timing.
- Initial backfill covers the whole Ember continuation history gradually in bounded batches, with priority for live messages and explicit incomplete coverage until processed. There is no date cutoff and saved items do not expire based on age. Historical provenance and later-correction checks still apply.
- Temporal coalescing, subscription filtering and routine-completion suppression are deferred until enriched-event measurements identify remaining cost. Approval of enrichment does not approve those behavior changes.

The user selected both recommendations. Remaining model budgets, excerpt tuning and qualification thresholds are implementation gates within this scope, not unresolved choices about periodic review or a historical cutoff.

## Delivery phases and dependencies

| Phase | Scope | Task |
| --- | --- | --- |
| 1 | Current-deployment baseline and narrow contracts | [06014](./06014-p1-ready--ember-tool-contract-and-consistency-base.md) |
| 2 | Independent roadmap retirement | [06015](./06015-p1-ready--retire-stale-delivery-roadmap.md) |
| 3 | Exact completion-evidence binding | [06020](./06020-p1-ready--bind-watch-events-to-ending-response-evi.md) |
| 4 | Useful bounded event context and incremental qualification | [06021](./06021-p1-ready--enrich-ember-event-context-and-measure-f.md) |
| 5 | Durable tracking store/application service/tools | [06016](./06016-p1-ready--ember-durable-tracking-store-and-tools.md) |
| 6 | Fresh continuity context and inspection | [06017](./06017-p1-ready--ember-tracking-continuity-and-inspection.md) |
| 7 | Cheap origin-aware scanner | [06018](./06018-p1-ready--ember-cheap-transcript-safety-net.md) |
| 8 | Final cross-feature acceptance/cutover | [06019](./06019-p1-ready--ember-coordination-end-to-end-qualificat.md) |

Dependencies are feature-local: the event-relevant baseline/contract in phase 1 unblocks 3→4; the tracking-relevant subset unblocks 5→6→7. Neither requires all of phase 1 to finish. Phase 3 owns its occurrence/retention/privacy/capture contract, phase 5 its record/native-tool/reconciliation contract, and phase 7 its frontier/invalidation/application/model-budget contract before activation. Scanner design cannot block events or direct tracking. Phase 2 is independently shippable; phase 8 combines delivered slices. Each slice qualifies before integration; event measurements do not wait for the scanner.

Use logical PRs with the mandatory phase tasks visible. Ready filenames denote planned work, not satisfaction of dependencies or delivery.

## Retire the roadmap without erasing evidence

Supersede ADR-069; preserve REQ-ROADMAP-001–014 IDs/original text as an explicitly historical, non-normative protocol. Update executive and active AGENTS/canonical development-skill duties. Reconcile active task instructions without erasing quoted commissions or unrelated work.

Remove reducer workflow, implementation and dedicated tests plus their CI invocation deliberately. Closing the Issue alone does not stop the reducer. Live cutoff: inspect current runs/config/body; disable and quiesce the updater; clear roadmap variables; remove activation marker and add a historical-only notice while retaining projection/comments; verify stable state. Branch deletion of automation is not proof live automation stopped.

No bulk import or dual writing. Carry forward only currently confirmed obligations against actual owners, tasks, PRs and user instructions. Specs, ADRs, task filenames, PRs and deployed evidence keep their existing authorities.

## Qualification and bounded risks

Required event scenarios: A ends, B responds before A delivery; exact steering/interaction race; tool-only/empty ending; oversized Unicode and explicit truncation; source edit/delete; historical absent carrier; private/shared projections; peer legacy capability; replay/reopen; failure/wait/cancellation/mandatory cleanup and unwatch/continuation invariants.

Follow-up evidence tests include an oversized response whose uncaptured conclusion changes and an edit between read pages. No changed tail or current surrounding context may acquire an occurrence-time label.

Required memory scenarios: user detour and return; explicit request versus bookmark; quoted correction and structured answer; pause preserved until lift; milestone done while user outcome remains; intentionally dropped investigation; two items in one message; retirement/inference race; changed wording duplicate; partial output; unavailable cheap route; same-transcript live tail during backfill.

Also require missed pause → evidence append → continuation → stale-step attempt, including overflow and revision-raced reconciliation. After completed coverage through sequence 100, edit message 5 with no append, miss the kick and restart: invalidate/reset and finish bounded rescanning, distinguish new content from exact retries, and avoid duplicate exact evidence. Contention attempts stay bounded and recover after contention stops.

Reuse the existing current routing work, task 28007's event presentation and its owner, and the existing search-performance investigation. Reconcile native Close work with its current owner instead of declaring every specialized tool permanent or launching another Close project.

The initial 18 disposable SQLite checks cover transaction/coverage sketches and a semantic-duplicate counterexample, not production Rust/model correctness. Existing coordinator-drive-eval has four fake tools/canned SQL/archived prompts and cannot qualify the complete path. Use production constructors and sanitized disposable runtime fixtures, with model/cost trials separate from deterministic tests.

Three independent review lenses (product, reliability and agent usability) informed this plan; final review tightened settlement ordering, capture/render bounds and promotion criteria. Preserve the existing conversation identity/lifecycle program (task 92009). Use Allium where scanner snapshot/apply/coverage or settlement concurrency warrants a precise lifecycle; ordinary CRUD does not independently require it. Real-model scanner evaluation uses independently reviewed expected captures, not the model's own recall grade.

The [incoming adversarial plan review](https://github.com/scottopell/phoenix-ide/pull/1082#pullrequestreview-5907499356) is incorporated in the existing slices: feature-local gates (1/3/5/7), version-checked event follow-up (3/4), mandatory reconciliation after evidence append (5/6/7), and post-coverage generation invalidation (7). These are planning contracts and required acceptance cases, not claims of runtime implementation or deployed qualification.

Select checks through ./dev.py check-plan. Run task validation and relevant spec/Rust/fmt/clippy/codegen/UI/native lanes for actual changes; honor existing host resource owners. Forward same-database recovery, historical quotation retention, excerpt budgets and remote capability are feature-scoped contracts; no downgrade, live DB replacement, cross-version or perfect-recall promise is implied.

Out of scope: broad tool consolidation, new wrappers around every API, already-delivered schema/peer repairs, shared trackers, remote corpus scanning, generic schedulers, automatic roadmap import, automatic scanner retirement/reopening, model-written event summaries and unapproved wake-policy changes.
