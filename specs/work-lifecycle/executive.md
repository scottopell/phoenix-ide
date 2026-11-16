# Work Lifecycle — Executive Summary

## What This Spec Covers

Explicit Close for ProductConversations: exact loss inspection, reconstructible-only automatic deletion, owned-resource shutdown and retirement, failure visibility, mandatory Global delivery, fresh safe retry under original authority, and one proof-gated completion-only successor. Branches and pull requests are never Close-owned artifacts.

## Current Reality

**Candidate / implementation acceptance unvalidated.** ADR-088 remains historical; proposed ADR-245 would narrowly supersede only its absolute explicit-only retry phrasing for one proof-gated completion-only successor; its stop-first-failure, startup observation-only, and explicit resource retry boundaries remain. Unified `close_conversation`, retained idempotence, and the bounded retry are unimplemented pending semantic evidence; green mechanical checks are not acceptance evidence. This spec slice does not certify a source candidate, migration, deployment, or live behavior. No production cleanup or database repair is authorized by this document. The separate mechanical baseline is commit `5afc014d6f312e32b223194a4be8b45feb811a18`, full hosted run `43620449466` attempt 1: independently reconciled native approval 70 + owning Close 23 actual named passes. Those receipts do not implement or qualify this draft. No local Allium validator was run; the draft still requires exact normative/design review and hosted validation. The v1, v2 and v3 exact eight-document freezes received request changes; proposed v4 corrects actual created-record bindings, exact-receipt replay, required-payload decision variants, immutable run-bound retry plans, direct cancellation/source-fence terminalization, live completion-driver connectivity, typed primary-versus-resource-failure provenance, legacy no-allowance startup, and History-preserving uncertainty. V3 adds pre-state captured unconditional terminalization for automatic and explicit admissions, no lost-worker callback dependency, actual typed normalized resource children through every retirement failure producer, a monotonic native-versus-retained-modern retry admission partition with qualified REQ-WL-002c, the remaining History-preserving uncertainty correction, and single-expression invariant bodies. Previously admitted non-native work is not stranded by receipt adoption; legacy HTTP permissions remain unchanged. V4 preserves independent WorkScope retirement as an exact ordered planned effect with full identity/gate/safety/outcome/completeness binding and scope-only retry, but does not expand process-epoch retry permits. Exact retained cancellation atomically resolves Open+confirmed to History/cleanup attention, Open+uncertain to Open/CloseIncomplete, and prior History to History, preserving cancelled outcome and terminal failure/outbox without later classifier repair. A claimed extra WorkLifecycle generic producer was retracted as nonexistent; it is not a v4 repair. These corrections remain unaccepted pending superseding whole-freeze review; no parser result is claimed. ADR-245 lists static falsification traces, not executed tests.

**Superseding validation / dormant prerequisite status (2026-11-14).** The v4 normative/typed design received independent and accountable static acceptance. Its first hosted run exposed duplicate rule bindings and exact analysis-baseline changes; the separately reviewed correction `de01cccda80d204ae50f99c22012040a808c4110` passed full hosted run `43635599067` attempt 1 (15 successful jobs, 3 expected skips), including Allium and UI/specs. Independent raw inspection reconciled the existing 70 native + 23 Close named regressions; this is not new unified-Close semantic qualification. The earlier UI focus failure did not recur; its cause is unclassified. A first **unqualified source prerequisite** now adds only original-operation allowance storage for newly created operations and a monotonic native-management marker with the generic retry transaction fence. Legacy and direct creation allocate one allowance atomically; pre-migration operations are not backfilled. No production path marks an operation native-managed or consumes/suppresses the allowance. Request receipts/atomic native capture, live source-before-replay and Stop serialization, tool adapters/discovery, explicit and automatic unified execution, and removal of `retry_close_safely` remain unimplemented. New prerequisite tests are authored, not executed locally; exact-diff review and hosted validation remain required. The historical review-pending descriptions above do not supersede this status, and none of these results authorizes registration, merge, deployment, or production recovery.

