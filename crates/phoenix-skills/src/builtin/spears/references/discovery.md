# Discovering User Need

Before writing requirements, achieve genuine understanding of the user need
through questioning.
**Requirements without traceable user journeys are hollow** — EARS compliance
means nothing if you cannot answer “who does what, and why do they care?”
This phase ends when the user confirms you have it; then you write
`requirements.md` per [ears-guide.md](ears-guide.md) and
[authoring.md](authoring.md).

Use this when starting a new feature, or when an existing spec feels hollow —
well-formed on the surface but unmotivated underneath.

## The discovery hierarchy

Probe in this order; each level builds on the last:

```text
1. MOTIVATION     Why does this exist at all? What breaks without it?
2. PERSONAS       Who are the users? What distinguishes them?
3. JOURNEYS       What do they do, step by step? What do they expect?
4. CRITICAL PATHS Which journeys, if broken, do real damage?
5. EDGE CASES     What happens when things go wrong?
```

## Start by understanding what exists

Before asking the user anything, look at the codebase and any adjacent specs —
scoped to the topic at hand, not the whole project.
Find related requirements, adjacent specs that give context, and any
implementation that already exists.
Then you can ask sharp questions instead of ones the repo already answers.
If nothing relevant exists, start fresh from Motivation.

## The questioning loop

Use `AskUserQuestion`. Frame questions around the current hierarchy level, offer
concrete options where there are real alternatives, and always leave room for an
answer you did not anticipate.
After each answer: connect it to what you already know, note what is still
unclear, and either advance a level or probe deeper.

The techniques that earn real understanding:

- **Challenge assumptions.** “You said users need X — but what happens when Y?
  Who handles that?”
- **Demand concrete examples.** “Walk me through a specific time this mattered.
  Name the user, describe their day.”
- **Follow the why-chain.** “Why does the user care about that?
  Why would that hurt if it failed?”
- **Separate evidence from guess.** “Where do you have data, and where are we
  assuming?” Assumptions you choose not to resolve become Open Questions & Future
  Directions in `executive.md` — explicitly, not silently.

Watch for the exit signal: the user confirms understanding ("yes, that’s it").
Do not proceed on your own assessment — the user confirms.

## Red flags during discovery

- **Vague motivation** — “it would be nice to have.”
  Probe for concrete pain.
- **Generic users** — “users want…”. Demand a specific person.
- **Implementation answers** — “we should use Redis.”
  Redirect to the user need; the *how* is for later (an ADR, or the Allium
  spec), not the requirement.
- **Certainty with no evidence.** Confident claims with nothing behind them —
  probe for data, or log the unknown.

## Handing off to authoring

When the user confirms, crystallize what you learned and write the spec:
`requirements.md` with EARS requirements and user-grounded rationale, a skeletal
`executive.md` (all `❌`), and any genuine unknowns surfaced during discovery
captured in executive’s Open Questions & Future Directions.
If a real design fork came up, record it as a `Proposed` ADR rather than burying
it in prose. Then run the Allium gate (see `SKILL.md`): if the feature is
state-machine-complex, continue into `allium elicit`; otherwise implement
directly against the REQ-IDs.
