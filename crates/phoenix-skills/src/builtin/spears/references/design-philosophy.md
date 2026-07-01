# Why spEARS Splits the Design Document

*The temporal argument behind the spEARS artifact model.*

* * *

This document explains why spEARS has no *design document* — no single,
hand-maintained file describing how the system currently works (the `design.md`
that most methodologies ship) — and what replaces it.
If you have used a requirements methodology before, the absence of a living
design document is the most surprising thing here.
It is also the most deliberate.
Read this once; everything else in the skill follows from it.

> **spEARS is bootstrapped on itself.** The decision this document argues for —
> no living design document — is spEARS’s founding decision, and it is recorded
> the way spEARS records every decision: as a frozen, point-in-time ADR,
> [`adrs/000_no-design-document.md`](../adrs/000_no-design-document.md).
> This essay is that decision’s *exposition* — the timeless, teachable account
> the ADR points back to.
> The split between the two is the model in miniature: the ADR holds the
> deliberation, the prose states the standing truth.
> That spEARS’s first artifact is an instance of spEARS is not a gimmick — it is
> the strongest evidence the form carries its weight.

## The Losing Battle

Picture a design document written the day a feature was designed.
It describes the architecture: a single writer owns the on-disk state, readers
take a snapshot, a background task reconciles.
It is accurate, careful, and genuinely useful — for about three weeks.

Then the writer grows a second code path.
A cache slips in front of the readers.
The reconciler gets split in two.
Nobody updates the document, because updating it is unrewarding work that no
test fails for skipping.
Six months later the document is worse than useless: it is *confidently wrong*.
A new engineer reads it, builds a mental model from it, and then fights the
codebase for a day before realizing the map and the territory diverged long ago.

This is **the Losing Battle**: a hand-maintained prose description of *how the
system currently works* goes stale the instant the code moves, and the code
always moves.
You can fight this with discipline — review checklists, “update the
design doc” PR templates, periodic audits — but discipline is a tax you pay
forever, and the moment you stop paying it the document rots.
I have never seen a team win this fight for long.
I do not want to play.

## The Real Problem Is the Medium

The obvious lesson is “don’t describe current design.”
That lesson is wrong, and following it throws away something valuable.
The interesting question is *why* the design document rots when a
`requirements.md` written in the same Markdown, in the same repo, by the same
people, stays useful for years.

The answer is that **they are describing different relationships with time, in a
medium that only knows how to express one of them.**

`requirements.md` is timeless by nature.
“When a user submits an unapproved task, the system shall block it” is true
today, was true before it was built, and stays true across three rewrites of the
blocking logic. Prose is comfortable here because the *content* is timeless.

The design document is describing the present tense — *this is how it works
right now* — and the present tense is a moving target.
Markdown has no way to know it has fallen out of sync.
It is prose, and **prose is inherently point-in-time and path-dependent.** That
is not a defect of prose.
It is the *nature* of prose, and it is exactly what makes prose powerful: a
paragraph carries context, hedges, weighs one thing against another, and records
a perspective held at a moment.
When you force prose to be a timeless, always-current mirror of running code,
you are using a screwdriver as a hammer.
It will sort of work, and it will be miserable forever.

So the problem was never “we described current design.”
The problem was **a medium mismatch**: we used a point-in-time medium (prose)
for a job that demands either machine-checkable currency or honest timelessness,
and prose offers neither for free.

## The Reframe: Match the Medium to the Job

Stop fighting the nature of prose.
Split the old design document along the seam that was always there — the seam
between *how the system behaves now* and *why we chose to build it that way* —
and give each side a medium that fits.

**For “how it behaves now,” use a purpose-built DSL: Allium.** A `.allium` file
describes current behavior — states, transitions, preconditions, invariants — in
a formal language with a checker.
It earns its keep in three ways prose never could:

- It is *verifiable*. Allium syntax checks, generated tests, and available
  spec↔code drift checks mean a `.allium` file is less likely to silently drift
  from the code the way prose does — drift becomes something tooling can report,
  not an invisible lie.
- It is *dense*. A formal spec for a real feature fits comfortably in an agent’s
  context window, where the equivalent careful prose would not.
- It is *only useful as an attempt at the current design*. There is no
  temptation to make it timeless, because its entire value is being an accurate
  present-tense model precise enough to generate tests and surface ambiguities.
  The medium’s job and the medium’s nature finally agree.

Allium is a heavier tool, and you do not reach for it on every feature — only
where behavior is complex enough to warrant it (see the gate in `SKILL.md`). But
where you do, it does the job the design document was failing at, and it does it
without the Losing Battle.

**For “why we chose this,” use prose — pointed at the job prose is actually good
at.** This is where ADRs come in.

## ADRs: Prose Doing What Prose Is Best At

