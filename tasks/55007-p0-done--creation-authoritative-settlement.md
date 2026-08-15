# Creation authoritative runtime-bootstrap settlement

Child of broad task 40004. Repair only the runtime-bootstrap settlement regression in the existing durable conversation-creation path:

- checkpoint the current claimed creation job durably to Finalize before authoritative CreationProvisioned delivery;
- deliver through the acknowledged RuntimeManager::send_event boundary and require settlement;
- atomically settle CompleteCreation with the dispatchable runtime state before RequestLlm in fresh and resumed creation transitions so retry-scheduled reconstruction cannot dispatch before current-claim completion;
- propagate CompleteCreation persistence errors, including startup recovery, while preserving typed stale ClaimLost behavior;
- add deterministic ordering, acknowledgement, and persistence-failure regressions.

This child does not own worker lifecycle or shutdown, claim admission, request draining, managed tasks, PR polling, HTTP/TLS orchestration, lease-loss continuation, direct turns, browser/MCP, Repository authority, or ProductConversation/Close activation. It does not complete or replace task 40004.
