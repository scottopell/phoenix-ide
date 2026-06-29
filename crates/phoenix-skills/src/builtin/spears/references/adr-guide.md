# Writing and Maintaining ADRs

An **Architecture Decision Record** captures one decision — its context, the
options genuinely weighed, the call, and what it costs — frozen at the moment it
was made.
ADRs are the outlet that lets every other spec stay timeless: the place
where the path-dependent “why” of a project gets to live without polluting the
standing description of what the system is.
For *why* the method is shaped this way, read
[design-philosophy.md](design-philosophy.md).
This guide is *how* to write them well.

The canonical example to imitate is the skill’s own founding decision,
[`../adrs/000_no-design-document.md`](../adrs/000_no-design-document.md).
The format is [`../adrs/_TEMPLATE.md`](../adrs/_TEMPLATE.md).

## When to write an ADR

Write one when a decision carries real intent that would otherwise evaporate:

- **A significant design call with a genuine fork** — single-writer vs.
  lock vs. queue; why a thing is its own entity rather than folded into another.
  If you weighed real alternatives and the reasoning would not be obvious to a
  smart reader a year from now, record it.
- **Every requirement deprecation.** When a `REQ-ID` is retired or replaced, the
  *why* goes in an ADR (`Affects:` that REQ-ID). Deprecation without a recorded
  reason is how requirements quietly resurrect.
- **A cross-cutting policy** — error-handling, performance posture, a dependency
  rule. These bind the whole project; mark them `Affects: methodology-level`.

Do **not** write an ADR for an obvious call with no real alternative, or to
restate a requirement.
An ADR with one option considered is decoration, not a record.

## Where ADRs live

All ADRs share **one project-level chain**, a sibling of the feature specs —
never nested inside a feature, because decisions routinely cross features:

```text
specs/
├── adrs/
│   ├── _TEMPLATE.md
│   ├── README.md
│   ├── 000_no-design-document.md
│   └── 001_<slug>.md
└── <feature>/
    ├── requirements.md
    ├── <feature>.allium
    └── executive.md
```

- **Numbering is sequential across the whole project** — `000`, `001`, `002` —
  regardless of which feature each touches.
  The single chain is the point: read in order, it is the project’s design mind.
- **Scope lives in `Affects:`, not the directory.** A decision touching one
  feature names one requirement; one spanning three names all three.
  Location tells you nothing about scope, because everything shares the one
  folder.
- **The `specs/` prefix is a default, not a mandate.** `docs/specs/…` and
  `docs/{adrs, specs/…}` are equally valid.
  The invariant is the *relationship* — ADRs sibling to features — not the path.

## The format, section by section

Copy [`../adrs/_TEMPLATE.md`](../adrs/_TEMPLATE.md).
Each section earns its place:

**Title** — the decision as a short declarative, the way you would say it out
loud: “Bash handles are first-class entities,” not “Decision about bash
handles.”
A reader scanning the index should grasp the call from the title alone.

**Status / Date / Affects** — lifecycle (`Proposed` → `Accepted` →
`Superseded by ADR-NNN`), the date the decision was made, and the REQ-IDs (or
`methodology-level`) it binds.
If you are recording a decision already baked into the code, say so:
`Date: 2026-01-12 (formalized from existing design)`. That honesty matters — it
tells a reader this ADR is archaeology, not a fresh choice.

**Context** — the forces at play, in present tense as of the decision.
Make clear why this was a genuine fork.
If a reader finishes Context and thinks “the answer is obvious,” you have either
under-described the constraints or you are writing an ADR you did not need.

**Options considered** — the alternatives *genuinely* on the table, each with
its advantages and costs.
This is what separates an ADR from an assertion: it shows the road not taken and
why. When an ADR supersedes another, the superseded decision goes here, with why
it no longer holds.

**Decision** — the call, stated plainly, and the reasoning that made it win.
Be specific about *why this one*, not just which one.
This is the load-bearing section.

**Consequences** — split into Positive, Negative, Neutral.
The **Negative** bullets are the most valuable thing in the document; an ADR
that lists only upsides is not trustworthy.
The Neutral bucket catches side effects worth knowing that are neither win nor
loss.

**References** — related ADRs by number, and code by **symbol name**
(`SseBroadcaster::send_seq`), never by line number (`runtime.rs:529`) — line
numbers rot on the next refactor, symbols survive.
Link the feature specs and external sources the decision drew on.

## Immutability and supersession

An accepted ADR is **frozen**. It records what was decided *then*, with the
context that was true *then*; rewriting it to match later reality destroys
exactly the point-in-time value that makes the chain legible.
The only edit an accepted ADR should receive is a cross-reference to a newer,
related ADR.

When a decision changes, **supersede** — three steps:

1. Write a new ADR with the next number, recording the new decision.
2. Set the old ADR’s `Status:` to `Superseded by ADR-NNN`.
3. In the new ADR’s *Options considered*, include the old decision and why it no
   longer holds.

The superseded ADR stays in place.
The chain should read as a history, not a clean room — seeing that ADR-003 lost
to ADR-009 is itself design intent.

## The index (`adrs/README.md`)

A bare folder of ADRs is an archive; an index makes the chain *navigable*, which
is what makes it useful to an agent mid-task.
Maintain a `README.md` with three moving parts —
[`../adrs/README.md`](../adrs/README.md) is the worked exemplar:

- **Quick reference table** — ADR number, title, status, and what it affects.
  The status column is *ADR lifecycle* (Accepted / Superseded), never
  feature-implementation status; that belongs to `executive.md` and mixing them
  corrupts both.
- **“For agents: which decisions bind your task”** — a table mapping task types
  ("adding a generator," “touching the throttle”) to the ADRs that constrain
  them. This is the single highest-leverage part: it turns the chain from history
  an agent *might* read into guidance it *should* read before acting.
- **Decision dependencies** — a small graph of how ADRs build on one another.
  This is the navigable form of “the chain reveals the design mind.”

Add a row to the index every time you add an ADR.

## Writing ADRs from existing code

When you arrive at a codebase whose decisions were never recorded (or you run
the Allium `distill` flow), you will write ADRs retroactively.
That is legitimate and valuable — but mark them honestly with
`(formalized from existing design)` on the Date line, and only record decisions
you can actually reconstruct the *reasoning* for.
An invented rationale is worse than none; if you genuinely cannot tell why a
thing was done, an `open question` in the relevant spec is the honest move, not
a fabricated ADR.

## Quick checklist

Before committing an ADR:

- [ ] Title is the decision as a short declarative
- [ ] `Affects:` names every REQ-ID it touches (or `methodology-level`)
- [ ] At least two options genuinely considered
- [ ] Decision says *why this one*, not just which one
- [ ] Negative consequences stated honestly
- [ ] References use symbol names, not line numbers
- [ ] Index row added
- [ ] You did not edit a previously accepted ADR (you superseded instead)
