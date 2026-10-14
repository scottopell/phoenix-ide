# Work Lifecycle — Executive Summary

## What This Spec Covers

Explicit Close for Git-backed ProductConversations: exact loss inspection, reconstructible-only automatic deletion, owned-resource shutdown and retirement, failure visibility, mandatory Global delivery, and fresh safe retry under the original Close authority. Branches and pull requests are never Close-owned artifacts.

## Current Reality

**Candidate / implementation acceptance unvalidated.** ADR-088 supersedes automatic Close recovery, same-execution retry, and the rule that all cleanup failures must remain Open. This spec slice records the settled contract; it does not certify a source candidate, migration, deployment, or live behavior. Historical green checks under the superseded contract are not acceptance evidence for this contract. No production cleanup or database repair is authorized by this document.

Lifecycle ended and resources retired are separate facts. Confirmed conversation **and** owned-process shutdown permits read-only History with conspicuous `cleanup_attention` after cleanup failure. Uncertain shutdown stays Open with `CloseIncomplete`. A stopped execution is never resumed; startup observes only. Explicit user or Global safe retry allocates a new durable run ordinal under the original Close authority after fresh safety proof. Changed risk, unique/uncertain discard, expanded effects, or database surgery requires a concrete proposal and separate approval.

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
| Unwatched source / source enters History | Mandatory Global delivery survives watch absence and lifecycle change; exactly one event per failure |
| Safe explicit retry | Fresh resolved-precondition proof, new ordinal under original authority, exact remaining effects only, old stopped evidence unchanged |
| Unresolved / risky retry | Reject without effects; unique discard, changed risk, expanded targets, or DB surgery requires separate concrete proposal |
| Retry fails again | New stopped run, distinct run-bound failure, its own once-per-failure Global event |
| Reused tmux socket / worktree path / missing live permit | Preserve unproven resource; no PID/path-based authority or fabricated shutdown |
| Branches, PRs, recovery artifacts | No deletion or other ref/PR mutation, and no automatic branch/tag/stash/patch/snapshot creation |

## Normative Authority

`requirements.md`, `work-lifecycle.allium`, `specs/bedrock/requirements.md`, and `specs/bedrock/bedrock.allium` define Close behavior. `specs/git-repository/git-repository.allium` defines retained restart observations. ADR-088 records the superseding policy; ADR-026's authority separation and ADR-040/042/080's retained identity/private-namespace safety remain relevant within that bounded policy.

## Validation Notes

### Inventory preparation versus host cleanup

`prepare_retirement_captures_and_seals_one_exact_shared_worktree` exercises the production first-pass inventory preparation boundary using retained SQLite identities, without a Git repository, watcher shutdown, or filesystem deletion. It replaces `archive_chain_captures_first_pass_close_inventory`, whose inventory assertion depended on full host cleanup succeeding. The regression checks one exact target for a three-member shared chain, retained snapshot, repeat preparation, admission fences, and no retirement evidence or lifecycle transition from capture alone.

Aggregate archival remains covered by `archive_chain_handler_completes_one_aggregate_close`. Runtime cleanup-failure finalization, outbox rollback, and process-step failure ordering have separate deterministic tests; the inventory test does not duplicate those contracts. Git observation and exact-object deletion adapter tests remain separate coverage.

### Safe-retry admission versus the live writer probe

The exact safe-retry admission boundary rejects a fresh run (allocating no new run ordinal) exactly when the retry writer probe reports an external writer or fails to observe a candidate, and admits only when every candidate is absent or carries no positive writer evidence. `validate_close_safe_retry` runs this probe before `admit_close_safe_retry`, so a probe rejection short-circuits ordinal allocation.

`probe_retry_worktree_writers` now threads the per-candidate observation (existence plus `quarantine_has_external_writer`) through an injectable closure consumed by the pure decision `probe_retry_worktree_writers_with`. Production passes the real observation; the decision is unchanged (same candidate order original -> quarantine -> tombstone object, same existence gate, same probe, same rejection message).

