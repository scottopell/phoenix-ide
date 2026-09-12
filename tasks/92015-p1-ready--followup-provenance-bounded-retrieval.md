# Gate 6 — Follow-up provenance and bounded retrieval

Child of task 92009. Add History-only Start follow-up, visible typed provenance and deleted-source state, host-bound own-conversation/source search, exact result navigation, and non-cascading deletion/FTS consistency while preserving Coordinator global recall separately.


## Predecessor recall subtask

[Task 58058](58058-p2-blocked--predecessor-transcript-recall.md) owns REQ-RET-009:
ordinary-parent discovery, search, and read of the strict predecessor prefix,
including restricted planning parents. Its runtime work waits for ProductConversation
integration and final QA; it does not block the existing integration milestone.
This gate retains whole-conversation/source recall, provenance, and UI navigation.
Reuse the same retrieval substrate, but do not conflate predecessor scope with
all aggregate members or typed follow-up sources.
