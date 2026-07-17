---
name: phoenix-deployment
description: Production deployment for Phoenix IDE. Use when running ./dev.py prod deploy, checking production status, diagnosing a failed deploy, or stopping the production service.
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
./dev.py prod stop                      # Stop the backend-owned runtime
```

Local-HEAD deployment runs `./dev.py check` first and aborts on failure. Published-release deployment verifies the selected target asset, checksum, tag commit, and embedded identity without compiling locally.

## ⚠️ Do NOT

- **Do not** run `cargo run` or start the binary directly — use `./dev.py prod deploy`
- **Do not** run `systemctl stop/start phoenix-ide` or `launchctl load/unload` manually — `./dev.py prod deploy` handles this

## Mode-specific details

For full details on each mode (ports, paths, log locations, LLM config), read the relevant file:

- **Native launchd (macOS):** read `skills/phoenix-deployment/LAUNCHD.md`
- **Native systemd (Linux):** read `skills/phoenix-deployment/SYSTEMD.md`
- **Persistent supervisor (Linux, no systemd):** read `skills/phoenix-deployment/DAEMON.md`

## Publishing a Release (GitHub)

See [`skills/phoenix-release/SKILL.md`](../phoenix-release/SKILL.md) for the end-to-end flow (version bump, tag, CI build, sub-agent-drafted release notes).

Stable asset URLs use `https://github.com/scottopell/phoenix-ide/releases/latest/download/phoenix_ide-<target>` for supported macOS and Linux targets.
