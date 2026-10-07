# ADR-081: Global Coordinator borrows WorkScope SVG source authority

- **Status:** Accepted
- **Date:** 2026-09-20
- **Affects:** REQ-SVG-001, REQ-GR-007

## Context

ADR-058 exposed durable SVG publication to Direct/Work and Work subagents, whose tool contexts already have filesystem authority. The singleton Global Coordinator is intentionally filesystem-free, but it can run Bash only against one explicit active WorkScope resolved by Phoenix. Users also need that Coordinator to publish visuals without creating another store, publisher, retrieval API, or ambient filesystem capability.

The WorkScope that contains a staged source is not the conversation that requested the visual. Treating its ordinary conversation as artifact owner would misattribute history and replay; accepting a caller-supplied root would bypass persisted WorkScope authority.

## Options considered

1. Add ordinary `present_svg` to the generic Coordinator registry: minimal registration, but every call fails without filesystem context or requires granting broad filesystem authority.
2. Publish through the selected WorkScope's ordinary conversation: reuses its authority, but gives the artifact the wrong transcript and invocation identity.
3. Let the Global Coordinator borrow one active WorkScope solely to read a contained staged source while retaining its own publication identity.

## Decision

Expose a Coordinator-specific `present_svg` contract through the real application-supplied Global registry. Require the same explicit active `work_scope_id` used by targeted Coordinator Bash. Resolve the WorkScope server-side through persisted live-owner authority; do not accept a caller-supplied root.

The borrowed capability is one source read, not ambient filesystem authority. On supported Unix hosts, open the absolute WorkScope root and every source component descriptor-relatively without following symlinks. Fail closed on platforms where that guarantee is unavailable. Reuse ADR-058's validator, atomic snapshot store, routes, replay, and share behavior unchanged.

Artifact ownership remains the executing Global transcript, and invocation identity remains its exact assistant-message/tool-use pair. Replay checks that durable identity before requiring the selected WorkScope to remain active or the staging file to exist.

## Consequences

The Global Coordinator can publish visuals staged through one explicitly selected active WorkScope without gaining patch, generic filesystem, or unrelated write tools. Ordinary Direct/Work and attached-child capability matrices remain independent.

Valid Global provider calls have a distinct typed input carrying `work_scope_id`; they are not persisted as malformed ordinary SVG calls. Authority absence, persistence failure, and unreadable roots remain distinguishable. The no-follow guarantee currently makes Global publication unavailable on non-Unix hosts rather than silently weakening containment.
