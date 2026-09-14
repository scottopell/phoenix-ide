# Ask User Question Tool — Executive Summary

## Requirements Summary

AskUserQuestion pauses a parent conversation for one to four decisions. Users
select choices, include a custom answer, and qualify any answer with notes.
Responsive layouts preserve reading context and reachable actions. Native form
controls separate selection, keyboard focus, and explicit sending. Responses and
dismissals belong to a specific pending tool request; uncertainty retains the
submitted operation without silently substituting another draft.

## Technical Summary

The approved redesign reuses the persisted `tool_use_id` in
`AwaitingUserResponse`, threads it through web, CLI, native iOS, API payloads,
and consuming runtime events, and scopes asynchronous completion to that
identity. Browser layout and answer semantics are implemented against
[ADR-053](../adrs/053_question-interactions-bind-explicit-answers-to-request-identity.md).
Native iOS receives protocol adaptation, not the browser visual redesign.
The legacy `design.md` remains historical and is not the redesign authority.

## Status Summary

| Requirement | Status | Notes |
| --- | --- | --- |
| REQ-AUQ-001: Structured Question Presentation | Implemented | Unanswered initialization; explicit native selection, navigation, and send |
| REQ-AUQ-002: Rich Option Previews | Implemented | Selected-only preview, absent/Other states, rendered-line disclosure, stationary narrow choices |
| REQ-AUQ-003: Flexible Response Collection | Implemented | Retained custom drafts, explicit inclusion, universal notes, exact payload tests |
| REQ-AUQ-004: Response Delivery to Agent | Implemented | Atomic response consumption; dismissal settles without queued resume |
| REQ-AUQ-005: Prevent Ambiguous Question Responses | Complete | Tool question/option count and uniqueness validation exists |
| REQ-AUQ-006: Parent Conversation Availability | Complete | Tool registry excludes sub-agent invocation |
| REQ-AUQ-007: Real-Time Waiting Feedback | Implemented | Pending identity across web, CLI, iOS; status and errors reflect operation outcome |
| REQ-AUQ-008: Low-Overhead Tool Availability | Complete | Tool supports deferred discovery |
| REQ-AUQ-009: Responsive Reading and Reachable Actions | Implemented; device qualification pending | 144 Chromium/WebKit journeys; real product shell, eight viewport sizes, 2×/4× equivalent reflow; physical keyboard/manual zoom gate in task 10006 |
| REQ-AUQ-010: Accessible Native Interaction | Implemented; AT qualification pending | Native controls, concise accessible names, scoped shortcuts, modal isolation; VoiceOver/TalkBack gate in task 10006 |
| REQ-AUQ-011: Request-Bound Responses and Dismissal | Implemented | Admission/transition checks, atomic persisted identity, request-scoped callbacks, all-client tests |
| REQ-AUQ-012: Truthful Sending and Uncertain Outcomes | Implemented | Frozen retries, malformed-status protection, duplicate/partial-persistence regressions |
| REQ-AUQ-013: Draft Lifetime and Request Isolation | Implemented | Mounted same-request draft retention; different identity resets, including identical text |

**Progress:** All 13 requirements implemented. Task 10005 owns implementation;
task 10006 owns remaining physical-device/assistive-technology release qualification.

## Validation gate

The approved proposal's viewport, browser, keyboard, screen-reader, and runtime
matrix is the release gate. Fixture checks alone do not establish integration or
real-device behavior. Mark untested required platforms as qualification gaps.
The design and implementation audits are complete. Automated evidence and the
remaining manual qualification limits are recorded in
[implementation validation](../../docs/proposals/ask-user-question-validation.md).
