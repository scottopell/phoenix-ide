# Deliver a local Global Coordinator-owned WorkScope and persistent home

## Commission, ownership, and mandatory gates

Dedicated LOCAL delivery, directly managed by and reported to Global. The user extracts only the Coordinator persistent execution WorkScope/home requirement from federation task59008. Federation retains every other original five-tool, client, connection-notes, destination provenance/navigation, message acceptance, and approved subscription requirement. Do not silently reduce or implement those scopes here. Shared WorkScope contracts remain usable by later federation integration; federation does not own this implementation.

This is an Explore proposal, not runtime approval. Approval is required before source edits or implementation. Work in the approval-created isolated checkout; preserve dirty parent and project-owner checkouts. No heavy local build without actual self-service resource admission. Use bounded hosted qualification as appropriate. No merge or deploy without Global admission.

## Observed journey

Global currently borrows ordinary worker scopes to stage evidence and has improvised a handoff file beneath a worker target tree (user report). That is not an owned durable home. The desired journey is server initialization/reconciliation -> stable Coordinator-owned active WorkScope and non-project home -> explicit scoped tool execution -> retained files and concise context pointers -> continuation/restart with unchanged ownership and home -> ordinary worker close/delete/cleanup without loss of Coordinator state.

## Verified findings and recovery status

- Investigation checkout: `c8e60468-104e-41db-87c7-0d7f08ebd931`, clean source at `07940b7ce1d1f577fe8d047961549af7c0f2017c`; no implementation edits or builds performed.
- `tasks/59008-p1-in-progress--federation-e2e.md`, introduced in `32c5d4599`, states exactly: “Give the Coordinator a persistent execution WorkScope that exists with zero ordinary conversations.” It lists the other federation slices separately and reserves publication/integration/deployment/live pairing to Global.
- `specs/global-recall/requirements.md` REQ-GR-006 excludes ordinary Coordinator WorkScope attachment/Close/Delete; REQ-GR-007 requires explicit active scope targeting and persisted canonical path resolution. This delivery must update the normative ownership contract deliberately, not bypass it.
- `crates/phoenix-core/src/work_scope.rs`: `ResourceScopeKey::Coordinator` is distinct from `Work`, `EnvironmentContext` currently distinguishes allocated worktree, unowned cwd, and none; `WorkScopeRecord` holds identity/authority/lifecycle. A server-owned non-project home cannot be mislabeled an unowned cwd or fake project/worktree.
- `WorkScopeCoordinatorBash` in `crates/phoenix-ide/src/coordinator_tools.rs` requires `work_scope_id`; `GlobalReadService::resolve_active_work_scope_bash_target` in `api/global_read.rs` resolves active persisted scope/environment plus live owner and canonical path. `present_svg` shares the scope resolution boundary.
- `system_prompt::build_coordinator_system_prompt_with_catalog` explicitly says there is no default cwd and first-conversation creation is unavailable without an active scope. Update this truth through owned authority, not a raw cwd escape hatch.
- Bedrock compaction/continuation requirements protect accepted persisted handoff provenance; filesystem files are not automatically ingested into compaction.
- Owner checkout `/Users/scottopell/dev/phoenix-ide/.phoenix/worktrees/7c2d9446-46a1-42b4-bbad-c8ddfbfddf7f` exists and has dirty source/spec/untracked work. It was inspected read-only and must remain untouched.
- Full pinned federation design, exact identities of all five approved tools, and retained private scope/home implementation are NOT yet recovered. No claim that a private-work request has been sent. Ordinary-parent transcript lookup was unavailable. GitHub roadmap #806 could not be verified: Explore OS sandbox blocks network access.

## First approved step: recover exact inputs before interface decisions

1. Read roadmap #806 when access permits and relevant normative specs, compatibility requirements, ADRs, cleanup and resource authority contracts.
2. Recover original federation requirement/design from task history, retained project transcript/artifacts and source provenance; quote and pin exact source identities in the delivery evidence. Preserve the original remaining scope without guessing tool names or subscriptions.
3. Send a targeted request to project `7c2d9446-46a1-42b4-bbad-c8ddfbfddf7f` through the supported authenticated coordination channel for original design and retained private home/WorkScope work, including commit/patch paths and interface assumptions. Record acceptance/result or exact failure. Read/reuse only relevant authorized artifacts; do not overwrite the dirty checkout or duplicate existing implementation.
4. Do not wait on federation pairing or the exhausted remote owner. Use recovered evidence and implement the local commission independently; if exact inputs remain unavailable, keep that specific blocker visible, seek Global's resolution, and do not present guessed interfaces as recovered contract.

## Design and interaction map

Server data-root configuration -> safe singleton initialization/reconciliation -> relational Coordinator ownership + active WorkScope/environment -> scope resolver -> Bash and applicable scoped tools. Coordinator runtime -> typed authoritative prompt/tool context -> compaction request and accepted handoff -> successor runtime. Ordinary retirement/Close/Delete/reaper -> ownership-aware eligibility -> cannot retire/delete Coordinator state. Explicit owner backup/export/cleanup -> retained home bytes plus authoritative metadata under documented compatibility policy.