**Superseding source progress (2026-11-16; not acceptance).** The candidate now contains source-owned initial/observation/explicit-retry receipts, exact retained retry plans and physical retry execution, a clone-stable actor/backend database-authority check, and a shared acknowledged retry-handler journey. The Coordinator `close_conversation` adapter routes strict action-specific inputs to those native manager queues and reports admission/replay separately from fresh retained observation. `initial` still admits only; end-to-end initial execution and current-result equivalence are not qualified. `retry_close_safely` remains deliberately registered until replacement equivalence is established. The authored retry journey now enters through the production manager route into the bounded source actor and refuses receipt replay from a live Idle source; this does not exercise production startup or the full event loop. Automatic completion retry, full public actor journeys, Stop/startup/cancellation coverage, migration-lineage reconciliation, integrated review, and exact-source green hosted qualification remain outstanding. Authored tests and source-only review are not execution evidence; no local native execution, merge, or deployment is claimed.

The retirement inventory persistence candidate gives supported transaction writers atomicity between a fresh full capture and its positive original-snapshot anchor. The schema enforces full sealed shape and immutable positive anchors/negative exclusions; this does not promise temporal protection against arbitrary future SQL writers. All pre-migration attempts, including zero-ambiguity attempts with no retained inventory, are negatively excluded; there is no positive backfill or native-adoption reversal. Later inspection generations preserve an existing original anchor. Legacy shared capture, reinspection, observation, and generic retry continue without native original-anchor proof; partial repair and copied inventory writes cannot originate it. The migration and focused regressions remain source-candidate work and have not been locally executed or independently qualified.

Lifecycle ended and resources retired are separate facts. Confirmed conversation **and** owned-process shutdown permits read-only History with conspicuous `cleanup_attention` after cleanup failure. Uncertain shutdown before History stays Open with `CloseIncomplete`; later uncertainty does not reopen History. A stopped execution is never resumed; startup observes only. Explicit user or Global resource retry allocates a new durable run ordinal under original authority after fresh safety proof. The sole automatic exception is a fresh completion-only successor after exact proof that all planned effects durably succeeded and no resource/destructive effects remain; its one durable budget is consumed atomically and cannot renew. Stop/pause/cancellation suppresses pending admission. Changed risk, unique/uncertain discard, expanded effects, or database surgery requires a concrete proposal and separate approval.

## Requirements and Acceptance Status

