# Durable conversation goals M1 — storage, attribution, and immutable execution fencing

## Implementation handoff — supersedes preparation state

- Normal approval receipt: `44c2f6c0-28ad-4dc1-9825-ab01e6a0f8c6` at 2026-10-19 15:46:52 UTC, independently verified by the parent. Read-only app projection confirmed active WorkScope `cf401196-6988-470d-b822-6f55bcf1f16e`, `authority_kind=work`, `environment_kind=allocated_worktree`, at this exact allocated cwd. Parent also verified persisted approved-task authority/objective records. No manual approval/API bypass was used.
- Task approval created local commit `cedb8e938b29e9d0d22088c59e791a954618e0ba` atop source base `69b5d1b7018c62fb8c20fc9080e75b77131bc12b`; branch remains supported detached HEAD.
- Authored: focused core domain + DB service, staged goal-only SQL, storage requirements/executive, 8 pure domain and 16 DB tests. No runtime/HTTP/tool/UI/watch activation or safety-budget/release/resume policy was added.
- Two independent read-only source reviewers completed; no implementation subagent was used. Addressed pure inactive-report forgery, SQL report owner/fence correspondence, assistant ToolUse locator consistency, late-stage rollback coverage, and parent review's supported-hard-delete regression. Full-migration user-goal/clear/delete and NoGoal-only-binding/delete tests are authored but not run.
- Normal parent messages delivered/queued milestone receipts, exact scope/pins and reviewed fixes. The preparation note about messaging unavailability was an initial tool-inventory mistake; normal `send_conversation_message` was subsequently used, with no indirect relay or API bypass.
- Source-only validation: existing standalone rustfmt 1.95 (not Cargo/compiler) and Git whitespace checks. No local compiler/Cargo/test/SQLite/migration/browser/VM/install/server execution and no hosted CI qualification. Authored tests are **unqualified**.
- Parent explicitly retained actual atomic turn-admission integration for M3. The capture primitive is crate-private/dormant and cannot prove the caller co-created the turn; it must not be used later to retrofit a missing observation. Exact message/tool-to-execution assignment remains trusted runtime context; typed ToolUse validation is consistency, not authentication. Immutable evidence locators can outlive deleted source rows.
- Integration blockers: parent-coordinated numeric migration/ADR reservation and resulting registry integration; exact-commit hosted compile/test/check qualification. The schema is deliberately unregistered. Parent has requested no additional source-review agents.
- Product gates remain open. Parent relayed withdrawal of a restrictive conversation-wide lifetime/shared-budget recommendation; M1 has no such schema or arbitrary renewal/release defaults. Deliberate authorized Global steering is not itself a failure. No replacement policy was chosen.
- Handoff task state: **blocked** pending coordinated registration/ADR and hosted qualification. Source is authored, not shipped/qualified; do not close on authored test counts. Parent retains integration; no shared-main edits, merge, push, deploy, release or production migration occurred.

## Commission and boundary

Implement only the dormant domain/persistence foundation for goals on stable ordinary ProductConversations. Parent integrator: `@conv:b477b7e7-0e64-4c4f-bba1-d0b101df98e9`; design conversation: `f9c038d2-9112-47f6-b08a-4e533ea4c77d`.

The current user commissions bounded M1 and requires normal task approval before implementation. Referenced primary commission: `@transcript:8521dc04-bdf5-49c5-a61e-526fdc9431db#message-8521dc04-bdf5-49c5-a61e-526fdc9431db:f18ff8eb-e5e7-4601-b378-871e34a750d8`, quoted by the user: “Do this, and pass through adversarial reviewers with goal of finding pit of success failures or design ambiguities. Surface the most interesting 4 open findings to me for my input.” The transcript reader did not resolve this historical reference during preparation; do not claim independent verification. The present user instruction supplies the operative M1 scope.

Historical design evidence: `/Users/scottopell/.phoenix-ide/coordinator/goal-design/durable-goals/design.md`. Its latest prefix supersedes older proposals; the present user instruction supersedes conflicting historical details. Historical notes are not normative source specifications.

