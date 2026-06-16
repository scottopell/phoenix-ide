---
title: <Noun, singular — e.g. "Modes">
summary: <One sentence. Renders in search results and the overlay header.>
category: concepts
keywords: [<lowercase>, <search>, <terms>]
related: [<relative/path.md>, <relative/path.md>]
---

# <Title>

<!--
CONCEPT TEMPLATE — orientation. Serves the terseness leg of ../AUTHORING.md
§ The North Star.
Answers "what is this and why does it exist?", not "how do I do X".
Audience: a product user driving Phoenix, not a contributor.

SHAPE (see ../AUTHORING.md, Principle 3):
- Lead with the mental model in ONE sentence, then a visual that lets the reader
  grasp the whole space at a glance — a spectrum, a lifecycle diagram, or a
  comparison table. Don't open with abstract "why it exists" prose.
- Keep it tight — concepts target ~45 lines of content. Procedures, UI label
  dumps, and per-control walkthroughs belong in the How-to, not here.
- Name surfaces, not controls. "What you'll see" points at WHERE a thing appears
  (a badge, a panel, a page); it does NOT quote button text, placeholders, or
  states (`Sending…`, `Ask`). Those are chrome — route them to the How-to and
  Reference. Showing a data shape that IS the model (a filename grammar) is fine.
- Close with one "Remember" callout: the single structural guarantee or
  invariant to retain after closing the page.

REQUIRED PRE-FLIGHT (see ../AUTHORING.md):
A concept names surfaces, not controls — so it should quote few or no UI strings.
If you catch yourself quoting a button/placeholder/state, that content belongs in
a how-to or reference card. Run the pre-flight checklist before committing.
-->

One sentence that defines the concept, followed by the visual that frames it:

```
<spectrum / lifecycle diagram / or delete this block and use a comparison table>
```

## <The core — e.g. "The four modes">

The mental model made concrete. Prefer a table when the content is enumerable;
keep prose to what the table can't carry.

## What you'll see

Name the *surface* where the concept becomes visible — a badge, a panel, a page —
so the reader recognizes it. One or two lines. Do NOT quote control labels,
placeholders, or states; those are chrome and live in the How-to and Reference.

> **Remember:** <the one guarantee or invariant the reader should keep.>

## See also

- [<Related concept>](<relative/path.md>)
- [<Relevant how-to>](<relative/path.md>)
- [<Relevant reference card>](<relative/path.md>)
