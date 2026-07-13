# Make macOS launchd production deployment safe when initiated from Phoenix

## Problem

On the local macOS production deployment, `./dev.py prod deploy` may be invoked from an in-app Phoenix terminal or by a Phoenix agent. The current activation phase is executed by that Phoenix-owned process:

1. `launchctl bootout` unloads the running Phoenix LaunchAgent.
2. Phoenix shutdown closes direct PTYs and deliberately SIGKILLs every live agent `bash` process group.
3. The same now-vulnerable deploy process is expected to unlink/copy/sign the binary, rewrite the plist, bootstrap the service, and check health.

An agent-run deploy is therefore killed by construction during the operation that must recover the service. A terminal-panel deploy only survives incidentally when its shell is attached to the external per-scope tmux server; the direct-PTY fallback dies with Phoenix.

The current activation is also not transactional:

- the healthy service is stopped before the replacement binary and plist are fully staged;
- the installed binary and plist are replaced non-atomically;
- a fixed one-second sleep substitutes for confirming launchd teardown;
- concurrent deploys are not serialized;
- bootstrap success is inferred partly from error text;
- health timeout is only a warning, any responding `/version` value is accepted, and `deployed.sha` is then written from the invoking checkout rather than the selected deploy ref;
- no automatic rollback restores the last known-good binary and plist after activation failure.

Socket activation keeps the listening socket stable only while its launchd job remains loaded. It does not preserve WebSockets, direct PTYs, agent bash handles, or an in-process deploy command across `bootout`.

## Required safety rules

Specify and enforce these rules for native macOS launchd deployment:

1. **External ownership:** the process performing stop/swap/bootstrap/verification must be owned independently of the Phoenix service being replaced. It must survive the target LaunchAgent's shutdown and Phoenix's child cleanup.
2. **Prepare before disruption:** local checks/build or release download/verification, code signing, plist generation/validation, and artifact staging complete while the old service remains healthy.
3. **Single writer:** activation is serialized with a host-level deploy lock. A second deploy must fail clearly without touching production.
4. **Immutable manifest:** activation consumes a staged manifest identifying the source kind, exact source commit/tag, expected embedded version and git SHA, candidate binary, candidate plist, target paths, and rollback artifacts. The helper must not depend on mutable checkout state or the network after handoff.
5. **Atomic artifacts:** candidates are staged on the destination filesystem, fsynced as appropriate, then installed with atomic renames. Never unlink the live binary first.
6. **Condition-based launchd transitions:** inspect command results and poll launchd state/PID transitions with deadlines; do not use blind sleeps or stderr substring matching as proof of success.
7. **Exact verification:** success requires the target launchd job to be running and `/version` to equal the manifest's expected version. A health timeout or version mismatch is failure, not a warning.
8. **Rollback:** after any failed activation, restore the previous binary and plist atomically, bootstrap the previous service, and verify it. Preserve explicit status if rollback itself fails.
9. **Truthful durable result:** write `deployed.sha` for the actual selected source commit only after exact verification. Persist a redacted deploy status/log outside the initiating terminal so the result remains inspectable after reconnect or agent interruption.
10. **Idempotent recovery:** an interrupted helper or stale staged transaction must be detectable and safely recoverable on the next status/deploy invocation.
11. **No secret leakage:** staged manifests, status, logs, tests, and command output must not expose plist environment secrets.

## Implementation plan

### 1. Add a normative deployment specification

Create a focused spEARS v2 deployment spec covering launchd self-deploy initiation, preparation, handoff, activation, exact verification, rollback, concurrency, durable status, and recovery. Add an Allium spec because this is a multi-step lifecycle with ordering requirements and partial-failure states. Keep implementation status in the executive document and capture the external-helper/transaction decision in a project ADR.

Reconcile deployment guidance in `.agents/skills/phoenix-deployment/LAUNCHD.md` with the new behavior, including what users should expect when the initiating Phoenix WebSocket disconnects and how to inspect the durable result after reconnect.

### 2. Support both local-source and published-release candidates

