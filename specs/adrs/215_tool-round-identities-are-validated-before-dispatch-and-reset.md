# ADR-215: Tool-round identities are validated before dispatch and reset

- **Status:** Accepted
- **Date:** 2026-11-02
- **Affects:** REQ-BED-004, REQ-BED-007

## Context

Tool-round assembly checks cardinality while finalized database persistence separately validates identity correspondence. The restart materializer can therefore accept equal-count duplicate or unknown results. A rejected count or undecodable held state is skipped before a later global reset, which can erase the only durable assistant and completed results. Tightening assembly alone also turns live reducer `expect` calls into reachable panics when a provider sends malformed call identities.

## Options considered

1. **Validate only the constructor.** This creates a shared proof but leaves unsafe live rejection and startup reset behavior.
2. **Reject malformed input before dispatch and refuse malformed startup materialization.** This preserves valid serial behavior and existing provider-error handling while retaining unsupported buffered input for diagnosis.
3. **Add a rejected-round lifecycle and durable quarantine.** This could retain arbitrary malformed live state while serving, but introduces new state, schema, admission, cleanup, and UI policy beyond input admission.

## Decision

Choose option 2. A shared identity validator establishes uniqueness and one-to-one correspondence without sorting. Provider response admission runs before ordinary tools and parent/child special-tool interception. Invalid response schedules use the existing bounded invalid-response retry and terminal-error policy, before any tool effect has occurred. Each tool channel binds its outcome to its owning invocation; a mismatched identity becomes an explicit failure for that invocation instead of attributing unknown output or rerunning the tool.

Startup materialization returns an error for an undecodable or invalid held round before orphan repair and global reset. The persisted buffer remains unchanged, and the server does not admit execution from it. This is a refusal policy, not an automatic repair or a new cross-version compatibility promise. Arbitrary manually corrupted live state does not gain a quarantine or recovery workflow.

All selected rounds are read and validated into write plans before any recovery mutation begins. Per-row validation followed immediately by commit is insufficient: settling a valid parent can overwrite a later selected child's malformed buffer before that child's validation runs. The scoped materializer still ignores buffers outside its selected conversation set.

## Consequences

- **Positive:** Both live admission and restart assembly use the same identity rule, and valid supplied order, metadata, and trusted-skill receipts are preserved.
- **Negative:** One malformed persisted tool round can prevent startup until its unsupported data condition is resolved offline. No automatic repair is inferred.
- **Neutral:** Serial scheduling, persistence formats, provider retry policy, and tool execution permissions remain unchanged.

## References

- [Bedrock requirements](../bedrock/requirements.md)
- `ToolRoundAssembly`, `validate_response_tool_calls`, `forward_tool_outcome`, `Database::reset_all_to_idle`
- [ADR-036: local SQLite authority loss](036_local-sqlite-authority-loss-fails-stop.md) governs unclassified local persistence outcomes; a known identity rejection does not select that uncertainty policy.
