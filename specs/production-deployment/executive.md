# Cross-Platform Production Deployment — Executive Status

## Current reality

The shared contract covers launchd, systemd, and bare Linux with common candidate preparation and backend-owned activation. Native macOS launchd uses a distinct one-shot LaunchAgent. Linux systemd uses a validated root-owned transaction and transient activation unit. Bare Linux uses a persistent same-user supervisor with an owner-only Unix socket and direct Phoenix child ownership. All three backends require a modern candidate's complete 40-character embedded SHA to equal its selected source commit, provide immutable handoff, atomic installation, durable status and claim fencing, and verify runtime-artifact rollback. A captured already-installed rollback predecessor may retain a legacy 12-character identity, but that acceptance does not extend to candidates, controllers, or general downgrade compatibility.

The Lima/VZ harness proves successful systemd activation and exact-identity rollback with real socket/service units, changed `MainPID`, truthful `deployed.sha`, terminal claim release, and survival after termination of the initiating SSH process group. It also verifies the bare-Linux transaction engine's direct child ownership, `/proc` start-time binding, exact identity, verified rollback, and child-only stop. Bare supervisor startup reconciles interrupted durable phases and re-establishes exact direct-child ownership after restart. Production-style detached-start acceptance verifies survival after launcher exit, socket-only commit and rollback, and child-only stop. Installation configures owner `@reboot` cron when available and otherwise emits exact same-user host rc guidance without claiming persistence. Systemd acceptance verifies committed runtime recovery across VM reboot with changed `MainPID`, exact identity, and unchanged durable status. Transaction journeys use the deterministic fixture runtime; a separately built aarch64 musl Phoenix binary has also been smoke-tested in disposable Lima with exact `--build-identity` and `/api/version` verification.

Ordinary rollback restores runtime artifacts and configuration without a database compatibility guarantee. The explicit prepared-artifact launchd ProductConversation upgrade is a narrow exception: a verified offline SQLite snapshot, binary, and configuration are restored together before predecessor startup; failed recovery retains its ownership fence and attempts teardown. On bare Linux, a running supervisor is also a persistent activation authority rather than a protocol-compatible replaceable controller: deployment fails before disruption when its installed artifact differs from the selected supervisor artifact, even if protocol versions match.

### Protected systemd published-release exception

REQ-PD-023–027 / REQ-COMP-014 and [ADR-116](../adrs/116_protect-exact-65-to-128-systemd-published-release-activation.md) define only the exact complete registry-prefix65 to fixed `v0.13.0-rc.4` target128 transition, with a separately explicit different controller commit and retained helper hash. The authored controller/regression implementation preserves installed env/service/socket/canonical DB path, requires an existing predecessor, stops both units and proves exclusivity, retains a pristine SQLite backup, and rehearses the exact candidate on an isolated backup copy before the durable live `migration_started` boundary. Rehearsal/capacity failure remains eligible only for unchanged-predecessor recovery; after the marker, only exact-candidate recovery from a same-registry prefix65..128 is admitted. The pristine backup remains after success. No launchd capacity floor is imported.

Persistent transaction-bound service/socket start inhibits are installed/fsynced/reloaded/verified before stop and survive reboot across disrupted pre-marker and unresolved post-marker states. Verified inhibit removal follows authorized unchanged-predecessor artifact restoration before restart, or immediately precedes exclusively verified exact-target candidate start; tamper fails closed. Status remains `activating` with marker/boolean evidence, not a separate `migration_started` state; protected pre-marker rollback failure is `activation_failed_rollback_failed`.

`recover-systemd-current` (helper `recover-current`) selects by marker presence. [ADR-117](../adrs/117_recover-interrupted-pre-marker-systemd-activation.md) adds explicit markerless recovery for interrupted `activating`/pre-marker rollback-failed transactions with exact ownership/status/manifest, intact inhibits and durable transaction-bound predecessor unit state. It stops both units, probes lsof, proves exclusive integrity/FK/source-prefix65 and compares any stopped snapshot proof. It restores/verifies predecessor runtime/config before inhibit removal, then restores/verifies active/enabled state and applicable identity before durable rollback status and exact claim release. It never enters candidate migration/start or replays a backup.

