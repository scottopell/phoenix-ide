# macOS launchd Deployment

`./dev.py prod deploy` installs Phoenix as a per-user launchd agent on macOS:

| Property | Value |
|----------|-------|
| Port | `8031` |
| Binary | `~/.phoenix-ide/phoenix-ide` |
| Database | `~/.phoenix-ide/prod.db` |
| Logs | `~/.phoenix-ide/prod.log` |
| launchd label | `com.phoenix-ide.server` |
| plist | `~/Library/LaunchAgents/com.phoenix-ide.server.plist` |

## Socket activation and `.local` hostnames

Phoenix's macOS production plist declares a launchd socket named `Listeners`.
The binary calls `launch_activate_socket("Listeners", …)` at startup and adopts
that socket when launchd provides it. In socket-activated mode, SIGHUP exits
immediately so launchd can restart Phoenix while keeping the listener open.

The generated plist includes this socket dictionary:

```xml
<key>Sockets</key>
<dict>
  <key>Listeners</key>
  <dict>
    <key>SockFamily</key>
    <string>IPv4v6</string>
    <key>SockProtocol</key>
    <string>TCP</string>
    <key>SockServiceName</key>
    <string>8031</string>
    <key>SockType</key>
    <string>stream</string>
  </dict>
</dict>
```

`SockFamily=IPv4v6` makes launchd create one dual-stack listener. That is useful
for Bonjour / mDNS names such as `my-mac.local`: iOS Safari often tries IPv6
before IPv4, so the launchd-owned listener accepts both families without
changing Phoenix's normal non-activated bind address.

## Verification

After deployment, check the production log for:

```text
Using launchd-provided TCP listener
```

Then verify both loopback families work:

```bash
curl http://127.0.0.1:8031/
curl http://[::1]:8031/
```

`lsof -p <pid> -iTCP` should show Phoenix using an IPv6-family listener supplied
by launchd.

## Operations

Use the deploy helper instead of manual `launchctl load` / `launchctl unload`:

```bash
./dev.py prod deploy
./dev.py prod status
tail -f ~/.phoenix-ide/prod.log
./dev.py prod stop
```

## Prepared-artifact paired upgrade (manual recovery)

The supported paired path is explicit and launchd-only:

```bash
./dev.py prod deploy --prepared-artifact DIR --expected-full-commit FULL_SHA --paired-database-upgrade
```

`DIR` must contain the protected prepare-main receipt and exact standalone
`phoenix_ide-{host-target}-prepared-{FULL_SHA:12}` binary. The candidate is
strict-codesign checked without ad-hoc resigning; the clean controller checkout
supplies and byte-binds the activation helper. Installed plist environment and
PATH are reused, and the configured database path must match candidate and
predecessor plists.
The artifact directory is trusted operator input downloaded from the protected
preparation run. The controller validates local contents, not the receipt's
workflow origin; establish the run/source association before invocation.

Before production stop, paired activation physically reserves private snapshot
and atomic-restore capacity sized for the database plus committed WAL and margin.
It snapshots the stopped legacy database through SQLite's backup API only after
bounded `lsof` exclusivity proof. It requires an existing
migration ledger at version 69 or earlier and no ProductConversation tables.
The private transaction directory, backup, proof, and active claim are retained
for recovery. Candidate health failure restores the verified matching database,
binary and plist before predecessor startup. If stop, ownership, snapshot or
restore proof fails, service remains stopped with rollback failure; do not delete
that transaction or restart blindly. Inspect the proof, run SQLite integrity
checks offline, and restore the matching predecessor binary, plist, and database
before starting it.

For a paired transaction interrupted in `prepared`/`activating`, with missing
status, or retained in `activation_failed_rollback_failed`, never remove the
active marker merely because its helper exited. Repair the named failed proof,
then run `./dev.py prod recover-paired TRANSACTION_ID`. This hands recovery to the
byte-bound retained helper, which verifies offline ownership and snapshot/context,
restores the matching database/binary/plist, and verifies predecessor identity
before claim release. Any unverified recovery attempts teardown and retains the
claim. The command is not a general downgrade or an activation retry.
Before a snapshot proof exists, recovery is permitted only after verifying that
the installed runtime/configuration still match the captured predecessor and the
exclusively owned database remains legacy. It resumes that verified unchanged
predecessor without fabricating a snapshot or claiming database restoration.
A missing proof with changed runtime or modern database fails closed; it is not permission to discard the ownership fence.
An interrupted `preparing` transaction before handoff uses the same command,
even if its immutable manifest was already persisted:
only matching preparation metadata, a dead recorded PID, and confirmed helper
absence permit terminal status and owned-claim release. A live or reused PID,
missing metadata, or unknown launchctl error refuses recovery without touching
runtime/database. Initial preparation status is durable before claim publication.

