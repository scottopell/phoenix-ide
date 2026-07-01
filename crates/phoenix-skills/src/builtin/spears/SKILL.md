---
name: spears
description: >-
  spEARS (Simple Project with EARS) is a requirements-first specification
  methodology for AI agents and humans. It pairs timeless EARS requirements
  (requirements.md) with point-in-time design decision records (ADRs), an
  on-demand precise behavioral layer (Allium), and a status doc (executive.md),
  giving a clear shared understanding of how a system should work with grep-able
  traceability from requirement to test to code. Use this skill whenever the user
  mentions spEARS or EARS specs; works with a specs/ directory or files named
  requirements.md, executive.md, or adrs/; asks to write, plan, or validate
  requirements; wants to capture an architecture or design decision; or asks how
  requirements trace to tests and code — even if they don't say "spEARS" explicitly.
auto_trigger:
  - file_patterns: ["**/requirements.md", "**/executive.md", "**/adrs/*.md"]
  - keywords: ["spEARS", "spears spec", "EARS requirement", "requirements.md", "ADR", "design decision record"]
---
# spEARS: Simple Project with EARS

spEARS is a lightweight, requirements-first methodology for building software
with AI agents. Its north star: **be explicit about design, so that humans and
agents share a clear, accurate understanding of how the system should work** —
and keep that understanding honest as the code evolves.

A feature is described by four kinds of artifact, each matched to a different
relationship with time: timeless **requirements** (what, and why a user cares),
point-in-time **ADRs** (why a decision was made), an on-demand precise
**Allium** spec (how it behaves now), and an **executive** status doc (where
things stand). Matching each kind of knowledge to a medium that fits it is the
whole idea — and it is why spEARS has no always-current “design document,” the
one artifact that reliably goes stale.
The reasoning is the heart of the method; read
[references/design-philosophy.md](references/design-philosophy.md) once before
working with spEARS in earnest.

## Core principles

1. **Requirements first.** Define what needs to exist, and why a user cares,
   before writing tests or code.
2. **Match the medium to the job.** Timeless requirements in prose; precise
   current behavior in a checkable DSL (Allium); point-in-time decisions in
   prose ADRs. Never ask a prose document to be a live mirror of running code.
3. **Immutable traceability.** Requirements get permanent IDs (`REQ-XX-###`)
   that thread through specs, tests, and code and never change.
4. **YAGNI made detectable.** Over-engineering has a precise definition here:
   code that cannot be traced back to a `REQ-ID`. That makes it something you
   can find, not just scold.
5. **Specs stay timeless; status stays separate.** Every artifact except
   `executive.md` describes a standing ideal, as if it had always been that way
   — no changelogs, no “we recently switched,” no status tables.

## The artifact model

spEARS uses four kinds of artifact, each matched to a different relationship
with time. Feature specs hold their requirements, status, and (on demand)
behavior; **ADRs live in one shared, project-level chain**, because design
decisions routinely cross features.

```text
specs/
├── adrs/                        # ONE shared, cross-cutting ADR chain (sibling of features)
│   ├── _TEMPLATE.md
│   ├── README.md                # index: quick-ref table + task→ADR routing + dependency graph
│   └── NNN_<slug>.md            # WHY — context, options, the call — point-in-time, frozen
└── feature-name/
    ├── requirements.md          # WHAT + user-facing why — EARS, immutable REQ-IDs, timeless
    ├── feature-name.allium      # HOW, EXACTLY — states/transitions/invariants — present tense, on demand
    └── executive.md             # WHERE ARE WE — status — the one "now" document
```

An ADR names its scope in an `Affects:` field (`REQ-TA-001`, or several), never
by which folder it sits in — every ADR shares the one `specs/adrs/` directory.
The `specs/` prefix is a recommended default, not a mandate: `docs/specs/…` or
`docs/{adrs, specs/…}` are equally fine.
The one invariant is that ADRs are a *sibling* of feature specs, never nested
inside a feature.

| Document | Contains | Never contains |
| --- | --- | --- |
| `requirements.md` | EARS requirements, user-facing rationale, REQ-IDs | Status, implementation detail, decision logs |
| `adrs/*.md` | One decision: context, options weighed, the call, consequences | Timeless rule claims, Allium syntax, feature-implementation status |
| `*.allium` | Allium declarations of current behavior (normative) | Defer to Allium conventions |
| `executive.md` | Status table, brief summaries, verification coverage | Code blocks, decision logs, path-dependent narrative |

**What is normative** (the code must obey it): `requirements.md` and `.allium`.
If the code disagrees with either, something is wrong — fix the code, or, if the
spec is wrong, correct it deliberately (a real change of decision earns an ADR).
Do not silently “fix” the spec to match the code.
ADRs are authoritative *history*, not a third contract; you read them to
understand intent, not to check compliance.

For the full reasoning on why each artifact has the temporal character it does,
see [references/design-philosophy.md](references/design-philosophy.md).

## When to reach for Allium

Allium is a heavier, precise tool.
It is **precision-on-demand**, not required on every feature.
Reach for a `.allium` file when the feature has real behavioral complexity:

- **State machines** — multiple states with non-trivial transitions
- **Lifecycle flows** — preconditions that must hold (approve, complete,
  abandon)
- **Multi-step operations** — ordering matters and partial failure is possible
- **Cross-boundary contracts** — two specs interact

