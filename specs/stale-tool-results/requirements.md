# Stale Tool Result Clearing — Requirements

## Purpose

A coding agent's conversation is dominated by tool output: file reads, command
output, search results, screenshots. Early in a session that output is the
agent's working set. As the session runs for hundreds of turns, most of it
becomes dead weight — the file read three hundred turns ago is almost never
relevant to the current step, yet it is re-sent to the model on every request,
consuming context-window space and input tokens.

This feature lets the agent run far longer before it exhausts the context
window or triggers a wholesale summarization, and lowers the per-turn cost of a
long session, by removing tool output the agent no longer needs from the model's
view while keeping the full record for the human.

## Users and Journeys

**The operator running a long agent session.** They give the agent a large task
that takes many turns — reading dozens of files, running builds, grepping the
tree. Without this feature the session hits the context limit partway through
and either errors or gets summarized into a lossy digest, losing fidelity right
when the work is deepest. With it, the stale reads quietly fall out of the
model's view and the session keeps going at full fidelity on the parts that
matter.

**The operator paying per token.** Every turn re-bills the entire transcript as
input. A megabyte of stale file reads re-sent on every one of two hundred turns
is the dominant cost line. Clearing that output once and keeping it cleared
turns an O(n²) cost curve back toward linear.

**The human reading the conversation later.** They scroll back through the
session to understand what the agent did. The output the model stopped seeing is
still there in the transcript — clearing changes what the *model* receives, never
what the *record* contains.

## Requirements

### REQ-STR-001 — Sustain Long Sessions Without Losing Fidelity

**Rationale:** The operator wants a long task to run to completion at full
fidelity, not stall at the context limit or collapse into a lossy summary.

WHEN the tokens required to send the conversation to the model exceed a
high-water mark below the context window, THE SYSTEM SHALL reduce the tokens sent
by removing tool results the agent no longer needs.

### REQ-STR-002 — Only Remove Re-obtainable Information

**Rationale:** The operator must be able to trust that nothing irreplaceable is
lost. The distinction is not whether a tool reproduces the same bytes — in a
workspace the agent is changing, re-reading a file yields its *current* content,
not the earlier snapshot — but whether the tool *reads state the agent can query
again*. A file read, a search, a command that inspects the workspace can all be
re-run to re-obtain whatever the agent needs about the current state, so dropping
an old result loses only a stale snapshot the agent has already acted on. What
cannot be reconstructed is information that is not a re-queryable read: an answer
the human typed, or the record that a change was applied. Removing those would
silently corrupt the agent's reasoning.

IF a tool result is the sole record of information the agent cannot re-obtain by
re-invoking a tool — a human-supplied answer, or the effect of a state-changing
action — THE SYSTEM SHALL NOT remove that result.

THE SYSTEM MAY remove a result whose producing tool reads re-queryable state,
accepting that the exact earlier snapshot is not preserved; the agent retains the
ability to re-read the current state if it later needs it.

### REQ-STR-003 — Preserve the Agent's Immediate Working Set

**Rationale:** The most recent tool output is the agent's active working memory.
Removing it would break the current step.

THE SYSTEM SHALL retain in full the tool results from the most recent rounds of
tool use.

### REQ-STR-004 — Leave a Marker, Never a Silent Gap

**Rationale:** The agent must be able to tell the difference between "this
information was removed to save space" and "this information never existed."
A silent gap invites the model to hallucinate that a tool was never called.

WHEN a tool result is removed, THE SYSTEM SHALL leave in its place a marker that
indicates a result existed and was removed, and SHALL preserve the record of the
tool call that produced it.

### REQ-STR-005 — Never Lose the Record

**Rationale:** The human reads the transcript to understand the work. Removal is
a model-context concern, not a history concern.

THE SYSTEM SHALL preserve the complete conversation, including every tool result
in full, in durable storage regardless of what is removed from the model's view.

### REQ-STR-006 — Make Each Removal Pay for Itself

**Rationale:** Removing output that has already been cached for reuse forces the
model to reprocess the surrounding context at full price. A removal that frees
only a handful of tokens costs more than it saves.

WHEN removing tool results, THE SYSTEM SHALL remove enough at once that the
tokens freed exceed both an absolute minimum threshold and a minimum fraction of
the prompt whose cached prefix is disturbed; otherwise THE SYSTEM SHALL remove
nothing on that turn.

### REQ-STR-007 — Keep Reuse Savings Stable Across Turns

**Rationale:** Reuse of previously-processed context is what makes a long
session affordable, and reuse holds only while the early part of each request is
identical to the previous request. If the set of removed results changed from
turn to turn, the early part would differ and every turn would reprocess the
conversation from the first changed point, defeating the reuse.

ONCE a tool result has been removed from the model's view, THE SYSTEM SHALL keep
it removed for the remainder of the session; the set of removed results SHALL
only ever grow, and SHALL change only when additional older results are removed,
so that the removed portion presented to the model is identical from one turn to
the next except on the turns that remove more. THE SYSTEM SHALL NOT remove
results from the most recent rounds that anchor the reusable portion of the
request.

### REQ-STR-008 — Make Removal Observable

**Rationale:** Silent removal is indistinguishable from a bug. An operator
investigating a surprising agent decision must be able to see that context was
removed and how much.

WHEN tool results are removed, THE SYSTEM SHALL record how many results were
removed and an estimate of the tokens freed.

### REQ-STR-009 — Apply Regardless of Model Provider

**Rationale:** The operator chooses a model for capability and price; the benefit
of a long, affordable session should not depend on that choice.

THE SYSTEM SHALL remove tool results before the request is translated to any
provider's wire format, so the behavior is identical across providers.

### REQ-STR-010 — Tune Retention Per Deployment

**Rationale:** A 200K-window model and a 64K-window model want different
high-water marks; an operator may want to keep more or fewer recent rounds.

WHERE the high-water mark, the number of recent rounds retained, or the minimum
tokens freed per removal are configured, THE SYSTEM SHALL honor the configured
values.

### REQ-STR-011 — Govern a Result's Images and Text Together

**Rationale:** Images are the heaviest part of a tool result, so it is tempting
to retire them on a tighter, separate schedule than text. But a second retention
boundary that retires images a fixed number of rounds behind the newest one moves
forward every turn, and each step rewrites a recent part of the conversation —
which is exactly the rewrite that destroys reuse and re-bills the recent context.
A single decision per result, covering text and images alike, keeps the removed
portion stable.

THE SYSTEM SHALL decide retention once per tool result, and that one decision
SHALL govern both the result's text and its images: IF a result is retained its
images are retained, and IF a result is removed its images are removed. THE
SYSTEM SHALL NOT maintain a separate retirement schedule for images that removes
them from a result still otherwise retained.
