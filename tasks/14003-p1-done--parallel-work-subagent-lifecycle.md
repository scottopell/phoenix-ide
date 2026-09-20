# Model-qualified parallel Work subagent orchestration

## Product outcome

A parent conversation whose **actual resolved parent model** has been manually qualified for Phoenix orchestration can admit and run multiple Work children concurrently. Those children intentionally share the parent environment and act as trusted collaborators: the parent partitions and integrates work, while children preserve unrelated edits and report conflicts honestly.

Parents using an unqualified model retain sequential Work-child admission. Explore/read-only children retain their existing parallel behavior. Child model overrides, named-agent persona/defaults, reasoning effort, service tier, configuration execution tier, and provider version comparisons do not influence parent qualification.

This task supersedes its earlier universal write-conflict/isolation requirement. Structural prevention of overlapping writes is not a product requirement for this feature. The earlier task history and PR #635 remain architecture and race-analysis evidence, not an implementation branch to revive.

## Qualification policy

Use one explicit allowlist over the resolved model ID frozen in the parent runtime (`ConvContext.model_id`). Never infer qualification from `version >= 5.6`, provider family alone, a child model, or a newly discovered model.

| Resolved parent model | Parallel Work admission |
|---|---:|
| `gpt-5.6-luna` | No |
| `gpt-5.6-sol` | Yes |
| `gpt-5.6-terra` | Yes |
| `gpt-6-astra` | Yes |
| Each specifically listed, supported Opus 5+ built-in ID | Yes |
| Other Anthropic models | No |
| Custom/provider-configured models | No |
| Unknown or newly introduced models | No |

At implementation HEAD, enumerate the exact supported Opus 5+ IDs present in Phoenix's catalog; do not invent IDs or qualify future Opus releases implicitly. Apply the same decision at every reasoning effort supported by each qualified model.

A later resolved switch/fallback to an unqualified parent model does not cancel, reinterpret, or lose already admitted children or results. Subsequent Work admission follows the parent's then-current resolved model. Keep this pragmatic: model upgrades are already settled-only, and admission/model change races need one deterministic before-or-after outcome rather than a new model-snapshot subsystem.

## Existing systems to preserve

Build on the current architecture rather than introducing a second orchestration framework:

- The child state machine remains authoritative for child execution.
- The parent state machine remains authoritative for pending/completed children and fan-in.
- Existing early-result buffering and pending-set membership handle out-of-order results.
- Existing child terminal evidence and startup reconciliation remain the durable recovery path.
- The runtime map and channels are in-process routing only, never lifecycle truth.
- WorkScope remains resource ownership authority and always outlives attached children.
- ProductConversation Close uses ordinary parent cancellation and fan-in settlement; this task does not implement WorkScope retirement or a Close-specific child coordinator.
- Existing authority, cwd, exact WorkScope attachment, maximum ten tasks per call, max-turn, and timeout bounds remain.

## Design

### 1. Consistent schema and runtime admission

Expose truthful `spawn_agents` schema guidance for the resolved parent model and enforce the same policy at runtime:

- Qualified parent: a bounded call may contain multiple Work tasks.
- Unqualified parent: at most one pending/admitted Work child across calls and batches.
- Explore parents still cannot spawn Work children.
- Explicit child model selection remains independent of parent qualification; for example, a qualified Sol/Terra/Astra/Opus parent may run several Luna Work children concurrently.

Replace the process-local `active_work_subagents` policy with durable pending/admission truth. Concurrent calls, multiple calls in one tool round, mixed batches, and restart reconstruction must reach the same admission decision; channel-consumer or `select!` ordering must not determine semantic admission.

### 2. Atomic batch admission

Resolve and validate every task first, then atomically persist the admitted batch and its parent pending membership before any child can start. A call either admits its complete known child set or admits none; per-child channel sends must not allow an untracked partial batch to escape.

Use the smallest normalized schema that makes admitted children, cancellation, initial-start eligibility, and terminal acceptance unambiguous. The concrete batch-header versus transactional child-row shape is an implementation decision. Preserve migrations and compatibility guarantees; do not put a child collection into a JSON blob.

### 3. Surgical startup and cancellation correctness

Close the four current supported-journey races recorded in task 14004 while reusing existing runtime machinery:

1. **Pre-install cancellation:** cancelling an admitted child before dequeue or during materialization revokes/owes cancellation durably. The child cannot later begin ordinary work after the parent has accepted a cancelled result.
2. **Cancellation head-of-line blocking:** cancellation of an installed child must not wait behind unrelated slow materialization, including a child of another parent.
3. **Fresh spawn versus reconstruction:** fresh creation and `get_or_create` join the existing per-conversation single-flight boundary, producing one runtime and one initial task bootstrap.
4. **Reconstructed child result delivery:** reconstruction restores delivery by durable parent identity; it cannot lose a terminal result because a cached sender was absent or stale.

