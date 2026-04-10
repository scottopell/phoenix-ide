---
name: phoenix-deployment
description: Production deployment for Phoenix IDE. Use when running ./dev.py prod deploy, checking production status, diagnosing a failed deploy, or stopping the production service.
---

# Phoenix IDE Deployment

## Step 1: Identify your deployment mode

Run `./dev.py prod status` to see which mode is active. Do NOT choose or configure the mode manually — `./dev.py prod deploy` detects it automatically.

```
macOS
└── Lima VM installed?  → YES → Lima mode
                        → NO  → Error (install Lima first)

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
- **Do not** run `systemctl stop/start phoenix-ide` manually — `./dev.py prod deploy` handles this
- **Do not** deploy in Lima mode without pushing first — the VM has its own git clone and cannot see unpushed commits; the deploy will fail

## Mode-specific details

For full details on each mode (ports, paths, log locations, LLM config), read the relevant file:

- **Lima VM (macOS):** read `skills/phoenix-deployment/LIMA.md`
- **Native systemd (Linux):** read `skills/phoenix-deployment/SYSTEMD.md`
- **Daemon (Linux, no systemd):** read `skills/phoenix-deployment/DAEMON.md`

## Publishing a Release (GitHub)

To publish a new versioned binary to GitHub Releases:

```bash
./scripts/tag-release.sh v0.2.0
```

This script:
1. Validates the tag is in `vX.Y.Z` format
2. Checks for a clean working tree
3. Creates an annotated git tag and pushes it

Pushing the tag triggers `.github/workflows/release.yml`, which:
- Builds the UI (`npm ci && npm run build`)
- Compiles a static `x86_64-unknown-linux-musl` binary
- Creates a GitHub Release with the binary as an asset

The stable download URL:
```
https://github.com/scottopell/phoenix-ide/releases/latest/download/phoenix_ide-x86_64-unknown-linux-musl
```