Paired activation quarantines the published predecessor before candidate mutation
and bootstraps the candidate from a private plist. It publishes the candidate
LaunchAgents plist only after exact identity and durable commit. Atomic hard-link
publication preserves the loaded private plist inode, so installed-runtime restart
continues to require the exact same file rather than accepting lookalike contents. Paired rollback
also bootstraps privately and publishes only after matched database authorization
and predecessor identity. Successful commit removes only the database-adjacent
restore reservation; private audit backup/proof survive. Verified no-snapshot
resume removes only the unproven backup seed and temporary restore reservation. A post-commit publication
or cleanup error is displayed as a committed warning, never an automatic rollback.
Durable commit records a pending warning before these attempts; interruption
retains the claim and warns that publication/cleanup is incomplete. Do not
invoke database rollback or clear that committed claim merely because its helper
is absent.
A missing published plist means login/reboot persistence is not established even
though the verified candidate is running; do not infer recovery from claim release.

Unresolved paired rollback quarantines the runtime's LaunchAgent plist outside
`~/Library/LaunchAgents`, with durable directory synchronization, so login does
not auto-start an unverified job. Verified paired recovery reinstalls the matching
predecessor plist after database restoration. Quarantine or teardown failure is
reported explicitly; an active claim alone is not proof the operating system
cannot run a job.

### Interrupted committed paired finalization

Run `./dev.py prod finalize-paired TRANSACTION_ID` only for the displayed retained
committed transaction. The synchronous retained helper proves old-helper absence,
exclusive ownership and the exact running candidate/configuration, then retries
only private-plist publication and temporary restore-reservation cleanup. It never
stops/bootstraps the runtime or reads/writes the database. Success prints
`Paired finalization complete: …; no runtime/database action.`; interrupted/error
attempts retain pending status and claim. A completed rerun verifies the same
candidate and published inode rather than applying current repository settings.

Recovery records a durable startup checkpoint before predecessor startup. After
that point, a retry verifies/finalizes the running predecessor without replaying
the original snapshot; unknown or stopped predecessor remains fenced for explicit
operator investigation. This avoids deleting writes accepted after earlier recovery.

## Explicit ordinary modern migration (stopped/fenced)

This is separate from legacy prepared paired deployment. It accepts normal local HEAD or published release sources only and preserves installed environment/PATH:

```bash
./dev.py prod deploy --migration-backup-receipt /absolute/private/receipt.json
./dev.py prod resume-migration TRANSACTION_ID
```

The service must already be stopped with exclusive database ownership. Before receipt creation, safely checkpoint/close SQLite and prove no remaining WAL/SHM/journal sidecars; do **not** discard possibly uncheckpointed sidecars. Create a private SQLite-backup-API snapshot and a restoration rehearsal file, fresh-open integrity/ledger checks, and fsync files/parents. Reserve capacity for retained copies in addition to operator backups. Receipt schema1 has exactly these keys (digests are lowercase SHA256; paths canonical absolute regular files):

```json
{"schema":1,"database_path":"/private/live.db","database_sha256":"HEX64","backup_path":"/private/backup.db","backup_sha256":"HEX64","rehearsal_path":"/private/rehearsal.db","rehearsal_sha256":"HEX64","previous_binary_sha256":"HEX64","previous_plist_sha256":"HEX64"}
```

The selected-source helper must advertise this capability; an older release helper refuses before handoff. It validates full logical schema/all-row equivalence between stopped database and backup/rehearsal, not just equal ledger or operator hashes. Backup bytes may differ from source after SQLite backup/VACUUM; rehearsal bytes must match backup. Receipts/backups/configuration stay private; they do not authenticate release provenance. Receipt, backup and rehearsal originals with any group/other permission bits are rejected before staging; private retained copies do not make exposed originals safe. Ledger admission uses the selected source's byte-bound actual migration version/name registry, including intentional version gaps; it does not require every integer through the maximum. Aborted preparation before bootstrap durably removes staged private copies before releasing its claim. Cleanup interruption retains preparing ownership for explicit dead-controller/absent-helper cleanup retry, with no runtime or database action.

After disruption, failure never auto-starts predecessor or restores SQLite. It attempts confirmed teardown, quarantines autoload plist, and retains transaction/claim with `migration_failed_stopped`; deploy/restart/stop refuse. Manually restore the matched backup offline, safely settle sidecars, and then invoke `prod resume-migration TXN`. That retained helper verifies exact restored bytes/content, captured binary/configuration, stopped ownership and helper absence before explicit predecessor startup. A persisted resume-start checkpoint refuses startup replay rather than risking writes accepted after startup. Unknown proof stays fenced; no manual claim removal, prepared eligibility widening, generic downgrade or Linux recovery is offered. If durable `activation_failed_rolled_back` with `recovery_mode = migration_resumed` still owns the claim, the same resume command verifies the running captured predecessor and published/private configuration, then releases only that claim. This terminal retry never stops or restarts the predecessor or rechecks/restores a database that may contain later accepted writes; verification failure leaves terminal status and the fence unchanged.
