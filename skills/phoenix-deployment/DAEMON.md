# Daemon Deployment Details

Applies when: Linux without systemd (containers, workspaces, VMs without systemd).

## Runtime details

| Property | Value |
|----------|-------|
| Port | 8031 |
| Database | `~/.phoenix-ide/prod.db` |
| Logs | `~/.phoenix-ide/prod.log` |
| PID file | `~/.phoenix-ide/prod.pid` |

## LLM configuration

`deploy` probes for a gateway in this order:
1. `LLM_GATEWAY` env var (if set)
2. Local proxy at `http://127.0.0.1:8462`
3. Link-local gateway at `http://169.254.169.254/gateway/llm`
4. Falls back to `ANTHROPIC_API_KEY` env var

If none are available, deploy exits with an error. Set `ANTHROPIC_API_KEY` before deploying.

## Checking status

```bash
./dev.py prod status          # Shows PID and port
tail -f ~/.phoenix-ide/prod.log   # Follow live logs
```

## If the deploy fails

- `ANTHROPIC_API_KEY not set` → export the key and retry
- Port already in use → run `./dev.py prod stop` then retry
- Build failure → check `./dev.py check` output