Refactor candidate preparation into two explicit sources that converge on the same staged artifact manifest:

- **Local source:** preserve deployment from `HEAD` for development builds. Run the full checks, build the UI and native Rust binary, and record the exact source commit and embedded build identity.
- **Published GitHub release:** add a command surface for a specific release tag and for the latest published release. Resolve the immutable release/tag once, select the asset matching the current macOS architecture, download it from `scottopell/phoenix-ide` with `gh`, and verify its published SHA-256 checksum before staging. This path must not run repository checks, install UI dependencies, create/update the build worktree, or compile code.

Delete the legacy positional `version` argument and its source-tag build path. The command surface is deliberately unambiguous:

- `./dev.py prod deploy` builds and deploys local `HEAD` after checks.
- `./dev.py prod deploy --release vX.Y.Z` installs that published GitHub release.
- `./dev.py prod deploy --release latest` resolves and installs the latest stable published GitHub release.

A positional argument must be rejected with migration guidance to use `--release`; there is no local build-from-tag mode. Reject prereleases by default unless the user names their exact tag. Fail before disruption when the requested release, host architecture asset, checksum entry, or embedded identity is missing or inconsistent.

Expand `.github/workflows/release.yml` so each GitHub release publishes native macOS binaries for every supported Mac architecture (including this host's `aarch64-apple-darwin`) with stable asset names, plus a `SHA256SUMS` manifest covering every artifact. Release jobs must derive all assets from the same tagged commit, and release documentation must stop claiming that only the Linux binary exists. Keep local ad-hoc code signing during staging so the installed binary retains Phoenix's stable macOS identifier and local privacy grants.

For either source, preparation must then:

- derive the exact expected `/api/version` package version and git SHA from the candidate rather than trusting a caller-supplied label or plist environment value;
- generate and validate the complete plist;
- copy the candidate onto the destination filesystem, verify it, and sign it before stopping production;
- snapshot the current binary/plist as rollback inputs;
- write a secret-safe immutable activation manifest;
- hand activation to a separate one-shot launchd helper job with a distinct label and log/status paths.

The one-shot helper must be launched by launchd (not merely `Popen(start_new_session=True)`, `nohup`, or the conversation tmux server), so its lifetime and process coalition are independent of the target Phoenix job. Use only stable, host-resident inputs after handoff; do not require the initiating worktree or network to remain available.

Keep `./dev.py prod deploy` as the local `HEAD` workflow. Both local and release workflows should report successful handoff before the expected connection loss. When the caller remains connected, it may follow the durable status; correctness must not depend on that follower surviving.

### 3. Implement transactional activation and rollback

In the helper:

- acquire the deploy lock;
- revalidate staged artifact hashes/signature and manifest preconditions;
- transition the target launchd job using checked commands and bounded state polling;
- atomically install candidate artifacts;
- bootstrap and verify a new PID plus the exact expected package version and git SHA from `/api/version`;
- only then record the deployed source commit/tag and committed status;
- on failure after disruption, atomically restore prior artifacts, bootstrap them, and verify rollback;
- persist structured terminal status for success, activation failure with successful rollback, and activation failure with failed rollback;
- clean up the one-shot helper job and bounded old staging data without deleting evidence needed for diagnosis.

Do not weaken Phoenix's normal shutdown cleanup to exempt deploy commands: ordinary agent bash jobs should still be killed on restart. The fix is to transfer ownership before disruption, not to create a special child-process escape hatch.

### 4. Make status and diagnostics honest

Extend `./dev.py prod status` (or a narrowly scoped `prod deploy-status` command if clearer) to report:

- active launchd PID/state and current `/version`;
- last deploy transaction state, expected version/SHA, timestamps, and failure/rollback summary;
- stale/in-progress activation with actionable recovery guidance.

Never print or persist plist environment values. Replace checkout-relative `write_deployed_sha` behavior so local-HEAD and published-release deployments record the candidate's actual embedded git SHA. Status must distinguish a locally built candidate from a published release and show the resolved release tag when applicable.

### 5. Add deterministic tests

Extract command execution/filesystem boundaries enough to test the state machine without mutating the developer's real LaunchAgent. Cover at least:

- preparation failure leaves the running service untouched;
- helper ownership/handoff occurs before target `bootout`;
- concurrent deploy rejection;
- atomic candidate install without an unlink gap;
- delayed stop/start uses polling rather than sleeps;
- bootstrap command failure;
- launchd crash loop or missing PID;
- health timeout and wrong-version response both trigger rollback;
- successful rollback and failed rollback have distinct durable statuses;
- exact selected commit is written only after successful verification;
- interrupted/stale transaction recovery;
- manifests/status/logs redact environment secrets;
- the removed positional version argument is rejected with guidance to use `--release`, and no build-from-tag path remains;
- release selection maps `latest` to one immutable tag before download;
- release deployment selects the correct macOS host-architecture asset, verifies its checksum, and records its embedded git SHA and release tag;
- missing asset, missing/mismatched checksum, wrong architecture, prerelease ambiguity, and embedded-version mismatch all fail before disruption;
- the published-release path performs no checks, dependency installation, worktree update, or compilation;
- release workflow tests assert stable macOS asset names and complete `SHA256SUMS` coverage;
- existing healthy launchd socket configuration remains valid.

Add a macOS-gated integration harness using disposable launchd labels, paths, database, and an ephemeral port. It must prove that a helper initiated from a process deliberately terminated at handoff still completes activation and exact-version verification. It must never touch `com.phoenix-ide.server` or production data.

### 6. Validate safely, with an explicit gate for live production

Run all automated and failure-injection journeys first against disposable launchd labels, temporary install/plist/status paths, an isolated database, and an ephemeral port:

1. From a terminal process backed by disposable tmux, initiate deployment, terminate the initiating process at handoff, and confirm the terminal session and external helper complete as designed.
2. From a simulated Phoenix agent-owned process group, initiate deployment, kill that group at handoff, and confirm the independently owned helper still installs and verifies the candidate.
3. Deploy a known published macOS release by explicit tag into the disposable target, confirm no local build/check phase runs, and verify `/api/version` matches that release.
4. Inject bad candidates and startup failures into the disposable target and confirm automatic restoration of its known-good version.

The harness and ordinary test commands must structurally refuse the live label `com.phoenix-ide.server`, production port, production database, production plist, and production install/status paths.

**Do not run any deploy, restart, stop, rollback, or failure injection against the live production Phoenix as part of implementation or QA without asking the user immediately beforehand and receiving explicit confirmation.** Production is actively used; task approval is not approval for a live deploy. Before requesting that confirmation, present the exact command, candidate version/SHA, expected disconnect, rollback plan, and completed disposable-test evidence. If confirmation is not given, finish with the disposable evidence and leave live validation pending.

If explicitly approved, validate the minimum live journey necessary: perform one successful deploy from the user-selected initiating context, avoid deliberate failure injection, wait for durable exact-version verification, and report the result after reconnect. Never use the live production database or service for destructive fault testing.

Run `./dev.py check`, the disposable launchd integration harness, and the spec-authoring pre-flight checklist before committing.

## Acceptance criteria

- Running `./dev.py prod deploy` from Phoenix's local production terminal or through a Phoenix agent cannot strand production merely because the initiating Phoenix process, WebSocket, PTY, or bash handle is terminated.
- No production disruption occurs until all candidate and rollback inputs are prepared and validated.
- Activation success is reported only when `/api/version` matches the candidate's exact package version and git SHA.
- A specific or latest published GitHub release can be installed on macOS without repository checks or local compilation; its architecture, checksum, release tag, and embedded identity are verified before disruption.
- Every post-disruption failure attempts and verifies rollback, with durable and secret-safe diagnostics.
- Concurrent and interrupted deployments have deterministic outcomes.
- The disposable launchd integration target completes cleanly with the new version and survives an injected failed upgrade by restoring the previous version.
- Live-production validation is performed only after separate, immediate user confirmation; without that confirmation, it remains explicitly pending rather than being inferred from disposable testing.
