---
created: 2026-04-12
priority: p2
status: in-progress
artifact: pending
---

# seeded-conversations-and-shell-integration-assist

## Problem

Two things that want to be one thing.

**1. The immediate UX pain:** when a user's terminal is in the `absent`
shell-integration state, Phoenix currently shows them a snippet modal
and asks them to paste it into their rc file. Phoenix is a coding
agent — it can do this work for the user. It just needs a way to
create a scoped sub-conversation with a specific directory and a
pre-filled task.

**2. The general pattern:** "spawn a new conversation with a pre-loaded
task in a specific directory" is a primitive that will get reused for
the tasks panel, future "work on this" flows, and whatever comes next.
Build it cleanly as a primitive the first time.

## Scope

### Primitive (`seeded-conversations` spec)

Minimal, KISS, no LLM awareness or lifecycle coupling between parent
and spawned conversations.

- New `POST /api/conversations/new` parameters:
  - `parent_conversation_id: Option<Uuid>` — decorative link only
  - `seed_label: Option<String>` — decorative label for the UI
- DB: two new nullable columns on `conversations`
- Draft prompt lives in localStorage (`seed-draft:<conv-id>`), set
  by the spawning code before navigation, hydrated on the new
  conversation's mount, cleared after read
- Navigation: standard `navigate('/c/<slug>')`, browser back works
- Breadcrumb UI: if `parent_conversation_id` is set, render a small
  `← Back to: {parent title}` link at the top of the spawned
  conversation. If the parent has been deleted, render unlinked text
- Mode resolution: caller passes `conv_mode` explicitly. `auto`
  detection is documented in the spec but not implemented in the
  primitive — callers use their own judgment. Shell integration picks
  `direct`. Taskmd will pick `worktree` when it lands.
- No auto-submit. User always reviews the prompt and hits Send.

### Terminal consumer (REQ-TERM-020)

New button in the "Enable shell integration" modal:
`[Copy to clipboard] [Let Phoenix set this up for me]`

Click behavior:
1. Construct a prompt tailored to the user's shell (from the server-
   side `$SHELL`) that instructs Phoenix to investigate dotfiles
   setup, detect framework/manager, and apply the snippet safely.
2. POST a new conversation with `cwd=$HOME`, `conv_mode=direct`,
   `parent_conversation_id=current`, `seed_label="Shell integration
   setup ({shell})"`
3. Set `localStorage[seed-draft:<new-id>]` to the prompt
4. Navigate to the new conversation
5. The new conversation page hydrates the input from localStorage,
   user reviews, hits Send, and Phoenix does the work

### Out of scope (v1)

- Taskmd panel integration (directionally informs the design but
  doesn't ship in this task)
- Worktree auto-detection based on `cwd`
- Backend-persistent draft prompts
- Any spawn-result notification back to the parent
- Scoped capability restrictions on the spawned agent
- Multi-parent / graph conversations

## Deliverables

1. `specs/seeded-conversations/requirements.md` (REQ-SEED-001 through -004)
2. `specs/bedrock/bedrock.allium` — add parent/seed_label fields to
   Conversation entity, minimal additions
3. `specs/terminal/requirements.md` — REQ-TERM-020
4. DB migration: `parent_conversation_id`, `seed_label` columns
5. Backend handler update for `/api/conversations/new`
6. `home_dir` field alongside `shell` in conversation API response
7. Frontend:
   - Updated snippet modal with second button
   - New handler that builds the prompt and creates the seeded conv
   - Conversation header breadcrumb when `parent_conversation_id` set
   - InputArea hydration from `seed-draft:<id>` localStorage key
8. Bonus: fix the chevron button alignment wobble from the previous
   commit — stick it to the right via `margin-left: auto`
9. Task file → done

## Related

- Parent task: 24665 (terminal HUD state-model polish)
- Informs but does not ship: taskmd panel "start task" flow
