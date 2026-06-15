---
title: <Goal phrased as an action — e.g. "Run a managed task">
summary: <One sentence describing the outcome the reader achieves.>
category: howto
keywords: [<lowercase>, <search>, <terms>]
related: [<relative/path.md>, <relative/path.md>]
---

# <Title>

<!--
HOW-TO TEMPLATE — goal-oriented walkthrough. Serves the terseness leg of
../AUTHORING.md § The North Star.
Answers "how do I accomplish X?". One goal per page. Assume the reader knows
the relevant concepts (link to them) and wants the steps.
Audience: a product user driving Phoenix.

SHAPE (see ../AUTHORING.md Principles 6 & 7):
- One screen of steps, happy path only. A step with NO user action is not a
  step — fold the wait into its neighbour.
- When a step's behaviour depends on 3+ runtime conditions (e.g. PR-state ×
  gh × phase), do NOT narrate it — link to a reference state table.
- Each quoted control must be one the reader acts on; tooltips and exhaustive
  label sets belong in a reference card.

REQUIRED PRE-FLIGHT (see ../AUTHORING.md):
Before writing the steps, open the rendering component(s) under ui/src/ for this
flow and extract the EXACT control strings plus the conditions each appears
under. Write each step against those verbatim strings — never paraphrase a
control. Then run the ../AUTHORING.md pre-flight checklist before committing.
-->

<One sentence: what this accomplishes and when you'd want it.>

## Before you start

- <Prerequisite — e.g. "Open a git repository as a project.">
- <Prerequisite — link to the concept that explains it.>

## Steps

1. **<Action.>** What to click/type and what happens. Reference exact keys as
   `Ctrl/Cmd+P`; link to the relevant reference card for exact values.
2. **<Action.>** …
3. **<Action.>** …

## Result

What success looks like — the state you end up in, the badge that appears, the
file that now exists.

## Troubleshooting

- **<Symptom>** — cause and fix.

## See also

- [<Related concept>](<relative/path.md>)
- [<Related how-to>](<relative/path.md>)
