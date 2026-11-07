# Own-conversation Goal tools

## User need

An ordinary agent can explicitly enable automatic continuation for its own conversation and separately save the objective it is commissioned to pursue, without raw HTTP, arbitrary-target authority, or implicit scheduling.

## Requirements

### REQ-SGT-001 — Own ordinary conversation only
When an ordinary agent invokes `enable_auto_continuation` or `create_goal`, the server shall derive the owning ProductConversation, current transcript, durable execution, generation, and canonical tool-call identity from its execution context. Neither tool shall accept a target, actor, execution identifier, version, or request identifier. Coordinator and subagent execution shall not receive these tools or gain their effects through a forged invocation. Remote Coordinator Goal operations shall retain their separate authority and admission rules.

### REQ-SGT-002 — Separate enable-only operation
When an admitted agent invokes `enable_auto_continuation` with an empty argument object, Phoenix shall persist the enabled preference through the existing owner preference boundary. This operation shall not disable automation, release Stop, dispatch or retry work, rearm exhausted work, reactivate a Goal, or create a Goal. Enabling an already-enabled preference shall not create additional execution authority.

### REQ-SGT-003 — Create only
When an admitted agent invokes `create_goal` with a nonempty objective, Phoenix shall create a saved Goal through the shared transactional Goal writer only if automatic continuation is enabled and no Goal identity already exists for the owner. A Goal of any disposition shall cause refusal rather than edit, replacement, or reactivation. Creation shall not enable automatic continuation. Separate calls shall remain separate checkpointed operations, with no combined enable-and-create shortcut.

### REQ-SGT-004 — Admission and human-disable fencing
Phoenix shall bind self-management authority at durable turn admission, including turns with no Goal. First application shall require the current ordinary owner and transcript, live Runtime ownership, exact admitted generation, unchanged Goal execution controls, an open lifecycle with no pending Close, and an active tool-execution checkpoint. Stop, pause, stale execution, transcript succession, or Close shall not be bypassed by either tool. Admission shall capture the existing Stop-control version or its absence; finalization shall require that observation to remain unchanged and the control to be unstopped. These tools shall not require installation of automatic Goal-execution policy or initialize Stop controls.

Every explicit preference disable, including a disable of an already-disabled preference, shall advance a durable owner-scoped disable epoch. If a disable commits after a turn's admission and before its self-management checkpoint, that turn's operation shall refuse even if another writer subsequently enables the preference. If enable commits first and disable commits second, the preference shall remain disabled. A genuinely new turn may receive fresh admission; retry or recovery of an old turn shall not refresh its authority.

### REQ-SGT-005 — Atomic finalization
Phoenix shall represent uncommitted self-management intents as typed pending results rather than finalized successes. At the existing canonical tool checkpoint it shall validate the persisted assistant message, unique tool-use identity, name, and exact arguments, then commit the effect or semantic refusal together with its canonical tool result, selected state, and any provider replay, task proposal, or terminal obligation carried by that checkpoint. Storage and integrity failures shall not be disguised as semantic refusals or leave partial effects.

### REQ-SGT-006 — Immutable replay
When a committed self-management tool round is retried after response loss or restart, Phoenix shall return the retained result without repeating the mutation. Changed source, arguments, execution binding, or retained canonical evidence shall be rejected. A retained refusal shall remain a refusal after conditions change. Replaying a successful enable after a human disable shall not enable the preference again.

### REQ-SGT-007 — No authority expansion
Saving an objective or enabling the preference shall not expand the commission, tool access, budgets, or approval scope; undo Stop or pause; reactivate Suspended work; or admit a turn. Creating a Goal shall leave the current turn's immutable Goal admission binding unchanged. Subsequent execution shall independently satisfy existing admission and lifecycle rules.

### REQ-SGT-008 — Existing boundaries retained
The tools shall preserve user and Coordinator settled-target Goal writes, explicit replacement semantics, remote authority, and ordinary progress-reporting rules. The self-management path shall not expose general disposition changes, arbitrary targets, or a new automation scheduler.

Related contracts: [Goal continuation](../goal-continuation/requirements.md), [stale tool results](../stale-tool-results/requirements.md), and [compatibility](../compatibility/requirements.md).
