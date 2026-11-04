# Coordinator Authority — Executive Summary

## Requirements Summary

Coordinator Authority provides one local standing-instructions document that
Ember receives independently of transcript history and compaction. The user
describes intent conversationally; Ember clarifies material ambiguity, presents
the complete proposed wording, obtains conversational approval and saves it with
`set_authority`. `read_authority` exposes current instructions. Empty text clears
the document, and the maximum accepted size is 16,384 UTF-8 bytes.

Approval is an instruction to the agent, not a server-verified human action.
Ordinary conversations and sub-agents do not receive the tools or the document.
Standing grants support confident in-scope work while existing structural
execution restrictions and later user instructions remain binding.

## Technical Summary

The design uses one indivisible text value in the existing scalar application
settings store, with no schema migration, revision history or proposal state.
Tools bind the source from trusted execution context and check current
Coordinator identity at the read boundary and inside the setter transaction.
Writes use the existing local persistence ownership and outcome classification.

Live model requests and the system-prompt inspector share a renderer that reads
current instructions without clipping. Missing saved text means no additional
grant; a failed read must remain an error. Constant text renders identically
across calls and is loaded afresh after continuation. ADR-221 records the choice
to keep human approval conversational rather than add an interaction workflow.

## Status Summary

| Requirement | Status | Notes |
| --- | --- | --- |
| **REQ-AUTH-001:** Retain One Instance-Wide Authority Document | ✅ Complete | Validated scalar setting; exact replacement, clearing and file-backed reopen tests pass. |
| **REQ-AUTH-002:** Read and Replace Through Current-Coordinator Tools | ✅ Complete | Closed read/set schemas, trusted source binding, registry isolation and stale-source rejection tested. |
| **REQ-AUTH-003:** Obtain Conversational Approval Before Saving | ✅ Complete | Both prompt variants and setter guidance require interpreted wording, full replacement review and conversational approval. Model compliance is not established by prompt assertions. |
| **REQ-AUTH-004:** Supply Current Authority in Model Context | ✅ Complete | Shared live/inspector renderer, per-request refresh, successor context, stable text and visible read-failure tests pass. |
| **REQ-AUTH-005:** Establish Replacement Through Normal Persistence Ownership | ✅ Complete | Owned write follows existing local outcome classification; rollback, simulated lost acknowledgement and cancellation boundary tests pass. |
| **REQ-AUTH-006:** Apply Grants Within Existing Execution Boundaries | ✅ Complete | Prompt applies saved grants subject to later user constraints and structural restrictions; ordinary/sub-agent capability isolation tested. |

**Progress:** 6 of 6 implemented with bounded local qualification; not deployed.

## Verification Summary

Focused tests cover local persistence, clearing, UTF-8 byte limits, malformed
arguments, ordinary/sub-agent/stale-Coordinator refusal, fresh live/inspected
context, continuation, unchanged-context stability and visible read failures.
Current-source validation occurs inside the committing transaction; source
rotation tests use an already-rotated source rather than an injected transaction
race. Cancellation coverage drops the requester before write admission, and lost
commit acknowledgement is simulated rather than a SQLite fault injection.

All 31 system-prompt tests, application test-target compilation, formatting,
Allium, requirement anchors and task validation passed before integration with
main's durable reminders. Combined-source test-target compilation, 22 focused
tests and a bounded review of the merge seams passed at `366fa38a0`. Final
Clippy and formatting passed after equivalent explicit-default test annotations
in `4d5b4a3ad`; details are recorded in task 33003 and PR #1026. The broader local
spec-shape
structural scan passed, but its Python unit suite unexpectedly started a real
backend build and was deliberately stopped; task 33005 tracks that separate
validation hazard. A full repository check is not claimed.

Prompt assertions verify instructions; they do not prove the LLM will always
obtain approval or classify every action correctly. No live provider behavior
has been qualified and no production Authority has been seeded or changed.

## Scope Boundaries

No approval card, proposal history, revision/CAS protocol, conversation-state
variant, background workflow, timer, remote Authority target or semantic action
permission engine is included. Durable interactions and bounded waits are
tracked independently in task 33004 and do not gate delivery of these tools.
