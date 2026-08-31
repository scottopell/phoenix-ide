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
| [019](019_runtime-ownership-requires-positive-evidence.md) | Runtime ownership requires positive evidence | Accepted | REQ-DEPLOY-002A, REQ-RU-004A |
| [020](020_durable-workflow-core-matches-one-scheduler-and-durable-acknowledgement.md) | Durable-workflow core matches one scheduler authority and durable acknowledgement | Accepted | REQ-DWF-002, REQ-DWF-006, REQ-DWF-014, REQ-DWF-017, REQ-DWF-029–042, wake and creation profile reshaping |
| [021](021_coordinator-surface-is-chat-only.md) | The Coordinator surface is chat-only | Accepted | REQ-GR-001, REQ-GR-004, REQ-GR-005, REQ-GR-010, REQ-GR-011 |
| [022](022_coordinator-uses-relational-evidence.md) | The Coordinator uses bounded relational evidence | Partially superseded by ADR-027 | REQ-GR-001–005, REQ-GR-007–011A |
| [023](023_projects-accept-taskmd-and-plain-markdown-briefs.md) | Projects accept taskmd files by default and plain markdown briefs through one task-source seam | Accepted | REQ-PROJ-003, REQ-PROJ-004, REQ-PROJ-006, REQ-PROJ-012, REQ-PROJ-033, REQ-PROJ-034, REQ-PROJ-037 |
| [024](024_direct-turn-authority-is-partitioned-by-semantic-fact.md) | Direct-turn authority is partitioned by semantic fact | Accepted | REQ-DWF-CHAT-001 through REQ-DWF-CHAT-014 |
| [025](025_continuation-compaction-is-an-idempotent-durable-operation.md) | Continuation compaction is an idempotent durable operation | Accepted | REQ-BED-020 |
| [026](026_workscope-owned-lifecycle-unifies-conversation-handoffs.md) | Product conversation lifecycle is separate from WorkScope resource ownership | Accepted | REQ-BED-019, REQ-BED-028, REQ-BED-029, REQ-BED-030, REQ-PROJ-004, REQ-PROJ-015, REQ-PROJ-WS-001, REQ-WL-001, REQ-WL-002, REQ-PRA-000, REQ-CHN-008, REQ-GR-001 |
| [027](027_write-capable-product-conversations-use-global-evidence.md) | Write-capable ProductConversations use bounded global evidence | Accepted | REQ-GR-004, REQ-GR-007, REQ-GR-012 |
| [028](028_ios-companion-includes-read-only-project-context-and-prose-review.md) | The iOS companion includes read-only project context and prose review | Superseded by ADR-029 | REQ-IOS-019, REQ-IOS-020, REQ-IOS-021 |
| [029](029_ios-companion-uses-session-scoped-prose-feedback.md) | The iOS companion uses session-scoped prose feedback | Superseded by ADR-030 | REQ-IOS-019, REQ-IOS-020, REQ-IOS-021 |
| [030](030_ios-prose-review-authority-survives-composer-handoff.md) | iOS prose-review authority survives the composer handoff | Accepted | REQ-IOS-002, REQ-IOS-003, REQ-IOS-021; `ProseReviewAuthority` |
| [031](031_productconversation-persistence-uses-staged-single-authority.md) | ProductConversation persistence uses staged single authority | Accepted | REQ-BED-029, REQ-BED-030, REQ-BED-030A, REQ-BED-031B, REQ-CHN-002/003/005/007/008/009/010, REQ-GR-001/002/005/009/011, REQ-PROJ-014/015/019, REQ-PROJ-WS-001, REQ-WL-001/002; `ProductConversation`, `Conversation`, `CloseObligation`, `CloseAttemptMember`, `AttachedWorkScope` |
| [032](032_gitrepository-is-hidden-infrastructure-project-is-retired.md) | GitRepository is hidden infrastructure; Project is retired | Accepted | REQ-GITREP-001–008, REQ-PROJ-015/020/021/024/025/028a, REQ-WL-002b; `GitRepository`, `WorkScope.repository`, `RestartRepairEvidence`, `RetirementAbsenceEvidence` |
| [033](033_offline-database-rollback-and-foundation-storage.md) | Database rollback is offline and Foundation observations use relational scalar storage | Accepted | REQ-GITREP-001–004; `RepositoryLocatorObservation`, `DefaultBranchObservation` |
| [034](034_compatibility-guarantees-are-explicit-and-data-aware.md) | Compatibility guarantees are explicit and data-aware | Accepted | REQ-COMP-001–005 |
| [035](035_repository-authority-activation-is-consumer-triggered-and-offline.md) | Repository authority activation is consumer-triggered and offline | Accepted | REQ-GITREP-004/009; `GitRepository`, `WorkScope.repository`, repository authority generation |
| [036](036_local-sqlite-authority-loss-fails-stop.md) | Local SQLite authority loss fails stop | Accepted | REQ-DWF-043, REQ-DWF-CHAT-013, REQ-BED-033 |
| [037](037_legacy-direct-turn-terminal-ambiguity-is-retired.md) | Legacy direct-turn terminal ambiguity is retired as failure | Accepted | REQ-DWF-CHAT-013, REQ-DWF-CHAT-014, REQ-COMP-004 |
| [038](038_commission-review-is-retired-with-forward-history-recovery.md) | Commission review is retired with forward history recovery | Accepted | REQ-CR-001–018, REQ-COMP-001–005, REQ-VS-006/016 |
| [039](039_durable-runtime-resource-identity-fails-closed.md) | Durable runtime resource identity fails closed outside proven containment | Accepted | REQ-WL-002b, REQ-WL-002d, REQ-COMP-001 |
| [040](040_close-uses-scope-gates-and-tmux-only-durable-identity.md) | Close uses WorkScope gates and tmux-only durable identity | Accepted | REQ-WL-002b, REQ-WL-002d, REQ-PROJ-WS-001 |
| [041](041_minimal-history-finalization.md) | Minimal History finalization | Accepted | REQ-WL-002b, REQ-CONV-001 |
| [042](042_close-directory-retirement-trusts-private-namespace.md) | Close directory retirement trusts its private namespace | Accepted | REQ-WL-002b |
| [043](043_creation-staging-uses-a-private-locked-namespace.md) | Creation staging uses a private locked namespace | Accepted | REQ-CCR-005; product-creation worktree ownership and cleanup |
| [044](044_creation-publication-uses-request-bound-identity-and-immutable-pins.md) | Creation publication uses request-bound identity and immutable starting pins | Accepted | REQ-CCR-001/002/003/005/005A/006A, REQ-DWF-CREATE-001/002/003/004, REQ-PROJ-017/022, REQ-GITREP-003 |
| [045](045_provider-prompts-use-persisted-generation-fenced-projections.md) | Provider prompts use persisted generation-fenced projections | Accepted | REQ-BED-018A, REQ-BED-020, REQ-BED-030A |

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
| Specifying request-bound creation identity, immutable creation pins, atomic creation publication, or staging cleanup ownership | 043 for the private staging boundary, then 044, 007, and 031 |
| Specifying multi-PR branch observation, active PR targeting, or bash terminal-edge reconciliation | 008 |
| Specifying native process resource sampling, Work Scope health, or resource-observation freshness | 009 |
| Specifying native macOS self-deployment, activation, or rollback | 010 |
| Specifying wake-plane registration receipts, durable wake observations, or wake resume outbox | 006, 011, 012 |
| Specifying the shared durable workflow engine, profiles, migration, or drain | 013, 014, 015, 016, 019, 020, 024 |
| Specifying product conversation lifecycle versus WorkScope resource ownership, continuation topology, or worktree lifecycle across continuations | 026 |
| Specifying ProductConversation persistence identity, Close-attempt ownership, or staged lifecycle/attachment authority cutover | 031, then 026 |
| Specifying hidden GitRepository identity, mutable repository locator/default-branch observations, database replacement/rollback, retained restart-repair evidence, repository authority activation, or repository survival beyond one deleted conversation | 035 for activation, then 033, 032, 031, and 026 |
| Specifying workflow CAS, effect claims, leases, ambiguity, or compensation | 014, 019 |
| Specifying local SQLite authority classification, persistence-health fail-stop, or restart reconstruction | 036, then 024, 020, 014 |
| Specifying observations, receipts, reducer delivery, or runtime acceptance | 015, 019 |
| Specifying cross-platform production deployment, Linux activation, or shared candidate preparation | 017, 010 for launchd refinements |
| Adding compatibility, downgrade, rollback, database-replacement, or internal SQLite timestamp guarantees | 034, then the owning feature ADRs (033 for GitRepository Foundation) |
| Retiring commission-review execution, pending approval state, or specialized history/viewer authority | 038, then 034 |
| Specifying in-app published release discovery, approval-bound self-update, or post-reconnect release-update status hydration | 018, 017 |
| Specifying the Coordinator surface, current-activity orientation, or database read boundary | 027 for tool eligibility, then 022 and 021 for Coordinator-specific evidence and UI history |
| Specifying projects task-file shapes, proposal classification, or managed approval behavior across taskmd and plain markdown briefs | 023 |
| Specifying continuation summary retry, restart recovery, or exactly-once commit | 025 |
| Specifying provider prompt persistence authority, bounded transcript projection, or continuation prompt freezing | 045, then 025 and 031 |
| Specifying iOS grounding, server-backed file browsing, prose reading, or anchored comments | 030, then 029, 028, and 026 for draft authority, reader sessions, the companion boundary, ProductConversation, and WorkScope ownership |

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
      ├── ADR-034 (Compatibility guarantees are explicit and data-aware)
      │   └── ADR-038 (Commission review is retired with forward history recovery)
      ├── ADR-036 (Local SQLite authority loss fails stop)
      │   └── applies ADR-014, ADR-020, ADR-024, and ADR-034 at the local persistence-health boundary
      ├── ADR-037 (Legacy direct-turn terminal ambiguity is retired as failure)
      │   └── applies ADR-024 and ADR-034 to pre-obligation materialized direct turns
      ├── ADR-010 (launchd deployment uses an independent transaction helper)
      │   └── ADR-017 (production deployment shares preparation but keeps backend-owned activation)
      │       └── ADR-018 (release updates use published release previews and approval-bound installations)
      ├── ADR-011 (Wake-plane core uses registration receipts and durable runtime observations)
      │   └── ADR-012 (Wake-resume scheduling uses a durable acceptance outbox)
      └── ADR-013 (Durable workflows use normalized core and typed profiles)
          ├── ADR-014 (Workflow CAS and leased effect authority)
          ├── ADR-015 (Observation, receipt, and runtime acceptance are distinct)
          ├── ADR-016 (Durable-workflow boundaries include client acceptance and adoption)
          ├── ADR-019 (Runtime ownership requires positive evidence)
          └── ADR-020 (Durable-workflow core matches one scheduler authority and durable acknowledgement)
              └── ADR-024 (Direct-turn authority is partitioned by semantic fact)
      ├── ADR-025 (Continuation compaction is an idempotent durable operation)
      │   └── ADR-045 (Provider prompts use persisted generation-fenced projections)
      ├── ADR-021 (The Coordinator surface is chat-only)
      │   └── ADR-022 (The Coordinator uses bounded relational evidence)
      │       └── ADR-027 (Write-capable ProductConversations use bounded global evidence)
      ├── ADR-023 (Projects accept taskmd files by default and plain markdown briefs through one task-source seam)
      ├── ADR-024 (Direct-turn authority is partitioned by semantic fact)
      └── ADR-026 (Product conversation lifecycle is separate from WorkScope resource ownership)
          ├── ADR-028 (iOS companion adds read-only project context and prose review)
          │   └── ADR-029 (iOS companion uses session-scoped prose feedback)
          │       └── ADR-030 (iOS prose-review authority survives the composer handoff)
          └── ADR-031 (ProductConversation persistence uses staged single authority)
              ├── ADR-043 (Creation staging uses a private locked namespace)
              ├── ADR-044 (Creation publication uses request-bound identity and immutable starting pins)
              └── ADR-032 (GitRepository is hidden infrastructure; Project is retired)
                  ├── ADR-033 (Database rollback is offline and Foundation observations use relational scalar storage)
                  └── ADR-035 (Repository authority activation is consumer-triggered and offline)
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
