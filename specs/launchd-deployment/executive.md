# Native macOS launchd Deployment — Executive Status

## Current reality

Native macOS production deployment prepares local `HEAD`, a checksummed published release, or the explicitly selected protected prepared-artifact paired source, requires the candidate's complete 40-character embedded SHA to equal the selected source commit, stages rollback inputs and a redacted manifest, and transfers activation to a distinct one-shot LaunchAgent. The helper serializes activation, performs atomic replacement, requires exact `/api/version` identity, and attempts verified runtime-artifact rollback on failure. A captured already-installed predecessor may retain a legacy 12-character identity only for that rollback verification. Durable status is reported by `./dev.py prod status`.

Installed-runtime restart is a separate launchd-owned transaction. It sends SIGHUP without unloading the socket-activated target, verifies a new PID with the same exact identity, preserves the binary, plist, environment, listener, and deployed SHA, and reports its own durable status without replacing deployment status.

Ordinary rollback restores runtime artifacts and service state, not the SQLite database. It therefore does not guarantee that a restored older binary can use data migrated by the failed candidate and does not provide general downgrade compatibility.

Live production deployment remains an explicitly gated operator action; automated validation uses disposable resources.

The prepared-artifact paired path is deliberately narrower than ordinary release deployment: the prepared standalone binary remains byte-identical and is only strict-codesign checked (not ad-hoc resigned), while the clean controller `HEAD` supplies the helper. Its private transaction directory retains the SQLite backup/proof and active claim for manual recovery; snapshot, open-file, path, ledger, or health proof failures fail closed rather than silently starting an unproven runtime.

## Paired prepared-artifact surface

- `./dev.py prod deploy --prepared-artifact DIR --expected-full-commit SHA --paired-database-upgrade` is macOS launchd-only and rejects release aliases, first install, or a mismatched installed database path.
- The receipt must identify the exact host-target standalone basename, submission UUID, full SHA, Developer ID/hardened/timestamp codesign evidence, and qualified notarization/ticket/Gatekeeper/helper evidence. Standalone `spctl` is not imposed because notarization may be stapled only to the containing app.
- Recovery is fail-closed: do not delete the paired transaction or claim, manually inspect the retained proof and run offline SQLite integrity checks before restoring/starting the predecessor.

## Paired interruption and ownership table

| Boundary | OS / filesystem state | Supported outcome and ownership |
| --- | --- | --- |
| Preparing, before handoff (manifest absent or persisted) | Predecessor unchanged; matching status/PID precede claim | Dead preparation PID plus confirmed helper absence permit terminal status then owned-claim release; missing evidence/live or reused PID refuses |
| Helper admitted, before stop | Predecessor runs; private snapshot and DB-adjacent restore capacity physically allocated | Capacity failure reports precondition failure without disruption |
| Stopped, before snapshot proof | Target service absent; predecessor plist quarantined; binary unchanged | Exclusive legacy DB plus captured binary/config hashes allow private predecessor resume; no DB restoration claimed; release unproven temporary allocations after verified terminal resume. Unknown ownership or modern DB stays stopped/fenced |
| Snapshot verified, candidate installed/started | Matching snapshot/proof retained; candidate bootstraps private plist; no auto-loaded target plist | Install/start/health interruption retains claim and audit proof; login cannot discover an unverified candidate plist |
| Failed candidate, paired restore | Service-target teardown; exclusive offline snapshot restore before predecessor binary/start | Private predecessor bootstrap; publish matching plist only after predecessor identity/deployed SHA; failed proof attempts teardown and retains claim/quarantine |
| Verified predecessor, terminal persistence interrupted | Running predecessor and durable recovery checkpoint; claim remains, even if the terminal rename is readable | Subsequent deploy/restart cannot infer claim release. `prod recover-paired` verifies running predecessor/private configuration and republishes status only, without stop/start or snapshot replay |
| Exact candidate durably committed | Candidate identity and deployed SHA verified; commit durable before plist publication | Persist pending publication/cleanup warning and retain claim through both attempts. Release only restore reservation, retaining backup/proof; publish prepared candidate plist last preserving the private bootstrap inode for strict restart verification. Cleanup/publication error is a displayed committed warning, never rollback; missing publication does not prove reboot persistence |
| Terminal failed paired status | Recovery claim unresolved | `prod status` always presents `prod recover-paired` guidance, never generic marker removal |

