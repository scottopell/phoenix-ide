---
name: spears
metadata:
  author: scottopell
  version: "1.0.0"
description: >
  spEARS (Simple Project with EARS) methodology for requirements-driven development.
  Provides Socratic discovery of user needs, EARS-formatted specification writing,
  implementation tracking, and spec validation -- all built on a strict three-document
  pattern (requirements.md, design.md, executive.md). MUST BE USED when the user
  mentions spEARS, EARS requirements, spec writing, requirements discovery, or
  wants to create/update/validate feature specifications. Also use when the user
  wants to plan a feature with clear acceptance criteria, track implementation
  progress against requirements, or do a context-window reflection for spEARS work.
  Use proactively when the user describes wanting to "spec out" or "define requirements"
  for any feature, even if they don't mention spEARS by name. Also triggers for
  "lint my specs", "clean up my design docs", or "check my requirements".
---
# spEARS: Requirements-Driven Development

spEARS provides explicit traceability from business requirements to tests to
code using EARS (Easy Approach to Requirements Syntax) format.

## The Foundational Principle

**Specs without traceable user journeys are hollow.** EARS format compliance
means nothing if you cannot answer: “Who does what, and why do they care?”

A structurally perfect spec that describes a feature nobody needs is worse than
no spec at all -- it creates the illusion of rigor while wasting effort.
Every requirement must trace back to a real person doing a real thing and
experiencing a real benefit.
If you cannot name the user, describe their journey, and explain why they care,
the requirement is hollow regardless of how well-formatted it is.

This matters especially when working with coding agents, which are prone to
generating compliant-looking specs that satisfy structural rules while solving
no real problem. An agent can produce a perfectly formatted requirements.md with
EARS statements, immutable IDs, and user-benefit titles -- and still produce
busywork if the requirements don’t trace to actual user needs.
The structural rules in this skill exist to serve this principle, not the other
way around.

When writing, reviewing, or linting specs, always ask: **does this requirement
exist because a real user has a real need, or does it exist because it seemed
like something the system should do?** The latter is how hollow specs happen.

## Workflow Selection

Choose the right workflow based on what the user needs.
When uncertain, default to **Discover** -- understanding the problem before
writing anything is always the right first step.

| User intent | Workflow | Reference |
| --- | --- | --- |
| “I want to build X” / new feature / vague idea | **Discover** | `references/discover.md` |
| Writing or updating spec documents | **Write Specs** | `references/ears-format.md` |
| Implementing requirements from existing specs | **Implement** | `references/implement.md` |
| Checking spec status accuracy against codebase | **Validate** | `references/validate.md` |
| Enforcing quality rules, fixing violations | **Lint** | `references/lint.md` |
| Ending a session, need continuation prompt | **Reflect** | `references/reflect.md` |

When transitioning between workflows (e.g., discovery complete, now writing
specs), read the next reference file before proceeding.

* * *

## The Three-Document System

Every feature gets a spec directory with three files that have strict,
non-overlapping responsibilities:

```text
specs/feature-name/
  requirements.md   # WHAT to build (EARS format, immutable IDs)
  design.md         # HOW to build it (architecture, implementation)
  executive.md      # WHERE we are (status, progress, milestones)
```

### Document Boundaries

This is the most-violated principle.
Getting content in the wrong document corrupts the entire system because each
document has a different relationship with time and serves a different audience.

| Document | Contains | NEVER contains |
| --- | --- | --- |
| **requirements.md** | EARS requirements, rationale, user stories | Status, implementation details, progress, milestones, future ideas |
| **design.md** | Architecture, data models, API contracts, trade-offs | Status, requirement definitions, content without a REQ-* trace |
| **executive.md** | Status table, summaries, milestones, MVP scope | Code blocks, detailed requirements, architecture details |

### The Temporal Model

Each document has a different relationship with time.
Understanding this prevents most errors:

- **requirements.md** is **timeless**. It defines the ideal end state.
  Unimplemented requirements (with ❌ status in executive.md) are still valid
  scope -- they just aren’t built yet.
  Requirements never describe current system state, migration paths, or
  backwards compatibility.
  They describe what the system should do, period.

