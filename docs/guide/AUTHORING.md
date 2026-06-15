---
title: Authoring the Phoenix User Guide
summary: The principles every guide page follows, and the pre-flight checklist to run before committing one.
category: meta
keywords: [authoring, contributing, style, principles, pre-flight, grounding]
related: [_templates/concept.md, _templates/howto.md, _templates/reference-card.md, SUMMARY.md]
---

# Authoring the Phoenix User Guide

This guide documents Phoenix for the people who *use* it to get coding work
done. It is not contributor documentation — that lives in `README.md`,
`AGENTS.md`, and `specs/`. Everything here follows the principles below. The
`phoenix-guide-sync` skill enforces most of them automatically; the rest are on
you.

If you're writing a page, start from a template in [`_templates/`](_templates/)
and run the [pre-flight checklist](#pre-flight-checklist) before you commit.

---

## The North Star

Every page aims at one aesthetic, borrowed from three sources:

> **macOS Help terseness × Bloomberg-terminal depth × no-nonsense reference
> precision.**

- **Terseness** (macOS Help) — say it in the fewest words that still execute; a
  hurried reader skims the bold skeleton and succeeds. Bites hardest in
  **how-tos** and **concepts**.
- **Depth** (Bloomberg terminal) — cover the whole state space, including the
  failure and blocked branches; an expert never has to leave the page for the
  answer. Bites hardest in **reference cards**.
- **Precision** (no-nonsense reference) — exact strings, exact values, exact
  conditions; never "should" when you can state what *is*. Binds **every** layer.

These three pull against each other on purpose. The resolution is **structural,
not a compromise**: when depth threatens terseness, *split* — a terse how-to plus
a dense reference companion — rather than bloat one page. Every principle below
serves this statement. Cite it as **§ The North Star**; do not re-paraphrase it.

---

## Principle 1 — Ground every UI claim in the rendering component *(required)*

This is the principle that most often fails, and it fails silently: a plausible
paraphrase reads fine but names a control that doesn't exist.

**Any string you present as something the user clicks or reads — button text,
card titles, menu items, banners, badge labels and their tooltips, confirmation
dialogs — must be the *verbatim* string from the component that renders it.**

It is a **required step, done before you write the prose**, not a fact-check
afterward:

1. Find the rendering component(s) under `ui/src/` for the surface you're
   documenting (e.g. `ConversationSettings.tsx`, `WorkActions.tsx`,
   `ShortcutHelpPanel.tsx`).
2. Read out the literal user-visible strings and the conditions under which each
   appears. A label built from a variable or a conditional has *several* real
   values — capture the ones the user can actually hit.
3. Write the page against those strings, quoting them verbatim in `code` or
   **bold**. Describe what the user sees, in the order they see it.

Use the spec (`specs/<feature>/`) for *behavior and rules* (what a mode permits,
what a limit is), but the **UI component is the authority for labels and flow**.
Never document the spec's vocabulary as if it were on screen — the canonical
example: there is no "Managed" button; the user picks a **Workflow** card such
as **"Chat in a fresh worktree."**

**Behavioral claims need a source too.** A sentence asserting what *happens* —
"approval renames the branch", "Phoenix pre-fills your first message" — must
trace to a spec REQ-ID or a component code path, not merely sound plausible. An
ungrounded behavioral claim is as much a defect as an invented label; if you
can't cite it, you're guessing.

> If you can't point at the exact string in a component (or the REQ behind a
> behavioral claim), you haven't grounded it — don't ship it.

## Principle 2 — Single source, dual render

Every page renders both on GitHub and inside Phoenix from the *same* file. That
constrains the format:

- **Plain GitHub-Flavored Markdown only.** No MDX, no JSX, no app-only syntax —
  GitHub must remain a correct fallback renderer.
- **Required frontmatter** on every page (schema below).
- **Inter-page links are relative `.md` paths** (`../reference/keyboard.md`) —
  clickable on GitHub, rewritten to in-app routes by the help renderer.
- **[`SUMMARY.md`](SUMMARY.md) is the one ordering authority.** Add, remove, or
  move a page → update `SUMMARY.md` in the same change, or the two renderers
  drift.

## Principle 3 — Three layers, distinct jobs

Don't blend them. A page that tries to be all three serves none.

| Layer | Answers | Template |
|-------|---------|----------|
| **Concept** | *What is this, and why?* | [`_templates/concept.md`](_templates/concept.md) |
| **How-to** | *How do I accomplish X?* | [`_templates/howto.md`](_templates/howto.md) |
| **Reference** | *What's the exact value/key/flag?* | [`_templates/reference-card.md`](_templates/reference-card.md) |

Concepts stay short and define vocabulary; how-tos are numbered steps to one
goal; reference cards are dense lookup (tables over prose). Each links across to
the others rather than restating them.

**Concept pages specifically** earn their length by building intuition fast:

- **Lead with the mental model in one sentence, then a visual** — a spectrum, a
  lifecycle diagram, or a comparison table — so the reader grasps the whole
  space at a glance. Don't open with abstract "why it exists" prose.
- **Hold a ~45-line budget.** Procedures, verbatim UI-label dumps, and
  per-control walkthroughs belong in the How-to. If a concept page is teaching
  *steps*, it has drifted into the wrong layer.
- **Close with one "Remember" callout** — the single structural guarantee or
  invariant to retain after closing the page.

## Principle 4 — Density and precision *(serves § The North Star)*

Match Phoenix's own UI philosophy: information density, not minimalism. Density
is measured **per claim**, not per page — a claim earns its place by being exact.

- Name the **exact control**, don't gesture at it ("press **Send**", not "submit
  your message").