Here is the observation that makes the whole model click.
The phoenix-ide project adopted a strict rule that specs must be *timeless* — no
decision logs, no “we switched to this in April,” no dated resolved questions.
That rule is correct, and it keeps the specs clean.
But it created a homeless population: **real projects accumulate significant
decisions, and there is a natural, healthy desire to write down why.**

Why do bash handles exist as their own entity instead of being folded into the
session? There is a real answer, with real tradeoffs that were weighed.
Under a strict timeless-specs rule, that answer has nowhere to live.
It does not belong in `requirements.md` (it is not a user-facing requirement).
It does not belong in the `.allium` file (which states what *is*, not why).
So it either pollutes the timeless specs with decision-log prose, or — far more
commonly — it evaporates, and a year later nobody remembers why, so someone
“simplifies” it and reintroduces the bug it was avoiding.

**An ADR — Architecture Decision Record — is the outlet that lets the rest of
the specs stay timeless.** It is a single document that takes one decision, lays
out the context, weighs the options that were genuinely considered, and records
the call that was made and what it costs.
Crucially, it is *explicitly point-in-time*: an ADR is true as of when it was
written and is never retroactively edited to pretend otherwise.
It is the one place in the system where the path-dependence of prose is not a
bug but the entire point.

This is why ADRs are written in prose and not as “lists of principles.”
A bulleted principle — “prefer single-writer designs” — carries almost no
semantic weight. An ADR that shows the single-writer decision *in context*,
against the two alternatives that were on the table, with the specific failure
mode it was avoiding, carries the actual *design intent*. Rich, concrete
examples transmit intent that abstract principles cannot.

And the payoff compounds.
**A single ADR explains one decision; a chain of ADRs reveals the design mind of
the project.** LLMs and humans are both excellent at pattern recognition, and a
sequence of point-in-time decisions — each weighing its own tradeoffs — lets the
true priorities and instincts of the project emerge in a way no summary
statement of values ever could.
You do not declare “this project values correctness over cleverness”; you make
twenty decisions that show it, and the pattern speaks.
The chain *is* the design philosophy, written in the only language that can
honestly hold it.

This is where the design document goes: not deleted, but *decomposed*. The
timeless “what it does now” goes to Allium where a checker keeps it honest.
The point-in-time “why we did it” goes to a chain of ADRs where prose does what
prose is born to do.

And the two halves rhyme.
Just as a `.allium` file expresses *how-specifically* — states, transitions,
invariants — with a density no prose could match, an ADR chain expresses *why* —
the values and priorities behind the system — with a density no list of stated
principles could match.
Each is dense expression in the medium fit for its kind of knowledge: a
checkable DSL for behavior, path-dependent prose for intent.
That is the whole method in one line.

## The Four Artifacts and Their Relationship to Time

The split leaves spEARS with four kinds of artifact, each matched to its
relationship with time:

| Artifact | Carries | Relationship to time |
| --- | --- | --- |
| `requirements.md` | the **what** + user-facing why (REQ-IDs) | timeless — true before, during, and after implementation |
| `*.allium` | the **how, exactly** (normative, on-demand) | present tense, kept honest by a checker |
| `specs/adrs/NNN_*.md` | the **why** — context, options, the call | point-in-time, frozen at the moment of decision |
| `executive.md` | the **where are we** (status) | now — the single artifact whose job is to track the present |

Note what is *normative* — what the code must obey — versus what is
*authoritative history*. The binding contract is `requirements.md` (what must
exist) and `.allium` (how it must behave).
If the code disagrees with either, something is wrong and gets tracked as an
open question. ADRs are not a third contract to satisfy; they are the
authoritative record of *why* the contract reads the way it does.
You consult the ADR chain to understand intent, not to check compliance.

And `executive.md` is the deliberate exception to timelessness.
Every other document describes an ideal standing state; `executive.md` exists
precisely to say “here is where reality currently stands against that ideal — 7
of 10 requirements done, this one in progress.”
Status is inherently a snapshot, so it gets its own document and is kept out of
everything else.

## Where the Analogy Ends

It is tempting to push this to “all prose is point-in-time, all truth is in the
DSL.” That overshoots.
`requirements.md` is prose and it is timeless — because user need genuinely *is*
timeless at the level EARS captures it, so there is no medium mismatch to
resolve. The principle is not “prose bad, DSL good.”
The principle is narrower and more useful:

**Don’t make a medium fight its own nature.
Use a checkable DSL for present-tense truth that must stay current, prose for
timeless requirements and for the path-dependent record of why — and never ask a
hand-maintained prose document to be a live mirror of running code.**

That single document — the always-current design doc — is the one thing this
model refuses to create, because it is the one thing the medium cannot honestly
be.
