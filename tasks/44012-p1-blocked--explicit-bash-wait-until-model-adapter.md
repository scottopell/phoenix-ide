# Explicit Bash wait_until adapter

Depends on task 44011 and its merged authority/model PR.

Integrate explicit Bash `wait_until` as the first adapter to the authoritative wake aggregate. Background/synchronous Bash response paths remain outside wake contracts. Registration is explicit, produces one contract, parks only a fully successful tool round, and leaves normal `ToolExecuting -> Idle` settlement authoritative.

Prove normal completion, sibling failure, typed user cancellation, deadline, waiter panic with real occurrence time, Phoenix restart resource loss, canonical terminal evidence, exactly-once transcript delivery, and at-most-once automatic resumption. Reuse/cherry-pick good #559 Bash tests and typed evidence commits; remove duplicate runtime/repository authorities as this slice lands.
