# Native launchd Deployment Details

Applies when: macOS. This is the only macOS production mode. `./dev.py prod deploy` checks and builds local `HEAD`; `./dev.py prod deploy --release vX.Y.Z` or `--release latest` installs a checksummed host-architecture GitHub release without local compilation.

```bash
./dev.py prod restart  # Restart the installed process; preserve installed config
```

## Runtime details

| Property | Value |
|----------|-------|
| Port | 8031 |
| Binary | `~/.phoenix-ide/phoenix-ide` |
| Database | `~/.phoenix-ide/prod.db` |
| Logs | `~/.phoenix-ide/prod.log` (structured, daily rotation), `prod-fatal.log` (latest Rust fatal, maximum 64 KiB), and `prod-launchd-stderr.log` (pre-main loader errors) |
| launchd label | `com.phoenix-ide.server` |
| plist | `~/Library/LaunchAgents/com.phoenix-ide.server.plist` |
| OS-owned rotation | `/etc/newsyslog.d/com.phoenix-ide.server.conf` — launchd stderr at 64 KiB with 2 generations; it never touches process-owned `prod.log` |
| Process-owned rotation | Phoenix rotates `prod.log` daily, gzip-compresses closed archives, and retains 14 generations |

The binary is ad-hoc codesigned (`codesign --sign -`) on each deploy so the OS will run it.
The launcher fixes the three `PHOENIX_LOG_*` destinations above; `.phoenix-ide.env`
cannot redirect structured output into launchd's discarded stdout stream.

## Transaction ownership

Preparation completes while the existing service remains healthy: candidate identity/signature, plist validation, destination-filesystem staging, and rollback snapshots. Activation is then bootstrapped as a distinct one-shot LaunchAgent under `~/.phoenix-ide/deploy/`. It does not depend on the initiating Phoenix process, terminal, WebSocket, worktree, or network.

The initiating connection is expected to close when the target LaunchAgent is unloaded. Successful handoff is printed first. After reconnecting, inspect the durable result with:

```bash
./dev.py prod status
cat ~/.phoenix-ide/deploy/status.json
cat ~/.phoenix-ide/deploy/activation.log
```

Status includes source kind/tag, expected version/SHA, terminal outcome, and rollback failure if any. It never includes plist environment values.

## Socket activation

The launchd plist owns Phoenix's production listener through a socket named
`Listeners`. The Phoenix binary calls `launch_activate_socket("Listeners", …)` at
startup; if launchd supplies that socket, Phoenix adopts it instead of binding a
new port. SIGHUP begins bounded shutdown in this mode: Phoenix stops accepting
connections, drains requests for up to 30 seconds, flushes tracing, and cleans
up live Bash process groups before exiting. launchd keeps the listener open and
restarts the process; the restart helper reserves 35 seconds for this shutdown
before its separate replacement-process deadline begins.

`./dev.py prod restart` uses that path through an independent one-shot
LaunchAgent. It verifies a new PID with the same exact runtime identity and
preserves the installed binary, plist, environment, listener, and deployed SHA.
It does not read `.phoenix-ide.env`; run `./dev.py prod deploy` when configuration
must change. Restart status is durable under `~/.phoenix-ide/restart/` and is
shown by `./dev.py prod status`.

`./dev.py prod deploy` writes this socket dictionary into
`~/Library/LaunchAgents/com.phoenix-ide.server.plist`:

```xml
<key>Sockets</key>
<dict>
  <key>Listeners</key>
  <dict>
    <key>SockFamily</key>
    <string>IPv4v6</string>
    <key>SockProtocol</key>
    <string>TCP</string>
    <key>SockServiceName</key>
    <string>8031</string>
    <key>SockType</key>
    <string>stream</string>
  </dict>
</dict>
```

`SockFamily=IPv4v6` gives Phoenix a single dual-stack listener. That matters for
Bonjour / mDNS names such as `my-mac.local`: iOS Safari often tries IPv6 before
IPv4, so the launchd-owned dual-stack socket makes both
`http://127.0.0.1:8031/` and `http://[::1]:8031/` work without changing
Phoenix's normal bind address.

