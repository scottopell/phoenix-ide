# Authoring the Markdown Layer

This guide covers writing and maintaining the two always-present spEARS
documents: `requirements.md` and `executive.md`. It assumes you already know how
to write a single requirement ([ears-guide.md](ears-guide.md)), capture a
decision ([adr-guide.md](adr-guide.md)), and specify precise behavior (the
`allium` skill).
Here the concern is assembling those into coherent documents and
keeping the discipline that makes the system trustworthy.

## requirements.md

The home of timeless `what` and user-facing `why`. One per feature, under
`specs/<feature>/`.

```markdown
# [Feature Name]

## User Story

As a [user type], I need to [capability] so that [benefit].

## Requirements

### REQ-[ABBREV]-001: [User Benefit Title]

WHEN [trigger condition]
THE SYSTEM SHALL [observable behavior]

WHEN [edge case]
THE SYSTEM SHALL [error handling]

**Rationale:** [Why does the user care?]

**Dependencies:** REQ-[ABBREV]-002 (if any)

---
```

It holds requirements, their rationale, dependencies, and IDs — and nothing
else. No status, no implementation detail, no “updated 2026-03-01” notes, no
decision logs. Each of those has a home: status in `executive.md`, behavior in
`.allium`, the *why-we-chose-it* in an ADR. Git carries the edit history, so the
document itself never narrates its own changes.

## executive.md

The one document whose job is to reflect *current reality* — the temporal link
between the timeless spec and the moving codebase.

```markdown
# [Feature Name] — Executive Summary

## Requirements Summary

[≤250 words, user-focused: what problem this solves, what users can do.]

## Technical Summary

[≤250 words, architecture-focused: how it is built, key decisions — link to ADRs.]

## Status Summary

| Requirement | Status | Notes |
| --- | --- | --- |
| **REQ-XX-001:** [Title] | ✅ Complete | Verified via [method] |
| **REQ-XX-002:** [Title] | 🔄 In Progress | [Component] done, [rest] pending |
| **REQ-XX-003:** [Title] | ❌ Not Started | — |

**Progress:** X of Y complete

## Open Questions & Future Directions

- [Evidence gap or usage-dependent direction, e.g.] Do approvers batch-approve in
  practice? If usage shows they do, bulk-approval becomes a likely next requirement.
```

Two hard rules, because they are the ones most often broken:

- **No code blocks. None.** Not even a one-liner.
  Inline backticks for technical terms, file paths, and `REQ-IDs` are fine
  (`RateLimiter`, `config/limits.yaml`, `REQ-RL-001`); triple-backtick blocks
  are not. The executive summary is for a reader who wants the essential facts,
  not the implementation.
- **No fluff.** “Tests run on every PR,” "built with care" — cut it.
  Every sentence should carry a fact.

Include the requirement *title* in each status row (not just the ID), keep each
summary under 250 words, and keep a running progress count.

### Open Questions & Future Directions

This section is `executive.md`’s home for the one kind of unresolved thing that
has no other home: **discovery unknowns and usage-dependent future directions**
— “depending on how this gets used, X could become worth building.”
It belongs here because it is forward-looking and provisional, and
`executive.md` is the one document allowed to track the present and near-future.

Crucially, **this is not a blocker list.** Unlike Allium’s `open question`
declarations (which are must-resolve, because an unresolved behavioral ambiguity
may hide a bug), these entries can persist for the life of the feature.
They inform future requirements; they do not gate “done.”

The other kinds of “unresolved” do *not* live here, because each resolves
elsewhere:

| Unresolved thing | Where it goes |
| --- | --- |
| Undecided behavior ("what happens to in-flight sub-tasks?") | decide it into the requirement, raise a `Proposed` ADR, or — if the feature has one — an Allium `open question` |
| Two requirements contradict each other | a validation defect — fix it (one wins, often via an ADR; or deprecate one) |
| A design fork awaiting a call ("single-writer or lock?") | a `Proposed` ADR (options laid out; flip to `Accepted` when decided) |
| Deferred scope ("bulk-approval in v1?") | here, or the status notes — it is a planning question |

So spEARS has no single “open questions” list.
Behavior gets decided, contradictions get fixed, design forks become Proposed
ADRs, and only forward-looking unknowns land here.

### Status legend

| Symbol | Meaning |
| --- | --- |
| ✅ | Complete |
| 🔄 | In Progress |
| ⏭️ | Planned |
| ❌ | Not Started |
| ⚠️ | Manual verification only |
| 🟡 | Functional with gaps |
| N/A | Not applicable |

## The timeless rule (everything except executive.md)

Every spEARS artifact except `executive.md` describes a standing ideal — the
system as if it had always been this way.
They are guidebooks for understanding the design, not changelogs of how it got
here. Write what *is*, never what changed.

These phrases signal a violation.
If you write one, rewrite it as a standing fact:

- “as before” / “previously” / “as currently implemented” / “as it does today”
- “maintain existing behavior” / “continue to work as expected” / “unchanged
  from current”
- “recently” / “for now” / “will soon” / “Phase 1 (current)” / “MVP”
- “same as [other feature]” / “following the established pattern”

A resolved question becomes a bare fact in the spec, with the deliberation moved
to an ADR — never a “Q3 RESOLVED 2026-05-10: …” note left in place.
Task/PR/issue numbers as the *reason* for a behavior do not belong in specs
either; cite the invariant or bug class in timeless terms ("an emit-vs-persist
race that drops a finalized message"), and cross- reference other specs by path,
not tasks by ID.

When you touch a spec, leave it more timeless than you found it — even drift you
did not introduce.

## Document separation: what goes where

The most corrupting mistakes put content in the wrong document.
The rule:

| Content | Home |
| --- | --- |
| What must be true + why the user cares | `requirements.md` |
| Why a decision was made (options, the call) | an ADR in `specs/adrs/` |
| Exactly how the system behaves now | `.allium` (when warranted) |
| Where things currently stand | `executive.md` |

So: no status or implementation in `requirements.md`; no requirement definitions
or code blocks in `executive.md`; no decision logs anywhere but ADRs.

## Deprecating a requirement

Never delete a requirement — its ID must stay greppable.
Mark it deprecated, preserve the original EARS text, and back the change with an
ADR (`Affects:` the deprecated REQ-ID). See [adr-guide.md](adr-guide.md).
Then update its `executive.md` row.

## Updating status as you implement

spEARS does not need a heavy implementation ceremony — clear requirements plus
(where warranted) a checked Allium spec do the load-bearing work.
What remains is small and mechanical:

1. When work starts on a requirement, flip its `executive.md` status `❌ → 🔄`.
2. Implement, with `// REQ-XX-###` comments linking code back to the
   requirement. Build only what the requirement specifies — code with no `REQ-ID`
   behind it is the definition of over-engineering here.
3. When tests pass (and, for a feature carrying a `.allium`, available Allium
   drift-check tooling reports no divergence), flip `🔄 → ✅` and note the
   verification method.

Status only ever moves in `executive.md`. The requirement text, the ADRs, and
the Allium spec do not record progress.
