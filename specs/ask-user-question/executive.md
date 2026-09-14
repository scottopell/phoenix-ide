# Ask User Question Tool — Executive Summary

## Requirements Summary

AskUserQuestion pauses a parent conversation for one to four decisions. Users
select choices, include a custom answer, and qualify any answer with notes.
Responsive layouts preserve reading context and reachable actions. Native form
controls separate selection, keyboard focus, and explicit sending. Responses and
dismissals belong to a specific pending tool request; uncertainty retains the
submitted operation without silently substituting another draft.

## Technical Summary

Pending questions now carry a server-generated `request_id` independently of
provider `tool_use_id`. Web, CLI, native iOS, API admission, runtime events, and
atomic SQLite consumption use the incarnation identity. Migration 097 preserves
pending questions while assigning missing identities and preserves historical
dismissal pauses. A durable pause row holds queued steering until acceptance of
a new explicit user message; deferred objective delivery cannot release it.
The next drain delivers queued inputs FIFO with one LLM dispatch. Local SQLite
question commands use exact outcome classification and the existing fail-stop
boundary. Browser behavior follows [ADR-053](../adrs/053_question-interactions-bind-explicit-answers-to-request-identity.md);
[ADR-052](../adrs/052_question-incarnations-and-durable-dismissal-resumption.md)
refines request identity and dismissal resumption ownership. Native iOS receives
protocol adaptation, not the browser visual redesign. The legacy `design.md`
remains historical and is not the redesign authority.

## Status Summary

| Requirement | Status | Notes |
| --- | --- | --- |
| REQ-AUQ-001: Structured Question Presentation | Implemented | Unanswered initialization; explicit native selection, navigation, and send |
| REQ-AUQ-002: Rich Option Previews | Implemented | Selected-only preview, absent/Other states, rendered-line disclosure, stationary narrow choices |
| REQ-AUQ-003: Flexible Response Collection | Implemented | Retained custom drafts, explicit inclusion, universal notes, exact payload tests |
| REQ-AUQ-004: Response Delivery to Agent | Implemented | Atomic response consumption; durable dismissal pause; explicit resumption drains FIFO |
| REQ-AUQ-005: Prevent Ambiguous Question Responses | Complete | Tool question/option count and uniqueness validation exists |
| REQ-AUQ-006: Parent Conversation Availability | Complete | Tool registry excludes sub-agent invocation |
| REQ-AUQ-007: Real-Time Waiting Feedback | Implemented | Pending identity across web, CLI, iOS; status and errors reflect operation outcome |
| REQ-AUQ-008: Low-Overhead Tool Availability | Complete | Tool supports deferred discovery |
| REQ-AUQ-009: Responsive Reading and Reachable Actions | Implemented; device qualification pending | 160 Chromium/WebKit journeys; real product shell, eight viewport sizes, 2×/4× equivalent reflow; physical keyboard/manual zoom gate in task 10006 |
| REQ-AUQ-010: Accessible Native Interaction | Implemented; AT qualification pending | Native controls, concise accessible names, scoped shortcuts, modal isolation; VoiceOver/TalkBack gate in task 10006 |
| REQ-AUQ-011: Request-Bound Responses and Dismissal | Implemented | Admission/transition checks, fresh incarnation identity, forward migration, request-scoped callbacks, all-client tests |
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
