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
| [004](004_inline-terminal-shared-history-per-command.md) | Inline terminal records user commands as per-command bash rounds in shared history | Accepted | REQ-IT-003, REQ-IT-004, REQ-IT-005, REQ-IT-007 |
| [005](005_user-tool-invocation-self-service-scope.md) | User tool invocation is limited to self-service tools; director tools deferred | Accepted | REQ-UTI-003, REQ-UTI-006, REQ-IT-002 |
| [006](006_wake-contracts-are-persisted-conversation-scoped-terminal-waits.md) | Wake contracts are persisted conversation-scoped terminal waits | Superseded by ADR-011 | REQ-WAKE-001, REQ-WAKE-002, REQ-WAKE-003, REQ-WAKE-004, REQ-WAKE-005, REQ-WAKE-006, REQ-WAKE-009, REQ-WAKE-010, REQ-WAKE-012, REQ-WAKE-013, REQ-WAKE-016, REQ-WAKE-017, REQ-WAKE-018 |
| [007](007_conversation-creation-uses-fenced-reconciliation.md) | Conversation creation uses fenced reconciliation | Accepted | REQ-CCR-002, REQ-CCR-003, REQ-CCR-004, REQ-CCR-005, REQ-CCR-007, REQ-CCR-008, REQ-CCR-010 |
| [008](008_multi-pr-selection-uses-durable-branch-observations.md) | Multi-PR selection uses durable settled-branch observations plus explicit active-PR targeting | Accepted | REQ-PRA-000, REQ-PRA-000a, REQ-PRA-000b, REQ-PRA-000c, REQ-BASH-010a, REQ-BASH-010b, REQ-BASH-010c |
| [009](009_shared-demand-driven-resource-observations.md) | Native process metrics use shared demand-driven observation generations | Accepted | REQ-DEPLOY-007a, REQ-PINSP-004, REQ-PINSP-008, REQ-WSUI-006, REQ-WSUI-010 |
| [010](010_launchd-deployment-uses-independent-transaction-helper.md) | launchd deployment uses an independent transaction helper | Accepted | REQ-LDD-001 through REQ-LDD-010 |
| [011](011_wake-plane-core-uses-registration-receipts-and-durable-runtime-observations.md) | Wake-plane core uses registration receipts and durable runtime observations | Accepted | REQ-WAKE-001, REQ-WAKE-002, REQ-WAKE-003, REQ-WAKE-004, REQ-WAKE-006, REQ-WAKE-008, REQ-WAKE-009, REQ-WAKE-012, REQ-WAKE-013, REQ-WAKE-016, REQ-WAKE-017, REQ-WAKE-018 |
| [012](012_wake-resume-scheduling-uses-a-durable-acceptance-outbox.md) | Wake-resume scheduling uses a durable acceptance outbox | Accepted | REQ-WAKE-004, REQ-WAKE-005, REQ-WAKE-008, REQ-WAKE-012 |
| [013](013_durable-workflows-use-normalized-core-and-typed-profiles.md) | Durable workflows use an engine-owned normalized core and typed profiles | Accepted | REQ-DWF-001–003, REQ-DWF-013, REQ-DWF-015–016, wake and creation profiles |
| [014](014_workflow-cas-and-leased-effect-authority.md) | Workflow transitions use CAS and every claimed effect uses leased authority | Accepted | REQ-DWF-004–012, REQ-DWF-018 |
| [015](015_observation-receipt-and-runtime-acceptance-are-distinct.md) | Observation, receipt, and runtime acceptance are distinct durable facts | Accepted | REQ-DWF-001–002, REQ-DWF-007, REQ-DWF-012, REQ-DWF-017, profile acceptance |
| [016](016_durable-workflow-boundaries-include-clients-and-adoption.md) | Durable-workflow boundaries include client acceptance and profile adoption | Accepted | REQ-DWF-013, REQ-DWF-019, REQ-DWF-022, REQ-DWF-029–032, creation acceptance, wake delivery |
| [017](017_production-deployment-shares-preparation-not-activation-ownership.md) | Production deployment shares preparation but keeps backend-owned activation | Accepted | REQ-PD-001 through REQ-PD-017 |
| [018](018_release-updates-use-published-release-previews-and-approval-bound-installations.md) | Release updates use published release previews and approval-bound installations | Accepted | REQ-RU-001 through REQ-RU-010 |

