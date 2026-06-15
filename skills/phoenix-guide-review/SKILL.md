---
name: phoenix-guide-review
description: Quality-review the Phoenix User Guide (docs/guide/) against its North Star — terseness, depth, precision, and the layer split. Use periodically, before a milestone, or when asked to "review the guide", "is the guide any good", or "quality-check the docs". Distinct from phoenix-guide-sync, which only checks correctness.
---

# Phoenix Guide Review

This is the **"is it still *good*?"** loop. Its sibling,
[`phoenix-guide-sync`](../phoenix-guide-sync/SKILL.md), answers
**"is it still *true*?"** — links resolve, labels match, values are current.
That's necessary but not sufficient: a page can be perfectly correct and still
be bloated, shallow, or off-voice. This skill judges the things a grep can't.

**It never silently edits for tone.** Quality is a judgment call; this skill
*flags and proposes*, and a human decides. Apply a fix only when the user
approves it or it's a trivial, unambiguous follow-through.

On the two principles that straddle both loops (1 grounding, 7 state coverage),
you own the **judgment** slice — is a new page's claims grounded and specific
enough (1), and did a surface *deserve* full coverage (7)? `phoenix-guide-sync`
owns the regression slice (existing quotes still match, a documented control
grew a new state). Don't re-file its mechanizable checks here.

## The bar

Everything is measured against `docs/guide/AUTHORING.md` **§ The North Star**:

> macOS Help terseness × Bloomberg-terminal depth × no-nonsense reference precision.

Read that section first. The rubric below is just its three legs made checkable.

## Rubric

For each page (and for the system as a whole), ask:

1. **Terseness (Principle 6).** Does every sentence earn its place? Flag:
   no-action steps, tooltip/label dumps in a how-to, facts restated across
   intro/Result, parentheticals that editorialize. Can a hurried reader execute
   from the bold skeleton alone?
2. **Depth (Principle 7).** For any stateful surface, are *all* branches covered
   — terminal, blocked, loading, error — or only the happy path? What question
   would an expert have to leave the page to answer?
3. **Precision (Principle 4).** Exact strings, values, conditions — or hedging
   and "should"? (Verbatim *correctness* is `phoenix-guide-sync`'s job; here you
   judge whether the page is *specific enough* to be useful.)
4. **The split.** Where depth and terseness collide, did the author *split*
   (terse how-to + dense reference card) or *bloat* one page? A page that is both
   long and incomplete has failed both legs — the tell-tale sign a split is owed.
5. **Layer fit & intuition.** Concepts: lead with a model + visual, hold the
   budget, close with a "Remember". How-tos: one goal, steps. Reference: tables.
   Does the page teach, or just enumerate?
6. **Voice & vocabulary.** Consistent imperative voice; terms match
   `reference/glossary.md`; no re-coining.

## Method

1. **Scope it.** A few changed pages, a layer, or the whole guide. Read
   `AUTHORING.md` § The North Star and the relevant pages.
2. **Triangulate for anything substantial.** For a full-guide or
   multi-page review, spawn **2+ reviewer sub-agents with distinct lenses**
   (e.g. terseness, depth/precision, system-coherence), each told the North Star
   and asked to *diagnose, not rewrite* — return findings with quoted examples
   and a suggested direction. Independent lenses that converge on the same page
   are your highest-confidence findings.
3. **Synthesize themes,** ranked by how many lenses hit them and by leverage.
   Don't dump every finding — group them.
4. **Present and let the user steer.** Use `AskUserQuestion` to surface the
   themes and the candidate directions; the user picks what to pursue. Apply only
   what's chosen.

## Output: quality report

```
GUIDE QUALITY REVIEW — <date>   (scope: <pages/layer/all>)

Themes (ranked):
- <theme> — hit by <which lenses>; <leverage>
  direction: <suggested fix, not a rewrite>

Per-page flags:
- <page>: <terseness | depth | split | voice> — <one line>

Recommended next: <the 1–2 highest-leverage moves>
```

A genuinely clean review says so — "On North Star; no themes." — and stops.

## What this skill does NOT do

- It does not fix broken links, stale values, or label drift — that's
  `phoenix-guide-sync`.
- It does not rewrite prose for tone without sign-off.
- It does not invent missing content; a coverage gap becomes a proposed task.

## Scheduling

Slower cadence than drift-sync — quality erodes gradually. Use `/loop` at a long
interval (e.g. `/loop 7d /phoenix-guide-review`) or run before a milestone. A
clean review ends quietly; escalate only with themes worth a decision.
