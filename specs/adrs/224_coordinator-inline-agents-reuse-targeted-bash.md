# ADR-224: Coordinator inline agents reuse targeted Bash

- **Status:** Accepted
- **Date:** 2026-11-04
- **Affects:** REQ-GR-007, REQ-SA-002, REQ-SA-007, REQ-BASH-012, REQ-BASH-013

## Context

Ember's inline agents can inspect global evidence and targeted files, but searches,
pipelines, and scripts still consume the Coordinator's own context. The user
approved trusted shell delegation using the existing Coordinator Bash tool and
explicit WorkScope targeting.

## Options considered

1. Retain ADR-223's enforced read-only registry. This leaves substantial shell
   investigation with Ember and limits the benefit of delegation.
2. Give children an implicit Coordinator working directory and ordinary Bash.
   This introduces ambient filesystem context and a second targeting convention.
3. Reuse the Coordinator's explicitly targeted Bash adapter alongside the existing
   evidence tools. This grants host write capability while preserving one target
   resolver, handle implementation, and execution contract.

## Decision

Choose option 3, superseding ADR-223's read-only capability decision. The dedicated
child registry includes `WorkScopeCoordinatorBash`. Every run names an active
persisted WorkScope, resolved server-side. Ember supplies its home scope for
general work or a selected ordinary scope for project work. The scope chooses the
starting directory and resource owner; it does not confine filesystem access.

Preserve the typed environment without an ambient cwd, no WorkScope attachment,
durable Coordinator-parent capability selection, worker/model choices, and normal
inline admission, limits, cancellation, terminal submission, and fan-in. Children
do not inherit the Coordinator's home attachment; batch admission still checks
the parent's persisted scope snapshot. Typed tool-call parsing accepts the same
explicit WorkScope contract for both the Coordinator and its filesystem-free children.
Children still use Explore spawn mode; that selector does not imply a Bash sandbox for
this dedicated registry. No new Bash variant or sandbox policy is introduced.

Prompts describe unsandboxed write capability and require explicit assignment
authority within existing user authorization. Ember retains synthesis, decisions,
and responsibility for actions. Messaging, Authority, lifecycle, MCP, browser,
and recursive delegation tools remain absent.

## Consequences

- Shell research and authorized scripts can run in a cheaper focused agent.
- Both parent and children reuse authoritative WorkScope resolution and existing
  Bash execution and handle bounds, including hosts without Explore sandbox support.
- These are trusted agents with host write and network capability. Prompt limits
  on messaging and lifecycle actions are behavioral instructions, not an OS
  enforcement boundary against performing those actions through Bash.
- Children retain their own handle controller identity; this does not add
  Coordinator continuation control over child-created handles.
- No deployment, automatic background delegation, or new model default follows
  from this decision.

## References

- [ADR-223](223_coordinator-research-uses-explicit-file-targets.md)
- [Global capability contract](../global-recall/requirements.md)
- [Sub-agent behavior](../subagents/subagents.allium)
- [Bash contract](../bash/requirements.md)
- `WorkScopeCoordinatorBash`, `ToolRegistry::coordinator_research`
