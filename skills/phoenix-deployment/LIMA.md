# Lima VM Deployment Details

Applies when: macOS + Lima VM is installed.

## Before every deploy

```bash
git push    # REQUIRED — do this before ./dev.py prod deploy
```

The VM maintains its own git clone. It fetches from origin during deploy. Any commits not pushed to origin will not be included.

## Runtime details

| Property | Value |
|----------|-------|
| Port | 8031 (inside VM) |
| Database | `~/.phoenix-ide/prod.db` (inside VM filesystem) |
| Service manager | systemd (inside VM) |
| LLM config | EnvironmentFile inside VM |

## Checking status

```bash
./dev.py prod status     # Shows VM service state via dev.py
./dev.py lima status     # Shows Lima VM itself (running/stopped)
```

## If the deploy fails

- `git checkout error` → you forgot to push; run `git push` and retry
- VM not running → run `./dev.py lima start`
- LLM config missing → re-run `./dev.py prod deploy` (prompts for API key if needed)
