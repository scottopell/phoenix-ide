# Ember phase 6: tracking continuity and inspection

Parent: [06013](./06013-p1-in-progress--ember-consistency-and-durable-tracking.md). Depends on [phase 5](./06016-p1-in-progress--ember-durable-tracking-store-and-tools.md). This slice proves the feature helps after continuation before adding a model scanner.

## Outcome

Ember sees a fresh compact index of what it is tracking on subsequent dispatches and continuations, and the user can inspect the saved record and evidence without maintaining a second project-management system.

Priority 1 lane A completes the manual memory milestone with phase 5, independently of event enrichment and before scanner activation. The user's “where were we / what am I forgetting?” journey must work with explicitly recorded requests/bookmarks and no background extraction. This is the foundation's usefulness gate, not a new dashboard or comprehensive model-free executive summary.

Land that usable foundation before tuning sophisticated relevance or claiming automatic omission recovery. Basic foreground resurfacing and its quiet/no-op checks stay in this existing task; no new ranking/checker service is required to finish it. Use early real usage to identify missing visibility rather than expanding the first release preemptively.

## Work

- Add a bounded tracking observation at an already admitted Coordinator turn/wakeup boundary. Include registry revision, active count, review-needed state, entries/read pointers and explicit partial-list/pagination information. Pagination is user-approved; exact context budget and selection order still need qualification.
- Preserve the existing cached prefix with respect to tracking updates. Keep static tracker-use instructions in stable system guidance; changing registry counts, notes, timestamps and page selections must not rebuild the system prompt, be prepended before prior history, replace an earlier observation or mutate old tool results. ADR-066/PR #810 removed that changing-snapshot cache failure; do not restore it for tracking.
- Append a typed, system-generated data observation through the owning durable turn/projection boundary when the registry revision or selected page changes. Reuse it through model/tool loops and retries; no per-model-call snapshot or always-changing timestamp. Existing persisted meta observations and generation-fenced prompt tails are starting mechanisms, not proof that a tracker-specific carrier is already implemented. Freeze its exact append/replay identity before implementation. Idle scanner writes do not start a foreground turn.
- Mark prior observations as historical by their captured revision, without rewriting their text. Same-turn tool mutations supply committed revisions through their appended results; a later normal turn/wakeup obtains current tracking data. Stored notes remain untrusted data, not system instructions. Continue/restart must recover through durable evidence and current registry reads. Use existing continuation/capacity policies for growing history; do not invent deletion of earlier prefix content. Measure accumulated observation context as well as one page's size.
- Keep full notes/evidence behind bounded native reads. Never present a prefix as the complete or fully reviewed list. Define ordering and overflow behavior; no invented priority scores.
- Carry phase 5's mandatory review-needed state through list/detail/fresh context. New evidence since explicit review invalidates the saved next step; present it as unreviewed historical note, never a ready action. Require trusted source reconciliation before using it for follow-through. Overflow reports omitted and review-needed counts with bounded read pointers; omitted entries or an older context index cannot establish current readiness. Reconcile only the selected item, without blocking unrelated work or introducing a review scheduler.
- Make uncertainty readable: distinguish the saved historical next step, new evidence awaiting reconciliation, and a reconciled resumption point. Show source gaps and omitted entries. Do not present scanner coverage, explicit review or a recorded Goal/turn/PR outcome as a fresh comprehensive status assessment. Preserve these meanings in existing tool/conversation wording.
- Use stable singleton ownership through continuation topology. Exact source citations remain pinned; a current conversation link resolves its actual successor.
- Treat notes and extracted source as untrusted stored data. Do not promote prose into Authority or reported live state. Clearly separate a saved bookmark/request and supporting agent report from permission to act.
- Update global-recall requirements/ADRs for the deliberate new memory projection. Preserve the decision to fetch live activity on demand rather than reviving an inferred global activity capsule.
- Keep /global chat-first. Establish user inspectability via explicit read/list and rendered tool/conversation results. Any bounded list/detail control must name its web/native scope and validation; no new workstream navigation identity or dashboard.
- Distinguish record existence, last explicit review and scanner coverage. A successful scan does not mean Ember reviewed or completed the obligation. The scanner may add while idle without waking work.
- Follow Issue #1075's thread-keeping behavior: retain user-requested work and useful question bookmarks, answer the current question first, avoid repeating the list each response, and provide a short refresher/resumption point when the user returns. Topic change alone neither abandons prior work nor commissions a new task. User selected review during normal turns and existing watch/reminder wakeups. Saving or scanning does not create new periodic review wakeups or dispatch work.

## Attention and follow-through design

Persistence plus pagination is not acceptance of useful follow-through. The gap is forgotten user intent from earlier conversations/continuations, including requests absent from the current handoff, not known-worker turn settlement. Existing subscriptions/reminders already handle concrete execution checkpoints; do not invent a second checkpoint lifecycle or attention framework around them. Prove rediscovery without the user naming the old request, followed by current-source reconciliation and appropriate use of existing tools. Bookmarks remain non-commissioned; pauses and healthy progress remain respected.

