# Work Lifecycle — Executive Summary

## What This Spec Covers

Explicit Close for Git-backed ProductConversations: exact loss inspection, reconstructible-only automatic deletion, owned-resource shutdown and retirement, failure visibility, mandatory Global delivery, and fresh safe retry under the original Close authority. Branches and pull requests are never Close-owned artifacts.

## Current Reality

**Candidate / implementation acceptance unvalidated.** ADR-088 supersedes automatic Close recovery, same-execution retry, and the rule that all cleanup failures must remain Open. This spec slice records the settled contract; it does not certify a source candidate, migration, deployment, or live behavior. Historical green checks under the superseded contract are not acceptance evidence for this contract. No production cleanup or database repair is authorized by this document.

Lifecycle ended and resources retired are separate facts. Confirmed conversation **and** owned-process shutdown permits read-only History with conspicuous `cleanup_attention` after cleanup failure. Uncertain shutdown stays Open with `CloseIncomplete`. A stopped execution is never resumed; startup observes only. Explicit user or Global safe retry allocates a new durable run ordinal under the original Close authority after fresh safety proof. Changed risk, unique/uncertain discard, expanded effects, or database surgery requires a concrete proposal and separate approval.

## Requirements and Acceptance Status

| Requirement | Contract | Status |
|-------------|----------|--------|
| REQ-BED-029 | One normal run stops at first failure; proven shutdown enters History independently of cleanup success | Candidate / unvalidated |
| REQ-WL-001 | Close is the only ordinary terminal action; legacy inputs must preserve its contract | Candidate / unvalidated |
| REQ-WL-002 | Exact loss inventory; automatic deletion needs fresh reconstructibility proof | Candidate / unvalidated |
| REQ-WL-002a | Discard confirmation binds exact original authority, run ordinal, generation, and fingerprint | Candidate / unvalidated |
| REQ-WL-002b | Exact scope/process/private-directory authority; stop on first failure; observation-only startup; no repository mutation or recovery artifact | Candidate / unvalidated |
| REQ-WL-002c | Explicit fresh safe retry; no unresolved replay, completed-effect repetition, or authority expansion | Candidate / unvalidated |
| REQ-WL-002d | Tmux retirement needs exact socket plus server-token authority | Candidate / unvalidated |
| REQ-WL-004 | Durable mandatory once-per-failure Global event through unified delivery, even unwatched or in History | Candidate / unvalidated |
| REQ-PROJ-028a | Missing/inaccessible worktrees remain exact immutable observations, not startup cleanup authority | Candidate / unvalidated |
| REQ-PROJ-WS-001 | One ordinary owner per WorkScope; subordinate participants do not become owners | Candidate / unvalidated |
| REQ-WL-003 | PR state is advisory and never triggers Close | Candidate / unvalidated |

## Required Behavioral Acceptance

These are acceptance obligations, **not tests reported as passing**:

| Scenario | Required evidence |
|----------|-------------------|
| Normal clean Close | Fresh reconstructibility/identity proof, confirmed shutdown, exact successful disposition, atomic History outcome |
| Unique or uncertain work | Preserve resources; exact inventory and concrete discard proposal/decision; no inferred deletion authority |
| First failure before confirmed shutdown | No subsequent Close effect; Open `CloseIncomplete`; exact durable failure and mandatory Global event |
| Cleanup failure after confirmed shutdown | No subsequent Close effect; History `cleanup_attention`; retained residuals; no false cleanup-success message |
| Crash and startup | Read-only observations and truthful shutdown classification only; no settlement/cleanup/retry dispatch |
| Delayed progress or duplicated command | Stopped run cannot dispatch; stale ordinal cannot advance a fresh run; no duplicate failure event |
| Unwatched source / source enters History | Mandatory Global delivery survives watch absence and lifecycle change; exactly one event per failure |
| Safe explicit retry | Fresh resolved-precondition proof, new ordinal under original authority, exact remaining effects only, old stopped evidence unchanged |
| Unresolved / risky retry | Reject without effects; unique discard, changed risk, expanded targets, or DB surgery requires separate concrete proposal |
| Retry fails again | New stopped run, distinct run-bound failure, its own once-per-failure Global event |
| Reused tmux socket / worktree path / missing live permit | Preserve unproven resource; no PID/path-based authority or fabricated shutdown |
| Branches, PRs, recovery artifacts | No deletion or other ref/PR mutation, and no automatic branch/tag/stash/patch/snapshot creation |

## Normative Authority

`requirements.md`, `work-lifecycle.allium`, `specs/bedrock/requirements.md`, and `specs/bedrock/bedrock.allium` define Close behavior. `specs/git-repository/git-repository.allium` defines retained restart observations. ADR-088 records the superseding policy; ADR-026's authority separation and ADR-040/042/080's retained identity/private-namespace safety remain relevant within that bounded policy.

## Validation Notes

This is a specification-only slice. Targeted Allium parsing and repository spec-shape/anchor checks must be distinguished from whole-repository analyser baseline checks and implementation acceptance. No runtime, UI, production, or migration acceptance is claimed here; the required matrix above remains unvalidated until a source candidate is qualified against it.
