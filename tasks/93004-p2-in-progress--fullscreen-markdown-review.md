# Add focused full-screen Markdown review

## User outcome

A user reading or annotating Markdown beside a conversation can expand it from the constrained wide-desktop pane into a full-screen focus view and review it with the same notes and send-feedback tools. Full-screen is a bounded review session: before returning to the pane, the user must send or explicitly discard every pending note. The document and reading position remain stable across the transition, but unsent feedback never transfers implicitly out of full-screen.

This applies to both Markdown files and finalized conversation messages. Commission-review results receive the same focus affordance. Task approval and fork-proposal review remain purpose-built full-screen decision surfaces and provide the interaction/design precedent rather than being forced into the ordinary viewer slot.

## Product direction

Use Phoenix's existing `ViewerShell` content-viewer language and `takeover` mode. Add a clear icon-and-label **Fullscreen** / **Return to pane** control to the viewer header on wide desktop. Do not use the browser Fullscreen API or open a new browser tab: the conversation remains underneath, and Phoenix retains control of review actions and accessibility. **Return to pane** and Escape may return immediately only when there are no pending notes; otherwise they open the same explicit feedback-resolution prompt.

Treat presentation as typed viewer state, not an incidental CSS flag. Prose, message, and commission-review slots should carry an explicit `pane | fullscreen` presentation in the URL-derived viewer-slot model, comparable to diff. Existing and legacy URLs must normalize to a documented default.

On viewports where a pane is unavailable, these viewers continue to use their existing full-screen overlay presentation without showing a meaningless pane toggle.

## Scope

### 1. Specify the behavior

Update the relevant timeless requirements and viewer-slot behavior to define:

- user-selected desktop full-screen presentation for Markdown file and message review;
- commission-review pane/full-screen presentation;
- URL restoration and mutual exclusion through the viewer slot;
- Escape and close semantics;
- a bounded full-screen feedback session whose pending notes must be sent or discarded before returning to the pane;
- responsive fallback when split-pane presentation is unavailable.

Update executive coverage after implementation. Follow `specs/AUTHORING.md` pre-flight requirements before pushing spec changes.

### 2. Extend typed viewer presentation

In `ViewerSlotContext`, make presentation structurally explicit for:

- `prose`;
- `message`;
- `commission-review`.

Add focused commands that change only the active viewer's presentation and preserve its identity parameters (file/root/focus, message sequence, or review sequence). Keep old prose/message/review URLs compatible and rewrite them to the canonical shape on the next viewer transition.

Do not add presentation to browser or process-inspector slots without a demonstrated user need.

### 3. Add reusable focus controls to content viewers

Extend the shared viewer chrome with a reusable, accessible presentation control using `Maximize2` / `Minimize2` plus visible text or an equivalent responsive label:

- **Fullscreen** while in an available desktop pane;
- **Return to pane** while in focused takeover;
- accurate `aria-label` and tooltip text;
- no control when the viewport cannot provide the alternate presentation.

Use `ViewerShell`'s existing `takeover` semantics for focused mode. Avoid parallel full-screen shells and avoid duplicating Markdown rendering.

### 4. Wire the three appropriate surfaces

#### Markdown file/prose reader

Add the toggle to `MetaViewer` for Markdown payloads (and consider all annotatable text payloads only if reuse is natural and does not broaden the requirement). Preserve:

- annotations and pending notes while full-screen remains open;
- sending the full-screen note pile through the existing conversation feedback callback and returning to the pane after the send succeeds;
- an exit-resolution prompt with **Send feedback and return**, **Discard notes and return**, and **Keep reviewing** actions whenever pending notes exist;
- notes-panel/dialog state while the user keeps reviewing;
- find query and active match;
- scroll position and focused line/range across pane/full-screen transitions.

#### Conversation message Markdown

Add the same toggle to `MessageViewer`. Message-scoped annotations are created and sent directly from full-screen; a successful send exits full-screen and returns to the pane. Returning by the presentation control or Escape with pending notes uses the same three-action exit-resolution prompt; notes are never silently carried back to the pane. Preserve panel/dialog state while review continues and preserve scroll position across a resolved exit.

