# Ember phase 4: useful, bounded event context and qualification

Parent: [06013](./06013-p1-in-progress--ember-consistency-and-durable-tracking.md). Depends on [phase 3 exact evidence](./06020-p1-in-progress--bind-watch-events-to-ending-response-evi.md) and only the event-relevant [phase 1 baseline](./06014-p1-in-progress--ember-tool-contract-and-consistency-base.md), not a whole-program contract freeze. Lane B brings captured evidence into existing completion events independently of lane A's manual memory delivery; either useful slice may ship first. Neither waits for scanner activation.

## Outcome

Ember can often understand a finished response without first querying the worker's latest messages. The human can inspect who prompted that turn and the response it actually finished. No second message-sent alert is introduced.

## Projection

Implementation review chose one narrow interface adjustment: expose pinned recovery
as local-only `read_event_evidence` (`event_id`, `cursor`) rather than admitting a
local-only event reference into the shared local/remote `read_conversation` schema.
The closed request rejects peer fields structurally and cannot issue a remote RPC.
The generic conversation-read contract remains unchanged. ADR-261 records why the
correct-by-construction requirement outweighs the earlier same-read-surface preference.

- Read the occurrence-bound evidence captured by phase 3. Never query latest message at prompt rendering or delayed notification delivery.
- Compose the event with follow-up retrieval through an occurrence-bound evidence reference consumed by the existing read surface. It binds the event's exact message and full-content digest/version; generic transcript/message links remain explicitly current-source navigation, not recovery of occurrence-time omitted content. The pinned read path cannot silently drop that expectation or fall back to an unguarded current-message read.
- An evidence reference conveys identity/version expectations, not access authority. Existing authenticated read and private/shared boundaries still apply.
- Validate expected full-response content and return the requested bytes from one consistent read snapshot on every page. Matching content may supply an omitted tail; changed/deleted/uncaptured content returns explicit changed or historical-unavailable evidence. Current text, if offered, is separately labelled as current and cannot masquerade as the recorded response. Carry the pinned expectation through pagination; an edit between pages invalidates subsequent recovery. Surrounding current context must not be labelled as frozen occurrence evidence.
- Include compact settled-input origin/sender context, exact ending-response reference, bounded verbatim prose and explicit truncation/absence state. Initial capture and render caps are both 1,024 UTF-8 bytes of response prose, clipped on Unicode boundaries; tune only from recorded quality/total-context evidence before activation. Render at most the captured bytes, retaining capture-limit/version and omissions. A later higher capture cap applies to new occurrences; older events stay explicitly truncated without mutable-source rehydration. Triggering-message body is not copied by default.
- Keep the factual InputOrigin marker once. Avoid repeating event UUID and verbose labels in model context while retaining every original fact or an explicit typed evidence locator. Copied worker text is untrusted evidence, never user commission/Authority.
- Keep original persisted diagnostic/provider text and accepted identity intact. Model-private enrichment is a typed projection; private UI details use the same evidence. Shared/redacted reads must not gain private excerpts, sender identities or source-title lookups through either typed fields or raw diagnostic text.
- Extend existing event-card details only as needed. Preserve truthful Turn finished label, chronology, grouping, chapter exclusions, navigation/back behavior, live/history/pagination/continuation and accessibility. Reuse task 28007/specs/conversation-events. No separate dashboard.
- Old, missing or remote-v1 evidence has an honest fallback. Do not fail ordinary delivery or pretend a missing excerpt is empty prose. Remote protocol enrichment is outside this v1 slice and requires separate producer/receiver contract qualification before claiming parity.
- Preserve delivery frequency, timing and admission semantics. Temporal batching and suppressing routine completions remain deferred. User selected saved-item review during normal turns and existing wakeups; new periodic tracker wakeups are out of scope.

## Evidence and evaluation

Production observation, fixed Oct 15–Nov 14 window: 6,849/7,985 event-to-next-input windows first reached for SQL; 6,978 included message-reading SQL. Of 120 evenly-spaced message queries, 118 requested latest messages and 102 literally named the notified source. These are descriptive temporal windows, not proof every lookup was caused by its event.

Compare three isolated variants with identical model/effort/endpoint/tools/prompts/corpus/budgets:

A current deployed packet; B exact evidence + bounded excerpt/trigger; C B plus compact labels.

Measure requests/tool calls before correct next decision, total model-visible packet/tool-result context, source-message reads avoided, truncation-driven follow-up reads, latency, authority/paused-work correctness and outcome attribution. Report request caching/replay and tokenizer assumptions. Packet length alone and lower SQL counts alone are not success.

Declare the promotion rule before trials: no attribution/authority failures in the required corpus, a demonstrated reduction in targeted latest-message retrieval, and explicit allowed total-context/latency regressions in control cases (including tool-only and no-useful-excerpt endings). Set numeric quality/cost/latency tolerances before running the comparison rather than choosing them from favorable results. If gates fail, keep the evidence and revise; do not activate or claim an improvement merely because a report exists.

Use sanitized synthetic cases for ordinary useful prose, source emits B before A delivery, continuation, another conversation as sender, multiple admitted inputs, oversized Unicode, tool-only completion, missing source, failed/cancelled execution, question/approval waits, paused/dismissed work, execution completed but user outcome unresolved, and explicit unsupported remote evidence.

Add oversized-response follow-up reads where only the uncaptured conclusion changes, a source is deleted, and an edit occurs between pages. Assert explicit changed/unavailable results and no mixed-version historical claim. A separate current-source view stays clearly labelled. This is a narrow extension of existing read composition, not a historical-response store or universal retrieval wrapper.

## Starting files

coordinator_watch_delivery.rs; runtime/executor.rs::render_messages; coordinator_watches/presentation.rs; coordinator_tools.rs::LocalReadConversation; api/global_read.rs::{read_conversation_around_message, conversation read pagination}; API wire/message projections; ui/src/components/ConversationEventCard.tsx and tests; model/provider serialization; specs/global-recall, conversation-events and conversation-retrieval. Use ./dev.py codegen for typed wire changes and scope native impact explicitly. Existing coordinator-drive-eval's four fake tools cannot qualify this path.

## Done when

The exact source/absence/truncation contract survives model and private/shared UI paths, required checks pass, and a bounded full-runtime comparison meets the predeclared promotion rule and reports total follow-up work. Retain honest gates if real-model or deployed acceptance is unexercised. No raw production messages, IDs, credentials or database artifacts go into test fixtures or public PR evidence.

Out of scope: notification filtering/coalescing, another send alert, new automatic wakes, remote protocol enrichment, tracking/scanner implementation and a new event dashboard.

## Current delivery evidence

PR [#1092](https://github.com/scottopell/phoenix-ide/pull/1092) implements this
component. Scoped capture, settlement, native read/schema/privacy, genuine codegen
and fixture web checks pass. Independent review accepted compact projection C's
[complete 39-trial comparison](../scripts/qa/ember-event-eval/results-cfda0b0.md);
the earlier capped invocation remains partial/unaccepted. This task stays in
progress until memory migration 164 is integrated, event migration 165 is rebased,
exact-head CI/review pass and the PR merges. No deployment or combined Ember
qualification is claimed.
