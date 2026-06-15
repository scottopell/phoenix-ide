---
title: <Exact tool/surface name — e.g. "bash">
summary: <One sentence. What it is, in a glance.>
category: reference
keywords: [<lowercase>, <search>, <terms>]
related: [<relative/path.md>, <relative/path.md>]
---

# <Title>

<!--
REFERENCE-CARD TEMPLATE — dense lookup. Serves the depth + precision legs of
../../AUTHORING.md § The North Star.
Maximize information density: tables over prose, exact values, no warm-up, no
prose padding. For a TOOL card, write from the USER's vantage point — what the
agent does with it and what you see in the transcript — not an API call spec.
Every value here is a drift target the phoenix-guide-sync skill checks.

STATE COVERAGE (Principle 7): if you are documenting a surface whose label or
enabled-state is computed, enumerate EVERY reachable branch (terminal, blocked,
loading, error) as a state table — not just the happy path. This card is where
a how-to offloads its 3+-condition depth.

REQUIRED PRE-FLIGHT (see ../../AUTHORING.md):
Source exact values (limits, statuses, params) from the spec/Allium under
specs/<tool>/, and any quoted UI labels from the rendering component under
ui/src/, verbatim. Run the ../../AUTHORING.md pre-flight checklist before
committing.
-->

> **At a glance:** <one-line synopsis — what it does and its headline limit.>

## What it does

Two or three sentences, maximum.

## Operations / parameters

| Name | Meaning | Notes / limit |
|------|---------|---------------|
| `<op or field>` | <what it does> | <exact value> |

## What you'll see

The transcript/UI representation: status labels, badges, truncation, the panel
it opens.

## Limits & gotchas

- <Hard limit with its exact number.>
- <Surprising behavior worth flagging.>

## Related

- [<concept that explains the model>](<relative/path.md>)
- [<adjacent tool>](<relative/path.md>)
