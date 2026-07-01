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
| [001](001_bash-first-class-handles.md) | Bash handles are first-class process-local entities with wait windows | Accepted | REQ-BASH-001, REQ-BASH-002, REQ-BASH-003, REQ-BASH-005, REQ-BASH-009, REQ-BASH-014, REQ-BASH-WS-001, REQ-BASH-WS-002 |
| [002](002_bash-watch-backed-handle-state.md) | Bash handle state uses watch-backed exit notifications and snapshot shaping | Accepted | REQ-BASH-001, REQ-BASH-003, REQ-BASH-004, REQ-BASH-005, REQ-BASH-006, REQ-BASH-014, REQ-BASH-WS-002 |
| [003](003_bash-process-cleanup-uses-subreaper-kill-tree.md) | Bash process cleanup uses subreaper plus shutdown kill-tree | Accepted | REQ-BASH-003, REQ-BASH-006, REQ-BASH-007 |

## For agents: which decisions bind your task

Consult the relevant ADRs before starting work of each kind.

| Task type | Relevant ADRs |
| --- | --- |
| Creating or restructuring Phoenix spec artifacts | 000 |
| Deciding whether to create a new `design.md` | 000 |
| Migrating legacy `specs/*/design.md` content | 000, 001, 002, 003 |
| Specifying bash command execution / wait-window semantics | 001 |
| Specifying bash handle state observation or response shaping | 002 |
| Specifying bash process cleanup, shutdown cleanup, or kill escalation policy | 003 |

## Decision dependencies

```text
ADR-000 (adopt spEARS v2 for new work)
   └── establishes the shared ADR chain and incremental legacy-spec migration path
      └── ADR-001 (Bash handles are first-class process-local entities with wait windows)
         ├── ADR-002 (Bash handle state uses watch-backed exit notifications and snapshot shaping)
         └── ADR-003 (Bash process cleanup uses subreaper plus shutdown kill-tree)
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
