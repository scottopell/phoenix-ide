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
                        → NO  → Daemon mode
```

## Step 2: Deploy

```bash
./dev.py prod deploy          # Deploy HEAD (default)
./dev.py prod deploy v1.2.3   # Deploy a specific git tag
./dev.py prod status          # Check running state
./dev.py prod stop            # Stop the service
```

`deploy` always runs `./dev.py check` first and aborts on failure.

## ⚠️ Do NOT

- **Do not** run `cargo run` or start the binary directly — use `./dev.py prod deploy`
- **Do not** run `systemctl stop/start phoenix-ide` or `launchctl load/unload` manually — `./dev.py prod deploy` handles this

## Mode-specific details

For full details on each mode (ports, paths, log locations, LLM config), read the relevant file:

- **Native launchd (macOS):** read `skills/phoenix-deployment/LAUNCHD.md`
- **Native systemd (Linux):** read `skills/phoenix-deployment/SYSTEMD.md`
- **Daemon (Linux, no systemd):** read `skills/phoenix-deployment/DAEMON.md`

## Publishing a Release (GitHub)

See [`skills/phoenix-release/SKILL.md`](../phoenix-release/SKILL.md) for the end-to-end flow (version bump, tag, CI build, sub-agent-drafted release notes).

The stable download URL for the latest published binary:

```
https://github.com/scottopell/phoenix-ide/releases/latest/download/phoenix_ide-x86_64-unknown-linux-musl
```
