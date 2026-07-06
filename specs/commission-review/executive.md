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
| REQ-CR-004 | Infer the Review Target | ✅ Complete | Context-based git resolution reviews committed HEAD against a fetched origin base ref. |
| REQ-CR-005 | Refuse Dirty Working Trees | ✅ Complete | Dirty working trees are refused; there is no dirty-review opt-in. |
| REQ-CR-006 | Compare Against the Approved Origin Base | ✅ Complete | Comparator uses `refs/remotes/origin/<approved-base>` when approved, otherwise `refs/remotes/origin/HEAD`, and fails before LLM review when unavailable. |
| REQ-CR-007 | Hide Review Where Phoenix Cannot Infer Scope | 🔄 In Progress | Registry boundaries exist; stale replay handling needs approval-runtime polish. |
| REQ-CR-008 | Keep Review Read-Only | ✅ Complete | Harness uses read-only git commands. |
| REQ-CR-009 | Honor Cancellation | ✅ Complete | Cancellation token is checked during collection and LLM wait. |
| REQ-CR-010 | Report Skipped Review Material | ✅ Complete | Size-cap exclusions surfaced in a typed `unreviewed` result (forces partial top-level status); binary/unsupported produce warnings. |
| REQ-CR-011 | Distinguish Partial Review Output From Failure | ✅ Complete | Interrupted reviews preserve parsed findings/summaries as partial; no parsed output reports failed/unavailable. |
| REQ-CR-012 | Report Review Stage Status | ✅ Complete | Result includes typed stage status for target, diff, LLM, JSON parse, and finding extraction. |
| REQ-CR-013 | Summarize Findings and Important Warnings | ✅ Complete | Severity counts and deterministic warning summaries are included near status. |
| REQ-CR-014 | Keep User-Facing Results Free Of Token And Cost Metadata | ✅ Complete | LLM usage remains internal and skipped from serialized review/display payloads. |
| REQ-CR-015 | Include Stable Finding Navigation Hints | ✅ Complete | Findings preserve optional `symbol` anchors without making them required. |