Skip Allium when `requirements.md` already says enough: CRUD endpoints, pure
data transformations, UI components, tools with no lifecycle.
Adding Allium there is cost without payoff.
When you do write one, defer to the `allium` skill for syntax and to the
project's available Allium workflows for elicitation, test propagation, and
spec-code drift checks.

**spEARS is complete on its own.** Allium is an *optional* companion for the
complex minority of features — install it when you want formal behavioral specs
and generated tests.
Everything else (requirements, ADRs, executive status, question resolution,
validation) works with spEARS alone.
The references note the few places that gain extra power when Allium is present.

## The workflow: layered handoff

spEARS owns the requirements → ADR → executive markdown layer and hands off to
Allium at the behavioral-spec boundary.
The `REQ-ID` is the suture running through every layer.

```text
discover ──► requirements.md (REQ-IDs, EARS, user-why) + executive.md (skeleton, all ❌)
   │
   ▼  GATE: state-machine-complex? (lifecycle / preconditions / ordering / cross-boundary)
   │
   ├─ NO  ──► implement directly against REQ-IDs, stay in spEARS
   │
   └─ YES ──► elicit exact behavior ─► feature.allium (references the REQ-IDs)
              propagate tests from Allium (must fail first)
              implement ─► code with // REQ-XX-### comments
              check .allium ↔ code divergence with available Allium tooling
   │
   ▼
write an ADR in specs/adrs/ for any significant decision made along the way, and for
every REQ deprecation (Affects: names the REQ-IDs it touches)
   │
   ▼
validate (markdown layer) ──► executive.md status  ❌ → 🔄 → ✅
```

## Routing table

This SKILL.md is the hub.
Each task below has a home — a reference file in this skill, or the sibling
`allium` skill. Read the reference when you are doing that task.

| Task | Go to | When |
| --- | --- | --- |
| Understand *why* spEARS is shaped this way | [references/design-philosophy.md](references/design-philosophy.md) | Before working with spEARS the first time |
| Discover user need, write requirements | [references/discovery.md](references/discovery.md) | New feature, or an existing spec feels hollow |
| Write EARS requirements correctly | [references/ears-guide.md](references/ears-guide.md) | Authoring or reviewing `requirements.md` |
| Author/update requirements & executive | [references/authoring.md](references/authoring.md) | Writing the markdown layer |
| Capture a design decision | [references/adr-guide.md](references/adr-guide.md) | A decision has a real “why”; any REQ deprecation |
| Specify exact behavior, generate tests | `allium` skill and project Allium tooling | The Allium gate fired YES |
| Validate the markdown layer | [references/validation.md](references/validation.md) | Before merge |
| Check spec ↔ code divergence | Project Allium tooling | The feature carries a `.allium` |
| Establish/verify traceability | [references/traceability.md](references/traceability.md) | Linking requirement → test → code |
| See a full feature worked end-to-end | [references/worked-examples.md](references/worked-examples.md) | You want a concrete model to follow |

## Traceability in one line

Every requirement gets an immutable `REQ-XX-###` that you can grep across the
whole system: `requirements.md` defines it, `executive.md` tracks it, `.allium`
references it, ADRs cite the ones they affect, code and tests carry it in
comments. One `rg REQ-RL-001` shows you the requirement, its status, its precise
behavior, the decisions behind it, and every line of code that implements it.
Details and verification commands in
[references/traceability.md](references/traceability.md).

## A few rules worth stating up front

These are the ones that, when violated, quietly corrupt the system:

- **REQ-IDs are immutable.** Never renumber, never reuse.
  Deprecate (don’t delete), and back every deprecation with an ADR.
- **Status lives only in `executive.md`.** Not in requirements, not in ADRs, not
  in Allium.
- **Specs are timeless; ADRs are not.** Keep dates, “we decided,” and
  option-weighing *inside* ADRs and *out of* everything else.
  When you resolve a question, state the outcome as a standing fact in the spec
  and let the ADR hold the deliberation.
- **Don’t invent scope.** Code with no `REQ-ID` behind it is the definition of
  over-engineering here.
  If a capability is worth building, it is worth a requirement first.

## References

- [references/design-philosophy.md](references/design-philosophy.md) — why there
  is no `design.md`, and the temporal model behind the four artifacts
- [references/ears-guide.md](references/ears-guide.md) — the EARS format,
  patterns, and good vs.
  bad requirements *(stage 2)*
- [references/authoring.md](references/authoring.md) — writing `requirements.md`
  and `executive.md`; document-separation rules *(stage 2)*
- [references/adr-guide.md](references/adr-guide.md) — ADR format, lifecycle,
  supersession, the shared-chain model, and the index pattern.
  The format lives in [adrs/_TEMPLATE.md](adrs/_TEMPLATE.md);
  [adrs/README.md](adrs/README.md) is the worked index; and
  [adrs/000_no-design-document.md](adrs/000_no-design-document.md) is spEARS’s
  own founding ADR — the worked exemplar, and proof the method is built with
  itself.
- [references/discovery.md](references/discovery.md) — Socratic discovery of
  user need *(stage 3)*
- [references/validation.md](references/validation.md) — validating the markdown
  layer *(stage 3)*
- [references/traceability.md](references/traceability.md) — grep-based
  traceability and verification *(stage 3)*
- [references/worked-examples.md](references/worked-examples.md) — full features
  worked end-to-end *(stage 3)*

Sibling skill: **allium** — the precise behavioral layer.
spEARS records user need and traceability; Allium records exact behavior when a
formal layer is worth its cost. Allium without spEARS is unmoored from user
need.