Parent follow-up confirms provisioning published `ae2389e0-6819-406c-b67f-9a88c54406f6` and source pin unchanged. Read parent packet `/Users/scottopell/.phoenix-ide/coordinator/goal-design/durable-goals/implementation-review.md`: it attributes direct recovery of the original commission to the parent, records three independent read-only reviews, and confirms M1 only. Do not treat its recommendations for the four open questions as accepted policy. Parent owns migration/ADR reservation coordination: report intended goal/state/history/binding schema and request the coordinated reservation; do not allocate shared migration/ADR numbers independently. If registration requires a reservation, wait for the parent before that step.

## Exact preparation state and environment

- Task state: **ready; awaiting approval**. No source implementation, test execution, compiler run, or migration application has occurred.
- Supported allocated checkout/cwd: `/Users/scottopell/dev/phoenix-ide/.phoenix/worktrees/a52c58d8-3dcb-42c5-82fe-585e6126dcb0`.
- Source pin: `69b5d1b7018c62fb8c20fc9080e75b77131bc12b`; clean detached HEAD before this proposal draft. This is not shared main. A persisted WorkScope ID was not exposed by the available tools; do not invent one.
- On approval, verify the actual supported WorkScope, cwd, writable branch and source pin again. Work only there; stop and report if approval routes to an unsupported/shared scope. Do not reset or move another owner's branch.
- Read current checkout AGENTS guidance, VISION, compatibility requirements, stable ProductConversation Allium/domain boundaries, and trusted-input/watch requirements and ADRs during preparation.
- Roadmap Issue #806 could not be verified: `gh issue view` was blocked by sandbox network policy. Continue from domain authorities and report this evidence gap; do not claim roadmap coordination.
- Initial preparation did not deliver a separate parent message. This was corrected after approval using normal messaging; see the implementation handoff above for the authoritative reporting state.

## Observed journey and required truth

A user, owning agent, or commissioned Global Coordinator can eventually manage an ordinary conversation's durable goal. A transcript rollover or shared WorkScope must not transfer goal ownership. Goal edits and reports can race: an older execution reporting completion must not overwrite a newer objective, Stop, clear, or lifecycle state. History must preserve who actually requested/applied a change, without treating attribution as authorization or generated host text as user input.

M1 establishes that durable truth only. It does not make a goal run, admit another turn, enable a tool/API, steer a busy owner, or change watches.

## Verified starting seams

- `crates/phoenix-core/src/domain/product_conversation.rs`: `ProductConversation` has structurally distinct ordinary and Coordinator variants; goal ownership must be ordinary-only.
- `specs/bedrock/bedrock.allium`: ProductConversation owns stable aggregate identity; transcripts/execution participants are members, not the durable goal owner.
- `crates/phoenix-core/src/domain/sm_event.rs`: direct-turn identities and attempt authority already distinguish workflow, turn, attempt, generation and process incarnation. Reuse the actual durable execution identity rather than a mutable current/latest transcript lookup.
- `crates/phoenix-core/src/domain/db_schema.rs`: `InputOrigin` and direct-turn payloads preserve trusted provenance. Channel provenance alone does not prove a biological human; do not invent a principal absent from the trusted boundary.
- `crates/phoenix-db/src/workflow/direct_turn.rs`, `product_conversation_read.rs`, `ddl.rs`, `migrations.rs`: persistence and execution-binding integration seams to inspect precisely before editing.
- `specs/global-recall/requirements.md` REQ-GR-014/015 and ADR-070: trusted actor provenance and existing stable-conversation watches have separate authority. M1 must not alter their behavior.
- `specs/compatibility/requirements.md`: forward migrations only under existing policy; new internal timestamps use checked integer Unix microseconds. No downgrade/rollback/live-database-replacement promise is added.
- No existing goal domain/storage implementation was found in the inspected core domain and DB source.

## Interaction map

Future trusted user/owning-agent/Coordinator boundary → internal goal command with concrete attribution → ordinary ProductConversation-owned transactional state/history → immutable execution-bound snapshot → report fence → applied state change or explicit stale rejection/evidence outcome.