Keep the durable lifecycle minimal. Model only facts needed to decide admitted, cancellation requested, initial work dispatched, and terminal/accepted behavior; do not transplant PR #635's broad child workflow coordinator. Careful per-child async work plus an atomic start-if-not-cancelled gate should be preferred over a scheduler, global slots, or a general workflow framework.

After this implementation lands, task 14004 may be closed as satisfied by task 14003 while preserving task 14004's identity and evidence.

### 4. Shared addressed conversation dispatch

Split the direct-turn worker's existing in-process delivery concern from its direct-turn workflow semantics:

```text
conversation ID -> existing get_or_create single-flight -> current runtime inbox
```

Introduce a small internal addressed dispatcher/private primitive and keep thin typed semantic adapters:

- Direct-turn acceptance, leases, generations, replay, and terminal obligations remain direct-turn-specific.
- Persisted child-result delivery targets the durable `parent_conversation_id`, not a long-lived cached parent sender.
- Installed-child cancellation may use addressed dispatch only after durable cancellation/start resolution says runtime delivery is owed.
- Live-only projections remain live-only and need not be migrated.

Remove long-lived `parent_event_tx` authority from child lifecycle paths where the addressed dispatcher replaces it. This is not a general pub/sub bus: durable workflow state decides whether an event is owed; the dispatcher only resolves and wakes the current in-process consumer.

Persist terminal evidence before treating live result delivery as complete. If addressed delivery fails, kick/reuse existing reconciliation. Duplicate delivery must converge harmlessly: a child already accepted by parent fan-in does not append a second result, resume the parent twice, or cause a fatal transition.

### 5. Existing fan-in, reconstruction, and Close seam

Preserve and extend existing behavior for arbitrary completion order, early buffered results, multiple batches in a tool round, cancellation, timeout, restart, and reconstruction:

- every admitted child has exactly one durable identity and at most one live runtime;
- initial task bootstrap occurs at most once;
- durable terminal cause/result remains available until exact parent acceptance;
- reconstructed delivery resolves the current parent by durable identity;
- restart recovery does not duplicate a runtime, bootstrap, result, or successor;
- user cancellation covers queued, starting, and installed children;
- parent settlement does not complete while admitted child work remains unsettled;
- Close drains multiple starting/running children through ordinary cancellation and parent fan-in.

Audit the concrete ProductConversation Close seam at implementation HEAD, including devmbp PR #764 evidence and successor conversation `e609d6e0-557d-4848-a68b-177569448e5b`. Those are evidence only: do not revive #635, cherry-pick #764 wholesale, add a blanket portfolio wait, or take ownership of WorkScope retirement.

### 6. Trusted shared-worktree collaboration

Qualified parallel Work children intentionally share the parent cwd/worktree and exact WorkScope. Update parent and child prompt composition so it truthfully states:

- the parent should partition assignments and owns integration;
- other trusted collaborators may edit the same worktree concurrently;
- children must inspect and preserve unrelated edits;
- children must report conflicts, overlap, or uncertainty rather than silently replacing work.

Do not add child worktrees, merge/reconciliation orchestration, path declarations or locks, repository mutation locks, global worker slots, automatic overlap rejection, a broad capability framework, or guarantees that arbitrary Patch/Bash/Git/MCP writes are atomic.

### 7. Narrow Codex/ChatGPT catalog and compatibility

Retain Luna, Sol, Terra, and Astra as selectable Codex/ChatGPT models. Remove `gpt-5.4-mini`, `gpt-5.4`, and `gpt-5.5` from built-in selectable choices, discovery matches, default/policy helper lists, and schema reintroduction paths. Leave Anthropic and custom/provider-configured model catalogs untouched.

Use explicit compatibility mappings rather than the deployment default:

| Removed live pin | Supported approximation |
|---|---|
| `gpt-5.4-mini` | `gpt-5.6-luna` |
| `gpt-5.4` | `gpt-5.6-sol` |
| `gpt-5.5` | `gpt-5.6-sol` |

Apply the same explicit resolution to existing live conversations and named-agent pins. If the mapped replacement is unavailable, require clear reselection/error instead of silently falling onward to another model. Preserve historical conversation/turn attribution, pricing where required for old usage, migration history, and the identity of requests/runtimes already in flight.

Do not edit live configuration. Delivery defaults remain Astra as commissioned; this older devmbp instance currently defaults Sol and must not be changed to simulate delivery state.

## Normative updates

Record the standing behavior, not Rust implementation details:

