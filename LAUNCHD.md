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

Preparation validates and stages candidate identity/hash, signature, complete plist, environment and protocol-3 deploy helper before service disruption. The independent one-shot LaunchAgent survives the initiating shell or Phoenix connection. For an existing database, before quarantine and stop it checks capacity for a native SQLite backup plus small overhead. After stopping the predecessor, it holds a bounded SQLite `BEGIN IMMEDIATE` writer reservation across backup verification and durable candidate-boundary recording, retaining the verified backup and path/size/SHA-256 proof. A true first install instead requires an absent database and no managed predecessor, installed binary/plist or loaded service; private proof records `database_source: absent` and `backup: null`, and the helper rechecks path absence before the boundary. It checks bounded first-install capacity rather than making an empty backup. The helper checks a small operational reserve before the boundary; neither path guarantees migration peak space. Keep external database clients from writing throughout maintenance, including after candidate handoff, when the writer reservation no longer protects the database. It records the candidate boundary as attempted before atomically installing the candidate and durably records `migration_started` before invoking exact candidate `--migrate-only` once with a 600-second timeout and output in `activation.log`.

After successful migration it boots the private candidate plist, verifies a stable new PID and exact `/api/version` identity, atomically publishes the same plist inode, writes `deployed.sha` and `committed`, and attempts claim release. Durable committed is terminal for runtime behavior. Claim-release, reporting or cleanup errors preserve the healthy runtime and committed status; warnings are best effort. Incomplete claim release retains binary copies and any surviving marker for operator inspection. After completed release it attempts to remove only temporary candidate and previous binary copies. Preparation failure leaves the runtime intact and releases only its owned marker.

Before the candidate boundary, a failure may restart only the captured unchanged predecessor when its stop, PID absence and binary/plist identity are positively proven. The helper durably publishes its unchanged canonical plist, bootstraps that canonical path, then verifies exact health identity and stable new PID; only completed verification is reported as recovered. This preserves the loaded-plist same-file identity required by normal restart. If recovery is unproven or fails, it retains quarantine, claim and transaction for an operator. At or after an attempted or uncertain candidate boundary, failure attempts stop, removes attempted candidate publication, records `failed` with phase/error and teardown uncertainty, and retains claim, transaction and the verified backup for an existing database or first-install absence proof. It never automatically restores a database or starts the predecessor after that boundary. A teardown attempt is not proof of service absence.

## Manual recovery

An unresolved failed or interrupted deployment requires an operator. Read `./dev.py prod status`, `~/.phoenix-ide/deploy/status.json`, `~/.phoenix-ide/deploy/status.proof.json` (only when its transaction ID matches), `~/.phoenix-ide/deploy/activation.log`, the named transaction directory, and production logs. Transaction-matched proof reports boundary and either verified backup path, size and SHA-256 for an existing database, or private first-install `database_source: absent` with `backup: null`; status says unknown when proof is unavailable; a `failed` state is not proof of target absence. Inspect actual target and exact helper jobs with `launchctl print gui/$(id -u)/LABEL`. Preserve the transaction, any verified backup, absence proof, quarantined plist, manifest, helper and candidate evidence. Never manually publish an uncertain quarantined plist without resolving actual runtime and boundary state.

The markers are `~/.phoenix-ide/deploy/active` and `~/.phoenix-ide/restart/active`; their contents name their owning transaction. An active deploy or restart marker always blocks a new operation, including when status is terminal. Confirm that the exact helper is absent and resolve the actual target runtime state before explicitly clearing only the matching ownership marker as an operator action. Then deploy a chosen forward candidate as a new transaction. There is no automated recovery, resume, finalization, repair or replay command. The new helper refuses historical protocol-1 manifests; old on-disk transactions are evidence to inspect manually, not inputs to convert or execute.

After an attempted or uncertain candidate boundary, the candidate may have committed database writes. Starting an older binary then requires all database users stopped and a backup paired with that binary restored offline; a retained verified backup, when the database existed, is evidence to inspect, not an automatic restore or proof of postboundary compatibility. A verified unchanged preboundary predecessor recovery does not need a database restore. For an existing database the helper creates and integrity/logically verifies the native backup but does not parse migration registries, rehearse upgrades, or perform schema-specific admission; the selected candidate owns migrations and reconciliation. Keep the backup (when one was required) and transaction evidence after successful or failed deployments.

## Installed-runtime restart

```bash
./dev.py prod restart
```

Restart uses a separate independent protocol-2 helper and the shared operation fence. It preserves installed binary, plist, environment, listener and deployed SHA, sends SIGHUP without unloading the socket-activated job, and verifies a new PID with the same complete identity. It does not build or read `.phoenix-ide.env`. Restart status is retained under `~/.phoenix-ide/restart/` and shown by `prod status`.

## Socket activation and logs

The plist's `Listeners` socket uses `SockFamily=IPv4v6`, `SockProtocol=TCP`, `SockServiceName=8031`, and `SockType=stream`. Phoenix adopts the launchd socket; the startup log contains `Using launchd-provided TCP listener`. This dual-stack listener supports both loopback families and `.local` hostnames. SIGHUP begins bounded request draining, tracing flush and live Bash cleanup; launchd preserves the listener during replacement.

Phoenix rotates structured logs daily and retains 14 compressed generations. The fixed fatal log holds the latest Rust fatal diagnostic up to 64 KiB. OS-owned newsyslog rotation bounds loader stderr to 64 KiB with two generations. Deployment fixes these destinations rather than allowing structured output into discarded launchd stdout.

The normative contracts are `specs/launchd-deployment/requirements.md` and `launchd-deployment.allium`; ADR-230 records the original availability/manual-recovery tradeoff; ADR-263 qualifies its backup and preboundary recovery exclusions without reviving schema-specific upgrade routes. Historical ADRs and transaction artifacts describe retired routes and do not provide active operator commands.