Focused regression coverage includes real launchctl command shapes, both exact
missing-service spellings, wrong-target and I/O refusals, private bootstrap,
BaseException interruption at install/start/health, capacity-growth unchanged
resume, unsafe no-snapshot refusal, preparation ownership, SQLite interpreter
probes, and reservation-only cleanup. These controller refinements are local/CI
qualification; the devmbp deployment receipt below used an earlier controller
and is not live verification of these refinements.

## Main implementation gap

The accepted paired contract (REQ-LDD-017 / REQ-LDD-018) and behavioral model are normative,
but their controller implementation is not yet shipped on `main`. PR #836 owns
prepared receipt admission, private paired SQLite recovery, claim/status behavior,
and regression qualification. The documentation prerequisite PR #842 changes no
runtime. Until #836 lands, main does not implement these paired guarantees;
ordinary deployment/restart behavior is separate. Local/source evidence below
must not be read as main coverage or live verification of every refinement.

The committed finalization-only resumer is commissioned by Global as delegated
engineering scope, not a new quoted user release/deployment authorization.
It is qualified locally/CI only; no healthy production finalization was executed.

## Requirement coverage


| Requirement | Implementation / verification |
| --- | --- |
| REQ-LDD-018 | Finalization-only retry / no-runtime-no-DB fault and replay regressions; not shipped on main until #836 |
| REQ-LDD-001 | `_helper_plist`, `launchd_prod_deploy`; disposable integration harness |
| REQ-LDD-002 | `launchd_prod_deploy`, `_binary_identity`; preparation tests |
| REQ-LDD-003 | `_claim_launchd_deploy`, helper `flock`; concurrent-deploy tests |
| REQ-LDD-004 | helper `Manifest`; secret-safe metadata test |
| REQ-LDD-005 | `atomic_install`; atomic install test |
| REQ-LDD-006 | `Launchctl.stop`, `Launchctl.start`; transition tests |
| REQ-LDD-007 | `wait_for_identity`; candidate preparation and runtime verification require full-SHA equality rather than prefix matching |
| REQ-LDD-008 | `restore`, `activate`; rollback verification accepts only the captured predecessor's 12- or 40-character lowercase identity and reports runtime-artifact rather than database rollback |
| REQ-LDD-009 | `write_status`, post-verification `deployed.sha`; success test |
| REQ-LDD-010 | `launchd_prod_status`; stale-status test |
| REQ-LDD-011 | `_prepare_release_candidate`, `prod_build`; local and release candidates embed the exact selected 40-character source commit |
| REQ-LDD-012 | `main` prod parser; positional rejection test |
| REQ-LDD-013 | `tests/integration/launchd_deploy_harness.py`; macOS-gated harness |
| REQ-LDD-014 | `launchd_prod_restart`, `launchd_restart_helper.restart`; unit tests and disposable launchd restart journey |
| REQ-LDD-015 | `_claim_launchd_restart`, `/api/version` socket-activation report, restart helper LaunchAgent handoff; runtime-activation, mutual-exclusion, and secret-redaction tests |
| REQ-LDD-016 | `launchd_restart_helper.wait_for_identity`, restart status; restart requires an installed full-SHA identity and does not reuse the predecessor-only 12-character rollback allowance |
| REQ-LDD-017 | `_prepare_prepared_artifact`, paired structural manifest, SQLite backup API, legacy ledger/table preflight, and fail-closed proof validation; focused helper tests cover ordinary activation regression |

## Operator surfaces

- `./dev.py prod deploy` — checked local `HEAD` build.
- `./dev.py prod deploy --release vX.Y.Z` — exact published release.
- `./dev.py prod deploy --release latest` — latest stable release resolved once.
- `./dev.py prod status` — launchd PID/runtime identity and durable transaction result.
- `./dev.py prod restart` — restart the installed process in place without rebuilding or changing installed configuration.

## Exact-source devmbp upgrade receipt

On 2026-10-04, transaction `20261004T185609Z-8f1cdf96` committed the protected
prepared candidate `f5f98d7b2e51b112157e12c2b23d62c1a3c58d71` on devmbp, using
controller `a31cddc3c0165606bb98bbb0f62fe8dfea2999a1`. Authenticated `/api/version`
reported the exact full candidate SHA and `socket_activated=true`. Installed
environment equality was verified against the predecessor snapshot; migration
69 advanced to 112 while retaining 1009 physical conversation rows and all prior
visible conversation IDs/working directories. A verified private matched
predecessor database/binary/plist proof is retained server-local; no backup data
is published. Activation succeeded, so production rollback was not exercised.
Disposable failure/restore fixtures and 143 deployment/restart tests passed on
both hosts. Owner turn resumption remains separately verified; idle projection
does not prove provider rejection was resolved.
