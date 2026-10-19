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

## Exact protected systemd migrations

The protected systemd database transitions are the complete ordered candidate-registry prefix through65 to target128 under either the fixed published RC4 contract in `specs/production-deployment/requirements.md` REQ-PD-023–027 / `specs/compatibility/requirements.md` REQ-COMP-014 or the exact prepared Linux contract in REQ-PD-029 / REQ-COMP-016. They require an existing predecessor and preserve installed environment/service/socket and `/var/lib/phoenix-ide/prod.db`; do not substitute `.phoenix-ide.env`. The prepared source is a sibling qualification, not a release repin, and does not alter the published pins or protocol.

For an existing DB, ordinary deployment through the repaired controller derives the target by executing the exact candidate's `--migrate-only` on an isolated empty DB (isolated HOME/DB path), not by parsing source registry bytes. It verifies successful exit, integrity/FK, complete nonempty sorted unique ledger and unchanged candidate hash, then compares the full production ledger/integrity/FK before handoff. Failure, pending migrations or divergence refuses activation. A missing configured DB plus any installed binary/env/service/socket/deployed identity artifact or runtime identity refuses **before handoff**, including an unavailable mount or an unlinked DB held open by the predecessor. The current helper mirrors this with captured predecessor/rollback evidence. Only a genuine first install with no predecessor evidence may omit DB; exact-target/no-pending activation remains supported. This controller-side gate leaves the protocol-v1 old-helper manifest unchanged; historical controllers do not gain the guarantee.

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

The exact prepared source is eligible only after its producer and installer sources are qualified and merged, the host has downloaded the exact closed two-file bundle from an authenticated successful run at `CONTROLLER_SHA`, and a separate host admission authorizes that packet:

```bash
./dev.py prod deploy \
  --prepared-artifact PREPARED_DIR \
  --expected-full-commit 237100c79d9ce672867b284167a099fa0b7a2bcf \
  --controller-mode --controller-backend systemd \
  --controller-expected-full-commit 237100c79d9ce672867b284167a099fa0b7a2bcf \
  --systemd-current-schema-upgrade --controller-source-commit CONTROLLER_SHA
```

`PREPARED_DIR` must contain only the exact payload and Linux preparation receipt named in REQ-PD-029. The receipt controller commit must equal `CONTROLLER_SHA`; local JSON validation does not replace independent GitHub run/source/artifact association.

Required host dependencies: Python SSL/SQLite support and an exact usable `lsof` executable; manual TLS with `PHOENIX_TLS_CA_CERT_PATH` also requires an exact usable `openssl`. Root staging interprets the CA only with both manual cert/key paths. It checks absolute regular non-symlink paths and service-user traversal/read modes for all three, proves cert/key parse and match with `SSLContext.load_cert_chain`, parses CA with `load_verify_locations`, resolves/probes openssl, requires exactly one CA certificate within 32 KiB with CA Basic Constraints and signing Key Usage if present, then runs `openssl verify -trusted CA -partial_chain -untrusted CERT -purpose sslserver CERT`. This admits intermediates in the configured server chain without trusting ambient system roots. Missing/unreadable/invalid/unrelated CA or mismatched cert/key rejects before staging completes or disruption; no alternate CA is saved. Activation revalidates manual TLS and resolves/probes `lsof` **before inhibits or stop**, and exclusivity uses that exact resolved executable. Missing/unusable lsof is a precondition failure without inhibits, stop or a leaked claim. Do not install dependencies or rewrite TLS evidence as part of a stopped transaction recovery.

