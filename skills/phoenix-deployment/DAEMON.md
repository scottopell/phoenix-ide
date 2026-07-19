# Bare-Linux Supervisor Deployment Details

Applies when: Linux without systemd.

## Runtime details

| Property | Value |
|----------|-------|
| Default port | 8031 |
| Database | `~/.phoenix-ide/prod.db` |
| Logs | `~/.phoenix-ide/prod.log` |
| Supervisor socket | `~/.phoenix-ide/run/supervisor.sock` |
| Durable deploy status | `~/.phoenix-ide/deploy/status.json` |

A persistent same-user supervisor directly owns Phoenix. `prod.pid` is not an authority. The owner-only Unix socket authenticates Linux peers with `SO_PEERCRED`; activation accepts only an immutable transaction ID and manifest hash.

## Configuration and deployment

Edit `.phoenix-ide.env` in the repo checkout, then deploy either checked local `HEAD` or a checksummed published release:

```bash
./dev.py prod deploy
./dev.py prod deploy --release vX.Y.Z
./dev.py prod deploy --release latest
```

The deploy starts the supervisor independently for the active boot. When compatible owner crontab is available it installs an idempotent `@reboot` entry. Otherwise it prints the exact supervisor command to add to the host's same-user boot/rc mechanism and does not claim reboot persistence.

## Status and stop

```bash
./dev.py prod status
./dev.py prod stop
```

Status reports the supervisor PID, exact managed-child identity, `/proc` start time, and durable transaction result. Stop terminates only the Phoenix child; the supervisor remains available.

## Recovery

On startup the supervisor reconciles any active durable transaction before opening its socket. It restarts and exactly verifies a durably installed candidate or restores and verifies the previous runtime. It never infers ownership or success from a PID file or responding port alone.

If rollback fails, preserve `~/.phoenix-ide/deploy/transactions`, `status.json`, and the active claim for diagnosis. Do not submit a concurrent deployment until the unresolved transaction is understood.
