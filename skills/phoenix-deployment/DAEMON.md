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

Configure LLM access with `.phoenix-ide.env` before deployment. Common options:

1. `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`
2. `LLM_API_KEY_HELPER` with `ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL` for provider-compatible endpoints
3. In-app Codex login for ChatGPT/Codex-backed OpenAI models

If none are available, deploy exits with an error. Set an API key, helper, or Codex auth before deploying.

## Checking status

```bash
./dev.py prod status          # Shows PID and port
tail -f ~/.phoenix-ide/prod.log   # Follow live logs
```

## If the deploy fails

- `ANTHROPIC_API_KEY not set` → export the key and retry
- Port already in use → run `./dev.py prod stop` then retry
- Build failure → check `./dev.py check` output
