# Git Repository — Executive Summary

## What This Spec Covers

`specs/git-repository/` is now the normative home for Phoenix's hidden repository model: opaque local `GitRepository` identity, mutable locator observation, optional provenanced default-branch observation, singular nullable `WorkScope.repository` attachment, immutable restart repair evidence, repository-backed worktree registry, and branch-observation surfaces that read repository state without turning repositories into user-facing lifecycle objects.

## Current Reality

The shipped implementation still largely reflects the legacy `project`-named model. Repository identity and worktree ownership are still inferred from existing row fields, paths, and continuation relationships; the hidden `GitRepository` shape described here is not yet fully implemented as an explicit first-class authority. Branch discovery and remote-search behavior exist in the product, but the staged migration to opaque hidden repository identity, mutable locator status, typed restart repair evidence, and singular repository attachment remains normative future work.

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
| REQ-PROJ-020 | Local branch discovery uses only local data |
| REQ-PROJ-021 | Remote branch search is on-demand and cached |
| REQ-PROJ-024 | Existing-branch work is repository state, not creation mode |
| REQ-PROJ-025 | Prefer reusing live conversation context over silently duplicating ownership |

## Normative Authority

Current normative authority is `requirements.md` and `git-repository.allium`. ADR-032 records the hidden-repository identity and staged migration decision. This executive describes implementation drift from that target instead of claiming the hidden repository model is already shipped.

## Implementation Status

| Requirement | Status | Notes |
|-------------|--------|-------|
| REQ-GITREP-001 | Not implemented | Shipped code still uses legacy project/path-oriented identity surfaces instead of one explicit opaque hidden `GitRepository` authority |
| REQ-GITREP-002 | Not implemented | Mutable locator observations with `present` / `missing` / `inaccessible` status are a normative target, not a shipped contract |
| REQ-GITREP-003 | Partially implemented | Phoenix already observes default-branch facts for provisioning and branch UI, but the explicit optional-provenance contract is not yet the sole authority |
| REQ-GITREP-004 | Not implemented | Singular nullable `WorkScope.repository` attachment and pre-scope repository evidence remain migration targets |
| REQ-GITREP-005 | Partially implemented | Continuation already preserves one work context, and follow-up is specified as fresh work, but repository attachment is still carried through legacy surfaces |
| REQ-GITREP-006 | Not implemented | Immutable restart repair evidence bound to ProductConversation, WorkScope, hidden GitRepository, and fingerprint is not yet the shipped persistence contract |
| REQ-PROJ-015 | Partially implemented | Worktree reconciliation exists, but the explicit hidden-repository authority and typed repair evidence are still incomplete |
| REQ-GITREP-007 | Not implemented | Repository survival beyond one deleted conversation remains normative future work |
| REQ-GITREP-008 | Partially implemented | Phoenix does not ship a first-class repository management product surface, but many legacy `project` names still appear in code and docs |
| REQ-PROJ-020 | Complete (legacy current reality) | Branch listing is local-first and does not fetch on the no-query path |
| REQ-PROJ-021 | Complete (legacy current reality) | Remote search is on-demand via `ls-remote` with caching |
| REQ-PROJ-024 | Complete (legacy current reality) | Existing-branch work happens as repository operations inside the disposable worktree |
| REQ-PROJ-025 | Partially implemented | Product intent favors reuse of live work context, but hidden repository identity is not yet the sole authority behind those decisions |

## Migration Notes

This spec intentionally separates observable repository facts from SQL or row-shape claims. The staged migration target is:

1. preserve legacy behavior while introducing hidden repository identity and typed evidence;
2. keep one writable authority per repository fact;
3. move references away from retired `projects.allium` and project-as-product vocabulary;
4. let lifecycle, deletion, and Close consume repository evidence without turning repositories into user-facing owners.

## Cross-Spec Notes

- `specs/conversation-creation/` owns creation acceptance and canonical-default provisioning flow.
- `specs/bedrock/` owns ProductConversation lifecycle and fresh follow-up semantics.
- `specs/work-lifecycle/` owns Close retirement and exact-attempt adoption of retained repair evidence.
- `specs/projects/` is a retired index only.
