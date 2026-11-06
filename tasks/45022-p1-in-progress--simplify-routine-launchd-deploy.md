# Remove full-database scans from routine launchd deployment

## Observed journey and evidence

On 2026-11-05, ordinary local-HEAD transaction `goals152-9c5d471d-ess-v1` stopped production at 19:16:22 EST and committed at 19:36:43 EST. Cloudflare reported a Host Error while the origin refused port 8031. The installed candidate was commit `9c5d471d63b4ceb51bd43b3336c0b9f9db53e9c3`, version 0.13.1; local identity returned HTTP 200 after commit. Live process samples during the outage showed the independent Python helper reading SQLite. The production database was about 10 GB.

Audit the deployed commit or its successor, not this checkout's older main (27c514034). In the deployed helper's normal pending-migration success path:

- `activate` calls `ordinary_forward_plan` after stop and again before the invocation marker.
- Each plan runs `ordinary_forward_admission_plan` (full integrity and foreign-key checks) and `database_logical_digest` (every row, serialized and hashed in Python).
- `recover_ordinary_boundary`, reused for initial activation, runs another integrity/foreign-key check and a third full logical digest before invoking the candidate.
- `prove_ordinary_migration` runs another integrity/foreign-key check after migration.

These repeated scans explain expensive work on the outage path. Their individual durations and the migration's own duration were not instrumented, so no precise scan/migration time split is claimed. The helper is 5,418 lines at that commit versus 575 in this checkout.

## Owning contract and proposed bounded scope

Prepare one deletion-focused change to ordinary macOS local-HEAD/release deployment. Preserve the independent launchd-owned helper, serialized operation ownership, immutable candidate/configuration identity, atomic artifact installation, durable migration-invocation checkpoint, exact runtime health verification, and prohibition on predecessor startup after candidate mutation may have begun.

Remove ordinary deployment's whole-content database digest and repeated global integrity/foreign-key scans as mandatory admission and activation work. Keep bounded migration-ledger validation and backend stop/ownership checks. Put exhaustive integrity verification in an explicit diagnostic/maintenance operation rather than silently doing it during every deploy. Preserve the rule that unsupported independent database writers are not a compatibility obligation. An ambiguous migration attempt remains forward-only and does not authorize a predecessor restart.

Review and update normative requirements/Allium with the change and record a new ADR superseding ADR-190's content-digest decision; do not silently optimize around the old contract. Starting authorities are REQ-LDD-028, REQ-COMP-019, REQ-COMP-002–004, ADR-034, and ADR-190. Existing ADRs remain historical records.

## Interaction map

`dev.py` stages candidate/configuration -> independent helper owns stop and activation -> exact candidate `--migrate-only` owns schema writes -> launchd starts candidate -> `/api/version` verifies installed identity -> durable commit releases operation ownership. Interrupted or failed migration retains exact-candidate forward recovery.

## Acceptance evidence

- Real file-backed tests prove correct pre-invocation failure behavior and prohibit old-binary startup after failed, partial, or interrupted candidate invocation.
- An ordinary success-path regression rejects calls to logical digest and global integrity/foreign-key scans; artifact SHA-256 verification remains allowed and required.
- Exercise existing admission, recovery, configuration-preservation, and health-verification coverage through dev.py.
- Validate the complete path using the disposable launchd harness, including failed candidate health. Measure time from old service stop to verified candidate readiness with representative data, without touching production for a benchmark.

## Risks, unknowns, and exclusions

Changing digest-based recovery evidence is a policy change and must address old unresolved transactions explicitly. Do not mutate retained helper/manifest evidence to fit the new protocol. Historical completed transactions may remain inspectable without re-execution.

No Linux redesign, new controller/store/daemon, automatic database rollback, backup-retention subsystem, Rust controller adoption, or broad deployment rewrite. Schema-specific protected routes are a separate KEEP/CUT support decision; inventory their actual remaining operators and unresolved transactions before proposing retirement. Python parsing of Rust migration source is another simplification candidate, not part of this first change.

## Approved scope — 2026-11-06

User: "This is my desired tradeoff, adopt my old philosophy and aggressively delete and simplify please" referring to: "The candidate may have changed the database. I stopped. Here is what ran and failed; recovery requires an operator."