- `specs/subagents/requirements.md`: replace the universal one-writer guarantee with explicit model-qualified parallel Work, trusted shared-worktree collaboration, atomic admission, cancellation/start, and durable delivery needs.
- `specs/subagents/subagents.allium`: replace `SpawnRejectedMultipleWorkInBatch`, `SpawnRejectedWorkSubAgentAlreadyActive`, and `OneWorkSubAgentPerParent` with explicit qualification, conditional sequential admission, atomic batch admission, cancel-before-start, joined materialization, and durable parent-result-delivery rules.
- `specs/bedrock/bedrock.allium`: remove the unconditional `OneWorkSubAgent` invariant; tighten pending removal, idempotent duplicate result delivery, and settlement/fan-in invariants without adding a new Close state machine.
- `specs/llm/requirements.md` and executive status: record catalog pruning, explicit legacy mappings, and historical/in-flight identity preservation.
- Project ADR: record the manual model matrix; explicit non-inheritance for unknown/new models; trusted shared-worktree policy superseding universal write isolation; atomic admission; and addressed current-runtime dispatch extracted from direct-turn delivery rather than a general bus.
- Update affected executive documents after implementation. Run the authoring checklist and `allium check` for every touched Allium spec.

Do not encode channels, Rust worker names, or every transient construction phase as Allium domain state. Specify the durable admission, cancellation, delivery, and fan-in outcomes.

## Required evidence

### Qualification and admission

- Full supported-effort matrix: Sol, Terra, Astra, and each explicit Opus 5+ ID allow parallel Work; Luna denies it.
- Unknown/new, custom, other Anthropic, and unqualified parent models do not inherit qualification.
- Child model/persona/service tier/config tier do not affect parent qualification.
- A qualified parent concurrently runs multiple Luna Work children.
- An unqualified parent remains sequential across batches and multiple calls.
- Mixed Explore/Work batches, multiple spawn calls in one tool round, and concurrent admission races preserve one durable decision.
- A switch/fallback/downgrade to an unqualified model preserves admitted children/results and blocks new parallel Work admission.
- Invalid or failed batch admission leaves no escaped child.

### Lifecycle, cancellation, and delivery

- Cancellation before dequeue prevents child runtime work and produces exactly one parent outcome.
- Cancellation during materialization is observed before initial work begins or is delivered exactly once to the installed runtime, according to the atomic winner.
- Installed-child cancellation is not delayed by unrelated blocked materialization.
- Opening/resuming a child during fresh spawn yields one runtime and one task bootstrap.
- A reconstructed child reports through the current parent identity.
- Multiple children complete out of order and their results survive parent/runtime reconstruction and process restart.
- Failed live delivery remains durably owed and reconciliation delivers it once.
- Duplicate delivery is an idempotent success/convergence case.
- Terminal causes required by REQ-SA-009 remain distinguishable and durable.
- Cancel-all includes queued, starting, and installed children.
- Close drains multiple starting/running children without retiring an environment under writers.

### Catalog and compatibility

- `/api/models`, UI choices, spawn schema catalogs, and provider discovery retain Luna/Sol/Terra/Astra and cannot reintroduce 5.4-mini/5.4/5.5.
- Anthropic and custom/provider-configured choices are unchanged except for explicitly supported built-in Opus qualification.
- Existing live pins and named-agent pins use the explicit replacement matrix; unavailable replacements produce a clear error/reselection path, never default fallback.
- Historical usage remains attributed to the actual old model.
- An in-flight runtime/request retains its already-resolved identity.
- Defaults are not changed as part of pruning.

## Explicit non-goals

- WorkScope retirement implementation or ProductConversation lifecycle redesign.
- General event bus/pub-sub framework or broad API-handler migration.
- New child workflow coordinator, scheduler, global slots, or arbitrary concurrency framework.
- Child worktrees, merge framework, path-lock regime, structural conflict prevention, or arbitrary-write atomicity.
- Broader Anthropic/custom qualification or automatic family/version qualification.
- Non-blocking `spawn_agents` redesign.
- Live configuration edits.
- Merge or deployment.

## Delivery and convergence

Implement in reviewable commits, keeping the tree green between coherent units where practical:

1. normative specs/ADR and explicit qualification/catalog policy;
2. addressed dispatcher extraction while preserving direct-turn behavior;
3. atomic admission and shared single-flight materialization;
4. cancellation/result-delivery/recovery fixes;
5. qualified parallel Work admission and collaborator prompts;
6. model pruning/compatibility;
7. exhaustive regression and integration evidence.

Use `./dev.py` for development. Run focused tests, codegen, Allium validation, the spec authoring pre-flight, and full `./dev.py check`. Run heavy concurrency/restart/Close matrices on devmbp, then obtain hosted CI, exact-HEAD Codex review, and exhaustive Cursor audit; remediate findings through full convergence. Commit and push the completed branch. Do not merge or deploy.

## Preserved evidence and coordination history

- Task identity remains **14003**; this body supersedes its earlier blocked architecture while preserving the task's history.
- Old owner `3c840119` is terminal and must not be revived or duplicated.
- PR #635 and its exact-head review preserve abandoned implementation, race analysis, and tests as evidence only.
- Task 14004 records independently reachable current-main lifecycle races; solve them here without erasing that task's history.
- ProductConversation/PR #764 and successor `e609d6e0-557d-4848-a68b-177569448e5b` are audit evidence for the landed Close seam, not branches to resurrect wholesale.
- Roadmap Issue #651 must be read at implementation start when GitHub is available; local read-only orientation could not verify its generated body.