#### Commission review

Add the same pane/full-screen toggle to `CommissionReviewViewer`. Preserve the resolved request/result and scroll position. Do not invent annotation support for commission reviews in this task; its current review-result contract is read-only and has no notes model.

### 5. Keep approval surfaces purpose-built

Do not migrate `TaskApprovalReader` or `ForkProposalReview` into the viewer slot. They are already full-screen decision workflows with distinct lifecycle actions. Ensure their visual relationship to focused Markdown review remains coherent, but avoid an unrelated modal/accessibility refactor unless required by the implementation.

## Interaction details

- Entering full-screen carries any already-pending notes into the bounded full-screen review session; it must not discard or resend them.
- Sending notes from full-screen is a completion action: the existing header send-feedback action sends the full note pile and returns to the pane after the send succeeds. It must not leave the user in an apparently active full-screen review after feedback has been dispatched.
- **Return to pane** returns immediately when there are no pending notes. With pending notes, it opens a confirmation with exactly three outcomes: **Send feedback and return**, **Discard notes and return**, or **Keep reviewing**.
- The header send-feedback action and **Send feedback and return** share one send-and-exit operation. They return only after the send succeeds. If sending fails, full-screen remains open, notes remain intact, and the failure is announced visibly and accessibly.
- **Discard notes and return** requires explicit confirmation and clears the full pending-note pile before returning. There is no implicit transfer of unsent notes to the pane.
- Returning to the pane restores the same document, scroll location, and find state. Annotation dialogs and notes panels are full-screen session chrome and close after feedback is resolved.
- Escape in full-screen first dismisses inner UI in its established order (annotation dialog, exit confirmation, find). Once no inner UI is open, Escape follows **Return to pane**, including the pending-note resolution prompt.
- The header back/close action closes the viewer according to existing unsaved-note rules; it is distinct from **Return to pane** and must not bypass pending-note protection.
- If the viewport becomes too narrow for a pane while focused, the viewer remains usable as an overlay. If it becomes wide again, the explicit presentation state remains deterministic.
- Full-screen content width should favor sustained reading: use a comfortable centered Markdown measure while allowing code blocks, tables, and Mermaid diagrams to use available width without horizontal clipping.
- Notes remain visible and operable in focused mode; the content and notes panel should use the added space rather than retaining the one-third-pane constraints.

## Verification

Add or extend tests for:

- viewer-slot parse/write/normalization for prose, message, and commission-review presentation;
- opening one viewer still clears all other slot parameters;
- the focus control is present only when an alternate pane presentation is available;
- entering full-screen carries existing pending notes into the bounded review session without loss or duplication;
- Markdown annotation works in full-screen mode, and a successful direct send-feedback exits to the pane;
- direct-send failure leaves full-screen open with notes intact and an accessible error;
- returning with no notes is immediate;
- returning with notes offers send-and-return, discard-and-return, and keep-reviewing outcomes;
- failed sending leaves full-screen open with notes intact;
- no exit path silently transfers or drops pending notes;
- Escape precedence: annotation/exit-confirmation/find before return-to-pane, with pending-note protection, and return-to-pane before close;
- direct URL restoration into full-screen prose, message, and commission review;
- responsive fallback on narrow viewports;
- accessible dialog/region roles and labels in pane versus takeover mode.

Add Ladle coverage or an equivalent focused UI fixture for a long Markdown document with headings, code, table, Mermaid, multiple notes, and an open notes panel. Visually verify both wide pane and focused full-screen states.

Run the relevant UI tests and typecheck, then `./dev.py check`.

## Non-goals

- Browser-native fullscreen.
- New-window or new-tab review.
- Persistent notes across conversation visits.
- Adding annotations to commission-review findings.
- General modal-framework consolidation.
- Adding full-screen toggles to browser or process-inspector viewers.
