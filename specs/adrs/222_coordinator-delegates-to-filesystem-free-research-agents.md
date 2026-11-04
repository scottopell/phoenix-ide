# ADR-222: Coordinator delegates to filesystem-free research agents

- **Status:** Accepted
- **Date:** 2026-11-04
- **Affects:** REQ-GR-007, REQ-SA-001, REQ-SA-002, REQ-SA-007

## Context

The Global Coordinator spends its own model context investigating history and
database facts. Ordinary inline delegation already provides bounded execution,
isolated conversations, cancellation, and result aggregation, but assumes a
filesystem-backed parent and excludes global evidence tools from children.

## Options considered

1. Give the Coordinator ordinary filesystem agents. This requires an arbitrary
   cwd and does not give children the evidence tools needed for coordination.
2. Give children the Coordinator's full tool registry. This unnecessarily
   delegates messaging, subscriptions, unsandboxed Bash, and lifecycle access.
3. Reuse inline fan-in with filesystem-free global research children.

## Decision

Choose option 3. The durable Coordinator parent relation selects a builtin-only
research registry: bounded SQL, history search, transcript reading, reference
resolution, thinking, and terminal submission. Ordinary children retain their
existing capabilities. Research children have no cwd or WorkScope, and cannot
request Work authority. The spawn specification and admission carry a typed
execution environment; absence of a filesystem persists as SQL NULL rather than
an invented directory. Runtime recreation derives the same capability from the
durable parent relation.

The Coordinator loads configured workers and execution tiers, and is prompted to
choose suitable cheaper routes for focused research while retaining synthesis
and actions. No new model default or automatic background delegation is added.

## Consequences

- Existing admission, limits, cancellation, terminal evidence, and fan-in remain
  the lifecycle authority.
- Research remains useful without any active ordinary WorkScope or host sandbox.
- Research children cannot inspect source files or operate external tools;
  filesystem work remains with ordinary work conversations or Coordinator Bash.
- Cheaper model selection is available and encouraged, but actual token savings
  depend on the chosen route and assignment.

## References

- [Coordinator capabilities](../global-recall/requirements.md)
- [Sub-agent behavior](../subagents/subagents.allium)
- `RuntimeManager::admit_sub_agent_batch`, `ToolRegistry::coordinator_research`
