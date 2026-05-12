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
| Log rotation | `/etc/newsyslog.d/com.phoenix-ide.server.conf` (copy-truncate, 14 MB) |

The binary is ad-hoc codesigned (`codesign --sign -`) on each deploy so the OS will run it.

## LLM config

Place an env file at `~/.phoenix-ide/.phoenix-ide.env`. If it provides LLM config (e.g. `ANTHROPIC_API_KEY` / `api_key_helper`), that wins. Otherwise the deploy auto-detects a local LLM gateway, mirroring dev mode.

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
