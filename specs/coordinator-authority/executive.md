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
| **REQ-AUTH-001:** Retain One Instance-Wide Authority Document | 🔄 In Progress | Contract authored; storage implementation pending. |
| **REQ-AUTH-002:** Read and Replace Through Current-Coordinator Tools | 🔄 In Progress | Closed local read/set shapes and source boundary specified; adapters pending. |
| **REQ-AUTH-003:** Obtain Conversational Approval Before Saving | 🔄 In Progress | Agent policy specified; prompt wiring and qualification pending. |
| **REQ-AUTH-004:** Supply Current Authority in Model Context | 🔄 In Progress | Shared live/inspection rendering contract specified; integration pending. |
| **REQ-AUTH-005:** Establish Replacement Through Normal Persistence Ownership | 🔄 In Progress | Existing persistence classification selected; implementation pending. |
| **REQ-AUTH-006:** Apply Grants Within Existing Execution Boundaries | 🔄 In Progress | Prompt and capability contract specified; qualification pending. |

**Progress:** 0 of 6 complete.

## Verification Summary

Implementation verification must cover local persistence, clearing, UTF-8 byte
limits, malformed tool arguments, ordinary/sub-agent/stale-Coordinator refusal,
commit-time source validation, fresh live/inspected context, continuation,
unchanged-context stability and visible read failures. Prompt assertions verify
instructions; they do not prove the LLM will always obtain approval or classify
every action correctly. No production Authority has been seeded or changed.

## Scope Boundaries

No approval card, proposal history, revision/CAS protocol, conversation-state
variant, background workflow, timer, remote Authority target or semantic action
permission engine is included. Durable interactions and bounded waits are
tracked independently in task 33004 and do not gate delivery of these tools.
