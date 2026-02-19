---
name: phoenix-deployment
description: Production deployment for Phoenix IDE across three environments: Lima VM (macOS), native systemd (Linux), and daemon mode (Linux without systemd). Covers which mode is active, how to deploy, and how to manage the running service.
---

# Phoenix IDE Deployment

## Which deployment mode am I in?

`./dev.py prod deploy` auto-detects the environment. **You do not choose the mode manually.**

```
macOS
└── Lima VM installed?  → YES → Lima mode
                        → NO  → Error (Lima required on macOS)

Linux
└── systemd available?  → YES → Native systemd mode
                        → NO  → Daemon mode
```

Check which mode is active: `./dev.py prod status`

---

## Mode 1: Lima VM (macOS)

The production binary runs **inside a Lima VM**. The VM has its own isolated git clone.

### ⚠️ Critical: push before deploying

```bash
git push          # REQUIRED — Lima clones independently, can't see unpushed commits
./dev.py prod deploy
```

If you forget to push, the deploy will fail with a git checkout error.

### Commands

```bash
./dev.py prod deploy    # Push first! Then build + deploy inside VM
./dev.py prod status    # Show VM service status
./dev.py prod stop      # Stop service inside VM
```

### Details

- Runs on port **8031** inside the VM
- Database: `~/.phoenix-ide/prod.db` (inside VM)
- Service managed by systemd inside the VM
- LLM config stored in an EnvironmentFile inside the VM

---

## Mode 2: Native systemd (Linux with systemd)

The binary is installed as a systemd service on the local machine.

### Commands

```bash
./dev.py prod deploy    # Build + install systemd unit + start
./dev.py prod status    # Show systemd service status
./dev.py prod stop      # Stop systemd service
```

### Details

- Runs on port **8031**
- Database: `~/.phoenix-ide/prod.db`
- Managed by: `systemctl status phoenix-ide`
- `./dev.py prod deploy` handles stop/install/start — no manual `systemctl` needed

---

## Mode 3: Daemon (Linux without systemd)

The binary runs as a background process managed by `dev.py` via a PID file.

### Commands

```bash
./dev.py prod deploy    # Build + start background process
./dev.py prod status    # Show daemon status
./dev.py prod stop      # Stop daemon
```

### Details

- Runs on port **8031**
- Database: `~/.phoenix-ide/prod.db`
- Logs: `~/.phoenix-ide/prod.log`
- PID file: `~/.phoenix-ide/prod.pid`
- LLM gateway auto-detected (probes `127.0.0.1:8462`, then link-local); falls back to `ANTHROPIC_API_KEY`

---

## All modes: what `deploy` does

1. Runs `./dev.py check` (clippy + fmt + tests + task validation) — **deploy aborts on failure**
2. Builds the UI (`npm ci` + `vite build`)
3. Builds the Rust binary (musl target for Linux modes)
4. Installs/restarts the service

## Version pinning

```bash
./dev.py prod deploy          # Deploy HEAD (default)
./dev.py prod deploy v1.2.3   # Deploy a specific git tag
```