- Prefer a table to a paragraph when the content is enumerable.
- State exact values — limits, counts, key chords — and treat each as a drift
  target the sync skill will re-verify.

## Principle 5 — Write timelessly

The guide describes Phoenix as it *is*, for a reader who arrives today.

- No task/PR/issue references as the reason for a behavior. Describe the
  behavior, not its history.
- No `currently` / `recently` / `used to` / `will soon`. State what is.
- No status or roadmap chatter. A `*(planned)*` marker in `SUMMARY.md` for an
  unwritten page is the *only* place "not yet" belongs.

## Principle 6 — Cut what doesn't execute *(the terseness leg)*

Density (Principle 4) makes each claim exact; economy decides which claims
survive. They are not in conflict — keep the exact claim, cut the inert one.

The test, borrowed from the codebase's comment rule: **if you deleted this
sentence, would the reader fail the task or misunderstand a control?** If no,
cut it. Concretely:

- **A step with no user action is not a step.** Fold the wait into the
  neighbouring step ("…opens in **Explore**; the agent investigates read-only").
- **A quoted string the reader doesn't act on belongs in a reference card,** not
  a how-to step. Tooltips the user must hover to see are reference data, not
  procedure.
- **Say each fact once.** No restating the intro in "Result".

Budgets: a **how-to** is one screen of steps — if a step needs a paragraph of
*why*, the why belongs in a linked concept. A **concept** holds the ~45-line
budget (Principle 3). A **reference card** has no line cap but no prose padding
either — tables and one-line claims only.

## Principle 7 — Cover the whole state space *(the depth leg)*

A reference is judged by the question it *can't* answer. For any surface whose
label or enabled-state is computed (search the component for `?:`, `&&`, or a
derived-label helper like `deriveWorkLifecycleControls`), **enumerate every
reachable branch** — every terminal, blocked, loading, and error state — with
its verbatim label and the condition that produces it.

A how-to may summarise the happy path, but every state it skips must live
somewhere it links to. **Routing rule:** when a single step's behaviour depends
on 3+ runtime conditions (e.g. PR-state × `gh`-availability × phase), that
belongs in a reference card's **state table**, not in prose. The step links to
the card. This is how terseness and depth are reconciled in practice.

---

## The two loops

The guide has two distinct maintenance questions, and they must not be
conflated:

- **Drift-sync — "is it still *true*?"** Objective, mostly auto-fixable, frequent
  cadence. Owned by the [`phoenix-guide-sync`](../../skills/phoenix-guide-sync/SKILL.md)
  skill: links resolve, frontmatter valid, labels match components, values match
  specs, plus the mechanizable § North-Star checks (line budget, timeless-voice,
  link topology, term variants).
- **Quality-review — "is it still *good*?"** Subjective, never auto-fixed (only
  flagged for a human), slower cadence. Owned by the
  [`phoenix-guide-review`](../../skills/phoenix-guide-review/SKILL.md) skill: does
  it read like macOS Help, is the density right, does the split between how-to
  and reference hold, does it teach. This is the judgment a tool can flag but not
  settle.

### Two principles straddle the loops

Principle 1 (grounding) and Principle 7 (state coverage) each split across both
loops — deliberately:

- **Principle 1** — *sync* re-checks that already-quoted labels still match their
  component (a regression check); *review* judges whether a new page's claims are
  grounded and specific enough. The grounding *act* itself is the author's
  (pre-flight 1).
- **Principle 7** — *sync* flags a *documented* control that grew a new state
  (regression); *review* judges whether a surface deserved full coverage at all.

Rule of thumb: **sync answers "did a fact change?"; review answers "was this good
enough?"** Neither skill decides the other's question.

---

## Frontmatter schema

Every page begins with this block. GitHub renders it as a small table; the
in-app renderer uses it for navigation, search, and "see also".

```yaml
---
title: <Human title, matches the H1>
summary: <One sentence; shows in search results and the overlay header>
category: <concepts | howto | reference | meta | landing>
keywords: [<lowercase>, <search>, <terms>]
related: [<relative/path.md>, <relative/path.md>]
---
```

All five fields are required. `related` paths must resolve to real files.

---

## Pre-flight checklist

Run before committing a page. Skip irrelevant steps deliberately, not by
default.

1. **UI grounding (Principle 1).** Every quoted control string traced to its
   component and quoted verbatim; every behavioral claim traced to a REQ-ID or
   code path. *Done before the prose, not after.*
2. **Frontmatter.** All five fields present; `related` paths resolve.
3. **Layer fit.** The page does one job (concept / how-to / reference) and uses
   the matching template's shape.
4. **Links.** Inter-page links are relative `.md` paths and resolve; every
   non-landing page links to at least one *other* layer.
5. **Manifest.** New/renamed/removed pages reflected in `SUMMARY.md`.
6. **Timeless.** No task/PR refs, no "currently/soon", no status chatter.
7. **Behavior vs. labels.** Behavioral claims match the spec; label/flow claims
   match the component.
8. **Economy (Principle 6).** Every step has a user action; every quoted control
   is one the reader acts on; no fact stated twice; budgets held.
9. **State coverage (Principle 7).** Conditional controls enumerated or linked to
   a state table; no terminal/blocked/error branch silently dropped.

The `phoenix-guide-sync` skill audits the mechanizable checks: 2–7, the line
budget and topology, and the **regression** slice of step 9 (a *documented*
control that quietly grew a new state). The **judgment** slices — grounding new
claims (1), economy (8), and whether a surface *deserved* full coverage (9) — are
yours; `phoenix-guide-review` flags candidates. See [The two loops](#the-two-loops).