`retry_admission_rejects_when_a_target_holds_a_positive_writer`, `retry_admission_rejects_when_observing_a_target_fails`, and `retry_admission_admits_when_no_target_holds_a_writer` prove the decision over controlled observations with no filesystem or `/proc` access. They replace `safe_retry_writer_rejection_does_not_allocate_an_ordinal`, which scanned the host `/proc`, held a real descriptor into the captured worktree, and ran real Git plus real deletion retry to observe the ordinal effect.

### Writer-evidence scan coverage after live-test removal

The external-writer scans (`quarantine_has_open_descriptors`, `quarantine_has_process_cwd`, `quarantine_has_writable_mappings`) are inspected deterministically through their fake-`/proc` `_in` variants and pure parsers; the live host-scan tests were removed from the default module. The decision these scans feed is proven by the admission-seam tests above. Old-to-new mapping and retained dependencies:

| Removed live test | Behavior it exercised | Retained deterministic coverage | Coverage gap (reported, not faked) |
|---|---|---|---|
| `mapping_scan_detects_writable_shared_mapping_after_descriptor_is_closed` (linux) | real `mmap` MAP_SHARED writable mapping detected via real `/proc/self/maps` after the fd closed | `mapping_scan_ignores_ambient_unreadability_and_finds_readable_writer` (parses `rw-s` maps line, path-within match over fake `/proc`) | live `mmap` -> real `/proc/<pid>/maps` round trip no longer qualified in-suite |
| `quarantine_detects_external_process_working_directory` (linux + macos) | real child process cwd within quarantine returns positive | linux: `cwd_scan_ignores_ambient_unreadability_and_finds_readable_writer` over fake `/proc`; macos: none | linux live `/proc/<pid>/cwd` round trip and the macos `proc_pidinfo(PROC_PIDVNODEPATHINFO)` syscall path are unqualified in-suite |
| `quarantine_preserves_worktree_with_open_file_descriptor` (unix) | real open fd makes `quarantine_and_remove_exact_worktree` return `Residual` with writes landing in quarantine | `descriptor_scan_ignores_ambient_failures_and_finds_readable_writer` (fd symlink within quarantine -> positive over fake `/proc`) plus the admission-seam tests | end-to-end residual-on-open-descriptor path of `quarantine_and_remove_exact_worktree` is unqualified live |
| `cwd_scan_treats_same_user_nondumpable_process_as_observational` + its `linux_nondumpable_process_helper` (linux) | a same-user nondumpable process is attributed `Relevant` from kernel credentials, and its cwd unreadability stays observational | `linux_status_parser_rejects_missing_malformed_and_ambiguous_credentials` (credential parser) plus the fake-`/proc` cwd/mapping scan tests | live `PR_SET_DUMPABLE` credential-attribution round trip is unqualified in-suite |

Retained adjacent coverage unaffected by the removal: the descriptor-relative object-swap adapter tests, the Git evidence-generation adapter tests, and the fake-`/proc` canonicalization/no-evidence tests (`cwd_and_mapping_scans_treat_unavailable_proc_inventory_as_no_evidence`, `cwd_and_mapping_scans_keep_quarantine_canonicalization_authoritative`). Fail-closed semantics for invalid or unreadable evidence are unchanged: ambient `/proc` read errors remain observational (no proof of a writer), while a probe error at the admission boundary rejects the retry.

These gaps are a deliberate choice to drop live-syscall qualification from the default gate rather than assert fake equivalence for it. They are a coverage gap, not a behavior change; the production scans and their platform syscalls are unchanged.

The inventory and existing focused runtime tests above do not establish acceptance of the entire matrix. Targeted test and spec-check results must be distinguished from whole-repository qualification, UI acceptance, production deployment, and migration acceptance.

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
