# Make Explore-to-Work approval one atomic capability transition

## Commission and preservation constraints

This P0 owns the product defect end-to-end and is separate from task 98016, whose implementation is already dirty in another WorkScope. Preserve incident conversation `6854e5c5-080d-4d3f-8874-75a08df7f051`, WorkScope `2c27bc33-3dcc-4f1b-ba2c-a7851fe50deb`, and worktree `.phoenix/worktrees/b4546ad2-138e-4c72-b527-9028428b16eb` read-only while fixing this defect: never discard/reset its changes, never manufacture context exhaustion, and never use child delegation as a rescue workaround. No deployment is authorized.

## Postmortem

### Observed timeline

- `13:19:31Z`: production created and started the incident conversation runtime in persisted Explore mode (`~/.phoenix-ide/prod.log:4285`; production DB `conversations.cm_kind=explore`).
- `13:24:37Z`: approval transitioned `AwaitingTaskApproval -> LlmRequesting`; at `13:24:39Z` the log says `Tool registry upgraded to Work mode` and `Task approved` (`prod.log:5704,5712-5713`). The same timestamp is stored in `conversation_approved_task_objectives` and `work_scope_approved_task_authorities`; WorkScope `2c27…` now has `authority_kind=work`, while conversation mode intentionally remains Explore per REQ-BED-028.
- Approval had already done the task-artifact mutation successfully: task 98016 is `in-progress`, and commit `b613d6bf0` contains only that approval artifact. Later source `patch` calls also succeeded, proving some write capability was present.
- Subsequent Bash invocations retained Explore policy: `PHOENIX_SANDBOX_SCRATCH` pointed under `explore-bash`; taskmd rename, Git common-dir `index.lock`, normal `target`/codegen, and uv/cache writes failed. The transcript later recorded Work-child rejection: `Work sub-agents require the parent to be in a write-capable mode` (production DB messages `1901-1902`, `14:00:08Z`).
- After supported cancellation, a fresh user message in the **same** approved conversation requested a non-mutating capability probe (message `1908`, `14:02:49Z`). Bash still reported non-empty sandbox scratch and could not create/remove a uniquely named harmless file in the Git common directory; the turn stopped before mutation. This rejects a one-command or one-turn stale session.
- Production is version `0.12.0`, Git `19fe992c72e5`. Phoenix restarted at `13:57:13Z`, rematerialized this conversation at `13:57:27Z`, and immediately resumed it with a capability surface that stripped nine unavailable tools (`prod.log:10077,10123,10132`). Bash remained Restricted after that restart and Work-child admission still rejected at `14:00:08Z`. The defect therefore survives full process restart/rematerialization as well as actor reuse. No matching bounded VictoriaTraces trace was retained; the production DB transcript and structured log are the independent evidence surfaces.

### Failure chain and structural cause

```mermaid
flowchart LR
    A["TaskApprovalDecided"] --> B["ApproveTask effect"]
    B --> C["task artifact commit"]
    C --> D["atomic DB objective + WorkScope authority_kind=work"]
    D --> E["ConversationRuntime context.resource_authority = Work"]
    E --> F["ToolRegistryExecutor::upgrade_to_work_mode"]
    F --> G["registry definitions and dispatch"]
    G --> H["Bash launcher policy"]
    E --> I["spawn_agents Work admission"]
```

- The durable transition is internally atomic: `Database::persist_approved_task_authority` writes the objective, WorkScope/objective relation, and `authority_kind='work'` in one SQLite transaction (`crates/phoenix-db/src/lib.rs:7244-7320`). It is not atomic with runtime reconfiguration.
- The state reducer enters `LlmRequesting` and emits `ApproveTask` before `PersistState` (`crates/phoenix-state-machine/src/transition.rs:1842-1856`). `execute_approve_task` performs Git/fs mutation, then durable authority persistence, then mutates `ConvContext.resource_authority`, invokes an untyped `upgrade_to_work_mode()` hook, refreshes one derived cache, and resumes (`crates/phoenix-ide/src/runtime/executor.rs:8393-8451`). Errors after the durable write can therefore leave a partially transitioned actor; the hook returns no success/error or capability generation with which to gate resumption.
- The structural cause was **parallel authority representations**: Explore mode provenance selected some runtime capabilities while persisted WorkScope authority selected others. The approval path then regenerated those consumers piecemeal—actor context, registry definitions/dispatch, Bash launcher policy, tool context, prompt, and Work-child admission—without one fallible atomic publication or generation fence. The incident's mixed result (source patch writes succeeded while Bash remained sandboxed and Work-child admission rejected) is direct evidence of a partially projected capability, not merely one cached registry.
- The long-lived Explore-materialized `ToolRegistryExecutor` was one stale consumer, not the root authority. Bash behavior proves effective dispatch still reached `SandboxedBashTool -> BashSpawnMode::Sandboxed -> ExploreSandboxLauncher` (`crates/phoenix-tools/src/lib.rs:988-1061`; `crates/phoenix-tools/src/bash.rs:192-229`; `crates/phoenix-tools/src/bash/operations.rs:436-487`; `crates/phoenix-tools/src/bash/sandbox.rs:36-66`), while successful source patch calls prove another consumer had already acquired write capability.
- Work-child admission independently read the actor's copied authority and rejected unless it was `Work` (`handle_spawn_agents_tool`, `crates/phoenix-ide/src/runtime/executor.rs`). Its rejection proves mode-derived/cached provenance disagreed with durable WorkScope authority; conversation mode was never the legitimate capability source.
- Tool context and clearable tool names were separately derived actor snapshots. Fresh turns therefore reused divergent projections instead of reconstructing one capability from persisted WorkScope authority.
- LLM requests freeze tool definitions and Explore Bash prompt capability per request, while detached tool tasks clone an executor before spawning. Without a capability generation/barrier, approval could neither order already-admitted Restricted work nor reject queued/precreated stale calls deterministically.
- Rematerialization repeated the same structural error: it resolved persisted WorkScope authority and Explore/DetachedProductCreation provenance through separate registry/context branches. A full process restart therefore reconstructed the split rather than healing it. The fix must project every consumer from persisted authority through one published capability snapshot, not refresh a registry cache opportunistically.