Pre-snapshot normal pre-stop writes are admitted, not compared with a stale preflight digest; partial/malformed snapshot or state evidence remains fenced. A crash before predecessor-state publication leaves `prepared`, no disruption and an explicit validated abandonment path. With a marker, recovery stops both units/proves inactivity before detailed dependency/manifest/evidence/DB validation, then permits exact-candidate-only forward recovery. Missing lsof in either recovery branch stays stopped/fenced. These are authored boundaries, not real-systemd/reboot acceptance.

Manual TLS checks include manual-only CA interpretation, service-readable/traversable cert/key/CA paths, Python SSL cert/key parsing and matching, and single-CA/capability validation. Resolved/probed OpenSSL verifies server purpose and the supplied chain/intermediates against only that CA. Activation revalidates TLS and resolves/probes exact usable lsof before inhibits/stop; exclusivity reuses it. OpenSSL/Python SSL versus candidate rustls/webpki acceptance is not runtime-qualified.

CA/dependency, five disrupted pre-marker crash windows, pre-publication abandonment, incomplete evidence, failed state restoration, snapshot mutation, installed/unlinked-missing-DB and true-first-install regressions are authored, not executed. Validation of this corrective slice is limited to the requested Python compilation and diff whitespace checks; no test/build/check/Allium or runtime/deployment acceptance is claimed.

The repaired ordinary systemd path rejects observed pending/divergent ledgers before handoff, while exact-target/no-pending ordinary activation remains. This is not a retrospective qualification of historical controllers. Global operation locking and staging-status-before-claim distinguish safely retireable partial staging from ambiguous/prepared/post-marker fences. These source changes and their regression coverage still require implementation validation and disposable Linux systemd qualification; this documentation does not claim either or a production deployment. The historical Lima acceptance above does not prove this protected migration transition.

Live production deployment remains an explicitly gated operator action; automated integration validation uses disposable resources.

The paired launchd controller privately bootstraps candidate/predecessor plists
and delays auto-load publication until their required verification boundary.
The prepared current-schema launchd contract admits either the exact migration-123 baseline or a source ledger exactly equal to the migration-128 candidate registry under [ADR-105](../adrs/105_qualify-migration-128-for-bounded-current-schema-activation.md) and [ADR-114](../adrs/114_admit-exact-target-ledger-for-prepared-current-schema-activation.md), including named migrations 126/127/128 and immutable complete registry/SHA-256 evidence re-verified before migration, activation, and forward recovery. It requires exclusive offline snapshot and mandatory candidate invocation for both sources, forbids predecessor rollback or snapshot replay after invocation (including partial or ambiguous writes), and exposes separate current-schema recovery/finalization commands while retaining every prepared-artifact provenance check. The 128-to-128 controller and regression changes are authored; hosted CI qualification is pending. No runtime or deployment acceptance is claimed.
[ADR-115](../adrs/115_protect-exact-114-to-128-published-release-activation.md) documents one additional protected launchd admission: a published release with exact immutable identity and target registry 128 from the exact complete candidate-registry prefix through 114. Its helper is retained with SHA-256 from a separately explicit controller commit, not the release commit; binary and registry remain release-sourced. It preserves installed configuration and reuses protected capacity, exclusive offline snapshot, durable `migration_started`, and candidate-only post-invocation recovery. Prepared 123/exact-target and ordinary 123-to-128 qualification are unchanged. Controller/regression qualification and runtime/deployment acceptance remain pending; this documentation does not authorize production execution. Ordinary unqualified pending migration intervals reject before disruption.

Ordinary in-place activation remains backup-free; the authored policy uses a conservative forward-only boundary for that same exact 123-to-128 interval: after candidate startup is attempted, failure retains the candidate and claim and forbids predecessor launch; `resume-forward` requires explicit operator authorization and immutable complete registry/digest verification. Hosted CI qualification is pending.

Pre-snapshot failures resume only an unchanged, exclusively owned legacy pair;
post-commit warnings do not authorize rollback or establish reboot persistence.
The lifecycle/OS/filesystem/claim boundary table is in
`specs/launchd-deployment/executive.md`. These refinements are qualification-only,
not a later live deployment on devmbp.

## Main implementation gap

The accepted paired contract (REQ-PD-018, REQ-PD-019 and REQ-PD-020) and behavioral model are normative,
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