## For agents: which decisions bind your task

Consult the relevant ADRs before starting work of each kind.

| Task type | Relevant ADRs |
| --- | --- |
| Creating or restructuring Phoenix spec artifacts | 000 |
| Deciding whether to create a new `design.md` | 000 |
| Migrating legacy `specs/*/design.md` content | 000, 001, 002, 003, 004, 005, 006 |
| Specifying bash command execution / wait-window semantics | 001 |
| Specifying bash handle state observation or response shaping | 002 |
| Specifying bash process cleanup, shutdown cleanup, or kill escalation policy | 003 |
| Specifying inline-terminal history commit, per-command rounds, or user-origin attribution | 004 |
| Deciding which tools a user may invoke directly (user tool invocation eligibility) | 005 |
| Specifying wake contracts, async terminal waits, or sub-agent terminal wake delivery | 006 |
| Specifying durable conversation creation, worker claims, retries, or provisioning cleanup | 007 |
| Specifying multi-PR branch observation, active PR targeting, or bash terminal-edge reconciliation | 008 |
| Specifying native process resource sampling, Work Scope health, or resource-observation freshness | 009 |
| Specifying native macOS self-deployment, activation, or rollback | 010 |
| Specifying wake-plane registration receipts, durable wake observations, or wake resume outbox | 006, 011, 012 |
| Specifying the shared durable workflow engine, profiles, migration, or drain | 013, 014, 015, 016 |
| Specifying workflow CAS, effect claims, leases, ambiguity, or compensation | 014 |
| Specifying observations, receipts, reducer delivery, or runtime acceptance | 015 |
| Specifying cross-platform production deployment, Linux activation, or shared candidate preparation | 017, 010 for launchd refinements |
| Specifying in-app published release discovery, approval-bound self-update, or post-reconnect release-update status hydration | 018, 017 |

## Decision dependencies

```text
ADR-000 (adopt spEARS v2 for new work)
   └── establishes the shared ADR chain and incremental legacy-spec migration path
      ├── ADR-001 (Bash handles are first-class process-local entities with wait windows)
      │  ├── ADR-002 (Bash handle state uses watch-backed exit notifications and snapshot shaping)
      │  └── ADR-003 (Bash process cleanup uses subreaper plus shutdown kill-tree)
      ├── ADR-005 (User tool invocation is limited to self-service tools)
      │  └── ADR-004 (Inline terminal records per-command bash rounds in shared history — the bash specialization)
      ├── ADR-006 (Wake contracts are persisted conversation-scoped terminal waits)
      ├── ADR-007 (Conversation creation uses fenced reconciliation)
      ├── ADR-008 (Multi-PR selection uses durable settled-branch observations plus explicit active-PR targeting)
      ├── ADR-009 (Native process metrics use shared demand-driven observation generations)
      ├── ADR-010 (launchd deployment uses an independent transaction helper)
      │   └── ADR-017 (production deployment shares preparation but keeps backend-owned activation)
      │       └── ADR-018 (release updates use published release previews and approval-bound installations)
      ├── ADR-011 (Wake-plane core uses registration receipts and durable runtime observations)
      │   └── ADR-012 (Wake-resume scheduling uses a durable acceptance outbox)
      └── ADR-013 (Durable workflows use normalized core and typed profiles)
          ├── ADR-014 (Workflow CAS and leased effect authority)
          ├── ADR-015 (Observation, receipt, and runtime acceptance are distinct)
          └── ADR-016 (Durable-workflow boundaries include client acceptance and adoption)
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
