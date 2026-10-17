# Native Systemd Deployment Details

Applies when: Linux with systemd available.

## Runtime details

| Property | Value |
|----------|-------|
| Port | 8031 |
| Database | `/var/lib/phoenix-ide/prod.db` |
| Service name | `phoenix-ide` |
| Socket unit | `phoenix-ide.socket` (protected migration stops both socket and service) |
| LLM config | API keys, helper/base URL overrides, or Codex auth in service env |

## Service user

The service runs as a dedicated system user — **not** as the deploying user.
`dev.py` detects the user automatically in priority order: `phoenix-dev` → `exedev`.

If neither exists, deploy will fail with a clear error. Create one before deploying:

```bash
sudo useradd --system --no-create-home phoenix-dev
```

The service user also needs traversal access to any user home directories it will
work in. Add it to the appropriate group:

```bash
sudo usermod -aG <username> phoenix-dev   # e.g. -aG scottopell phoenix-dev
sudo systemctl restart phoenix-ide        # required for group change to take effect
```

Without this, the server cannot validate working directories under `~/<username>/`
and will return `{"error":"Directory does not exist"}` even for real paths.

## Claude OAuth credentials

`~/.claude/.credentials.json` (written by `claude login`) is 600 by default. The
service user cannot read it. Fix with one command — the group membership above is a
prerequisite:

```bash
chmod g+r ~/.claude/.credentials.json   # make it 640
```

After this, `./dev.py prod deploy` no longer needs to bake the token into the systemd
unit at deploy time. The binary reads the file directly on each request, so token
refreshes take effect immediately without redeploying.

## Checking status

```bash
./dev.py prod status               # Recommended
systemctl status phoenix-ide       # Direct systemd check (read-only)
journalctl -u phoenix-ide -f       # Follow live logs
```

## Exact protected published-release migration

The only protected systemd database transition is the complete ordered candidate-registry prefix through65 to the fixed RC4 target128 in `specs/production-deployment/requirements.md` REQ-PD-023–027 / `specs/compatibility/requirements.md` REQ-COMP-014. It requires an existing predecessor and preserves installed environment/service/socket and `/var/lib/phoenix-ide/prod.db`; do not substitute `.phoenix-ide.env`. For an existing DB, ordinary deployment through the repaired controller derives the target by executing the exact candidate's `--migrate-only` on an isolated empty DB (isolated HOME/DB path), not by parsing source registry bytes. It verifies successful exit, integrity/FK, complete nonempty sorted unique ledger and unchanged candidate hash, then compares the full production ledger/integrity/FK before handoff. Failure, pending migrations or divergence refuses activation; exact-target/no-pending activation and first installs without a DB remain supported. This controller-side gate leaves the protocol-v1 old-helper manifest unchanged; historical controllers do not gain the guarantee.

Command shape below is for an independently qualified, separately authorized Linux x86_64 execution, not permission to deploy. `CONTROLLER_SHA` must be the explicit full lowercase commit of the separately verified controller source, different from the fixed candidate commit; its retained helper hash is verified independently.

```bash
./dev.py prod deploy --release v0.13.0-rc.4 \
  --controller-mode --controller-backend systemd \
  --controller-release-tag v0.13.0-rc.4 \
  --controller-expected-full-commit 3f0ba4602faa71ebad5088e1ffc856f4c7d511d9 \
  --controller-expected-asset-name phoenix_ide-x86_64-unknown-linux-musl \
  --controller-expected-asset-sha256 bcb74d565c828843596e5109ea9034a2b0e505aab4ba6c18c5cca8e116f5677e \
  --systemd-current-schema-upgrade --controller-source-commit CONTROLLER_SHA
```

Before disruption, validate all immutable candidate/controller/source evidence, including the exact migration-source hash in REQ-PD-023. Before stopping either unit, the owner installs/fsyncs transaction-bound persistent service **and** socket start-inhibit drop-ins, reloads systemd and verifies both the files and effective configuration. The inhibit blocks manual/dependency/socket/reboot activation throughout disrupted pre-marker and unresolved post-marker states. Tampered/foreign/unsafe inhibit evidence fails closed: never blindly remove it or start a unit. The owner then stops **both** units, proves inactive/exclusive database ownership, and retains a pristine SQLite backup. It runs exact-candidate `--migrate-only` on an isolated backup copy and verifies exact target/integrity/FK and pristine backup before writing the live `migration_started` marker. Backup/rehearsal capacity or verification failure is pre-marker: only a proven unchanged predecessor may resume, never by replaying the backup. No launchd 20 GiB free-space floor applies. Status remains `activating` across the live boundary; the durable marker file and `migration_started` boolean carry the boundary, not a separate status state. Remove only the verified matching inhibit, with durable removal/reload/config verification, after authorized pre-marker unchanged-predecessor artifact restoration before predecessor restart, or immediately before exclusively verified exact-target candidate startup. Failed/unproven pre-marker rollback retains the claim/inhibit as `activation_failed_rollback_failed` (not the ordinary-mode failure state).

At or after the durable live marker, never restore the backup or predecessor artifacts or start the old binary. Retain the claim, exact candidate, backup and evidence. Only explicitly authorized `./dev.py prod recover-systemd-current TXN` (retained helper action `recover-current`) may continue. Marker presence, including unsafe marker evidence, selects the post-marker path; an absent marker refuses forward recovery without stopping the predecessor. The selected path stops **both** units and proves inactivity before detailed post-marker manifest/inhibit/marker/evidence/DB validation, then proves exclusive database ownership. It uses only the exact retained candidate/controller/helper, valid matching claim/marker and intact backup/rehearsal/configuration evidence with a complete same-registry prefix65..128; at exact target it can finalize without rerunning migration. Unknown or divergent evidence stays fenced; failure attempts persistent inhibit retention/reinstallation and both-unit stop. Inhibit removal is permitted only immediately before verified exact-target candidate start, never merely because migration or recovery was requested. Keep the pristine backup after success. Do not clear prepared/ambiguous/post-marker claims as stale staging; only positively identified partial staging may retire under the global operation lock.

## If the deploy fails

- `Failed at step USER … No such process` → service user (`phoenix-dev` or `exedev`) does not exist; create it (see above)
- `Permission denied` on startup → `/var/lib/phoenix-ide/` not owned by the service user; deploy handles this, but check with `ls -la /var/lib/phoenix-ide/`
- `{"error":"Directory does not exist"}` from client → service user lacks traversal on the home dir; add it to the user's group (see above)
- Check `./dev.py check` output — deploy aborts on test/lint failure
- Check `journalctl -u phoenix-ide` for runtime errors after deploy
- Do NOT manually run `systemctl start/stop` — use `./dev.py prod deploy/stop`