Expected startup log signal:

```text
Using launchd-provided TCP listener
```

## LLM config

`./dev.py prod deploy` reads `.phoenix-ide.env` from the **repo root** of the checkout you deploy from (via `_load_env_file`) and bakes those vars into the launchd plist. If it provides any LLM config — `LLM_API_KEY_HELPER`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or Codex auth — the deploy uses that configuration.

## Environment configuration

Edit `.phoenix-ide.env` in the repo root, then run `./dev.py prod deploy` to install the new snapshot transactionally. `prod set` and `prod unset` reject with this guidance and do not mutate the plist or create an override store.

## Checking status

```bash
./dev.py prod status                                  # Recommended
launchctl print gui/$(id -u)/com.phoenix-ide.server   # Direct launchd check (read-only)
tail -f ~/.phoenix-ide/prod.log                       # Follow live logs
tail -f ~/.phoenix-ide/prod-launchd-stderr.log        # Loader/pre-main failures
```

## If the deploy fails

- Preparation or handoff failure leaves the running service untouched.
- Ordinary activation failure after disruption attempts to restore and exactly verify the previous binary and plist. `activation_failed_rolled_back` means the predecessor was verified; `ordinary_activation_failed_rollback_failed` requires offline operator inspection.
- For prepared-artifact paired transactions, helper absence never authorizes removing `~/.phoenix-ide/deploy/active`, redeploying, or restarting. Inspect `./dev.py prod status` and the retained transaction log. Use `./dev.py prod recover-paired TRANSACTION_ID` for interrupted preparation or unresolved recovery: it requires matching ownership and confirmed old-helper absence. Pre-handoff abandonment does not disrupt the runtime or restore the database; post-start checkpoint recovery verifies/finalizes the running predecessor without snapshot replay. A retained `activation_failed_rolled_back` checkpoint still requires this finalization rather than manual marker removal.
- A committed paired transaction with pending publication/cleanup uses `./dev.py prod finalize-paired TRANSACTION_ID`, not rollback. The retained helper verifies the exact running candidate/configuration and retries publication and temporary reservation cleanup only: no stop/bootstrap/restart or database reads/writes. Failure/interruption retains its pending status and claim; missing publication does not establish login/reboot persistence.
- A stale ordinary `prepared` or `activating` status requires inspecting its activation log, matching status/claim, and proving its exact helper absent before any ordinary marker repair. Never apply ordinary marker repair to a paired transaction. See the repository `LAUNCHD.md` for the prepared-artifact paired lifecycle and supported proof boundaries.
- `./dev.py check` failure applies only to local-HEAD deployment and aborts before staging. Published-release deployment deliberately skips repository checks and compilation.
- Do not manually `launchctl load/unload` the production plist. Use the production commands and durable status evidence.

## Ordinary modern migration option

`prod deploy --migration-backup-receipt PATH` explicitly selects the stopped/fenced ordinary macOS policy. The service must be stopped, installed environment preserved, and a private stopped-state SQLite backup plus restoration rehearsal proven equivalent to source contents. Safely checkpoint/close SQLite before admission; never blindly drop WAL/SHM/journal data. Normal local HEAD/release source only; legacy paired eligibility does not widen.

Failure leaves `migration_failed_stopped` with retained claim and no automatic predecessor start/DB restore. Manual offline matched restore precedes `prod resume-migration TXN`; the retained helper verifies restored bytes/content and captured runtime/configuration before startup. Do not clear retained migration markers or bypass deploy/restart/stop fencing. An unresolved resume-start checkpoint refuses replay. A durable `migration_resumed` retry verifies the captured running predecessor and finalizes only its owned claim, with no stop/start or DB replay; verification failure preserves terminal status/fence. Receipt/backup/rehearsal originals require no group/other permissions before staging. See repository `LAUNCHD.md` for receipt fields, capacity and proof boundaries; no host activation is authorized by this guide.
