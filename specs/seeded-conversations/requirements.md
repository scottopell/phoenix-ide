# Seeded Conversations

## User Story

As a Phoenix user working inside a conversation, I sometimes encounter
moments where I want to spin off a focused sub-task in a different
directory, with a pre-filled prompt that the current conversation
understands but the fresh one doesn't yet. Examples:

- The terminal panel detects my shell integration is absent and offers
  to set it up for me. The work needs to happen in `$HOME`, and the
  task is scoped enough to deserve its own conversation rather than
  being mixed into whatever I was working on.
- The tasks panel shows me a task I want to start working on. I want a
  fresh conversation in the project's cwd with the task content pre-
  loaded into the input, ready for me to review and send.

Rather than manually creating a new conversation, picking the right
directory, remembering the context, and typing a prompt, I want a
single UI action that hands me a ready-to-go sub-conversation.

## Transparency Contract

The user must be able to confidently answer:

1. Where did this conversation come from?
2. What was pre-filled for me, and did I submit it?
3. How do I get back to the conversation that spawned this one?

## Requirements

### REQ-SEED-001: Conversation Creation with Pre-Filled Draft Prompt

WHEN a UI action invokes conversation seeding
THE SYSTEM SHALL create a new conversation with the caller-specified
working directory and conversation mode
AND SHALL accept an opaque draft prompt string that the caller wants
pre-populated into the new conversation's input area

WHEN the new conversation is created
THE SYSTEM SHALL navigate the user to the new conversation
AND SHALL hydrate the input area with the draft prompt on mount
AND SHALL NOT submit the draft automatically — the user must review
and explicitly hit Send

WHEN the draft prompt has been consumed (hydrated into the input)
THE SYSTEM SHALL clear the draft from any transient storage so that
subsequent visits to the conversation do not re-hydrate it

**Rationale:** Review-first keeps user agency intact. Auto-submit
normalizes "magic" behaviour that removes the user's veto power and
sets a bad precedent for the feature. The draft prompt is ephemeral
UI state (client-side localStorage is acceptable for v1); persistence
across devices is not required. Clearing after first read makes
re-visits predictable — the user sees the normal empty input, not a
stale draft.

---

### REQ-SEED-002: Caller-Specified Mode, No New Access Checks

WHEN a seeded conversation is created
THE SYSTEM SHALL use the conversation mode specified by the caller
(direct or managed) without new detection logic
AND SHALL use the existing access and validation checks for the
target directory that apply to unseeded conversation creation

WHEN the caller passes `mode = "auto"`
THE SYSTEM SHALL inspect the target cwd and resolve to `managed` if
the cwd is inside a git repository, or `direct` otherwise
AND SHALL surface the resolved mode in the conversation response so
the UI can render consistently
AND SHALL apply the same access and validation checks as an explicit
mode choice (no new trust boundary)

WHEN the target directory does not exist or is not accessible
THE SYSTEM SHALL reject the seeding request with the same error path
as unseeded creation
AND SHALL NOT create a partial conversation

**Rationale:** Callers usually know what mode they want. Shell
integration wants `direct` in `$HOME`. The tasks panel wants `managed`
in the project root. For callers that genuinely don't know — or that
want to mirror Phoenix's default new-conversation heuristic — `auto`
delegates the choice to the backend, which uses the same git-repo
detection that the regular new-conversation flow uses elsewhere. Auto
is opt-in: explicit `direct` and `managed` continue to mean exactly
what they say. No new capability layer — seeded conversations inherit
the same trust boundary as any other Phoenix conversation the user
can create themselves.

---

### REQ-SEED-003: Parent Link for UI Breadcrumb

WHEN a seeded conversation is created with a `parent_conversation_id`
THE SYSTEM SHALL persist the parent reference on the new conversation
AND SHALL expose it in the conversation API response

WHEN the user views a conversation that has a `parent_conversation_id`
THE SYSTEM SHALL render a navigable breadcrumb in the conversation
view that links back to the parent conversation

WHEN the user clicks the breadcrumb or uses the browser back button
THE SYSTEM SHALL navigate to the parent conversation via standard
routing (both paths SHALL work)

WHEN the parent conversation has been deleted or is not accessible
THE SYSTEM SHALL render the breadcrumb as unlinked informational text
(e.g. "← from: {label}") without breaking the page

**Rationale:** The parent link is purely decorative and navigational.
There is no lifecycle coupling: the spawned conversation runs
independently, the parent does not observe its progress, and no event
propagates between them. The link exists so a user who spins off a
sub-task can find their way back to the original context without re-
traversing the sidebar.

Using browser history AND an explicit breadcrumb gives users two
discoverable paths back. The explicit breadcrumb also survives reload,
which browser back does not when the user refreshes.

---

### REQ-SEED-004: Seed Label

WHEN a seeded conversation is created with a `seed_label`
THE SYSTEM SHALL persist the label on the conversation
AND SHALL surface it in the UI alongside the parent breadcrumb as
additional context about why this conversation was spawned

**Rationale:** Labels like "Shell integration setup (zsh)" give users
a human-readable anchor. Auto-generated conversation slugs tend to be
cryptic first-prompt excerpts; a short label from the spawner is more
useful at a glance. Labels are display-only — they do not affect
routing, lifecycle, or runtime behaviour.

---

## Non-Requirements (explicit out-of-scope for v1)

- Backend-persistent draft prompts (client-side localStorage is enough)
- Any form of spawn-result notification or lifecycle coupling
- Scoped capability restrictions on the spawned conversation
- Multi-parent or DAG-shaped conversation relationships
- A "spawn this" tool exposed to the LLM agent itself (this is a
  user-initiated UI action only)