- Persist exactly one authoritative local Coordinator scope/home binding independent of current transcript-row identity and independent of worker ownership. Use relational schema constraints and typed ownership/environment distinctions; no parallel authoritative copies in transcript blobs or notes. Identity and files survive Coordinator continuation and restart. Decide initialization sequencing, concurrency, partial failures, missing-directory reconciliation, and restore semantics explicitly; do not silently rebind or recreate missing retained state in a way that masks data loss.
- Supported baseline: a genuine server-managed non-project filesystem home under the configured server data storage; not a managed Project, repository checkout, or Git worktree. Do not initialize Git, invent branch metadata, or imply Git-only actions work there. Document which non-project scoped operations are supported and which Git/project-only operations fail explicitly. Choose exact layout and ownership types after contract recovery.
- Safe fresh initialization and existing-install reconciliation must be idempotent and nondestructive. Never auto-import, move, delete, execute, or trust improvised worker-tree files. Existing worker scopes and home contents remain intact. Reject conflicting ownership, invalid/missing target bindings, files in place of directories, unsafe symlinks/escapes, inaccessible paths, and inconsistent persisted environment with actionable errors and no spawned process or destructive repair.
- Keep the explicit local `work_scope_id` execution contract. Resolve the Coordinator scope through persisted owner/lifecycle/environment authority, with the same canonical path and applicable containment/symlink validation. No default arbitrary cwd, remote-server cwd interpretation, remote shell, credential bypass, or broader resource authority. Preserve existing ordinary scope targeting, handle semantics, audit trail and process bounds.
- Inject concise server-generated scope identity, home path, ownership/environment status and deliberately selected retained note/evidence pointers into Coordinator context and handoff lifecycle. Separate trusted metadata from untrusted historical file contents. Do not discover and ingest arbitrary home files as instructions or promise notes are automatically read by compaction. Retained notes are historical data, never authority; current claims require fresh runtime/source/test evidence.
- Define retention as independent from ordinary close/delete/history/reaper cleanup. Specify explicit owner-managed export/backup and cleanup behavior, including home files versus DB metadata, what existing exports do not include, and confirmation/eligibility for destructive cleanup. Do not expand rollback/restore/live replacement guarantees implicitly; update compatibility requirements and ADR if a new guarantee is necessary.
- Add timeless requirements and a superseding project ADR for changed Coordinator ownership/environment semantics. Add/update Allium only for initialization/reconciliation and lifecycle rules where precision is warranted. Keep executive verification claims aligned with actual evidence. Routine engineering choices belong to the implementer; no speculative generic memory platform.

## Implementation and focused validation

After recovery and design, implement bounded schema/domain changes, startup and existing-Coordinator initialization/reconciliation, authoritative resolver support, lifecycle/cleanup fences, and prompt/handoff discovery. Thread changed fields through every producer/consumer; regenerate actual Rust-derived TypeScript via `./dev.py codegen` only when wire types change, never hand-edit generated files. Add targeted DB, runtime, scoped-tool, context and lifecycle tests.

Required isolated E2E proof (not production or a worker's live directory):

1. Fresh database/server with zero ordinary conversations produces a genuine usable Coordinator home and active persisted WorkScope; explicit scoped Bash `pwd` resolves the canonical home.
2. Existing installation/Coordinator initializes safely without deleting or relocating existing files; repeated/concurrent initialization keeps one binding.
3. Write an evidence file, continue the Coordinator and restart the isolated server: scope identity, home path, bytes and ownership remain; prompt and handoff contain the intended metadata/pointers without ingesting arbitrary note instructions.
4. Create a separate ordinary scope and sentinel: independent scoped execution works; ordinary close/delete/cleanup leaves Coordinator binding/files intact and does not alter unrelated worker state.
5. Invalid/stale scope, corrupted binding, missing/inaccessible root, symlink/path escape and partial initialization errors produce explicit failure, no unauthorized process, and no destructive automatic migration. Test recovery/reconciliation behavior selected by the normative contract.
6. Preserve `present_svg` containment/ownership and ordinary scoped tool/resource authorization regressions; remote scope identity/path cannot authorize local cwd execution.

Run checks through `./dev.py`, under actual resource admission. Obtain independent adversarial review of exact final HEAD, address findings, and qualify that same HEAD in bounded hosted CI with retained check-run IDs/results. A changed HEAD invalidates earlier exact-head qualification. Retain failures; follow each failed/pending run through a supported bounded retry/fix/poll trigger to a terminal result or explicit blocker with owner and next action. Do not stop at a vague “CI pending.”

## Delivery evidence and operational lessons

Report directly to Global: final SHA/PR, source-contract recovery and request outcome, design decisions, tests/checks with terminal results, independent review findings/resolution, isolated E2E observations, retained failures and any concrete admission blocker. Publication/integration only within Global's authority; no merge/deploy here without admission.

Carry into implementation handoffs: quiet user updates are not passive coordination; delegation is not execution; fresh runtime/source/test evidence beats stale pending claims; persistent blockers remain visible; waits require supported concrete follow-up triggers. Notes do not automatically populate compaction and never replace authority.

## Non-goals

All remaining federation five-tool/client/notes/provenance/navigation/acceptance/approved-subscription work; remote pairing or protocol expansion; remote Bash/process management; generic memory platform; automatic ingestion of filesystem guidance; fake managed project; arbitrary host-cwd fallback; destructive migration of existing notes; broad compatibility/recovery promises; production enrollment, merge or deploy.

## Approval outcome

Approval authorizes this dedicated local design/implementation/test/review delivery in an isolated checkout, not heavy-build admission, merge/deploy admission, or a claim that runtime/source changes already exist. Contract recovery and private-work request remain explicit first-step obligations.
