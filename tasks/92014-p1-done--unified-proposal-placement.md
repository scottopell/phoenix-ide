# Gate 5 — Unified proposal placement

Child of task 92009.

Narrow this task to the commissioned ProductConversation-owned blocking approval UI/spec slice only. Do not implement the broader persistence, provisioning, or spawn-behavior language from the prior draft.

## Bounded scope

Make ProductConversation the canonical owner of the blocking task-approval overlay and approval-driven navigation for ordinary ProductConversation routes, while preserving existing proposal records and tool-result provenance as the authoritative source of approval state.

## Why this revision is needed

Current code/spec evidence shows the shipped surface is split and copy-drifted in ways that exceed the accepted scope:

- `ui/src/pages/ProductConversationPage.tsx` already derives and mounts a task-approval overlay from the latest open projection on the aggregate route.
- `ui/src/pages/ConversationPage.tsx` still auto-opens the same approval overlay on transcript routes, so ownership is split instead of canonical.
- `ui/src/components/TaskApprovalReader.tsx` currently labels the approval actions as **Start here** and **New chat**, with ARIA text tied to continuation wording.
- `specs/bedrock/requirements.md` uses the authoritative policies **Continue here** and **Start in new conversation** for AwaitingTaskApproval.
- `specs/prose-feedback/executive.md` already describes task approval as a separate phase-driven overlay layered above viewer/navigation surfaces.

## Proposed implementation scope

1. Make `ProductConversationPage` the canonical owner of the blocking approval overlay and approval-driven navigation behavior for ordinary ProductConversation routes.
2. Remove duplicate transcript-route ownership in `ConversationPage` so embedded/latest transcript rendering does not independently own or re-open the blocking proposal overlay.
3. Update `TaskApprovalReader` action copy to exactly:
   - **Continue here**
   - **Start in new conversation**
4. Align related helper text, ARIA labels, fixtures, and focused tests with that exact copy.
5. Update applicable specs/executive text only where shipped UI wording/ownership needs to match this bounded slice.

## Explicit non-goals

- No new persistence or SSE authority.
- No repository-sensitive backend changes.
- No changes to approval record authority or task/tool-result provenance; existing proposal records remain authoritative.
- No new backend proposal flow, spawn flow, provisioning authority, or fresh-spawn behavior.
- No fork retirement or decoupled fork-proposal redesign.
- No History/delete/close lifecycle work.
- No change to transcript execution ownership beyond preserving that execution still belongs to the current live transcript row.

## Acceptance criteria

- On ProductConversation routes, the blocking task-approval overlay is owned by the aggregate page rather than duplicated across both aggregate and transcript route surfaces.
- Approval action copy is exactly **Continue here** and **Start in new conversation** everywhere user-visible in this flow.
- Tests/fixtures covering the approval toolbar and aggregate approval behavior are updated to the new canonical copy and ownership model.
- Any spec edits stay narrowly focused on this UI/ownership/copy slice and do not introduce broader persistence or backend authority claims.

## Validation after approval

- Focused UI QA for `TaskApprovalReader`, `ProductConversationPage`, and any affected route ownership behavior.
- Applicable targeted checks only.
- Independent/adversarial review before handoff.
- PR exact-head gate and zero unresolved review threads before requesting final review.
- Do not merge or deploy.