User accepted that both resurfacing paths have a place, sharing one bounded cue surface rather than two independent systems:

- Contextual resurfacing finds an older user request with a useful connection to the current conversation. It must identify that connection without manufacturing a new commission. Start with existing foreground reasoning and bounded tracker reads; use cheap-model assistance only if separately justified and bounded, not an extra model call on every turn/tool round.
- Occasional resurfacing recovers quiet requests that may never become relevant on their own. Gate eligibility on unreviewed/new evidence or a meaningful wall-clock review interval, then prefer least recently explicitly reviewed with stable identity ties. Being oldest among two recently reviewed items is insufficient.

Share a cooldown, deduplication and an at-most-one cue budget per existing admitted turn/wakeup, often zero. A meaningful new connection/evidence may justify earlier attention; a burst of turns alone does not. Preserve opportunities for quiet requests when relevant ones keep arriving. Set the review interval, bounded context/read budget and simple arbitration before activation. No new periodic wakeups, random picker, priority scores, second checkpoint system or standalone ranking service. A concrete future wait/pause condition is not permission for repetitive owner queries.

Ember inspects and updates a surfaced item through existing tools, advances authorized work when appropriate, or records a concrete wait/restriction and moves on. Merely surfacing an item is not an explicit review. No useful eligible item means no extra review read or worker nudge to fill a quota. The paginated list remains available for the current request and other work. Avoid interrupting the user's question and report only material results/blockers. Current known-worker events continue through existing subscriptions; they do not alone demonstrate recovery of forgotten intent.

This is a planning direction, not activated behavior or a claim that prompting guarantees attention. Qualify both paths against forgotten-request recovery, pagination starvation, irrelevant detours, repetitive reviews and silent bookmark accumulation. Known-worker checkpoint handling is existing infrastructure, not a competing new feature.

Small-list regression: two recently reviewed unchanged items and many foreground turns inside the review interval produce no alternating review queries or messages. A new relevant fact or due existing follow-up can make an item eligible earlier; elapsed wall-clock time alone does not start Ember. Review selection and actual source review remain distinct.

Test a relevant request from two continuations ago that is absent from the current handoff, plus an unrelated neglected request recovered without a topical connection. The same item found by both paths produces one cue, quiet work is not starved and existing worker completion handling alone cannot pass either recovery test.

## Starting points

runtime/executor.rs durable meta observation persistence, active prompt projection/dispatch and stable system construction; system_prompt.rs for static guidance only; runtime/executor/continuation.rs; specs/global-recall/requirements.md; specs/adrs/066_coordinator-activity-is-queried-on-demand.md; specs/adrs/045_provider-prompts-use-persisted-generation-fenced-projections.md; coordinator_tools registration. For presentation inspect ui/src Coordinator/shared conversation tool-result rendering and ios client contracts before proposing extra UI.

## Done when

A record created before continuation is discoverable in the successor's fresh context without relying on the handoff summary; retirement/revisions survive restart; overflow is explicit; exact historical evidence and current conversation navigation remain distinct. Test malicious note text, absent evidence, ordinary/child context exclusion, empty registry and over-budget index.

Before adding the scanner, manually record a requested outcome and a useful question bookmark, detour and continue Ember, then answer “where were we / what am I forgetting?” using bounded list/read results and exact evidence. Resume the stopping point without making the bookmark an execution commission. A source gap/overflow yields honest uncertainty, not an invented comprehensive summary.

Negative acceptance: with no useful eligible item, no inventory recital, alternating unchanged-item checks, redundant worker nudge or new wakeup. A still-open tracking item alone cannot justify querying/nudging a healthy worker. Measure user reminder burden and accumulated context/retrieval cost alongside positive recovery.

Missed-pause journey: store “resume deployment,” append the later user's pause as evidence, continue Ember and attempt to use the old step. Fresh index/detail and overflow paths must expose reconciliation required; the stale step cannot appear ready. Trusted revision-checked reconciliation preserves the pause, and no scanner operation can clear it or supply permission.

Qualify through production context construction and a controlled real continuation journey. Existing four-tool coordinator-drive-eval does not prove this path. Run owning Rust/spec checks and UI/native checks only for actual presentation changes.

Compare successive assembled provider requests: registry/page updates may append context but must leave the tracker-independent system block and earlier message prefix unchanged. Test no-change loops, retry/restart, an idle scanner commit, same-turn tool mutation and continuation. Record request/context accounting; do not infer actual provider cache savings from prefix equality alone. A requested-work detour/continuation journey must recover useful follow-through on an existing wakeup without a fresh user reminder.

Out of scope: scanner inference, new periodic review wakeups, a workstream taxonomy/hierarchy, per-worker checker, silence sweep, executive dashboard/projection and human read markers.
