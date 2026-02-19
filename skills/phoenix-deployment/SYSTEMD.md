# Native Systemd Deployment Details

Applies when: Linux with systemd available.

## Runtime details

| Property | Value |
|----------|-------|
| Port | 8031 |
| Database | `~/.phoenix-ide/prod.db` |
| Service name | `phoenix-ide` |
| LLM config | `LLM_GATEWAY` env var in systemd unit |

## Checking status

```bash
./dev.py prod status               # Recommended
systemctl status phoenix-ide       # Direct systemd check (read-only)
journalctl -u phoenix-ide -f       # Follow live logs
```

## If the deploy fails

- Check `./dev.py check` output — deploy aborts on test/lint failure
- Check `journalctl -u phoenix-ide` for runtime errors after deploy
- Do NOT manually run `systemctl start/stop` — use `./dev.py prod deploy/stop`
