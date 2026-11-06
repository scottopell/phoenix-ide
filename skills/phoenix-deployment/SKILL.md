---
name: phoenix-deployment
description: Production deployment for Phoenix IDE. Use when deploying, restarting, checking status, diagnosing a failed production operation, or stopping the production service.
---

# Phoenix IDE Deployment

## Step 1: Identify your deployment mode

Run `./dev.py prod status` to see which mode is active. Do NOT choose or configure the mode manually — `./dev.py prod deploy` detects it automatically.

```
macOS
└── Native launchd mode (always)

Linux
└── systemd available?  → YES → Native systemd mode
                        → NO  → Persistent bare-Linux supervisor
```

## Step 2: Deploy

```bash
./dev.py prod deploy                    # Check, build, and deploy local HEAD
./dev.py prod deploy --release v1.2.3   # Deploy an exact checksummed release
./dev.py prod deploy --release latest   # Deploy latest stable release
./dev.py prod status                    # Check runtime and durable transaction
./dev.py prod restart                   # Restart the installed runtime (backend-specific config semantics)
./dev.py prod stop                      # Stop the backend-owned runtime
```

Local-HEAD deployment runs `./dev.py check` first and aborts on failure. Published-release deployment verifies the selected target asset, checksum, tag commit, and embedded identity without compiling locally.

## ⚠️ Do NOT

- **Do not** run `cargo run` or start the binary directly — use `./dev.py prod deploy`
- Use `./dev.py prod deploy` for routine activation. A failed macOS deployment requires operator inspection and manual recovery as described in [LAUNCHD.md](LAUNCHD.md).

## Mode-specific details

For full details on each mode (ports, paths, log locations, LLM config), read the relevant file:

- **Native launchd (macOS):** read [LAUNCHD.md](LAUNCHD.md)
- **Native systemd (Linux):** read [SYSTEMD.md](SYSTEMD.md)
- **Persistent supervisor (Linux, no systemd):** read [DAEMON.md](DAEMON.md)

## macOS failure contract

After disruption, deployment attempts stop and retains a failed claim and transaction. The candidate may have changed the database. Inspect logs and actual service/helper state, preserve evidence, and confirm helper absence before explicit operator marker clearance and a new forward deployment. An older binary requires its matching offline database restore. Active markers are never automatically reclaimed from terminal status. There are no macOS protected-schema, paired, repair, resume or finalization commands. Candidate-owned migration replaces deployment-side scans, snapshots and rehearsals; Linux contracts remain backend-specific.

## Publishing a Release (GitHub)

See [`skills/phoenix-release/SKILL.md`](../phoenix-release/SKILL.md) for the end-to-end flow (version bump, tag, CI build, sub-agent-drafted release notes).

Stable asset URLs use `https://github.com/scottopell/phoenix-ide/releases/latest/download/phoenix_ide-<target>` for supported macOS and Linux targets.
