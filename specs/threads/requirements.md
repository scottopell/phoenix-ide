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
not a continuation of the chain (the prior stream is done), but a *offshoot*
of the same thread that inherits the recap as starting context. The
offshoot is part of the thread (visible in the thread's member list and
sidebar block) but is structurally distinct from the main continuation
line.

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

WHEN a user wants to recall information from a thread (a group of
related conversations linked by continuation, kickstart, or both)
THE SYSTEM SHALL provide an interaction surface that returns an answer
derived from the content of every member of that thread, including
both main-line members and offshoot members
AND SHALL NOT require the user to extend any of those conversations or
to re-supply their content as input

**Rationale:** This is the headline benefit. Without it the user pays
full token cost twice — once to do the work originally, and again to
retrieve from it. The "every member" clause prevents a partial-recall
failure mode: if Q&A only saw the main line, a user asking "where did
we leave off?" on a thread whose latest meaningful work was on an offshoot
would get an incomplete or misleading answer.

---

### REQ-THR-002: Conversations Form Threads as Trees of Related Work

WHEN two or more conversations are linked through continuation
("continue in new conversation") or through kickstart (a Q&A-spawned
conversation pointing back to a thread)
THE SYSTEM SHALL present them as a grouped thread in conversation
navigation surfaces, identifiable by a shared thread identity (the
thread's root conversation)

THE SYSTEM SHALL distinguish two kinds of thread members:
the **main line** (the continuation chain rooted at the thread's root
conversation) and **offshoots** (kickstart-derived sub-chains rooted at
conversations whose seed lineage points to the thread's root)

WHEN a conversation has not been continued, was not itself a
continuation, and is not a kickstart-derived offshoot root
THE SYSTEM SHALL render it as a standalone (non-thread) navigation entry

**Rationale:** Thread membership emerges automatically from how the
user already structures work — both via continuations (resume same
direction) and via kickstart (new direction in same topic). Both kinds
of links represent topic continuity from the user's perspective and
should both surface as part of the thread. Keeping single conversations
ungrouped avoids visually inflating every conversation into a
degenerate one-member thread.

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

WHEN a stored Q&A answer was generated against an earlier snapshot of
the thread (the thread has gained new members or existing members have
gained messages since the answer was produced)
THE SYSTEM SHALL visually indicate the answer's snapshot staleness so
the user can tell at a glance whether re-asking would likely yield a
materially different answer

**Rationale:** Users return to threads. Without persistence, they lose
answers they paid to generate and have no record of what they have
already asked. The bottom-anchored input with chronological history
matches the messaging pattern users already know (Slack, iMessage) —
streaming flows downward into a stable visible region while the input
stays put. Snapshot-staleness indication prevents acting on stale
recall: a "where did we leave off?" answer captured before the latest
conversation was added would be misleading without this signal.

---

### REQ-THR-006: Consistent Quality As Q&A Accumulates

WHILE a user is asking questions on a thread page
THE SYSTEM SHALL produce answers whose quality, latency, and content do
not materially degrade as more questions and answers accumulate in that
thread's Q&A history

**Rationale:** Each question is answered against the canonical thread
content, not against the model's own prior answers. This prevents drift
(an early misunderstanding compounding into later answers) and bounds
cost as the Q&A history grows. The user-visible property is that the
tenth question feels as fast and accurate as the first.

**Implication:** v1 Q&A invocations are intentionally disjoint — the
model does not see prior questions or answers from the same thread. A
follow-up like "tell me more about #2" will not work unless the user
restates the prior context in the new question. See the non-requirements
list for the v1.5 path that addresses this without breaking
REQ-THR-006.

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
The no-auto-send rule preserves user agency and is consistent with
REQ-SEED-001 (seeded conversations require explicit user submission
of any pre-filled draft).

---

### REQ-THR-008: Kickstart Adds an Offshoot to the Thread, Not a Continuation

WHEN a kickstarted conversation is created from a thread Q&A
THE SYSTEM SHALL include the new conversation in the source thread as
a **offshoot member** — it appears in the source thread's member list and
sidebar block, distinct from the main continuation line
AND SHALL NOT add it to the source thread's continuation chain (its
relationship to the thread is via seed lineage, not via
`continued_in_conv_id`)

WHEN an offshoot member is itself continued via "continue in new
conversation"
THE SYSTEM SHALL include the resulting continuation as part of the
same source thread (rendered as a sub-chain extending that offshoot),
not as a separate thread

THE SYSTEM SHALL render the offshoot hierarchy on the thread page and in
the sidebar so the main line and offshoots are visually distinguished

**Rationale:** The user has explicitly distinguished two actions:
"continue where we left off" (the existing continuation flow, which
extends the main line) and "the topic is still active but the prior
stream is done — take a new direction" (kickstart, which adds a
offshoot). Both belong to the same thread of related work, but they have
different structural roles: the main line is the canonical narrative
of the thread, offshoots are divergent sub-explorations. Treating
offshoots as members rather than as separate orphaned conversations
matches how the user thinks about ownership of their own work — they
kickstarted from this thread, so this thread "owns" the offshoot.

