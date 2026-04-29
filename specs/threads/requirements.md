# Phoenix Threads

## User Story

As a Phoenix user, I run long streams of related work as chains of
continuations — for example, an "auth refactor" stream where conv #41 was
"continued in new conversation" into #42, which was continued into #44.
Days or weeks later I want to recall something specific from that chain
("What were the top optimizations we applied?") without:

- Continuing #44 just to ask, which pollutes ongoing work and spends
  context on retrieval rather than progress
- Starting a fresh conversation and re-explaining the entire scope before
  I can ask my actual question

I want to navigate to the chain as a unit (a *thread*), ask it a question,
and get a focused answer. Sometimes that answer makes me want to start a
new conversation taking the topic in a *slightly different direction* —
not a continuation of the chain (the prior stream is done), but a sibling
that inherits the recap as starting context.

## Why the User Cares

- **Recall without re-explanation** saves tokens and cognitive cost. A
  thread that contains weeks of work would otherwise require copying
  context manually or paying to replay it.
- **Asking a recall question without continuing** keeps work
  conversations focused on work, not retrieval. The user does not have
  to choose between "ask cheaply but pollute" and "ask cleanly but pay
  to re-explain."
- **Distinguishing "resume same direction" from "new direction informed
  by past work"** matches how the user actually thinks about ending and
  starting threads of work. The existing "continue" action and the new
  "kickstart" action are not interchangeable.

## Transparency Contract

The user must be able to confidently answer:

1. Which conversations are in this thread, and in what order?
2. What questions have I already asked this thread, and what answers
   did I get?
3. Where did this kickstarted conversation come from, and what was it
   seeded with?

## Requirements

### REQ-THR-001: Recall Past Work Without Re-Explaining Context

WHEN a user wants to recall information from a chain of related
conversations
THE SYSTEM SHALL provide an interaction surface that returns an answer
derived from the content of those conversations
AND SHALL NOT require the user to extend any of those conversations or
to re-supply their content as input

**Rationale:** This is the headline benefit. Without it the user pays
full token cost twice — once to do the work originally, and again to
retrieve from it.

---

### REQ-THR-002: Continuation Chains Surface as Threads

WHEN two or more conversations share a continuation lineage (one was
created via "continue in new conversation" from another)
THE SYSTEM SHALL present them as a grouped thread in conversation
navigation surfaces, identifiable by a shared thread identity

WHEN a conversation has not been continued and was not itself a
continuation
THE SYSTEM SHALL render it as a standalone (non-thread) navigation entry

**Rationale:** Thread membership emerges automatically from how the
user already structures work via continuations — no manual grouping
action is required. Keeping single conversations un-grouped avoids
visually inflating every conversation into a degenerate one-member
thread.

---

### REQ-THR-003: Thread Page as a Navigable Place

WHEN the user activates a thread group from a navigation surface
THE SYSTEM SHALL navigate to a thread page that lists the member
conversations in chain order and provides an entry point for asking
the thread questions

THE SYSTEM SHALL support standard browser navigation (back button,
deep linking, refresh) to and from the thread page

**Rationale:** "Going into a thread" must feel like going to a place,
not opening a dialog. Deep-linkable URLs and browser-native navigation
are the foundational UX guarantees of a place; absent them, a thread is
just a popup and the user cannot share, bookmark, or navigate with
confidence.

---

### REQ-THR-004: Ask the Thread, Get a Streamed Answer

WHEN the user submits a question on a thread page
THE SYSTEM SHALL produce an answer derived from the thread's
conversation content, streamed token-by-token to the user as it is
generated

WHILE an answer is being prepared but no tokens have arrived
THE SYSTEM SHALL display a progress indication that signals the request
is in flight

WHILE tokens are arriving
THE SYSTEM SHALL render them incrementally rather than waiting for the
full answer

**Rationale:** Q&A is the headline interaction; loading-state quality
is an explicit user requirement. A half-rendered loading state would
undermine confidence in the feature even when the answer itself is
good.

---

### REQ-THR-005: Q&A History Persists Per Thread

WHEN a user has previously asked questions on a thread
THE SYSTEM SHALL display the prior questions and answers when the
thread page is reopened

THE SYSTEM SHALL render the input box anchored at the bottom of the Q&A
panel, with Q&A history scrolling above it in chronological order such
that the most recent Q&A sits immediately above the input

**Rationale:** Users return to threads. Without persistence, they lose
answers they paid to generate and have no record of what they have
already asked. The bottom-anchored input with chronological history
matches the messaging pattern users already know (Slack, iMessage) —
streaming flows downward into a stable visible region while the input
stays put.

---

### REQ-THR-006: Consistent Quality As Q&A Accumulates

WHILE a user has asked multiple questions in a thread
THE SYSTEM SHALL produce answers whose quality, latency, and content are
not materially influenced by the number or content of prior questions
or answers in the same thread

**Rationale:** Each question should be answered against the canonical
thread content, not against the model's own prior answers. This
prevents drift (an early misunderstanding compounding) and bounds cost
as the Q&A history grows. The user-visible property is that the tenth
question feels as fast and accurate as the first.

---

### REQ-THR-007: Kickstart a New Conversation From an Answer

WHEN the user invokes a kickstart action on a Q&A answer
THE SYSTEM SHALL create a new conversation in the same project and
working directory as the source thread, with the answer's content
pre-populated for review and editing in the new conversation's input
area

THE SYSTEM SHALL NOT auto-submit any pre-populated content — the user
must explicitly send

**Rationale:** Kickstart is the bridge from "I asked" to "now I act."
The no-auto-send rule preserves user agency, consistent with how
seeded conversations already work in Phoenix.

---

### REQ-THR-008: Kickstart Diverges, Does Not Continue

WHEN a kickstarted conversation is created from a thread Q&A
THE SYSTEM SHALL NOT add the new conversation to the source thread's
continuation chain
AND SHALL display a navigable lineage from the new conversation back
to the source thread, distinct from any continuation breadcrumb

**Rationale:** The user has explicitly distinguished two actions:
"continue where we left off" (the existing continuation flow, which
extends the chain) and "the topic is still active but the prior stream
is done — take a new direction" (kickstart, which does not). Conflating
them breaks both. Lineage display ensures the user can find their way
back to the thread that informed the kickstart.

---

## Non-Requirements (explicit out-of-scope for v1)

- **Post-hoc thread membership.** Manually adding a conversation to a
  thread it is not continuation-linked to. v1 derives threads strictly
  from the continuation graph.
- **Thread renaming.** v1 displays the root conversation's title as the
  thread name.
- **Q&A editing or deletion.** Q&A history is append-only in v1.
- **Follow-up Q&A referencing prior Q&A.** Each question is answered
  against the thread, not against the prior Q&A history. A future
  "follow-up mode" can layer on this.
- **Cross-thread linking.** "This thread spawned that thread" is not
  represented in v1 beyond the kickstart breadcrumb on the kickstarted
  conversation.
- **Project-level summary or steering doc.** A separate concept,
  explicitly deferred.
