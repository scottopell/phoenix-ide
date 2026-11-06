# Native launchd Deployment Details

macOS uses the ordinary local/published deployment path:

```bash
./dev.py prod deploy                    # Check/build exact local HEAD
./dev.py prod deploy --release TAG      # Install exact checksummed release
./dev.py prod status                    # Runtime and durable transaction status
./dev.py prod restart                   # Preserve installed state; verify new PID
./dev.py prod stop
```

Read the canonical [macOS operator guide](../../LAUNCHD.md) for deployment,
socket activation, log locations and manual recovery. This file is also exposed
through `.agents/skills/phoenix-deployment`, a symlink to this tracked skill folder.

Production uses `~/.phoenix-ide/phoenix-ide`, database `~/.phoenix-ide/prod.db`,
label `com.phoenix-ide.server` and port `8031`. Deployment diagnostics are under
`~/.phoenix-ide/deploy/`; restart diagnostics are under `~/.phoenix-ide/restart/`.

After deployment disruption, failure attempts stop and retains the claim and
transaction. The candidate may have changed the database. Preserve evidence,
inspect actual service/helper state, and confirm helper absence before explicit
operator marker clearance and a new forward deployment. Older-binary startup
requires a matching offline database restore. No macOS resume, repair,
finalization, snapshot replay or schema-specific route exists.
