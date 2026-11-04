# Conversation Reminders — Executive Summary

## Current Reality

One-shot reminders are implemented in normalized storage through migration 151,
the host-bound native tool, ordinary input admission, HTTP management, web views
and connected-browser notifications. Ordinary conversations and the Coordinator
own reminders through stable ProductConversation identity without a WorkScope
requirement. Creating transcript, assistant-message and tool-call IDs remain
validated immutable provenance; the ProductConversation owns reminder lifetime.

The tool supports strict create/list/reschedule/cancel requests, saved IANA zones,
local-time/DST resolution and committed replay-safe receipts. Top-level Direct,
Explore, Work and Coordinator runtimes receive it; subordinate registries do not.
Explore-to-Work upgrades retain the bound capability.

The existing Direct Turn worker freezes due facts and submits saved text through
ordinary Send Chat. Busy inputs use steering; awaiting-user and task-approval
refusals retain the due occurrence until ordinary admission accepts it. Accepted
turn Stop and removed steering entries do not cause replacement input. There is
no reminder execution owner, acceptance flag, second queue or Close participant.

Discovery requires matching scheduled provenance in the existing direct-turn or
steering acceptance authority. Steering receipts retain their reminder ID and
original exact request fingerprint through queue copying, continuation, removal
and reopen. Keyset discovery advances past refused recipients without an accepted
batch hiding later candidates.

Shared fresh admission reserves raw reminder keys and their canonical aliases,
including aliases submitted from another conversation. Fresh manual continuation
rejects those keys before successor or opening-intent creation. Exact accepted
replay retains its contract. An existing validated continuation intent retains
its winning identity and can finish its owned opening; a losing retry cannot
substitute another key.

Cancellation and rescheduling consume existing ProductConversation lifecycle
admission inside their mutating transactions. Lists remain read-only, and an
already-due occurrence reports conflict without claiming retraction. Existing
aggregate deletion removes solely owned reminders after messages, turns and
receipts, including delivery across a continuation.

`ReminderWritePass` consumes typed command classification and closes local
authority if create, reschedule, cancellation or clock promotion loses its
result. Creation retains its exact invocation/fingerprint replay query; management
does not infer command ownership from a matching snapshot. Normal typed write
results and known validation/lifecycle refusals remain nonfatal. Management
infrastructure errors, including SQLite or serialization failures, remain
unclassified and use existing fail-stop authority. Create preflight validation/read
failures remain nonfatal. Promotion requires admission and uses the same fail-stop
contract for an unclassified result.

## Coverage

| Requirements | Status | Evidence |
|---|---|---|
| REQ-REM-001–002 ownership, durable create and replay | Implemented | Trusted host binding, full invocation identity and unresolved-request fingerprint |
| REQ-REM-003 time resolution | Implemented | Shared `ReminderWhen::resolve`, DST gap/fold/offset and saved-zone tests |
| REQ-REM-004, REQ-REM-006 due commitment and management | Implemented | Atomic arbitration, immutable terminal rows, revision checks and lifecycle refusal |
| REQ-REM-005 ordinary input admission | Implemented | Real Send Chat direct/steering, readiness, Stop, receipt, namespace and continuation regressions |
| REQ-REM-007 restart recovery | Implemented | File-backed late discovery and acceptance-receipt reopen tests; populated forward migration |
| REQ-REM-008 visible reminder and provenance | Implemented | Normalized origin, model rendering, wire/native decoding, API/cards and notification policy |
| REQ-REM-009 lifecycle boundary | Implemented | Existing admission, stable owner FK and aggregate-delete regressions; no Close participant |

## HTTP, Web and Native Surfaces

HTTP provides owner reminder lists, revision-checked cancel/reschedule and a
read-only global due feed. Owner lists bound each state independently at 50
records; the native tool uses 10 per state and reports that limit. The global feed
uses 100-row due-time/ID pages within the last day of actual observation, with a
ten-page refresh cap, current continuation/Coordinator targets and History
exclusion.

The web panel displays saved-zone times, immutable due cards and scheduled-only
management. Editing retains the opening revision; conflicts reload saved truth.
Abortable, non-overlapping polling runs at five seconds and on visibility, focus
and network return. Browser alerts use existing permission/settings/focus policy,
persisted `notify_reminder` defaulting enabled, a quiet initial baseline and
independent deduplication. Closed-app phone notifications are not provided.

The iOS transcript retains reminder ID, label and nonhuman provenance without a
cross-source link. Snapshot version 3 reads version-1/version-2 envelopes and
fences older writers.

## Qualification and Limits

[PR #1025](https://github.com/scottopell/phoenix-ide/pull/1025) is the authority for
review dispositions and CI gates at each exact PR head. Local and hosted results
below are scoped evidence, not a claim that every later head inherits a complete
qualification.

Local qualification includes storage, API, tool, input-origin, Send Chat,
continuation, migration, UI and notification-policy regressions; populated 151
preserves prior columns, ordinary receipts, Federation data and inert wake
history, and checks foreign keys and idempotent reentry. Immutable migration 150
is unchanged. Deterministic SQLite COMMIT gates cover abandoned ownership for
all four reminder writes without requiring committed intent to roll back.

The isolated real-binary reminder E2E journey uses a mock provider with production
invocation identity. It covers registration, committed receipt, restart before
due, HTTP rescheduling, stale/wrong-owner management, reserved raw/canonical
client IDs, due visibility, ordinary model follow-up and restart after acceptance
without another input. The harness isolates credentials and external services.

Before the review repairs, local checks passed codegen/staleness, full Rust test
compilation, frontend/type/lint suites, specification/static checks, development
harness tests, lockfile checks and isolated E2E scenarios. Broad local Rust
execution was deliberately interrupted after 598 passing tests because the host
could not allocate a pseudo-terminal; this was not a full Rust execution pass.
Local iOS app/unit-test sources compiled, with simulator execution blocked by the
same host limitation.

At [7f219472](https://github.com/scottopell/phoenix-ide/commit/7f21947249ca48dc93909a425f6b07f23f9c43ae),
hosted iOS, macOS controller, build, codegen, Clippy, UI and E2E checks passed.
Two Rust shards passed; two exposed historical fixture issues corrected in this
branch. Current-head qualification and subsequent review repairs are tracked in
the PR.

Desktop and 393×852 browser QA verified real saved-zone rescheduling,
cancellation, immutable due cards, scheduled input labels, ordinary follow-up
and a panel within the viewport. Browser/OS notification permission delivery and
physical devices were not exercised. No production deployment is claimed.

## Authorities

- `requirements.md`: product contract.
- `reminders.allium`: time-eligibility transitions.
- ADR-217: standard input admission architecture.
- ADR-218: reserved occurrence identity and existing receipt provenance.
- ADR-219: canonical aliases, fresh opening validation and existing opening authority.
- ADR-220: management infrastructure errors use existing fail-stop authority.
