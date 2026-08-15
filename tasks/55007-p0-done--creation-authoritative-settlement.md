# Creation authoritative runtime-bootstrap settlement

Child of broad task 40004. Repair only the runtime-bootstrap settlement regression in the existing durable conversation-creation path:

- checkpoint the current claimed creation job durably to Finalize before authoritative CreationProvisioned delivery;
- deliver through the acknowledged RuntimeManager::send_event boundary and require settlement;
- atomically settle the initial message, CompleteCreation, and dispatchable runtime state before RequestLlm so stale authority mutates nothing and retry-scheduled reconstruction cannot dispatch before current-claim completion;
- acknowledge stale authority without scheduling/releasing it, then retire the stale registered runtime before authoritative reconstruction;
- propagate CompleteCreation persistence errors, including startup recovery, while preserving typed stale ClaimLost behavior;
- add deterministic ordering, acknowledgement, and persistence-failure regressions.

This child does not own worker lifecycle or shutdown, claim admission, request draining, managed tasks, PR polling, HTTP/TLS orchestration, lease-loss continuation, direct turns, browser/MCP, Repository authority, or ProductConversation/Close activation. It does not complete or replace task 40004.
