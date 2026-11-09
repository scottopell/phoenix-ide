# Prove ProductConversation presentation in isolated fixtures

## User-visible journey

A continued conversation appears once under Open and opens one ordinary conversation page. Predecessor and successor transcript segments render chronologically, the exact persisted handoff summary appears once at the join, and only the latest tail is writable. In History, the same chronology remains readable without ordinary input or lifecycle mutation controls. Coordinator remains a separate chat-only surface rather than being labeled Open or History.

## Scope

- Add isolated ProductConversation fixture/scenario data and Ladle stories.
- Add pure stateless presentation components and colocated CSS where useful.
- Cover desktop and mobile Open/History navigation, long transcript virtualization, exact continuation-boundary presentation, Closing-needs-repair, and Coordinator states.
- Show where existing lineage Q&A loading, prior-answer, and failure content belongs on the normal conversation page.
- Keep any discriminated message-versus-handoff render model fixture-local and non-serialized.
- Produce deterministic fixture tests, screenshots, and an adapter checklist naming the backend fields required for production integration.

## Prohibited scope

- No production routes, Sidebar, ConversationPage wiring, API methods/types, SSE schemas/codegen, ConversationStore, atom authority, ChainProvider, browser persistence, Rust, SQLite, migrations, or backend endpoints.
- Do not alias transcript/root/chain identity to ProductConversation identity or legacy `archived` to History.
- Do not create a mock production endpoint, feature-flagged aggregate, writable client model, chain redirect/removal, Close behavior, proposal placement, follow-up, provenance, or retrieval behavior.
- Do not mark parent task 92013 complete; this task is presentation proof only.

## Acceptance evidence

- Continued work appears exactly once in desktop and mobile Open navigation with one consistent root title and no chain/member/Project identity container.
- Transcript segments render in lineage order; the exact handoff summary renders once and remains distinct from the successor's first user message.
- Only the latest Open tail presents the ordinary composer.
- History preserves chronology without composer, Close, Archive, or chain-management actions.
- Coordinator is not classified as Open or History.
- Long scrolling and rerendering neither duplicate nor lose the handoff marker.
- Fixture startup performs no network or browser-storage work.
- Existing production behavior remains untouched.
- Focused fixture tests and deterministic desktop/mobile captures pass.

## Disposition

Retired as obsolete without replaying the stale PR #702 implementation. Later merged work replaced the fixture-local proof architecture and changed parts of the behavior this task proposed:

- `productConversation` fixtures and stories provide mobile Open, History, Loading, Error, continuation, and 110+ message scenarios. `ProductConversationPage.test.tsx` covers typed handoff cardinality, latest-tail writability, and read-only History projection in the production component.
- The production handoff model does not preserve this task's proposed "marker before the successor's first user message" ordering. Current tests place the accepted successor opening before its continuation marker, so the old ordering is not acceptance criteria to replay.
- `VirtualTranscript.test.tsx` proves generic virtual scrolling, rerender continuity, target resolution, and row-resize anchoring. It does not prove ProductConversation handoff cardinality through virtual unmount/remount; that old fixture-specific assertion has no current replacement.
- `tests/e2e/run.py` provides deterministic API-level continuation, exact handoff-count, canonical-route-value, reload-fetch, and latest-writable checks. It is not a rendered-browser navigation or cold-reload qualification.
- Coordinator has both a separate fixture/story surface and shared continuous-history coverage in ProductConversation fixtures. Those tests do not preserve this task's proposed exclusive fixture classification.
- Current main does not retain a passing capture receipt for every desktop/mobile assertion in this task. Reviving the old captures would test stale routes and presentation instead of the shipped UI.
- Close recovery assertions from the historical branch cannot qualify the bounded fresh-run contract introduced by ADR-088. Current Close coverage is owned by the work-lifecycle, database, API, and WorkActions tests.

The branch-only browser suite still contains historical integration evidence, but it expects pre-canonical `/product` routing, synchronous creation responses, and pre-ADR-088 Close behavior. Porting it is not a small surviving fixture delta. Any desired rendered-browser or virtualized-handoff qualification should be specified against the current production contract as new work rather than merged from PR #702.

Historical QA remains recoverable at local preservation refs `task-17003-pre-current-main-checkpoint` (`490fe893f651888175b98e4a6d2b9f9c59821cf1`) and `task-17003-consolidated-qa-checkpoint` (`4bf12768b916684c560dafbd195681322c2aa477`). PR #702 should be closed without merging its stale implementation.

No production behavior is changed by this disposition.