This supersedes the narrower proposal above. Remove automated macOS rollback and retained recovery commands, protected historical schema routes, exhaustive DB scans and registry parsing. Keep one independent ordinary activation helper, artifact/configuration validation, serialized deployment/restart ownership, exact health verification, and actionable failure/stop diagnostics. Retain existing transaction files on disk as operator evidence without teaching the new helper to replay old protocols. Remove the unused standalone deployment controller and its dedicated CI rather than migrating deployment into it. Keep functioning Linux backends intact; remove obsolete macOS-specific options and adapters. No production activation is part of this source-change task.

Implementation sequence: simplify helper and regression contract; simplify installer/callers and retire old-path tests with coverage accounting; remove unused controller; reconcile specs/operator guidance; run focused tests, disposable launchd journey, and relevant dev.py check lanes; review and commit.

## Implementation and qualification

Implemented one protocol-2 ordinary macOS helper with candidate-owned migrations, no database scans or automatic recovery, explicit failed-phase/stop diagnostics, manual ownership resolution, same-inode plist publication, and best-effort success cleanup. Removed the unused 54-file Rust controller and dedicated CI/prepared validation tooling, with Cargo-managed unused dependency removal. ADR-230 and the current requirements/Allium/operator guidance withdraw the old contracts and account for retired tests. Linux activation contracts are preserved.

Focused verification passes: helper 25, installer 60, restart 38, systemd 32, bare Linux 21, harness safety 5, ReleaseUpdatePanel 32, UI lint, Cargo offline locked metadata, formatting, Allium, spec shape and requirement anchors. The 128 helper/installer/restart/harness-safety tests and four real disposable launchd scenarios pass after the review fixes. Full local TypeScript checking is blocked by older existing dependencies missing patched Markdown modules. The broader dev.py Python suite generated approximately 1.5 GiB of build output and exhausted local disk before its result could be reported; only this worktree's generated target output was removed. Full hosted qualification remains pending.

Independent review of `9c5d471d63b4ceb51bd43b3336c0b9f9db53e9c3..20eb5f5fbb921fce72d992082d26df1a017b826f` found three actionable defects: the Rust test still called retired Python admission APIs, late success-reporting errors could stop a committed service, and the release API did not recognize the new helper's handoff states. All three are corrected, with postcommit failure and handoff regression coverage. CI independently reproduced the retired Rust-test dependency and a macOS workflow reference to the deleted Python test module; both consumers are reconciled. The unchanged migration data/constraint and lineage tests remain. PR: https://github.com/scottopell/phoenix-ide/pull/1034.

The anchored final review of `9c5d471d63b4ceb51bd43b3336c0b9f9db53e9c3..b52ac478eccd14aa2292b123dcafef72f70cbf9e` found two remaining consumer defects: local-HEAD launchd failures were filtered out of the in-app recovery/approval guard, and confirmation/progress copy still mentioned automatic rollback/reconnection. Both are corrected narrowly, with a temporary-home Rust regression and updated UI assertions. The 32 panel tests, targeted lint, formatting and diff checks pass. The full macOS packaging/deployment workflow passed at b52ac478e; final exact-commit hosted CI remains pending. No further helper/runtime changes were needed.

Merged main `34a096e1e8c8f429cd1811010c72895f6c7cab6d` into the PR branch to resolve the compatibility executive conflict, retaining the new compiler-cache doctor status alongside the simplified deployment contract. Renumbered this unmerged deployment ADR from 229 to 230 because main published Ember as ADR-229; both records and their references remain distinct. Main's local-check, compiler-cache and SQLite profile changes remain intact. Qualification of the combined tree passes 307 focused Python tests, formatting, Allium, requirement anchors, task validation, and spEARS/ADR shape validation. Hosted CI must qualify the new merge commit; no production operation was performed.

During an initial retained old unit-test run, a stale test assumption escaped to live service stop at 02:20:53 EST on 2026-11-06. The same installed binary and configuration were restored; their hashes matched, and exact-identity HTTP 200 returned at 02:24:31. The installer tests now use temporary HOME/production paths and refuse every unmocked subprocess. Subsequent unit/harness checks did not touch production. No new source has been deployed. Regenerable unused browser/compiler cache entries were evicted during disk recovery; production data, backups and worktrees were preserved. Ongoing disk-pressure work is tracked separately by existing task 06012.
