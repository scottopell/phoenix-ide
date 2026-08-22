# Classify pending-delivery SQLite authority with the reusable fail-stop boundary

Task 97002's finite authoritative-command audit found that `WorkflowRepository::materialize_pending_delivery_message` does not share the direct-turn classifier contract. It retries retryable SQLite busy failures up to 20 times with sleeps and infers unique-constraint success through another query. This remains outside task 97002's enforced direct-turn slice.

## Scope

- Define the pending-delivery command's exact owning authoritative rows and closed typed outcomes.
- Return `LocalAuthorityResult::DurableFactEstablished(typed result)` or `DurableFactUnclassified` from the command boundary.
- Permit at most one exact authority classification query after an untyped local SQLite result.
- Route unclassified authority to the existing process fail-stop consumer.
- Replace retry/sleep and unique-constraint inference only where the new exact classifier proves the result.
- Add deterministic command, cancellation/disappearance, and restart tests.

Do not broaden this into every workflow command; audit and file additional bounded tasks for any other command that lacks a proven equivalent contract.
