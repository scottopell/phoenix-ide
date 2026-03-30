# Native Systemd Deployment Details

Applies when: Linux with systemd available.

## Runtime details

| Property | Value |
|----------|-------|
| Port | 8031 |
| Database | `/var/lib/phoenix-ide/prod.db` |
| Service name | `phoenix-ide` |
| Socket unit | `phoenix-ide.socket` (zero-downtime upgrades) |
| LLM config | `LLM_GATEWAY` env var in systemd unit |

## Service user

The service runs as a dedicated system user — **not** as the deploying user.
`dev.py` detects the user automatically in priority order: `phoenix-dev` → `exedev`.

If neither exists, deploy will fail with a clear error. Create one before deploying:

```bash
sudo useradd --system --no-create-home phoenix-dev
```

The service user also needs traversal access to any user home directories it will
work in. Add it to the appropriate group:

```bash
sudo usermod -aG <username> phoenix-dev   # e.g. -aG scottopell phoenix-dev
sudo systemctl restart phoenix-ide        # required for group change to take effect
```

Without this, the server cannot validate working directories under `~/<username>/`
and will return `{"error":"Directory does not exist"}` even for real paths.

## Claude OAuth credentials

`~/.claude/.credentials.json` (written by `claude login`) is 600 by default. The
service user cannot read it. Fix with one command — the group membership above is a
prerequisite:

```bash
chmod g+r ~/.claude/.credentials.json   # make it 640
```

After this, `./dev.py prod deploy` no longer needs to bake the token into the systemd
unit at deploy time. The binary reads the file directly on each request, so token
refreshes take effect immediately without redeploying.

## Checking status

```bash
./dev.py prod status               # Recommended
systemctl status phoenix-ide       # Direct systemd check (read-only)
journalctl -u phoenix-ide -f       # Follow live logs
```

## If the deploy fails

- `Failed at step USER … No such process` → service user (`phoenix-dev` or `exedev`) does not exist; create it (see above)
- `Permission denied` on startup → `/var/lib/phoenix-ide/` not owned by the service user; deploy handles this, but check with `ls -la /var/lib/phoenix-ide/`
- `{"error":"Directory does not exist"}` from client → service user lacks traversal on the home dir; add it to the user's group (see above)
- Check `./dev.py check` output — deploy aborts on test/lint failure
- Check `journalctl -u phoenix-ide` for runtime errors after deploy
- Do NOT manually run `systemctl start/stop` — use `./dev.py prod deploy/stop`