M1 implements the middle domain/DB contract and authors its tests. The future callers, automatic admission, prompt/tool surfaces, UI/SSE, cancellation integration, continuation worker behavior and goal-aware watch policy remain disconnected.

## M1 deliverables

### 1. Bounded domain contract

- Add a focused goal domain module using existing ID conventions: goal identity, ordinary stable owner, objective, explicit stored state/reason, objective revision and lifecycle/concurrency version.
- Support one current goal per ordinary owner with retained completed/cleared history; no mandatory WorkScope or coding artifact schema. Define clearly which field is authoritative and which values are immutable historical snapshots; avoid two writable current-goal authorities.
- Require observed-version compare-and-set for ALL actor writes, including future Global and UI writes, as well as create/edit/state change/clear and report application. No last-writer-wins or privileged bypass of fencing/history. A stale Active edit cannot resurrect a completed goal; fresh activation is distinct from an ordinary edit, with its unresolved release policy deferred.
- Objective edits invalidate older report authority; lifecycle changes invalidate older authority even when objective text is unchanged. Clear/recreate cannot revive a former goal's report authority. Exact retry identity must not duplicate accepted history or overwrite a different request.
- Keep status vocabulary separate from automatic transition/permission policy. Do not encode a complete resume/release/ceiling state machine in M1.

### 2. Concrete actor attribution, distinct from authority

- Use typed actor variants for user-origin action, owning-agent action, Coordinator action, and generated host activity. Persist the concrete identity available at the trusted boundary, causative input/action, and exact conversation/transcript/turn/tool-call locator where applicable.
- Preserve requested-by versus applied-by when different. Generated host activity stays generated even when caused by a user or Coordinator; no model-provided actor label can establish authenticated identity.
- Owning-agent attribution must match the ordinary owner; Coordinator attribution must identify the actual Coordinator and exact originating execution/action, not just an enum string.
- Reuse existing trusted identity/provenance where sufficient; explicitly represent what the existing boundary can prove. Do not fabricate a human identity or accept actor identity from future untrusted tool arguments.
- Attribution is audit data, not a deployment/merge/resource grant. M1 exposes no public authorization route and makes no decision about unresolved non-user release authority.

### 3. Normalized, atomic DB storage/history

- Add a focused DB module and forward schema migration/registration through existing conventions, with ordinary-only ownership enforced by the persisted shape and database constraints, not a caller comment alone.
- Normalize goal state, mutation history, and execution bindings into appropriate relational rows/columns. Collections are child rows, not JSON arrays. Use explicit ordering/identities and foreign keys/checks/uniqueness for owner membership, versions and history consistency.
- Store previous/new values or an equally lossless immutable mutation representation, exact actor/cause, and integer `_us` timestamps. Current state plus history updates commit atomically; failed/conflicting writes leave neither partial state nor a misleading accepted-history record.
- Concurrent edits using the same expected version yield one success and an explicit conflict for the other; creation/replacement/clear have equivalent fences. Bounded reads return current state and ordered history without reconstructing ownership from mutable transcript selection.
- Preserve existing conversation/message identities and no-goal behavior. Author migration tests on disposable fixtures only; never open or migrate a production database.

### 4. Immutable report execution binding

- Define and persist the immutable tuple that a report is about: ordinary stable owner, goal ID, objective revision, lifecycle/concurrency version, and actual admitted execution/turn identity (plus concrete report source identity as needed).
- Capture that tuple once at work/turn admission, never at late report/tool execution; no implementation may look up “the current goal” when a report arrives and silently attach a newer revision/version. M1 authors dormant admission-binding operations and tests without wiring live callers or automatic execution.
- DB/domain contracts reject cross-owner, unrelated-execution, stale-goal/revision/version and mutable-rebinding attempts. A retry of the identical binding is idempotent; a conflicting binding for that execution is not silently replaced. A late report may remain factual evidence but cannot change current lifecycle state.
- Do not infer success from Idle, final prose, a WorkScope attachment, a command dispatch, or an unrelated check. Completion is an auditable report under the captured authority, not a generic mandatory proof form.
- If existing durable execution identity cannot express the binding safely without broad runtime work, report that concrete boundary to the parent; do not widen M1 unilaterally.

