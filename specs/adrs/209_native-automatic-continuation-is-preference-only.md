# ADR-209: Native automatic continuation is preference-only

- **Status:** Accepted
- **Date:** 2026-10-31
- **Affects:** REQ-GR-007, REQ-GR-020; REQ-BED-020; REQ-API-029

## Context

The user requests agent-facing automatic-continuation controls alongside native Continue. The public settings routes expose the same aggregate-owned Boolean plus a richer projection of admission and failure history. Their setter can commit a preference before that projection fails. Calling that setter directly from a native mutation adapter would turn a post-write projection error into an ambiguous ordinary refusal. ADR-211 grants generated Continue, not preference mutation or scheduler authority.

## Options considered

1. Have the Coordinator invoke public settings routes through scoped Bash: preserves existing HTTP behavior but leaves native source authority and committing-write classification unresolved.
2. Wrap the public setter and project its full response: reuses its shape but cannot distinguish a failed projection from a failed write.
3. Share the preference SQL primitives and target semantics while giving native operations a source-checked, classified persistence boundary and a preference-only result: selected.

## Decision

Add a separate bounded REQ-GR-007 exception for `read_automatic_continuation` and `set_automatic_continuation`. Closed requests select either an exact published ordinary ProductConversation or the explicit singleton Coordinator aggregate. Set requires a Boolean; read accepts no mutation field. Trusted tool context supplies source identity. The database transaction revalidates the current Coordinator before resolving the target, including reads and invocations through retained tool handles.

Public and native operations use the same on-connection preference primitives. The HTTP application extraction preserves its existing aggregate/status projection and error behavior; it does not acquire a new compatibility or write-classification guarantee. The native setter reads its preference-only result within its committing transaction rather than performing a fallible history projection after commit. The native reader has no uncertain-write classification because it performs no write.

An owned application worker survives requester cancellation through write classification. Acknowledged commit or rollback can release its write owner. Unclassified commit or rollback, or loss of an armed write-classification guard, closes local runtime authority before that owner is released. Closed completion is not an ordinary success/refusal. The adapter withholds ordinary output on closure and relies on the existing fatal-cancellation supervisor for teardown; application and adapter checks alone do not prove executor disposal or final result-persistence exclusion.

Preferences remain prospective. Enabling after exhaustion neither admits nor wakes work; disabling does not revoke an existing admission. Neither tool changes dispatch intents, Goal metadata, stop/pause/closed state, recovery, retry/rearm or approval policy. This decision supplements ADR-191 and ADR-211 rather than expanding their individual capabilities. Goal admission policy remains separately owned.

## Consequences

- **Positive:** Native tools share the durable setting with public surfaces without duplicating scheduling policy or conflating post-write status errors with acknowledged rollback.
- **Positive:** Explicit target variants exclude transcript aliases and prevent accidental self-targeting through the ordinary branch.
- **Negative:** Native results intentionally omit admission/failure history; callers needing that status must use its separately supported read surface.
- **Negative:** Transaction uncertainty, requester cancellation and fatal result-publication boundaries require separate regression and executor evidence.
- **Neutral:** No retained receipt, cross-process fence, automatic recovery, eventual completion or startup-repair guarantee is introduced.

## References

- [Global recall requirements](../global-recall/requirements.md), REQ-GR-007 and REQ-GR-020.
- [Bedrock requirements](../bedrock/requirements.md), REQ-BED-020.
- [API requirements](../api/requirements.md), REQ-API-029.
- [Compatibility requirements](../compatibility/requirements.md).
- [ADR-036](036_local-sqlite-authority-loss-fails-stop.md), [ADR-191](191_native-create-and-rename-use-shared-application-actions.md), and [ADR-211](211_native-continue-reuses-generated-handoff-admission.md).
- `Database::read_native_automatic_continuation_settings`, `Database::set_native_automatic_continuation_settings`, `PreferenceWritePass`, and `AutomaticContinuationTool::run`.
