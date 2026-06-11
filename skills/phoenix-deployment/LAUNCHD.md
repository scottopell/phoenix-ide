# Native launchd Deployment Details

Applies when: macOS. This is the only macOS production mode — `./dev.py prod deploy` builds a native (host-arch) binary and installs it as a per-user launchd agent.

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

`./dev.py prod deploy` reads `.phoenix-ide.env` from the **repo root** of the checkout you deploy from (via `_load_env_file`) and bakes those vars into the launchd plist. If it provides any LLM config — `LLM_API_KEY_HELPER`, `LLM_GATEWAY`, `ANTHROPIC_API_KEY`, or `OPENAI_API_KEY` — the deploy uses that and does not auto-detect. Otherwise it auto-detects a local LLM gateway, mirroring dev mode.

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

- `launchctl bootstrap failed` → a stale instance may still be loaded; `./dev.py prod stop` then redeploy. The deploy already does a `bootout` first, so this is rare.
- Health check fails after 10s → check `~/.phoenix-ide/prod.log` for a startup error (bad env file, port in use, DB migration failure).
- `./dev.py check` failure → deploy aborts before touching production; fix tests/lint first.
- Do NOT manually `launchctl load/unload` the plist — use `./dev.py prod deploy/stop`.