### 5. Tests authored, source review and handoff

Author pure domain and disposable SQLite tests covering:

1. Ordinary-only ownership, one current goal, completed/cleared history and no WorkScope dependency.
2. Objective/lifecycle monotonic fences, same-version competing writes, create/clear/recreate ABA prevention, atomic rollback and exact retry behavior.
3. User, owning-agent, Coordinator and generated-host actor distinctions; concrete locators and requester/applier preservation; cross-owner/spoofed attribution rejection at the defined internal boundary.
4. Binding retained across transcript/current-owner pointer changes; immutable execution binding; cross-owner, unrelated execution, stale revision/version and conflicting retry rejection; stale evidence cannot apply a lifecycle mutation.
5. Migration constraints, integer timestamp shape, foreign keys and lossless history round trips; existing no-goal fixtures unaffected.

These are **authored tests, not qualified passing tests** until a named hosted CI run executes them at an exact commit. Do not run local Cargo, compiler, browser, VM, install, servers, migrations or other heavy work. Source-only/light checks and hosted CI are permitted after approval within commission. Preserve exact commands/CI URLs/commit pins and distinguish not-run, failed, and passed evidence.

Pass the exact diff to independent read-only adversarial reviewers when available; no additional implementation writer/subagent is authorized. Challenge pit-of-success failures: bypassable CAS/history, mutable report rebinding, forged/ambiguous actor identity, and ordinary/Coordinator ownership leakage. Surface substantive ambiguities to the parent rather than guessing. The parent owns overall review/integration and the requested four user findings; do not invent four new questions merely to fill a quota.

## Source milestones and parent reporting

- **S0 — approval/scope**: report task state, actual WorkScope/cwd/branch/pin and roadmap availability before source work.
- **S1 — domain contract**: report types, invariants and any blocking identity/policy seam.
- **S2 — dormant persistence**: report intended schema/history/CAS and immutable execution binding at exact source pin; obtain migration/ADR reservations from the parent before assigning shared numeric identities.
- **S3 — authored coverage/review**: report authored test names, adversarial findings, source checks and hosted CI if used; label unexecuted tests unqualified.
- **S4 — bounded handoff**: report changed surfaces, commit pins, remaining blockers and explicit confirmation that no runtime/public activation, merge, deploy, release or production migration occurred. Parent retains integration.

## Locked wider-design constraints to preserve, not activate

Ordinary owns the goal; Global can directly manage within commission; runtime admits local continuation; busy edits persist/steer without a competing turn; user Stop is protected; no boilerplate proof forms; reuse existing goal-aware watches; at most one terminal-only grace; no wake framework, external waits or profiles788.

M1 must leave these future features possible without wiring them up or pretending they are delivered. In particular, stale-report fencing must be capable of preserving a later Stop, but M1 does not change Stop/cancellation behavior.

## Four open product decisions — remain open and unchosen

1. Collective safety-ceiling scope and renewal.
2. Non-user release/resume authority.
3. Ambiguity of natural-language resume.
4. Healthy slow-wait behavior.

Do not pick defaults, schemas requiring a particular policy, automatic replenishment, permission flags or generic escape hatches that silently settle these decisions. Omit allowance/grace/resume/no-progress machinery from M1. If a proposed storage primitive requires one of these answers, stop that slice and ask the parent for a decision.

## Explicit non-goals and acceptance gate

No automatic turns, idle hooks, reminders, startup goal admission/recovery workers, budget enforcement, grace execution, goal-aware watch activation, new watch subscriptions, Stop/resume parsing, public mutation HTTP API or tool registration, UI/SSE/codegen changes, external completion/wake system, PR monitor work, profiles788, general authorization framework, merge/deploy/release or production migration. No other implementation writer/subagent. No shared-main writes.

Acceptance is a reviewed dormant M1 source increment with normalized storage, auditable actors, atomic optimistic fencing, immutable execution-bound reports and authored tests, plus an honest validation report. Normative additions are limited to this storage/identity/fencing contract and relevant ADR/executive status; runtime policy remains with the parent. Use source-only spec pre-flight review and mark unavailable validators honestly under the no-heavy-work restriction.
