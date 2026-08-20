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