### Rejected hypotheses

- **Conversation mode must become Work:** false. REQ-BED-028 requires mode to remain Explore; write authority belongs to the attached WorkScope/objective (`specs/bedrock/requirements.md:905-918`).
- **Approval persistence failed:** false. The objective, relation, WorkScope authority, and approval commit all exist.
- **Only one Bash handle/session was stale:** false. A supported cancel plus fresh same-conversation turn still dispatched sandboxed Bash.
- **Fresh user input reconstructs capability:** false. The actor survives turns and builds tool context from cached authority.
- **`/continue` is a rescue boundary:** false and unsafe. Continuation is defined only for context-exhausted conversations (REQ-BED-030, `specs/bedrock/requirements.md:1012-1049`); this parent is idle, not exhausted.
- **Child delegation can rescue task 98016:** false. Work spawn is correctly rejected from the stale Restricted parent, and delegation would avoid rather than repair the owning actor.
- **A restart heals the split:** false. Production restarted and rematerialized the same approved conversation before the fresh Bash and Work-child failures. Redeploying `origin/main` would also be ineffective because it is the same deployed SHA, `19fe992c72e5`.

## Owning invariant and normative changes first

Before implementation, update the normative artifacts and add a superseding ADR rather than relying on comments:

1. Strengthen REQ-BED-027/028 and `specs/bedrock/bedrock.allium`: approval is one durable capability transition. Before any post-approval provider request or tool admission, all authority consumers—actor context, registry definitions/dispatch, Bash launcher policy, tool-context projection/caches, and sub-agent admission—must represent the same persisted authority generation. A partial runtime transition fails closed and does not resume.
2. Specify crash/restart behavior at the persistence/reconfiguration seam: once Work authority is durably committed, rematerialization derives all consumers from it; a still-live pre-transition actor cannot continue serving Restricted capabilities as if approval had completed.
3. Strengthen `specs/bash/requirements.md` / `bash.allium` so approved WorkScope Bash is structurally unsandboxed and Explore remains OS-sandboxed, including normal project `target`/codegen, uv/cache, worktree, task, and Git common-dir paths.
4. Strengthen `specs/subagents/requirements.md` / `subagents.allium` so Work-child admission derives from the same typed capability snapshot/generation rather than mode labels or an independently cached field.
5. Confirm WorkScope ownership/restart boundaries against `specs/work-lifecycle/*`, retired redirects in `specs/projects/*`, compatibility REQ-COMP-001/ADR-034, and runtime-resource fail-closed ADR-039. Add no broad compatibility or live-resource-replacement promise.
6. Add a new ADR recording why capability transitions use one typed snapshot/generation and one publish point, and why piecemeal mutable fields/untyped no-op upgrade hooks are rejected. Run `specs/AUTHORING.md` pre-flight and Allium validation before pushing.

## Proposed implementation scope

1. Introduce a correct-by-construction runtime capability representation whose Work variant carries everything needed to construct both registry and execution context. Remove or replace the independently mutable `context.resource_authority` + registry + derived-cache transition protocol; test doubles must not silently no-op on a security transition.
2. Make approval orchestration explicit and fallible:
   - perform approval artifact mutation and durable typed authority persistence;
   - build/validate the complete next capability consumer set;
   - publish it once behind an actor-local generation/barrier before any next LLM request or tool admission;
   - emit an observable structured transition event/log carrying conversation, WorkScope, old/new authority, and generation;
   - if publish cannot complete, do not resume with a mixed snapshot. Reconcile/rematerialize from durable authority or remain in a typed failed-closed recovery state.
3. Fence provider/tool snapshots. Define deterministic behavior for a provider request or checked/precreated tool at the approval boundary: pre-transition work may finish only under its admitted Restricted generation or be cancelled; it may never execute after being reinterpreted as Work. No post-transition call may use an older generation.
4. Ensure `build_tool_context`, tool definitions, dispatch, `clearable_names`, Bash tool/launcher choice, system-prompt capability, and `spawn_agents` Work admission consume the same published capability snapshot.
5. Keep mode/provenance/cwd/WorkScope identity unchanged per REQ-BED-028. Do not alter Bash sandbox policy itself, WorkScope retirement, continuation rules, or task 98016 implementation.