The continuation chain remains a clean primitive (no kickstart
conversations in it), so the existing continuation flow and Q&A
context bundling for the main line are unaffected.

---

### REQ-THR-009: Resume the Latest Active Conversation in a Thread

WHEN the user navigates to a thread page
THE SYSTEM SHALL prominently display an action to navigate directly to
the thread's most recently active member — the member (main line or
offshoot) with the latest `updated_at` across all members

WHEN the user activates this action
THE SYSTEM SHALL navigate to that member's conversation detail page
in a state ready for the user to continue working: the message input
focused, the conversation history loaded, and the conversation in a
state that accepts new user messages

THE SYSTEM SHALL also let the user resume on a non-latest member by
clicking that member's card on the thread page, with the same
ready-to-work landing state on the destination

**Rationale:** A core value of the Threads concept is eliminating the
cognitive overhead of "hunting for the last conversation in a
sequence." The user has explicitly named this as one of three
first-class actions on a thread, alongside Q&A (recall something from
past work, REQ-THR-001/004) and Kickstart (new direction in same
topic, REQ-THR-007/008). Without a prominent Resume action the user
opens a thread and still has to visually scan for the latest active
member — particularly under tree membership where the latest activity
could be on the main line or any offshoot. Surfacing it as a one-click
action makes resume-where-I-left-off as fast as the other two flows.

The three first-class verbs on a thread page:

| Action | When the user wants to | Requirement |
|---|---|---|
| Resume | continue working in the same direction | REQ-THR-009 |
| Ask | recall something from past work | REQ-THR-001 / REQ-THR-004 |
| Kickstart | take a new direction in the same topic | REQ-THR-007 / REQ-THR-008 |

---

## Non-Requirements (explicit out-of-scope for v1)

- **Post-hoc manual thread membership editing.** A user-driven action
  to manually add an unrelated conversation as a member of an existing
  thread (or remove a member from one). v1 derives thread membership
  strictly from the conversation graph: continuation edges produce main
  line members, seed-pointer edges (kickstart) produce offshoot members.
  Kickstart is a system-generated membership action with a single
  well-defined edge type, not arbitrary user editing — it does not
  violate this exclusion.
- **Thread renaming.** v1 displays the root conversation's title as the
  thread name.
- **Q&A editing or deletion.** Q&A history is append-only.
- **Follow-up Q&A with prior-Q&A model context.** REQ-THR-006 keeps
  invocations stateless; the model never sees prior Q&A from the same
  thread. Named v1.5 path: a "reply" affordance on each prior Q&A that
  pre-fills the input with a quoted snippet from that Q&A. The user's
  question becomes self-contained (with the relevant prior context
  embedded as quoted text), so follow-ups work without breaking the
  stateless invocation contract that protects REQ-THR-006.
- **Cross-thread linking.** "This thread spawned that thread" is not
  represented beyond the single kickstart breadcrumb on the kickstarted
  conversation.
- **Project-level summary or steering doc.** A separate concept,
  explicitly deferred.
