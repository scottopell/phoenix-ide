# Goal continuation admission

## User need

A user or Coordinator admitting a Goal needs explicit continuation authority for that ProductConversation. Managing existing Goal evidence must not silently enable automation or take away safety controls.

## Requirements

### REQ-GOAL-AC-001 — Admission prerequisite
When a command creates or replaces a Goal, the shared transactional Goal writer shall require that the owning ProductConversation has automatic continuation enabled. This requirement applies equally to authenticated owner HTTP commands, native Coordinator commands and generated-host admission. A replacement of a Suspended, Complete or Exhausted Goal is a new admission with a new Goal identity; changing the disposition of that inactive identity shall not reactivate it.

### REQ-GOAL-AC-002 — Admission-time authority only
The automatic-continuation prerequisite shall be checked against the same ProductConversation inside the Goal-write transaction before any Goal, history, watch event or accepted command receipt is committed. Disabled admission shall produce an actionable refusal without changing the preference or Goal evidence. An exact retry of an accepted command shall return its retained historical result, including when the preference is disabled.

### REQ-GOAL-AC-003 — Existing evidence and safety controls
A user shall be able to disable automatic continuation while a Goal exists. Disabling shall not erase or replace the Goal or its history. Reading existing evidence, recording an existing active Goal's metadata or progress, and explicitly completing, suspending, exhausting or stopping work shall not require enabling automatic continuation.

### REQ-GOAL-AC-004 — No implicit execution
Neither Goal admission, an admission refusal, nor changing automatic continuation shall implicitly reactivate an inactive Goal, wake exhausted work, dispatch a turn or create an autonomous scheduler. Native lifecycle and settings adapters shall use the shared product authority and persistence boundaries rather than bypassing Goal admission.