| Requirement | Contract | Status |
|-------------|----------|--------|
| REQ-BED-029 | One normal run stops at first failure; proven shutdown enters History independently of cleanup success | Candidate / unvalidated |
| REQ-WL-001 | Close is the only ordinary terminal action; legacy inputs must preserve its contract | Candidate / unvalidated |
| REQ-WL-002 | Exact loss inventory; automatic deletion needs fresh reconstructibility proof | Candidate / unvalidated |
| REQ-WL-002a | Discard confirmation binds exact original authority, run ordinal, generation, and fingerprint | Candidate / unvalidated |
| REQ-WL-002b | Exact scope/process/private-directory authority; stop on first failure; observation-only startup; no repository mutation or recovery artifact | Candidate / unvalidated |
| REQ-WL-002c | Explicit fresh safe retry; no unresolved replay, completed-effect repetition, or authority expansion | Candidate / unvalidated |
| REQ-WL-002d | Tmux retirement needs exact socket plus server-token authority | Candidate / unvalidated |
| REQ-WL-002e | Immutable exact-run writer authority and inspection-gap child evidence | Candidate / exact-source hosted gate (PR #1043 receipts) |
| REQ-WL-002f | Unified source-bound idempotent Close; one exact-proof completion-only successor | Unimplemented / semantic evidence required |
| REQ-WL-004 | Durable mandatory once-per-failure Global event through unified delivery, even unwatched or in History | Candidate / unvalidated |
| REQ-PROJ-028a | Missing/inaccessible worktrees remain exact immutable observations, not startup cleanup authority | Candidate / unvalidated |
| REQ-PROJ-WS-001 | One ordinary owner per WorkScope; subordinate participants do not become owners | Candidate / unvalidated |
| REQ-WL-003 | PR state is advisory and never triggers Close | Candidate / unvalidated |

## Required Behavioral Acceptance

These are acceptance obligations, **not tests reported as passing**:

| Scenario | Required evidence |
|----------|-------------------|
| Normal clean Close | Fresh reconstructibility/identity proof, confirmed shutdown, exact successful disposition, atomic History outcome |
| Unique or uncertain work | Preserve resources; exact inventory and concrete discard proposal/decision; no inferred deletion authority |
| First failure before confirmed shutdown | No subsequent Close effect; Open `CloseIncomplete`; exact durable failure and mandatory Global event |
| Cleanup failure after confirmed shutdown | No subsequent Close effect; History `cleanup_attention`; retained residuals; no false cleanup-success message |
| Crash and startup | Read-only observations and truthful shutdown classification only; no settlement/cleanup/retry dispatch |
| Delayed progress or duplicated command | Stopped run cannot dispatch; stale ordinal cannot advance a fresh run; no duplicate failure event |
| Duplicate/concurrent/lost-response `close_conversation` | Same retained request, attempt, run, and result; no new topology or implicit confirmation |
| Automatic completion retry | Exact latest stopped ordinal >1; typed `AttemptInterrupted` primary / zero resource-failure children; nonempty successful plan or positive verified-completion chain; one atomic no-resource successor; no initial-run bootstrap, startup dispatch, or replenishment |
| Suppression/finalization race | Pre-state captures automatic and explicit successors; suppression wins: stopped successor plus exact primary/resource children/outbox independent of worker callback, no refund; finalization wins: truthful outcome/attention clear and conditional History publication; no stranded Running or stale callback mutation |
| Unwatched source / source enters History | Mandatory Global delivery survives watch absence and lifecycle change; exactly one event per failure |
| Safe explicit retry | Fresh resolved-precondition proof, new ordinal under original authority, exact remaining effects only, old stopped evidence unchanged |
| Scope-only exact retry | Persist only pending exact WorkScope effect after prior tmux/worktree successes; recheck sealed original authority/no live owner or resource, own exact outcome, no generic fallback or false zero-effect completion |
| Explicit-successor cancellation | Same-owner terminal cancelled outcome/failure/outbox plus Open+confirmed→History/attention, Open+uncertain→Open/CloseIncomplete, or preserved History; publish only actual entry and never await a later failure classifier |
| Unresolved / risky retry | Reject without effects; unique discard, changed risk, expanded targets, or DB surgery requires separate concrete proposal |
| Retry fails again | New stopped run, distinct run-bound failure, its own once-per-failure Global event |
| Reused tmux socket / worktree path / missing live permit | Preserve unproven resource; no PID/path-based authority or fabricated shutdown |
| Branches, PRs, recovery artifacts | No deletion or other ref/PR mutation, and no automatic branch/tag/stash/patch/snapshot creation |

## Normative Authority

`requirements.md`, `work-lifecycle.allium`, `specs/bedrock/requirements.md`, and `specs/bedrock/bedrock.allium` define Close behavior. `specs/git-repository/git-repository.allium` defines retained restart observations. ADR-088 records the historical stop-on-failure policy; proposed ADR-245 narrowly qualifies its explicit-only retry language. ADR-026's authority separation and ADR-040/042/080's retained identity/private-namespace safety remain relevant within that bounded policy.

## Validation Notes

### Terminal lifecycle versus Close retirement

Generic terminal lifecycle notification retires pending fork proposals and broadcasts terminal state; it carries no worktree-retirement authority. `terminal_exit_preserves_clean_worktree`, `terminal_exit_preserves_inherited_worktree_and_unique_material`, and `terminal_exit_preserves_nested_descendant_path` cover clean, tracked-dirty, untracked, and nested descendant-path preservation. Destructive retirement from a live `ProductConversation` remains under admitted Close or explicit hard-delete. Reclamation classifies ownership from the ordinary aggregate rather than transcript terminal state: Open and Close-fenced aggregates are retained, and History remains retained while a Close recovery projection is active. `preserves_clean_terminal_scope_while_product_conversation_is_open` and `preserves_history_scope_while_close_recovery_remains_active` cover those boundaries. Startup reclamation may remove a safely inspectable orphaned worktree, but it preserves the Git branch reference as required by `REQ-PROJ-015`.

### Inventory preparation versus host cleanup

`prepare_retirement_captures_and_seals_one_exact_shared_worktree` exercises the production first-pass inventory preparation boundary using retained SQLite identities, without a Git repository, watcher shutdown, or filesystem deletion. It replaces `archive_chain_captures_first_pass_close_inventory`, whose inventory assertion depended on full host cleanup succeeding. The regression checks one exact target for a three-member shared chain, retained snapshot, repeat preparation, admission fences, and no retirement evidence or lifecycle transition from capture alone.

Aggregate archival remains covered by `archive_chain_handler_completes_one_aggregate_close`. Runtime cleanup-failure finalization, outbox rollback, and process-step failure ordering have separate deterministic tests; the inventory test does not duplicate those contracts. Git observation and exact-object deletion adapter tests remain separate coverage.

### Safe-retry admission versus the live writer probe

The exact safe-retry admission boundary rejects a fresh run (allocating no new run ordinal) exactly when the retry writer probe reports an external writer or fails to observe a candidate, and admits only when every candidate is absent or carries no positive writer evidence. `validate_close_safe_retry` runs this probe before `admit_close_safe_retry`, so a probe rejection short-circuits ordinal allocation.

`probe_retry_worktree_writers` now threads the per-candidate observation (existence plus `quarantine_has_external_writer`) through an injectable closure consumed by the pure decision `probe_retry_worktree_writers_with`. Production passes the real observation; the decision is unchanged (same candidate order original -> quarantine -> tombstone object, same existence gate, same probe, same rejection message).

`retry_writer_precondition_rejects_when_a_target_holds_a_positive_writer`, `retry_writer_precondition_rejects_when_observing_a_target_fails`, and `retry_writer_precondition_accepts_when_no_target_holds_a_writer` prove the decision over controlled observations with no filesystem or `/proc` access. The helper is a writer precondition only: it decides whether a retry writer is proven absent and does not itself allocate or admit a run ordinal, so the test names and assertions say "precondition accepts/rejects", not "admits a run". Ordinal allocation is covered by the separate DB admission tests. These three tests replace `safe_retry_writer_rejection_does_not_allocate_an_ordinal`, which scanned the host `/proc`, held a real descriptor into the captured worktree, and ran real Git plus real deletion retry to observe the ordinal effect.

### Durable writer-evidence candidate (REQ-WL-002e)

Migration 153 adds normalized immutable `close_writer_observations` child rows. `Database::record_close_writer_observation` binds each row to the running admitted ordinal, captured scope, inspection snapshot and exact resource. Identical replay is a no-op; conflicting replay, UPDATE, DELETE and duplicate INSERT OR REPLACE are rejected, including with recursive triggers disabled. `CloseWriterProbe::retain` persists a blocking observation before the admitted cleanup adapter returns it; evidence-write failure preserves the target. Pre-admission safe-retry probing remains read-only.

Deterministic coverage in source (hosted execution; terminal-green exact-source receipts required in PR #1043):

- `writer_observation_is_normalized_immutable_idempotent_and_run_bound` includes typed positive/gap round trips, scope filtering and non-UTF-8 Unix paths. The retry test below also reads each PID incarnation under its own admitted run.
- `writer_observation_replacement_is_rejected_with_recursive_triggers_disabled` and `writer_observation_retry_binds_admitted_ordinal_and_retains_reused_pid_incarnations` cover immutable evidence, fresh ordinal 2 bindings, stale/cross-bound rejection and PID reuse.
- `linux_descriptor_scan_classifies_file_modes_and_directory_authority` separates read-only regular files from writable files and directory authority. `linux_readonly_descriptor_position_changes_are_not_writer_gaps` varies only fake fdinfo position.
- `linux_descriptor_scan_retains_inventory_and_descriptor_gaps`, `cwd_and_mapping_scans_fail_closed_for_same_user_gaps`, and `linux_pid_reuse_during_observation_retains_gap` cover incomplete attributable inspection and changing process incarnation through fake process directories.
- `macos_process_inventory_failures_are_absent_evidence` exercises failed, zero and malformed injected inventory returns; the detector treats unavailable inventory as a blocking inspection gap.

Production probes use process-incarnation and descriptor-identity checks. Unavailable inventory and same-user malformed/inaccessible evidence fail closed; unattributed ambient access denial is not positive writer evidence. This corrects the earlier observational-only description. `CloseWorktreeEffects` keeps default adapter tests off live host scans and daemon shutdown; production uses the real functions with a bound evidence-retention probe.

Qualification limits: hosted CI has executed Rust tests, but failed predecessor runs do not qualify the final candidate. The empty-but-live History regression accepts mandatory failure delivery before staging empty membership and rejects direct evidence deletion while the product survives. Public permanent Delete separately proves pending-event custody fences deletion, a failed or non-final deletion retains captured scopes and observations, and canonically settled final deletion cascades them with the product. The two fake-tmux identity lifecycle fixtures use per-manager test-only clean writer observations rather than ambient host scans; production scanning and evidence retention remain fail-closed. Exact-source hosted CI and independent exact-head review are required before merge-ready handoff, with final SHA/run receipts tracked in PR #1043. No live-host scan qualifies this candidate. A delegated `./dev.py check --lanes spec-shape,spec-anchors` preflight unexpectedly launched a dev.py unit-test subprocess that ran `cargo build --bin phoenix-ide` despite the local no-native-build gate. After verifying the isolated-worktree cwd and parent chain, the executor terminated only the owned process groups 20568 and 20533 with SIGTERM and verified both groups empty; no successful native result is claimed. Only the observed spec-shape artifact check passed; anchors remained interrupted/unverified. No local dev.py/Cargo lane is to be retried under this gate. Real Linux mappings/cwd/descriptor round trips and macOS proc syscalls remain unqualified by these deterministic tests. No browser API or UI is added. `Database::list_close_writer_observations(run, scope)` provides the typed DB read surface using the existing request type, exact retained bindings and strict decoding, including lossless Unix native paths; no runtime consumer is added. Migration/SQL checks and spec parsing are reported separately from whole-repository acceptance.

### Process sequencing properties

The seven `process_step_*` tests execute the same `run_close_process_steps` runner
used by `complete_close_resource_lease`, with controlled typed effect observations
and real in-memory Phoenix persistence. They replace the fixture that launched
`sleep` process groups and a shell PTY solely to manufacture ordering failures.
The tests assert exact effect order, first-failure stopping, persistence stopping
later dispatch, successes surviving a later scope failure and outbox rollback,
rejected observations producing no receipt, and a complete second pass dispatching
nothing. Bash/terminal registry adapter tests own generation and process-identity
validation; the scripted runner tests do not claim to re-test OS signaling.

Git inspection adapters retain local temporary repositories where needed to
produce evidence about staged, ignored, hidden and detached work. The ignored
nested-submodule and detached-submodule tests no longer require a final successful
deletion: observation of unique work is their property. Descriptor-relative
object-swap/deletion adapter tests remain local-file tests, separate from lifecycle.

### Narrow cleanup adapter boundary

Retained quarantine adapter tests supply explicit daemon-stop and writer-observation
results through `CloseWorktreeEffects`. Production binds the live functions and
retains admitted-run blocking evidence before returning it. This keeps local Git/descriptor-relative filesystem mechanism tests
without global host scans or daemon shutdown. A positive writer observation or
probe error is explicitly tested to preserve the quarantined bytes and Git admin
entry before the deletion/tombstone step. The same adapter tests retain original-
path recreation, changed-snapshot and exact bare-common-root assertions. The
source-text assertion forbidding the literal `lsof` command was removed: it did
not prove writer-detection correctness.