| Requirement | Current implementation / verification |
| --- | --- |
| REQ-PD-023 | Authored contract and controller/helper pin exact RC4 tag/commit/version/musl asset/hash and migration-source hash/target128; require exact prefix65, existing predecessor, different controller commit/helper binding and installed configuration preservation. Qualification pending. |
| REQ-PD-024 | Authored persistent transaction-bound service/socket inhibit installed/fsynced/reloaded/verified before stop, reboot-safe fencing and tamper refusal; offline exclusive pristine SQLite backup plus isolated exact-candidate rehearsal/proofs, exact target/integrity/FK checks and backup retention. Stopped-snapshot authority includes writes during stop; pre-marker safe rollback and post-snapshot mutation refusal regressions authored, not executed. Regression/real systemd acceptance pending; no 20 GiB floor claimed. |
| REQ-PD-025 | Authored activating + marker/boolean boundary, pre-marker unchanged-only recovery/activation_failed_rollback_failed, tightly ordered inhibit removal and post-marker no restore/predecessor. Explicit markerless interruption recovery restores only retained unchanged predecessor state with exact prefix65/integrity/FK and any stopped snapshot equality; five disrupted-window regressions plus pre-publication abandonment/incomplete-evidence/state-failure cases authored, not executed. Marker-selected recovery stops both units before detailed validation/dependency probes, then exact retained candidate prefix65..128 only. Recovery durably re-enters activating/migration_started=true after detailed admission, including exact-target finalization; updated exact-target status/start-order oracle authored, not executed. Interruption/failure and exact-target finalization qualification pending. |
| REQ-PD-026 | Authored ordinary systemd preflight executes the exact candidate --migrate-only on an isolated empty DB to derive/verify target, checks unchanged candidate hash and production ledger/integrity/FK, and rejects before handoff on failure/pending/divergence. Missing DB with installed predecessor artifacts/identity now rejects before handoff and in the current helper; unlinked-open-DB and true-first-install fixtures authored. Protocol-v1 ordinary helper manifest unchanged; exact-target/true-first-install remain. No historical controller guarantee. Qualification pending. |
| REQ-PD-027 | Authored global-lock/staging-status-before-claim and exact partial-staging retirement; ambiguous/prepared/post-marker fences retained. Crash-window qualification pending. |
| REQ-PD-022 | ADR-115 documents pre-disruption refusal of ordinary unqualified pending intervals; qualification pending. Authored; hosted CI pending for exact 123-to-128 ordinary forward-only status guidance, immutable complete registry/digest and helper evidence, and explicit exact-candidate `resume-forward` routing |
| REQ-PD-021 | ADR-115 protected published-release exact-114-to-128 admission, separate controller pin and retained helper hash documented; qualification pending. Authored; hosted CI pending for mutually exclusive paired/current prepared flags, receipt and exact-commit preservation, prepared exact-123 or exact-128 source admission to the exact migration-128 registry with named 126/127/128 entries, mandatory offline candidate invocation, and mode-matched recovery/finalization regressions |
| REQ-PD-020 | Launchd-only finalize-paired routing / no bootstrap, captured helper and claim verification; not shipped on main until #836 |
| REQ-PD-001 | `detect_prod_env`; backend status naming requires normalization |
| REQ-PD-002 | typed local/release candidate preparation requires the complete 40-character embedded SHA to equal the selected full source commit across launchd, systemd, and bare Linux |
| REQ-PD-003 | all three backends have complete local/release preparation paths and focused tests |
| REQ-PD-004 | launchd one-shot helper, systemd root transient unit, and persistent same-user bare supervisor own activation |
| REQ-PD-005 | all three backends use immutable handoffs; launchd/systemd include secret-redaction coverage and bare IPC accepts only transaction identity and manifest hash |
| REQ-PD-006 | all three backends implement durable claim/status fencing and terminal claim release |
| REQ-PD-007 | systemd root staging validates fixed targets, ownership, modes, hashes, units, users, and symlink safety |
| REQ-PD-008 | all three backends reserve destination-filesystem rollback capacity and atomically replace installation artifacts |
| REQ-PD-009 | all three backends verify exact version and full-SHA equality with the selected commit; bare Linux also binds its direct child by PID and `/proc` start time |
| REQ-PD-010 | all three backends verify runtime-artifact rollback; predecessor validation accepts only captured 12- or 40-character lowercase identities and makes no database recovery or general downgrade guarantee |
| REQ-PD-011 | all three backends persist durable terminal status and truthful SHA with fenced claim release |
| REQ-PD-012 | ordinary deployment snapshots `.phoenix-ide.env`; legacy launchd JSON and systemd drop-ins are neither consulted nor migrated. Protected launchd/systemd modes preserve their captured installed configuration instead. |
| REQ-PD-013 | local/release deploy, durable status, and stop exist for all three backends; `prod set`/`prod unset` reject without mutation and direct operators to `.phoenix-ide.env` |
| REQ-PD-014 | persistent same-user bare-Linux supervisor owns the direct Phoenix child and reconciles interrupted durable phases; deployment refuses a changed running supervisor artifact before disruption rather than treating protocol equality as controller compatibility |
| REQ-PD-015 | bare installation starts independently for the active boot, installs an idempotent owner `@reboot` entry when compatible crontab is available, and otherwise prints exact same-user host rc guidance without claiming persistence |
| REQ-PD-016 | launchd disposable harness; Lima/VZ systemd success, rollback, initiator-death, and committed reboot recovery; detached bare-supervisor commit/rollback/stop acceptance; disposable aarch64 musl Phoenix build-identity and version-endpoint smoke |
| REQ-PD-017 | release workflow builds native macOS and musl Linux assets for x86_64 and aarch64, refuses incomplete asset sets before checksumming, and deployment selection tests cover all four targets |
| REQ-PD-018 | explicit prepared-artifact launchd path verifies exact candidate identity separately from clean controller/helper binding; retains private matched database/runtime proof and fails closed on incomplete recovery |
| REQ-PD-019 | prepared path preserves installed environment and PATH without model/default changes |