## Deterministic reproduction and regression matrix

Build the test around a real temporary Git repository/worktree, real taskmd artifact, production `ToolRegistryExecutor`, and controllable approval/runtime barriers—not log assertions or a mock executor whose upgrade defaults to no-op.

### Same actor, same conversation

1. Materialize one Explore conversation and actor; precreate/freeze an Explore tool definition/request snapshot to prove the old generation exists.
2. Before approval, assert Bash has non-empty `PHOENIX_SANDBOX_SCRATCH` and cannot write source/task/Git common-dir; assert Work-child admission is rejected.
3. Approve a task in that same conversation. Assert taskmd `ready -> in-progress` and the approval-only commit occur, mode remains Explore, WorkScope/objective authority is Work, and exactly one observable capability-generation transition is published.
4. Without evicting/restarting or creating another conversation, use the same actor to prove:
   - Bash has no Explore scratch marker;
   - a uniquely named harmless Git common-dir lock-like fixture can be created and removed;
   - a second taskmd-style fixture can be renamed without touching the approved artifact;
   - normal repository `target`/codegen and uv/cache paths are writable without sandbox redirects;
   - a harmless source fixture can be committed and cleaned up inside the disposable repository;
   - Work child admission succeeds against the same WorkScope (use a deterministic fake child execution after admission, not an LLM/network dependency).
5. Send another user turn through the same actor and repeat the Bash/common-dir and Work-child assertions. This is the incident regression.

### Crash/restart and partial transition

- Inject a crash/failure immediately after the durable authority transaction and before runtime publication. Rematerialize from the DB and prove the new actor is wholly Work; the old actor/generation cannot admit calls or publish a conflicting transition.
- Inject runtime-build/publication failure after durable persistence. Assert execution does not resume and no post-approval LLM/tool call runs under either a stale Restricted consumer or a partially built Work consumer.
- Restart/rematerialize from an Explore-mode row plus typed approved objective/Work WorkScope and prove every consumer is Work. Also test a genuinely unapproved Explore row remains fully sandboxed.

### In-flight and precreated snapshots

- Hold an Explore LLM request at the frozen-tool-surface barrier while approval is attempted; prove the ordering policy is deterministic and no old response can execute a Work-sensitive call after publication.
- Hold a prelooked-up `SandboxedBashTool`/checked call and a prebuilt tool context across the boundary; prove generation fencing cancels/rejects it rather than allowing stale dispatch.
- Exercise actor reuse after cancel, idle, and a subsequent user message. No session/handle cleanup alone may be credited as the fix.

## Safe rescue path for task 98016 (operationally separate)

`POST /api/conversations/:id/upgrade-model` is the supported per-conversation actor replacement boundary (`crates/phoenix-ide/src/api/handlers.rs:5028-5129`; `crates/phoenix-ide/src/runtime.rs:4550-4707`), but it is **not a safe rescue for this incident**: a stronger full-process restart already rematerialized the same persisted conversation and reproduced Restricted capability. Do not invoke model upgrade, restart, or redeploy as rescue.

The only safe current path is preservation: leave task 98016's conversation, WorkScope, worktree, Git state, and dirty files untouched until this P0 fix is reviewed, merged, and a separately authorized production deployment installs it. After that deployment, rematerialize the same conversation and run only the Bash/common-dir plus Work-child admission probes. Continue task 98016 in its parent only if those probes pass; otherwise stop and preserve all state. Never use `/continue`, fabricate context exhaustion, create a replacement conversation/worktree, or delegate to a child.

## Validation and delivery

- Focused state-machine, DB, runtime, tool-registry, Bash, and sub-agent tests, including the deterministic end-to-end matrix above.
- `allium check` for touched Allium specs and the full spec-authoring pre-flight.
- `./dev.py check --all` on exact HEAD.
- Independent `phoenix-adversarial-review` focused on authority generations, crash windows, stale task execution, lock ordering, and test realism; resolve every finding.
- Commit this P0 separately from task 98016, push a separate branch/PR, use a concept-focused PR description, and wait for exact-head CI plus Codex review. Re-run required checks after any review change and make a concrete merge/no-merge decision from exact-head evidence.
- No production deployment.

## Risks and explicit non-goals

- **Privilege escalation:** generation fencing and pre-approval negative tests are mandatory; never infer Work merely from Explore mode plus a worktree.
- **Deadlock/race:** approval already crosses actor, blocking Git work, SQLite, registry locks, and detached request/tool tasks. Use an explicit transition boundary rather than nested ad hoc locks; exercise barriers deterministically.
- **Persisted Work with failed publication:** durable authority is canonical. Fail closed, then rematerialize/reconcile; never downgrade the DB to match a stale actor.
- **No incident mutation during product work:** production conversation/DB/logs and task 98016 worktree remain evidence only until the separately approved rescue step.
- No deployment, continuation redesign, Bash sandbox-policy weakening, WorkScope lifecycle redesign, child-delegation workaround, or implementation of task 98016 in this PR.
