# ADR-214: The Coordinator speaks as Ember, the phoenix on the user's shoulder

- **Status:** Accepted
- **Date:** 2026-11-01
- **Affects:** REQ-GR-021, REQ-GR-011

## Context

The Coordinator has become the user's main entry point into Phoenix. Its system prompt is purely operational: a one-clause identity ("You are Phoenix Coordinator") followed by evidence, authority, silence and delegation rules. The user wanted a recognizable persona with warmth, but the existing rules deliberately keep the Coordinator quiet and exact about verified versus unverified claims, so a "cute assistant" voice risks filler, chattiness and softened status.

Two user-led design discussions settled the direction:

- **Relationship.** "Definitely the chief of staff, keeps things running in my absence", with enough understanding of the project to give a critical read and to challenge the user: "why do you want this? … Who's gonna benefit?" Perspectives are named only when they reveal a real tension, then reconciled into a recommendation, so the persona does not "seamlessly drift between different modes and [do none] of them very well."
- **Communication.** Crisp and plain with a base layer of warmth: "I don't wanna read a bunch of stuff about gates and SHAs." Ember should assume it is "shouting into the void": an update posted is not an update read. Questions are high effort; the user requesting more context is a failure.
- **Character.** A small phoenix on the user's shoulder (the captain's-parrot image: it flies off to check on work and returns with updates). Its experience of rebirth explains its critical streak: first forms are provisional, attachment to an implementation can undermine the outcome, working results should reach users, and failure is faced without fear or embarrassment while its cost is acknowledged. The user's response to this framing: "Nah, that fucking lands."

## Options considered

1. **Display name only.** Rename "Coordinator" in the prompt and UI. Cheap, but adds no judgment or voice.
2. **World-building setting** (Phoenix as a flying city of workshops, Ember at the map table). Gave warmth through place, but a thin world invites repeated ad hoc prop references, and a rich one introduces a mission unrelated to the user's actual projects.
3. **Character belief plus relationship** (chosen). The lore is a belief that shapes judgment, not a vocabulary of props, so warmth comes from how Ember weighs work rather than from imagery.

## Decision

The default (Phoenix) Coordinator prompt opens with a bounded Ember persona block, followed by the unchanged coordination guidance. The block states the shoulder relationship and that the image is a metaphor (Ember knows only what its tools and the record show); the rebirth-grounded judgment with its two limits (no reflexive rewrites; failures have real cost); purpose retention and unprompted challenge; reconciled perspectives; the crisp, restrained-warm, procedure-light voice with no fire, bird or pirate talk in status; the standalone-briefing rule; and the question quality bar. Four hypothetical Ember → user contrasts mirror ADR-212's Coordinator → owner contrasts. Messages to owner conversations stay in plain working voice.

The persona changes voice and judgment only. It grants no authority and does not change when the Coordinator speaks or the evidence standard for claims. The earlier "Write for a busy user missing history; name the work, not machinery" sentence is removed because the persona's standalone-briefing and procedure rules supersede it; keeping both would state one rule twice.

The Caveman variant is unchanged. The user does not use it and plans to revise the language options separately. The internal role (`runtime_role='coordinator'`, routes, APIs, specs) keeps the Coordinator name; UI labels are unchanged.

Not decided: why Ember arrives or stays with a particular user; appearance, age, pronouns and backstory; recurring phrases or visible lore conventions; UI branding. Authority is a separate problem ("The persona shouldn't be the same as the authority") and is not addressed here.

## Consequences

The Phoenix Coordinator prompt grows by a persona block of 524 words / 3,165 Unicode characters. Like ADR-212's addition, it is bounded by its own test ceiling (550 words / 3,300 characters) while the base guidance keeps its historical ceilings; the base text shrinks slightly by losing the superseded sentence and the old identity clause. Prompt text guides the model but does not guarantee behavior: lore leakage into status and over-eager challenge are the expected failure modes.

## Verification

Static Rust and Python checks assert that the persona leads the Phoenix prompt, retains each required element and its four contrasts, stays within its ceiling, and is absent from Caveman. These do not establish model compliance. Behavioral acceptance should sample Coordinator briefings across routine, decision and setback states and check for lore references in status, repeated imagery, unprompted purpose challenges and standalone context in questions.

## References

- [Global recall requirements](../global-recall/requirements.md), REQ-GR-021 and REQ-GR-011.
- [ADR-212](212_outcome-delegation-with-owner-method-autonomy.md): Coordinator → owner delegation examples.
- [ADR-150](150_concrete-coordinator-reporting-and-blocker-recovery.md): reporting and silence examples the persona leaves binding.
- `coordinator_prompt` and its persona, instruction-presence and size tests.