Before disruption, validate all immutable candidate/controller/source evidence, including the selected exact migration-source hash in REQ-PD-023 or REQ-PD-029. Before stopping either unit, the owner installs/fsyncs transaction-bound persistent service **and** socket start-inhibit drop-ins, reloads systemd and verifies both the files and effective configuration. The inhibit blocks manual/dependency/socket/reboot activation throughout disrupted pre-marker and unresolved post-marker states. Tampered/foreign/unsafe inhibit evidence fails closed: never blindly remove it or start a unit. The owner then stops **both** units, proves inactive/exclusive database ownership, and retains a pristine SQLite backup. It runs exact-candidate `--migrate-only` on an isolated backup copy and verifies exact target/integrity/FK and pristine backup before writing the live `migration_started` marker. Backup/rehearsal capacity or verification failure is pre-marker: only a proven unchanged predecessor may resume, never by replaying the backup. No launchd 20 GiB free-space floor applies. Status remains `activating` across the live boundary; the durable marker file and `migration_started` boolean carry the boundary, not a separate status state. Remove only the verified matching inhibit, with durable removal/reload/config verification, after authorized pre-marker unchanged-predecessor artifact restoration before predecessor restart, or immediately before exclusively verified exact-target candidate startup. Failed/unproven pre-marker rollback retains the claim/inhibit as `activation_failed_rollback_failed` (not the ordinary-mode failure state).

### Explicit interrupted-transaction recovery

Use only the exact retained transaction: `./dev.py prod recover-systemd-current TXN` (helper action `recover-current`). This is not startup auto-recovery or permission to clear a fence. [ADR-117](../../specs/adrs/117_recover-interrupted-pre-marker-systemd-activation.md) records the bounded systemd-only policy; launchd is unaffected.

**No marker:** admit only the exact active protected transaction in durable `activating` or `activation_failed_rollback_failed`, `migration_started=false`, with matching inhibits and transaction-bound predecessor unit state. Validate retained manifest/controller/helper/rollback artifacts, stop both units, probe lsof after stop and prove exclusive live integrity/FK/exact source-prefix65. Normal pre-stop writes are allowed before a stopped snapshot; a stale preflight digest is not an authority. Any snapshot evidence requires the complete valid proof pair and exact live logical equality.

Restore and verify only captured predecessor runtime/config while inhibited; remove matching inhibits/reload/verify, then restore and verify active/enabled state and applicable predecessor identity. Publish durable `activation_failed_rolled_back` before exact claim release. Never replay a backup or invoke/start a candidate on this branch. Missing dependencies or malformed/ambiguous disrupted evidence stays fenced; failed restoration attempts inhibit reinstatement and both-unit stop.

**Prepared before disruption:** a crash before predecessor-state publication leaves `prepared`, the active claim and no inhibit/stop/marker. Recovery refuses to guess state. With validated retained evidence and no competing owner, use the retained root helper's `abandon --manifest /var/lib/phoenix-ide-deploy/transactions/TXN/manifest.json` path. Never manually clear the claim. A disrupted transaction missing state is not eligible for this prepared-only operation.

**Marker present (including unsafe marker evidence):** never restore a backup/predecessor artifacts or start the old binary. Recovery first stops both fixed units/proves inactivity, before detailed manifest/dependency/inhibit/marker/evidence/DB validation. It then probes lsof and proves exclusive ownership. Unavailable/unusable lsof fails stopped, not with the old runtime left running.

The post-marker branch uses only the retained exact candidate/controller/helper, matching claim/valid marker and intact backup/rehearsal/configuration evidence with a complete same-registry prefix65..128. At exact target it may finalize without rerunning migration. Unknown/divergent evidence stays fenced; failure attempts inhibit retention/reinstallation and both-unit stop. Remove inhibits only immediately before verified exact-target candidate start, not merely because recovery was requested. Retain the pristine backup after success. Do not clear prepared/ambiguous/post-marker claims as stale staging; only positively identified partial staging may retire under the global operation lock.

## If the deploy fails

- `Failed at step USER … No such process` → service user (`phoenix-dev` or `exedev`) does not exist; create it (see above)
- `Permission denied` on startup → `/var/lib/phoenix-ide/` not owned by the service user; deploy handles this, but check with `ls -la /var/lib/phoenix-ide/`
- `{"error":"Directory does not exist"}` from client → service user lacks traversal on the home dir; add it to the user's group (see above)
- Check `./dev.py check` output — deploy aborts on test/lint failure
- Check `journalctl -u phoenix-ide` for runtime errors after deploy
- Do NOT manually run `systemctl start/stop` — use `./dev.py prod deploy/stop`