- **design.md** is **slightly ahead of reality**. It describes HOW to build
  requirements. It may document approaches for not-yet-implemented requirements.
  But every section must trace to a REQ-* in requirements.md.
  Content without a corresponding requirement is scope creep.

- **executive.md** is **the temporal link**. It is the ONLY document that must
  reflect current reality.
  This is where status indicators, milestone tracking, MVP scope, “Phase 1 /
  Phase 2” breakdowns, and progress notes live.
  It tracks where the spec is in its development journey.

* * *

## Critical Guardrails

These rules exist because they address the most common failures observed in
practice. They are not style preferences -- violating them makes specs actively
misleading.

### 1. Self-Containment (Most Violated)

Every spec document must be fully understandable without external context.
If someone reads it in two years, they should understand it completely without
reading git history, other docs, or the codebase.

**The failure pattern**: An agent participates in a planning process where a
problem is discussed (e.g., thread safety issues), then writes design.md content
that references the discussion rather than the problem.
The result: “This design avoids thread safety violations” -- which is
meaningless to a future reader who wasn’t in that conversation.

**The fix**: Describe the constraints and properties of the problem objectively,
then explain how the design addresses them.
“The worker pool processes items concurrently across N threads.
Shared state is confined to the job queue, which uses a mutex-guarded channel.
Workers hold no references to each other’s state.”

**Banned phrases** (rewrite immediately if you catch yourself writing these):

- “as before” / “as currently implemented” / “previously”
- “maintain existing behavior” / “continue to work as expected”
- “unlike the old approach” / “the improved version”
- “same as [other feature]” / “following the established pattern”
- “standard validation rules” / “default timeout values”
- “backwards compatible” / “migration from current”

These all require the reader to know something that isn’t in the document.
Replace each with a concrete, self-contained description of the actual behavior
or constraint.

**Design.md phasing language** is a distinct failure pattern that deserves its
own callout. design.md describes HOW to build the system -- the stated technical
approach. It is not a project plan.
Phrases that describe sequencing, prioritization, or incremental delivery are
status/planning concerns that belong in executive.md.

Banned in design.md:

- “Start with X and add Y later” / “initially, just do X”
- “Deferred” / “deferred to Phase 2” / “future work”
- “Prioritize getting X working early”
- “For now, use X” / “as a first pass”
- “Once X is stable, add Y”

These are phasing decisions, not design decisions.
design.md should describe the full technical approach for each requirement.
If a requirement has a simpler fallback mode when data is unavailable, describe
both modes as part of the design -- that’s a real architectural property.
But “start simple and add complexity later” is a scheduling choice that belongs
in executive.md’s milestone tracking.

**Example:**

Bad (phasing in design.md): “Start with SQLite-only data and add LevelDB parsing
incrementally.”

Good (design with fallback): “The data layer reads from both SQLite
(transactions, balances) and LevelDB (account metadata, investment holdings).
When LevelDB data is unavailable, the system operates in reduced-fidelity mode:
accounts display as raw IDs and per-security allocation is unavailable.”

Good (phasing in executive.md): “MVP uses SQLite-only.
LevelDB parsing is planned for the next iteration.”

The distinction: the design describes what the system does in each state.
The executive tracks which state we’re currently targeting.

### 2. Requirements Describe the Ideal End State

Requirements define WHAT the system should do.
They never reference:

- Current system state ("backwards compatible with existing API")
- Migration concerns ("supports both old and new format during transition")
- Implementation technology ("uses Redis", “JWT tokens”, “HTTP 429”)
- Data structure internals ("the geohash field", “user_sessions table”)
- How the system achieves the behavior (belongs in design.md)

**Test**: Could someone implement this requirement from scratch, with no
knowledge of the current system, and get the right result?
If the requirement only makes sense in the context of an existing system, it
contains implementation leak.

**Bad**: “WHEN user authenticates, THE SYSTEM SHALL maintain backwards
compatibility with the v1 token format”

