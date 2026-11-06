# macOS launchd Deployment

`./dev.py prod deploy` checks and builds exact local `HEAD`; `./dev.py prod deploy --release TAG|latest` installs a checksummed published host-architecture release without local compilation. A published candidate must report the selected exact version and full source SHA. Ordinary operator deployment snapshots `.phoenix-ide.env`; the in-app release controller preserves installed environment and PATH. Prepared-artifact and schema-specific macOS modes are withdrawn.

| Property | Value |
| --- | --- |
| Port | `8031` |
| Binary | `~/.phoenix-ide/phoenix-ide` |
| Database | `~/.phoenix-ide/prod.db` |
| Structured log | `~/.phoenix-ide/prod.log` |
| Latest fatal diagnostic | `~/.phoenix-ide/prod-fatal.log` |
| Loader stderr | `~/.phoenix-ide/prod-launchd-stderr.log` |
| launchd label | `com.phoenix-ide.server` |
| Installed plist | `~/Library/LaunchAgents/com.phoenix-ide.server.plist` |
| Deployment evidence | `~/.phoenix-ide/deploy/` |

## Deployment

```bash
./dev.py prod deploy
./dev.py prod deploy --release vX.Y.Z
./dev.py prod status
./dev.py prod stop
```

Preparation validates and stages the candidate identity/hash, signature, complete plist, environment and protocol-2 helper before service disruption. The independent one-shot LaunchAgent survives the initiating shell or Phoenix connection. It quarantines the canonical plist into the transaction, observes predecessor stop, atomically installs the candidate, durably records `migration_started`, and invokes that candidate's `--migrate-only` once with a 600-second timeout and output in `activation.log`.

After successful migration it boots the private candidate plist, verifies a stable new PID and exact `/api/version` identity, atomically publishes the same plist inode, writes `deployed.sha` and `committed`, and attempts claim release. Durable committed is terminal for runtime behavior. Claim-release, reporting or cleanup errors preserve the healthy runtime and committed status; warnings are best effort. Incomplete claim release retains binary copies and any surviving marker for operator inspection. After completed release it attempts to remove only temporary candidate and previous binary copies. Preparation failure leaves the runtime intact and releases only its owned marker.

After disruption and before durable committed, any error attempts stop, removes any attempted candidate publication, records `failed` with the phase, error and any teardown failure, and retains the claim and transaction. It never restores a database or starts the predecessor. A teardown attempt is not proof that the service stopped.

## Manual recovery

A failed or interrupted deployment requires an operator. Read `./dev.py prod status`, `~/.phoenix-ide/deploy/status.json`, `~/.phoenix-ide/deploy/activation.log`, the named transaction directory, and the production logs. Inspect the actual target job and exact helper job with `launchctl print gui/$(id -u)/LABEL`; a failed stop can leave a process running. Preserve the failed transaction and its private plist, manifest, helper and candidate evidence.

The markers are `~/.phoenix-ide/deploy/active` and `~/.phoenix-ide/restart/active`; their contents name their owning transaction. An active deploy or restart marker always blocks a new operation, including when status is terminal. Confirm that the exact helper is absent and resolve the actual target runtime state before explicitly clearing only the matching ownership marker as an operator action. Then deploy a chosen forward candidate as a new transaction. There is no automated recovery, resume, finalization, repair or replay command. The new helper refuses historical protocol-1 manifests; old on-disk transactions are evidence to inspect manually, not inputs to convert or execute.

The candidate may have committed database writes before failure. Starting an older binary requires all database users to be stopped and the database backup paired with that binary to be restored offline first. A retained previous binary or plist does not establish database compatibility. Routine macOS deployment does not create a database backup, run integrity/foreign-key/full-content scans, parse migration registries, or rehearse upgrades; the selected candidate owns migrations and reconciliation.

## Installed-runtime restart

```bash
./dev.py prod restart
```

Restart uses a separate independent helper and the shared operation fence. It preserves installed binary, plist, environment, listener and deployed SHA, sends SIGHUP without unloading the socket-activated job, and verifies a new PID with the same complete identity. It does not build or read `.phoenix-ide.env`. Restart status is retained under `~/.phoenix-ide/restart/` and shown by `prod status`.

## Socket activation and logs

The plist's `Listeners` socket uses `SockFamily=IPv4v6`, `SockProtocol=TCP`, `SockServiceName=8031`, and `SockType=stream`. Phoenix adopts the launchd socket; the startup log contains `Using launchd-provided TCP listener`. This dual-stack listener supports both loopback families and `.local` hostnames. SIGHUP begins bounded request draining, tracing flush and live Bash cleanup; launchd preserves the listener during replacement.

Phoenix rotates structured logs daily and retains 14 compressed generations. The fixed fatal log holds the latest Rust fatal diagnostic up to 64 KiB. OS-owned newsyslog rotation bounds loader stderr to 64 KiB with two generations. Deployment fixes these destinations rather than allowing structured output into discarded launchd stdout.

The normative contracts are `specs/launchd-deployment/requirements.md` and `launchd-deployment.allium`; ADR-229 records the availability and manual-recovery tradeoff. Historical ADRs and transaction artifacts describe retired routes and do not provide active operator commands.
