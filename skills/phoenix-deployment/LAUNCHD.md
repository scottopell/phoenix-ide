# Native launchd Deployment Details

Applies when: macOS. This is the only macOS production mode. `./dev.py prod deploy` checks and builds local `HEAD`; `./dev.py prod deploy --release vX.Y.Z` or `--release latest` installs a checksummed host-architecture GitHub release without local compilation.

## Runtime details

| Property | Value |
|----------|-------|
| Port | 8031 |
| Binary | `~/.phoenix-ide/phoenix-ide` |
| Database | `~/.phoenix-ide/prod.db` |
| Logs | `~/.phoenix-ide/prod.log` (stdout + stderr) |
| launchd label | `com.phoenix-ide.server` |
| plist | `~/Library/LaunchAgents/com.phoenix-ide.server.plist` |
| Log rotation | `/etc/newsyslog.d/com.phoenix-ide.server.conf` — daily at 00:00, 14 generations, bzip2, copy-truncate (no size threshold) |

The binary is ad-hoc codesigned (`codesign --sign -`) on each deploy so the OS will run it.

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
new port. SIGHUP exits immediately in this mode so launchd can restart the
process while keeping the listener open.

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

## Environment overrides

```bash
./dev.py prod set RUST_LOG debug   # adds the var to the plist and reloads
./dev.py prod unset RUST_LOG       # removes it and reloads
```

## Checking status

```bash
./dev.py prod status                                  # Recommended
launchctl print gui/$(id -u)/com.phoenix-ide.server   # Direct launchd check (read-only)
tail -f ~/.phoenix-ide/prod.log                       # Follow live logs
```

## If the deploy fails

- Preparation or handoff failure leaves the running service untouched.
- Activation failure after disruption automatically attempts to restore and exactly verify the previous binary and plist. `activation_failed_rolled_back` means production was restored; `activation_failed_rollback_failed` requires operator attention.
- A stale `prepared` or `activating` status is reported by `./dev.py prod status`. Inspect the activation log and confirm no `com.phoenix-ide.deploy.*` helper is running before removing `~/.phoenix-ide/deploy/active` and retrying.
- `./dev.py check` failure applies only to local-HEAD deployment and aborts before staging. Published-release deployment deliberately skips repository checks and compilation.
- Do not manually `launchctl load/unload` the production plist. Use the production commands and durable status evidence.