**Good**: “WHEN user authenticates, THE SYSTEM SHALL issue a signed session
token that expires after 24 hours of inactivity”

### 3. Design Traces to Requirements

Every section of design.md must trace to a REQ-* in requirements.md.
If you cannot annotate a section with the requirement it supports, that content
belongs in an issue tracker, not the spec.

This prevents design.md from becoming a roadmap for undefined work.
Sections like “Future Considerations”, “Phase 2 Enhancements”, or
“Extensibility” that describe capabilities without corresponding requirements
are violations.

**Unimplemented requirements** (REQ-* with ❌ status) are fine to design for.
**Undefined features** (no REQ-* anywhere) are scope creep.

### 4. Before Creating a New Spec

Before creating a new spec directory, check whether an existing spec covers this
domain. The anti-pattern: creating a new spec for each work batch, producing
scattered context across multiple directories that all describe the same system.

**Check**: Does an existing spec cover the same user-facing capability?
Could this be a new requirement added to that spec?
Do the requirements share the same ID abbreviation namespace?

If yes, expand the existing spec.
Create a new spec only when the feature is genuinely independent -- different
users, different domain boundary, different codebase area.

### 5. Executive.md Constraints

- 250 words max per summary section
- ZERO code blocks (no triple backticks).
  Inline backticks for technical terms (`config.yaml`, `REQ-XX-001`,
  `src/auth.ts`) are fine.
- Status table must include requirement titles, not just IDs
- Status symbols: ✅ Complete | 🔄 In Progress | ⏭️ Planned | ❌ Not Started | ⚠️
  Manual verification only

### 6. Requirement Quality

**Titles** describe USER BENEFITS, not system features:
- Good: “Prevent Abuse Attacks”, “View Current Status Without Waiting”
- Bad: “IP-Based Rate Limiting”, “Cache Data Layer”

**Rationale** answers “Why does the USER care?”
not “Why is this technically interesting?”

**EARS statements** must be specific and testable:
- Bad: “THE SYSTEM SHALL be fast”
- Good: “THE SYSTEM SHALL respond within 2 seconds for cached data”

* * *

## Requirement ID Format

IDs are immutable. Once assigned, they never change, even if the requirement is
deprecated.

Format: `REQ-[ABBREV]-###`

- `[ABBREV]`: Short abbreviation for the feature (e.g., RL for Rate Limiting)
- `###`: Zero-padded sequential number (001, 002, etc.)

Examples: `REQ-RL-001`, `REQ-UA-003`, `REQ-BI-001`

* * *

## EARS Patterns (Quick Reference)

Full format guide with templates in `references/ears-format.md`.

| Pattern | Structure | Use when |
| --- | --- | --- |
| Ubiquitous | THE SYSTEM SHALL [behavior] | Always true |
| Event-driven | WHEN [trigger] THE SYSTEM SHALL [behavior] | State change triggers action |
| State-driven | WHILE [condition] THE SYSTEM SHALL [behavior] | Behavior depends on state |
| Unwanted | IF [condition] THE SYSTEM SHALL NOT [behavior] | Explicit prohibition |
| Optional | WHERE [config] THE SYSTEM SHALL [behavior] | Configurable behavior |

* * *

## Spec Directory Structure

```text
project/
  specs/
    feature-one/
      requirements.md
      design.md
      executive.md
    feature-two/
      requirements.md
      design.md
      executive.md
  src/
  tests/
```

Use **kebab-case** for directory names.
Match names to user-facing concepts (`quota-visibility`, not `redis-storage`).

* * *

## Traceability

The system is designed for grep-based traceability.
Every requirement ID should appear in:

```text
REQ-RL-001
  requirements.md   (definition)
  design.md         (implementation approach)
  executive.md      (status)
  tests/            (@requirement REQ-RL-001)
  src/              (// REQ-RL-001: description)
  git log           (--grep="REQ-RL-001")
```

Code references: `// REQ-RL-001: Rate limiting implementation` Test references:
`@requirement REQ-RL-001` or `// REQ-RL-001` Commit messages:
`Implement rate limiting (REQ-RL-001)`