## Operator surface

The target surface for each backend is:

- `./dev.py prod deploy` — checked local `HEAD` build.
- `./dev.py prod deploy --release vX.Y.Z` — exact published release.
- `./dev.py prod deploy --release latest` — latest stable release resolved once.
- Protected systemd: explicit `--systemd-current-schema-upgrade` in exact-release controller mode, with the separate `--controller-source-commit` and all fixed candidate pins in REQ-PD-023; full command shape is in `skills/phoenix-deployment/SYSTEMD.md`. Exact source-prefix65 only; authored, not deployment acceptance.
- `./dev.py prod recover-systemd-current TXN` — explicit exact-transaction recovery: without marker, only interrupted fenced pre-marker unchanged-predecessor runtime restoration; with marker, only exact-candidate forward recovery after both-unit stop and detailed validation. Required lsof is probed after stop in recovery; dependency failure stays stopped/fenced. Never replay a backup; never start a predecessor after the marker.
- `./dev.py prod deploy --prepared-artifact DIR --expected-full-commit SHA --paired-database-upgrade` — explicit legacy launchd ProductConversation upgrade from protected prepared bytes. All options are required; the directory is trusted operator input whose workflow/source association must be established independently of local receipt validation.
- `./dev.py prod deploy --prepared-artifact DIR --expected-full-commit SHA --current-schema-upgrade` — exact migration-123 or migration-128 prepared source to the exact migration-128 candidate registry under ADR-105 and ADR-114, from the same protected prepared bytes and supply-chain evidence (128-to-128 admission authored; hosted CI qualification pending).
- `./dev.py prod deploy --release TAG --expected-full-commit CANDIDATE_SHA --controller-source-commit CONTROLLER_SHA --current-schema-upgrade` — protected macOS launchd exact-114-to-128 release transition under ADR-115. Public invocation needs no hidden controller flags: normal backend/host-asset/checksum selection applies. Requires an exact non-alias release tag and distinct full lowercase candidate/controller SHAs; excludes prepared/paired modes. Candidate/registry remain release-sourced and retained helper path/hash remain controller-sourced. Hidden controller-mode tag/backend/asset bindings are unchanged. Public parsing/dispatch and provenance have local regression coverage; hosted/runtime qualification remains separate.
- `./dev.py prod recover-current TXN` / `finalize-current TXN` — fail-closed unchanged-source recovery before offline migration invocation or exact-candidate finalization after verified committed migration.
- `./dev.py prod resume-forward TXN` — operator-authorized continuation with the exact retained ordinary candidate after predecessor rollback becomes unsafe.
- `./dev.py prod status` — selected backend, runtime identity, and durable transaction result.
- `./dev.py prod stop` — backend-owned runtime stop.
- `./dev.py prod set` / `prod unset` — rejected with guidance to edit `.phoenix-ide.env` directly.

Backend-specific restart surfaces preserve their owning runtime contract: launchd restarts the installed process without changing installed state, while bare Linux re-snapshots `.phoenix-ide.env` through the supervisor transaction. Systemd does not expose `prod restart`.
