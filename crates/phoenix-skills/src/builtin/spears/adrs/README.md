# Architecture Decision Records

This is the shared ADR chain for the spEARS skill itself — spEARS, built with
spEARS. Each ADR records one decision, frozen at the moment it was made.
The chain, read in order, is the design mind of the project.
See [`../references/adr-guide.md`](../references/adr-guide.md) for how to write
and maintain these.

> This index is the worked exemplar of the spEARS ADR index pattern.
> With a single ADR it is necessarily small; the structure below is what it
> grows into. The **Status** column tracks ADR lifecycle (Accepted / Superseded),
> *never* feature-implementation status — that lives in `executive.md`.

## Quick reference

| ADR | Title | Status | Affects |
| --- | --- | --- | --- |
| [000](000_no-design-document.md) | spEARS has no living design document | Accepted | methodology-level |

## For agents: which decisions bind your task

Consult the relevant ADRs before starting work of each kind.
(Populated as the chain grows.)

| Task type | Relevant ADRs |
| --- | --- |
| Choosing where design “how” knowledge goes | 000 |
| Adding or restructuring an artifact type | 000 |

## Decision dependencies

How decisions build on one another.
(A graph emerges once ADRs reference each other.)

```text
ADR-000 (no design document)
   └── establishes the four-artifact model that later ADRs operate within
```

## Conventions

- **Numbering** is sequential across the whole project: `000`, `001`, … Copy
  [`_TEMPLATE.md`](_TEMPLATE.md) to `NNN_<slug>.md` and fill every section.
- **Scope** is declared in each ADR’s `Affects:` line, not by directory — every
  ADR shares this one folder.
- **Superseding:** create a new ADR, set the old one’s Status to “Superseded by
  ADR-NNN”, and cite the old decision in the new ADR’s *Options considered*.
  Never edit an accepted decision to match later reality.
- **After adding an ADR,** add its row here.
