# Replace the composer with compaction status

## Observed journey

- When a conversation enters compaction (`awaiting_continuation`), Phoenix renders a prominent “Compacting conversation…” progress panel above the message composer.
- The composer remains visible underneath with the same “Compacting conversation…” placeholder, creating a redundant stacked panel—especially costly on mobile where it consumes substantial vertical space.
- Desired behavior: the compaction status should occupy the composer’s slot and the message input should not be shown while compaction is in progress.

## Verified findings

- `InputArea` renders `.continuation-progress` whenever `convState.type === 'awaiting_continuation'`, before the attachment and composer sections (`ui/src/components/InputArea.tsx`).
- The `<form className="input-textarea-wrap">` is rendered unconditionally later in the same component, which directly causes both surfaces to appear together.
- `canAcceptChatMessage` returns false for `awaiting_continuation`, so the send action is suppressed and `handleSend` rejects submission. The textarea itself has no `disabled` attribute and remains mounted/editable despite sending being unavailable.
- The placeholder independently becomes “Compacting conversation…”, duplicating the status panel’s label.
- Existing `InputArea` tests verify that continuation progress appears, no Stop button appears, and click/Enter cannot send; they do not require the composer to remain visible.
- Continuation progress styling currently lives in global `ui/src/index.css`, while the owning component already imports `ui/src/components/InputArea.css`.
- The behavioral specs define `awaiting_continuation` and its non-cancellable lifecycle, but do not prescribe this local composer layout; no backend, state-machine, persistence, or wire change is needed.

## Inferences and unknowns

- Replacing only the composer form preserves other footer-owned feedback such as failed-message notices and attachment state while removing the redundant input. This is inferred from the request’s specific reference to replacing the message input, rather than replacing the entire footer.
- Draft text is stored outside the form through the draft store/controlled `draft` prop, so conditionally unmounting the form should not discard it. A rerender regression test should prove this rather than relying on the inference.

## Interaction map

- Conversation state (`awaiting_continuation`) → `InputArea` derived send capability → conditional footer rendering.
- During compaction: render accessible continuation status in the composer position; do not mount the textarea/form or composer actions.
- After compaction/state transition: remount the normal composer using the retained controlled draft and attachments.
- No persistence, SSE, reconnect, cancellation, or runtime transition behavior changes.

## Proposed scope

### Implementation

- Refactor `InputArea` so `.continuation-progress` and `.input-textarea-wrap` are mutually exclusive alternatives in the same composer slot.
- Remove the redundant continuation-only textarea placeholder branch if it becomes unreachable.
- Preserve the existing status text, `role="status"`, polite live-region behavior, indeterminate progress animation, reduced-motion behavior, and absence of Stop/Send/Queue actions during continuation.
- Keep failed-message and attachment feedback outside the replacement boundary unless implementation evidence shows they belong to the form itself.
- Colocate the clearly component-owned continuation progress styles in `InputArea.css` rather than leaving this bounded block in global `index.css`, preserving existing appearance and responsive behavior.

### Regression coverage

Update `InputArea.test.tsx` to verify:

- `awaiting_continuation` renders the accessible compaction status;
- the message textbox and composer actions are absent, not stacked below the status;
- no send is possible while the form is absent;
- a controlled draft survives a transition into and back out of `awaiting_continuation`;
- normal non-compacting states still render the composer.

### User-journey validation

- Exercise an `awaiting_continuation` fixture/state at desktop and narrow mobile widths.
- Confirm the progress panel occupies the former composer location with no duplicate “Compacting conversation…” input below it.
- Confirm the normal composer returns after leaving continuation and any prior draft is retained.
- Confirm reduced-motion mode leaves a static progress indicator.

## Risks and non-goals

- Risk: conditionally unmounting the textarea changes focus behavior. Do not auto-focus the status; ensure the normal existing focus behavior resumes without introducing focus stealing.
- Risk: attachment chips may remain visible above the replacement status if a draft had attachments. Preserve their data and avoid broad attachment-lifecycle changes in this UI-only task.
- Non-goals: changing compaction wording, backend continuation behavior, cancellation policy, progress timing, conversation state definitions, or other disabled/working composer states.
