# Add accessibility snapshots and actionable element references to Phoenix browser tools

## Goal

Let a coding agent inspect the rendered page as a compact accessibility tree and interact with discovered elements without inventing CSS selectors. Deliver this in Phoenix first; the standalone browser entrypoint is a dependent task built on the landed implementation.

## Verified starting point

- `crates/phoenix-tools/src/lib.rs::browser_tools` registers 11 tools, with no accessibility snapshot tool.
- `crates/phoenix-browser` owns the CDP engine and session manager; `crates/phoenix-tools/src/browser/tools.rs` owns interaction behavior, schemas, timeouts, and outputs.
- There is no accessibility-tree retrieval path in either crate. Heap snapshots and React context snapshots are different capabilities.
- Sessions are resolved through `ToolContext.browser()` and existing resource authority. Preserve this ownership boundary.

## Scope

- Add a first-class `browser_snapshot` tool backed by Chrome's accessibility information, with compact role/name/state output and an interactive-only option.
- Return element references that `browser_click` and `browser_type` can consume; preserve CSS-selector targeting. Model selector versus reference as an exclusive typed target, rather than accepting ambiguous combinations.
- Keep snapshot collection, reference resolution, and page/session binding in reusable browser code. Phoenix registration and ToolOutput adaptation stay in phoenix-tools. Do not introduce a generic transport framework.
- Bind references to the originating browser instance and document; never silently retarget a reference to a different element or replacement browser. Re-snapshot/replacement/navigation behavior must be explicit, including dynamic DOM changes.
- Use bounded collection/output with explicit truncation reporting and existing full-output artifact conventions as appropriate. Never claim that an incomplete tree is complete.
- Report stale/missing references with actionable instructions to take a fresh snapshot. Snapshot failure must be visible.
- Update `specs/browser-tool/requirements.md` and `executive.md`; update normative behavior where needed. Record any architecture decision in the shared ADR chain when warranted.

## Acceptance evidence

- Real-browser fixture covers accessible names, roles and states, nested structure, ignored/decorative content, duplicate labels, and interactive filtering.
- Snapshot -> click reference -> type reference -> inspect result works through Phoenix tools, including framework handlers.
- References cannot cross sessions, survive into an unrelated replacement document, or resolve to a different node after removal. Exercise navigation, DOM replacement, and session recreation.
- Existing CSS-selector interaction behavior and browser ownership remain covered.
- Tool registration/schema tests and focused browser tests pass; run `./dev.py check`, classifying unrelated failures.
- End-to-end tool output is visible to the agent, not only Phoenix's UI.

## Boundaries

No standalone executable, daemon, MCP adapter, profile import, credential extraction, or new persistence guarantee. Cross-origin-frame/shadow-root coverage must be stated accurately and tested for the supported scope; do not silently imply full coverage.

## Delivery

Implement, review, and land this independently before starting the standalone entrypoint implementation. Link its PR/merge commit from the dependent task.
