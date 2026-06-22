# Commission Review Executive Summary

## Summary

Commission review adds a Phoenix-native path for an agent to request an
independent review of active git work. The built tool validates a spend brief,
infers the target from the current repository context, collects read-only diff
material, uses Phoenix's configured default LLM, and returns structured findings
and warnings.

The durable first-class approval UI/state-machine flow is not complete. The
current implementation includes a tool-level approval gate and reports rejected
requests without calling the review LLM, but the product still needs the same
kind of persisted human approval surface that task approval uses.

## Status

| Requirement | Title | Status | Notes |
| --- | --- | --- | --- |
| REQ-CR-001 | Review Active Work Without External Setup | ✅ Complete | Uses Phoenix default LLM service. |
| REQ-CR-002 | Justify Large Review Spend | ✅ Complete | Empty briefs are rejected before git or LLM work. |
| REQ-CR-003 | Require Human Approval Before Review Execution | 🔄 In Progress | Tool-level gate exists; durable approval UI/state is still needed. |
| REQ-CR-004 | Infer the Review Target | 🔄 In Progress | Context-based git resolution exists; runtime should pass exact task base. |
| REQ-CR-005 | Prevent Accidental Dirty Worktree Reviews | ✅ Complete | Dirty worktree review requires explicit opt-in. |
| REQ-CR-006 | Show Dirty State in Review Scope | ✅ Complete | Dirty state and opt-in are in structured output. |
| REQ-CR-007 | Hide Review Where Phoenix Cannot Infer Scope | 🔄 In Progress | Registry boundaries exist; stale replay handling needs approval-runtime polish. |
| REQ-CR-008 | Keep Review Read-Only | ✅ Complete | Harness uses read-only git commands. |
| REQ-CR-009 | Honor Cancellation | ✅ Complete | Cancellation token is checked during collection and LLM wait. |
| REQ-CR-010 | Report Skipped Review Material | ✅ Complete | Skipped and truncated files produce warnings. |
