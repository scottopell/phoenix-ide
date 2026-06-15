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

> If you can't point at the exact string in a component, you haven't grounded
> the claim — don't ship it.

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

## Principle 4 — Density, named controls *(macOS Help × Bloomberg)*

Match Phoenix's own UI philosophy: information density, not minimalism.

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
   component and quoted verbatim. *Done before the prose, not after.*
2. **Frontmatter.** All five fields present; `related` paths resolve.
3. **Layer fit.** The page does one job (concept / how-to / reference) and uses
   the matching template's shape.
4. **Links.** Inter-page links are relative `.md` paths and resolve.
5. **Manifest.** New/renamed/removed pages reflected in `SUMMARY.md`.
6. **Timeless.** No task/PR refs, no "currently/soon", no status chatter.
7. **Behavior vs. labels.** Behavioral claims match the spec; label/flow claims
   match the component.

The `phoenix-guide-sync` skill audits 2–7 on a schedule. Step 1 is the one no
tool can fully replace — it's yours.
