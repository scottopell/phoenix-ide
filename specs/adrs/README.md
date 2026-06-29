# Architecture Decision Records

This is Phoenix's shared project-level ADR chain. Each ADR records one design or
methodology decision, frozen at the moment it was made. ADRs explain why a path
was chosen; they are authoritative history, not a live behavioral contract.

The live behavioral contract remains `requirements.md` plus `.allium` where an
Allium spec exists. Status and verification coverage live in `executive.md`.

## Quick reference

| ADR | Title | Status | Affects |
| --- | --- | --- | --- |
| [000](000_adopt-spears-v2.md) | Phoenix adopts spEARS v2 for new specification work | Accepted | methodology-level |

## For agents: which decisions bind your task

Consult the relevant ADRs before starting work of each kind.

| Task type | Relevant ADRs |
| --- | --- |
| Creating or restructuring Phoenix spec artifacts | 000 |
| Deciding whether to create a new `design.md` | 000 |
| Migrating legacy `specs/*/design.md` content | 000 |

## Decision dependencies

```text
ADR-000 (adopt spEARS v2 for new work)
   └── establishes the shared ADR chain and incremental legacy-spec migration path
```

## Conventions

- **Numbering** is sequential across the whole project: `000`, `001`, … Copy
  [`_TEMPLATE.md`](_TEMPLATE.md) to `NNN_<slug>.md` and fill every section.
- **Scope** is declared in each ADR's `Affects:` line, not by directory — every
  ADR shares this one folder.
- **Superseding:** create a new ADR, set the old one's Status to “Superseded by
  ADR-NNN”, and cite the old decision in the new ADR's *Options considered*.
  Never edit an accepted decision to match later reality.
- **After adding an ADR,** add its row here and update the task-routing table if
  the decision should be consulted for a common kind of work.
