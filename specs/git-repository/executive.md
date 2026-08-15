# Git Repository — Executive Summary

## What This Spec Covers

`specs/git-repository/` is now the normative home for Phoenix's hidden repository model: opaque local `GitRepository` identity, mutable locator observation, optional provenanced default-branch observation, singular nullable `WorkScope.repository` attachment, immutable restart repair evidence, repository-backed worktree registry, and branch-observation surfaces that read repository state without turning repositories into user-facing lifecycle objects.

## Current Reality

The additive Repository Foundation is shipped: deterministic Project-seeded hidden `GitRepository` rows and singular nullable `WorkScope.repository` attachments exist as dormant relational data, with query-only readiness validation. Legacy `Project` remains the sole live repository authority, and Foundation locator/default-branch observations may truthfully remain empty because Foundation performs no live probes. No ProductConversation or Close capability has made hidden authority live.

Authority activation is deferred until an owning normative requirement for an exact ProductConversation or destructive Close capability requires generation `2`. The offline operation stops Phoenix, acquires exclusive SQLite access, captures and verifies the exact pre-activation snapshot with its paired Project-authority binary, preserves seeded identities, migrates or quarantines every repository-sensitive reader and writer, and changes authority transactionally. Live cutover, runtime-wide drain, identity convergence, and production authorization from a source census are not supported activation behavior.

## Requirements Summary

| ID | Summary |
|----|---------|
| REQ-GITREP-001 | Hidden repository identity is opaque and Phoenix-local |
| REQ-GITREP-002 | Git common dir and management-root locators are mutable observations with explicit status |
| REQ-GITREP-003 | Default branch is optional, provenanced, and never fabricated |
| REQ-GITREP-004 | `WorkScope.repository` is singular, nullable, and ProductConversation-derived |
| REQ-GITREP-005 | Continuation retains repository attachment; follow-up gets a fresh scope that may target the same repository |
| REQ-GITREP-006 | Restart repair evidence is immutable, typed, and identity-bound |
| REQ-PROJ-015 | GitRepository worktree registry |
| REQ-GITREP-007 | Hidden repository identity may survive conversation deletion when Phoenix still needs repository truth |
| REQ-GITREP-008 | Hidden GitRepository owns no user-facing lifecycle or workflow surface |
| REQ-GITREP-009 | Repository authority activation is consumer-triggered and offline |
| REQ-PROJ-020 | Local branch discovery uses only local data |
| REQ-PROJ-021 | Remote branch search is on-demand and cached |
| REQ-PROJ-024 | Existing-branch work is repository state, not creation mode |
| REQ-PROJ-025 | Prefer reusing live conversation context over silently duplicating ownership |

## Normative Authority

Current normative authority is `requirements.md` and `git-repository.allium`. ADR-032 records the hidden-repository identity and staged single-authority decision. ADR-033 records offline paired database rollback, stopped-process replacement, and integer Unix-microsecond Foundation observation storage. ADR-035 records consumer-triggered offline authority activation and replaces ADR-032's coordinated live-reader/writer activation mechanism. This executive distinguishes the shipped dormant Foundation from the deferred authority transition.

## Implementation Status

| Requirement | Status | Notes |
|-------------|--------|-------|
| REQ-GITREP-001 | Foundation implemented; activation deferred | Opaque Project-seeded hidden identities exist dormantly; Project remains live authority |
| REQ-GITREP-002 | Foundation schema implemented | Mutable locator rows have typed status, but Foundation performs no live observation and valid tables may be empty |
| REQ-GITREP-003 | Partially implemented | Typed dormant observation storage exists and Phoenix observes branch facts through legacy paths, but the hidden model is not live authority |
| REQ-GITREP-004 | Foundation attachment implemented; activation deferred | Singular nullable `WorkScope.repository` is backfilled dormantly; ProductConversation and pre-scope hidden-authority consumers are not live |
| REQ-GITREP-005 | Partially implemented | Continuation already preserves one work context, and follow-up is specified as fresh work, but repository attachment is still carried through legacy surfaces |
| REQ-GITREP-006 | Not implemented | Immutable restart repair evidence bound to ProductConversation, WorkScope, hidden GitRepository, and fingerprint is not yet the shipped persistence contract |
| REQ-PROJ-015 | Partially implemented | Worktree reconciliation exists, but the explicit hidden-repository authority and typed repair evidence are still incomplete |
| REQ-GITREP-007 | Not implemented | Repository survival beyond one deleted conversation remains normative future work |
| REQ-GITREP-008 | Partially implemented | Phoenix does not ship a first-class repository management product surface, but many legacy `project` names still appear in code and docs |
| REQ-GITREP-009 | Not implemented; consumer-blocked | Project remains sole authority; no owning ProductConversation or destructive Close requirement explicitly mandates generation `2` |
| REQ-PROJ-020 | Complete (legacy current reality) | Branch listing is local-first and does not fetch on the no-query path |
| REQ-PROJ-021 | Complete (legacy current reality) | Remote search is on-demand via `ls-remote` with caching |
| REQ-PROJ-024 | Complete (legacy current reality) | Existing-branch work happens as repository operations inside the disposable worktree |
| REQ-PROJ-025 | Partially implemented | Product intent favors reuse of live work context, but hidden repository identity is not yet the sole authority behind those decisions |

## Migration Notes

This spec intentionally separates observable repository facts from SQL or row-shape claims. The dormant Foundation preserves legacy behavior while introducing hidden repository identity and typed evidence. It incurs temporary duplicate storage without creating a second writable authority.

Authority changes only when an exact owning consumer requirement explicitly mandates generation `2`. The offline operation preserves the Project-seeded identity partition; identity convergence has no contract in this feature and requires separate normative design. A source census proves reader/writer migration completeness in CI and review but is not production activation authority.

The additive Foundation uses relational INTEGER Unix-microsecond observation columns and rejects NUL in filesystem/Git text. Rollback acceptance restores a pre-upgrade database backup while Phoenix is stopped and boots the matching historical binary; an older binary opening a newer-migrated database and live same-path database replacement are not supported contracts.

## Cross-Spec Notes

- `specs/conversation-creation/` owns creation acceptance and canonical-default provisioning flow.
- `specs/bedrock/` owns ProductConversation lifecycle and fresh follow-up semantics.
- `specs/work-lifecycle/` owns Close retirement and exact-attempt adoption of retained repair evidence.
- `specs/projects/` is a retired index only.
