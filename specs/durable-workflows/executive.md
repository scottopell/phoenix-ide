# Durable Workflows — Executive Summary

## Purpose

Durable Workflows is the shared execution protocol for accepted asynchronous
Phoenix obligations. The product reducer remains the semantic authority; the
engine supplies normalized execution truth, atomic plans, leased effects,
reconciliation, durable scheduling, cancellation compensation, and runtime
acceptance bookkeeping where a profile requires it.

Wake and conversation creation are the two normative profiles. Wake is the first
production adoption target; creation follows through non-authoritative shadow
comparison and a versioned acceptance cutover.

## Current Reality

The normative requirements, architectural decisions, and Allium package are
specified. The package comprises the profile-neutral
`durable-workflows.allium`, `wake-profile.allium`, and `creation-profile.allium`.
Existing wake and creation implementations remain their respective execution
authorities until the evidence-based gate authorizes an engine-backed selector for
new acceptances.

## Delivery Sequence

1. Implement the pure engine and deterministic simulator for both profiles.
2. Persist atomic normalized transitions, effects, claims, evidence, receipts,
   barriers, deadlines, and optional owed-runtime-acceptance records.
3. Run wake in non-authoritative shadow mode, then select engine authority for new
   wake registrations while legacy registrations drain.
4. Run creation in non-authoritative shadow mode, then select engine authority for
   new creation acceptances while legacy jobs drain.
5. Retire each legacy scheduler only after durable zero-authority proof.

## Workflow Migration Register

Every temporal subsystem is tracked through one migration register. Each entry
records the subsystem, current semantic authority, target workflow profile,
shadow-parity evidence, cutover gate, rollback selector, complete drain-proof
identity, and remaining authority-retirement debt. Creation and wake are active
entries; sub-agents, tool execution, and later temporal subsystems remain visible
until their direct orchestration paths are retired.

| Subsystem | Current authority | Target profile | Migration state | Retirement debt |
| --- | --- | --- | --- | --- |
| Wake | Mixed legacy and engine paths | Wake workflow | Engine integration plus user-facing delivery adoption | Legacy registration/scheduling; automatic resume; REST/SSE status; web, iOS, and CLI presentation; cancellation UI |
| Creation | Legacy creation worker | Creation workflow | Non-authoritative shadow comparison | Legacy worker and cleanup orchestration |
| Sub-agents | Conversation runtime/state machine | Sub-agent workflow | Tracked for profile specification and shadow adoption | Direct spawn, timeout, cancellation, continuation, and terminal delivery |
| Tool execution | Conversation runtime | Typed effect execution | Profile inventory required | Ad hoc retry and recovery paths |

### Wake user-value completion gate

Wake does not complete at durable reducer-inbox delivery. Its cutover evidence must
show a real conversation parking on a long-running Bash or Tmux operation without
burning a turn, exposing the pending wait and cancellation from every supported
client, and resuming exactly once after terminal evidence. The shipped surface
must include REST/SSE status, web and iOS capability/presentation parity,
`phoenix-client.py wake-status`, restart-safe automatic resume, and pending/history
visibility. This delivery slice is tracked by task 25002 and is part of the Wake
migration's definition of done, not an optional adoption candidate.

### Adoption candidates

These flows are not equal to the load-bearing authority migrations above. They are
tracked because they cross restart boundaries, coordinate multiple durable or
external effects, or otherwise accumulate bespoke retry and completion logic.
Priority reflects migration value after the core engine profiles are operational;
it does not supersede creation, wake, or sub-agent cutover work.

| Candidate | Priority | Current choreography | Durable boundary to investigate | Tracking |
| --- | --- | --- | --- | --- |
| Address Feedback | High | Fresh GitHub capture through `pr-auto-fix-context`, feedback-baseline commit, then idle dispatch or queued `UserMessage` | Exact-PR-head actionable snapshot, idempotent handoff, busy/idle dispatch, supersession, and completion verification | Task 47009 |
| LLM turn dispatch | High | Runtime request, provider stream, persistence, retry, cancellation, and usage accounting | Durable request identity, ambiguous dispatch reconciliation, streamed-result receipt, and duplicate-result fencing | Task 40011 |
| Work lifecycle cleanup | High | Abandon, mark-merged, continuation, worktree removal, branch cleanup, and artifact preservation | Typed cleanup/compensation DAG with owned-resource evidence and terminal barrier | Task 40011 |
| Tool and process execution | High | Runtime-owned tool calls plus Bash/Tmux process and output ownership | Per-tool effect profiles, observable execution, cancellation, durable receipts, and orphan reconciliation | Task 40011 |
| Runtime start, stop, eviction, and reconstruction | Medium | Manager-owned in-process lifecycle and publication | Reducer-derived capability, durable lifecycle intent, reconstruction receipt, and SSE as projection | Task 40011 |
| Notifications, analytics, and optional projections | Low | Best-effort side effects coupled to product transitions | Optional effects with explicit retry/duplication policy that never block completion | Task 40011 |

