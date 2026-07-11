# Restore responsive switching to and from production-scale conversations

## Priority and user impact

P0: switching into or away from a large current conversation can leave Phoenix on a skeleton or otherwise make the primary workspace feel unavailable. This directly interrupts active production use.

The production database contains substantially larger conversations than ordinary fixtures. At commit `995044a`, representative active conversations include:

- `tool-result-rendering-qa`: 844 persisted messages / ~6.4 MiB message-content JSON
- `redesign-conversation-sync-around-separate-event-and-message-sequences`: 1,368 messages / ~4.9 MiB
- `add-phoenix-native-commission-review-tool`: 1,669 messages / ~3.2 MiB

A passive production observation of `/c/tool-result-rendering-qa` remained on `MessageListSkeleton` rather than reaching a rendered Virtuoso row during the profiling window. Initial application loading also fetched `/api/conversations` as an approximately 97 KB response. These are diagnostic leads, not yet proof of root cause.

## Goal

Make navigation both **into** and **away from** production-scale, tool-result-heavy conversations promptly interactive, without sacrificing message correctness, bottom anchoring, streaming isolation, or production safety.

## Investigation and implementation plan

1. Build a deterministic, sanitized fixture that preserves the relevant production shape: message count, content-size distribution, tool-result density, and exceptionally tall/complex render units. Do not copy private production message text into the repository.
2. Define two browser-profile scenarios using `browser_profile.run_scenario`:
   - a normal/small conversation → long conversation switch;
   - long conversation → normal/small conversation switch.
   The readiness mark must represent usable conversation content, not merely route change, skeleton mount, or the first incidental DOM row.
3. Capture raw multi-run baselines before changing code, with warmups, fixed CPU throttling, per-run GC, and production-build behavior. Record wall time, long tasks, script time, heap, DOM nodes, React commits/actual time when a profiling build is available, network request timing/size, and time spent showing the skeleton.
4. Use a CPU trace and React render evidence to separate the stages of the switch:
   - route transition and old-page teardown;
   - conversation list/store projection;
   - metadata and message API/database hydration;
   - message merge/render-unit construction;
   - Virtuoso mount, measurement, and visible-row rendering;
   - markdown, syntax highlighting, and tool-result rendering.
5. Fix the measured dominant cause rather than assuming virtualization alone is sufficient. Check specifically for synchronous work retained from the old conversation, over-fetching/full-history hydration, repeated whole-history transforms, unstable props or keys causing remounts, and expensive initial visible render units.
6. Preserve correct-by-construction data flow and existing `messagelist-render-units` and `conversation_atom` contracts. If behavior or a normative contract changes, update the appropriate requirements/Allium/executive artifacts after reading them and run the spec authoring pre-flight.
7. Add regression coverage and a repeatable performance scenario so production-scale behavior remains measurable in future changes.

## Production safety

Production profiling is read-only. It may authenticate, navigate among existing conversations, inspect DOM/network/performance data, and take traces or screenshots. It must not send messages, edit data, approve/reject work, cancel runs, archive/delete conversations, change settings, or call mutating APIs. Prefer the sanitized local fixture for repeated and instrumented profiling.

## Acceptance criteria

- Both switch directions have pre-change and post-change raw samples from the same deterministic scenario and environment; no reused historical baseline.
- The user-visible switch readiness time is at most 500 ms at 2× CPU throttle for the representative fixture, with no main-thread task over 100 ms. If the baseline is already below 500 ms in one direction, that direction must not regress by more than 5%.
- The dominant metric improves by at least 20%, clears the harness noise floor, and is statistically significant (`p < 0.05`) across sufficient runs. Report medians, individual raw samples, effect size, and variability rather than averages alone.
- Navigating away does not continue substantial avoidable rendering or transformation work for the abandoned conversation.
- Switching into the long conversation reaches usable content rather than leaving an indefinite skeleton; loading and failure states remain explicit.
- Message order/content, bottom-pinned initial position, jump-to-newest behavior, scroll stability, pending/streaming transitions, and tool-result rendering are unchanged and covered by focused tests/QA.
- Memory settles after repeated alternating switches; no monotonic retained-heap growth attributable to abandoned conversation trees, observers, subscriptions, or Virtuoso instances.
- `./dev.py check` passes, and the performance result is reviewed with the Phoenix performance review workflow before landing.

## Relevant surfaces

- `ui/src/pages/ConversationPage.tsx`
- `ui/src/components/MessageList.tsx`
- `ui/src/conversation/useConversationAtom.ts`
- conversation message hydration/cache/API paths
- `specs/messagelist-render-units/`
- `specs/conversation_atom/`

## Prior work to account for

MessageList already uses React Virtuoso following earlier virtualization work (`tasks/65002` and `tasks/60410`). Do not reopen the rejected guessed-height spacer design or treat the presence of Virtuoso as proof that the end-to-end switch is optimized. Profile the current implementation and production-shaped data first.
