# Render consecutive compact tool calls as a dense grid

## Observed journey

- With conversation density set to `compact`, a tool-heavy turn still consumes one full-width transcript row for each persisted assistant/tool-call message.
- The requested desktop presentation is a compact grid like the attached red-box mockup: adjacent lightweight tool calls (for example `read_file` and `search`) should tile across the available width rather than each reserving a row. Information-rich cards such as bash output may remain full-width so their identity, status, duration, and output tail stay legible.
- The grid must responsively reduce its column count at narrower widths and must retain click-to-expand access to the canonical full tool detail.

## Verified findings

- `CompactToolStripImpl` in `ui/src/components/MessageComponents.tsx` already renders one accessible button per tool and `.compact-tool-strip` in `ui/src/index.css` is already a responsive CSS grid.
- That grid exists only inside one `AgentMessageImpl`. `buildHistoricalUnits` in `ui/src/conversation/renderUnits.ts` emits every persisted agent message as a separate `agent_turn` render unit, even when several consecutive messages belong to the same agent run. Each render unit becomes its own measured `VirtualTranscript` row in `MessageList.tsx`, so a one-tool assistant message creates a one-item grid that expands to full width. Changing only the existing grid's column sizing cannot tile cards across those row boundaries.
- Tool results are structurally attached to their owning `agent_turn` as `toolResultsByUseId`; this ownership must remain unambiguous. `isFirstInTurn` suppresses repeated Phoenix headers across consecutive agent messages.
- `VirtualTranscript` assumes a sequential, independently measured vertical row per render unit. Making its physical row container a CSS grid would make multiple logical indexes share vertical positions and break measurement, scrolling, reveal, and history-anchor behavior.
- REQ-CONV-022 currently requires a single inline pill strip per agent turn, invocation order, preserved live facts, and click-to-expand. REQ-MLRU-001/002/003 and `render_units.allium` define render-unit cardinality, result ownership, and header suppression. A structural grouping change therefore requires aligned specs rather than render-time filtering or hidden empty rows.
- Existing `toolResults` Ladle fixtures cover compact/full tool families at desktop and mobile, but their dense discovery scenario places many tools in one assistant message and therefore does not reproduce the cross-message symptom. Existing capture has screenshots but no geometry/overflow assertion.

## Owning invariant

Compact density may reduce layout chrome but not truth: consecutive tool-only activity in one agent run should share available horizontal space without losing invocation order, tool/result ownership, lifecycle state, keyboard semantics, search/reveal targets, or access to full detail. Virtualization must continue to receive structurally honest, independently measurable units; the implementation must not visually merge arbitrary virtual rows with CSS or suppress rows during rendering.

## Proposed implementation

1. **Model a compactable tool-call run structurally.**
   - Extend the render-unit model/build step with a typed aggregate for a maximal consecutive run of tool-only agent messages within the same agent run, rather than maintaining a second UI-only grouping list.
   - Preserve each member message's stable identity and its own `toolResultsByUseId` ownership inside the aggregate.
   - Do not group across a user/skill boundary, assistant prose, a `think` aside, system content that must remain visibly ordered, or another non-tool surface. Preserve invocation order.
   - Keep the model density-independent: full density renders aggregate members at existing full fidelity in vertical order; compact density may render their eligible cards in one grid. This avoids changing transcript identity when the preference toggles.

2. **Render one compact grid for the aggregate.**
   - Add a focused aggregate renderer near `AgentMessageImpl`/`CompactToolStripImpl` that derives card data from all member messages and renders lightweight cards as grid cells.
   - Keep content-heavy compact cards that require horizontal context—at minimum bash with command/handle identity, lifecycle/duration, and bounded output tail—full-span within the grid. Do not reduce bash to the generic summary card.
   - Keep pending/running/error styling, the live elapsed timer, accessible labels, keyboard activation, and visible truncation/ellipsis behavior.
   - Clicking a card expands the canonical detailed rendering for that member/tool and scrolls it into view. External find/reveal requests must expand the correct member without expanding or losing unrelated content.
   - Use a bounded responsive column policy that yields a dense desktop row (targeting roughly four lightweight cards at the conversation width shown in the mockup), fewer columns on tablet, and one column on narrow mobile without page-level horizontal overflow.

3. **Preserve transcript integrations.**
   - Update chapter derivation, find/search projections, message-id lookup/deep links, latest-agent handling, live bash subscriptions, and render-unit keys/index targets to traverse aggregate members where applicable.
   - Keep VirtualTranscript as the sole physical layout/measurement authority; one aggregate is one measured virtual row.
   - Preserve full-density output byte-for-byte in visible order and ensure density toggling does not duplicate or omit messages.

4. **Align specifications and current-reality documentation.**
   - Update REQ-CONV-022 to state the cross-message compact-grid behavior and full-span exception for information-rich cards.
   - Update the message-list render-unit requirements and `render_units.allium` to define aggregate construction, ownership, boundaries, lookup, and cardinality. Remove or revise any rule that would otherwise require one physical `agent_turn` per persisted assistant message.
   - Update relevant executive documentation after implementation and run the spec-authoring pre-flight checklist plus `allium check`.

5. **Add regression and visual coverage.**
   - Add render-unit tests for maximal grouping boundaries, member/result ownership, stable keys, message lookup, ordering, and no grouping across prose/user/skill/think boundaries.
   - Add component tests for multiple one-tool assistant messages becoming sibling compact grid cards, full density remaining vertically faithful, full-span bash, running/error states, and expansion/reveal of the selected member.
   - Add a Ladle fixture that specifically uses several consecutive one-tool agent messages (not several tool blocks in one message), including lightweight calls, bash, long labels, pending/error states, and intervening prose.
   - Extend the tool-results capture with desktop/tablet-or-narrow/mobile geometry checks: multiple lightweight cards share a row at desktop, bash spans the row, narrow layouts remain bounded, and neither the fixture shell nor document acquires horizontal overflow.

## Acceptance criteria

- In compact density at the desktop width represented by the mockup, at least four consecutive lightweight one-tool assistant messages can occupy one grid row when space permits.
- Bash retains its compact identity/status/duration/output-tail facts and spans the available grid width; running and failed calls remain immediately distinguishable.
- Grid columns reduce responsively, reaching one column on narrow mobile with no horizontal clipping or document-level overflow.
- Card order matches invocation order. Assistant prose and `think` content remain fully represented in their original sequence and break aggregation where needed.
- Clicking or keyboard-activating any compact card reveals that exact tool's full detail and brings it into view. Conversation find/deep-link reveal does the same.
- Full density remains visually and behaviorally equivalent to the existing full transcript, including tool-result pairing and headers.
- History loading, virtualization measurement, navigation chapters, tail pinning, and density toggling do not jump, duplicate, omit, or reorder content.
- Focused UI/spec tests, responsive Ladle capture, `allium check` for the changed spec, and `./dev.py check` pass.

## Explicit non-goals

- Changing persisted message or SSE wire formats.
- Altering tool execution, result production, or provider behavior.
- Redesigning the full-density detailed tool renderers.
- Applying the grid to user messages, assistant prose, `think` asides, or unrelated sub-agent transcript surfaces.