A profile may accept engine-authoritative work only after:

1. deterministic and representative production schedule classes pass;
2. blocking divergence count is zero;
3. mixed-authority user semantics match;
4. required executors and codecs are available;
5. rollback selection is verified; and
6. operator authorization is recorded.

A protocol may retire only after its selector is closed and a category-complete
proof reports no nonterminal workflows, executable effects, live claims, pending
reducer inboxes, owed runtime acceptances, unresolved manual resolutions, or
blocking divergences.

New temporal features are reviewed against the same checklist: semantic
authority, restart boundary, idempotency key, claim and attempt fence, ambiguous
external-work reconciliation, lifecycle and drain behavior, and explicit
retirement debt for any path that does not extend the target workflow profile.

## Requirement Coverage

| Requirement group | Status | Intended verification / code surface |
| --- | --- | --- |
| REQ-DWF-001–005 authority, ownership, atomic plans, DAGs, barriers | Specified | Pure reducer/engine model; transactional persistence |
| REQ-DWF-006–009 leased execution and ambiguity | Specified | Claim/renew/takeover simulation; typed profile adapters |
| REQ-DWF-010–012 deadlines, cancellation, manual resolution | Specified | Virtual-time schedules; cancellation transaction; operator flow |
| REQ-DWF-013–017 capabilities, versions, migration, acceptance | Specified | API/runtime projection tests; shadow/cutover/drain tests |
| REQ-DWF-018 deterministic verification | Specified | Property schedules with checked-in minimized regressions |
| REQ-DWF-019 protocol admission and exact drain proof | Specified | Versioned authoritative query; category counts and identities; zero-proof retirement gate |
| REQ-DWF-020 divergence severity and operator action | Specified | Canonical eight-kind vocabulary; profile mappings; operator inventory and halt tests |
| REQ-DWF-021 evidence-based wake/creation cutover | Specified | Required deterministic and production schedule classes; authorization audit |
| REQ-DWF-022 mixed-authority semantic parity | Specified | Cross-protocol product projection and capability comparisons |
| REQ-DWF-029–032 client acceptance, projection parity, independent consumers, adoption boundary | Specified | Idempotent acceptance campaigns; cross-client projection tests; consumer isolation; profile-admission review |
| REQ-DWF-WAKE-001–005 wake profile | Specified | Bash/tmux registration-to-resume end-to-end campaigns |
| REQ-DWF-CREATE-001–005 creation profile | Specified | Shell-first creation, Git/resource, cancel/delete campaigns |

## Related Decisions

- ADR-013 owns the normalized-core/profile boundary and wake-first adoption.
- ADR-014 separates workflow-version serialization from leased effect authority.
- ADR-015 separates observation, receipt, and runtime acceptance and permits the
  normalized owed-acceptance capability only for profiles that need it.
- ADR-016 makes externally retryable acceptance a typed profile capability,
  extends semantic projection parity across supported clients, isolates additional
  inbox consumers, and defines the engine adoption perimeter.
- ADR-007 remains historical authority for creation's fenced reconciliation.
- ADR-011 and ADR-012 remain historical authority for wake registration,
  observations, and durable resume acceptance.

## Implementation Gate

Engine-backed authority is not selected for new wake or creation work until every
required deterministic and representative production schedule class has zero
unresolved blocking divergence, codec and reversible-selector checks pass,
mixed-authority user semantics match, and an operator explicitly authorizes the
cutover. Elapsed soak duration is not evidence. Accepted legacy and engine versions
drain under retained executors/codecs; retirement requires the category-complete
complete zero-count drain proof for every authoritative category, and no in-flight translation is permitted.
