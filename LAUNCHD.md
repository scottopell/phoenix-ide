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

## Sealed exact current-schema forward repair

**Controller protocol authored, not qualified for execution.** ADR-126 /
REQ-LDD-025 covers only the exact original prepared candidate → repair candidate
pair, embedded RC4 version and unchanged complete registry128 digest in the
[REQ-COMP-015 pin table](specs/compatibility/requirements.md#req-comp-015--exact-current-schema-candidate-only-forward-replacement).
The repair application source is the merged, hosted-qualified PR #946 successor,
which includes the PR #944 runtime repair and its build-fixture correction; the
separate controller protocol still requires its own hosted CI, exact-head
review, merge, prepared-artifact qualification, disposable macOS journeys and
production authorization. Full SHA pins are centralized in the table and helper
constants. Source qualification alone is not production authorization, and no
behavioral test/build/check/Allium run is claimed for this controller draft.

After those gates and explicit production authorization, use a clean, separate
controller checkout, the exact original transaction ID, and the qualified
prepared-artifact directory (server-local paths):

```bash
./dev.py prod repair-current TXN \
  --prepared-artifact DIR \
  --expected-full-commit REPAIR_FULL_SHA
./dev.py prod status
# Only to retry the already sealed child; no artifact/configuration arguments:
./dev.py prod resume-current-repair TXN
```

`REPAIR_FULL_SHA` must be the exact 40-character pin in REQ-COMP-015, never a tag,
short SHA, moving branch or arbitrary same-registry binary. Preparation requires
the original prepared qualification, committed/pending global status, committed
target128 proof, original active claim and operation lock, stopped service and
absent original/child helpers, exclusive valid target128 DB, retained restore
capacity plus the protected floor/evidence allowance, the captured private
parent plist, and either the exact installed original binary or the already
installed stopped repair fence. It does not read ambient configuration or
acquire a child claim.

The sole selection boundary is
`<parent>/current-schema-repair/manifest.json`. Before sealing, the controller
retains private candidate/helper/receipt/registry/plist and initial child status,
plus a byte-exact immutable **`parent-status.json` snapshot** of original
committed/pending status, whose SHA-256 is bound in the child manifest. After
full admission and before sealing, it atomically replaces the installed original
binary with a non-executable empty stopped fence and removes the published target
plist. This makes historical retained helpers and login/reboot persistence unable
to start the original candidate while leaving its private transaction evidence
intact. It never writes parent status or replaces parent artifacts/proof. While unresolved, live
global status must still match that snapshot. After child terminal commit,
historical audit uses the snapshot even if a later legitimate deployment
replaces global status. Status/ownership classification reads bounded stored
evidence and hashes, not the live DB or backup. Activation/resume additionally
validates full original backup/proof and the exclusive offline live target
before install/reconciliation/start.

| Child evidence / boundary | Supported action |
| --- | --- |
| Unsealed partial, no checkpoint | Only known private owned staging may be cleaned under the original lock; unknown evidence stays retained. No activation authority. |
| Sealed prepared / interrupted reconciliation | Resume only this same retained repair. Persist `reconciliation_started` before exact installed repair `--migrate-only`; ambiguous invocation may repeat only that repair. |
| Target proof / start attempted | Revalidate target and start only the repair; never repeat reconciliation. |
| Committed, finalization pending | If exact repair is running, verify bytes/loaded captured plist/health and retry publication/cleanup without live-DB reads or runtime restart. If stopped, verify offline target and restart only the repair. Missing already-cleaned reservation does not force reconciliation. |
| Terminal committed | Historical classifier uses snapshot/proof. Explicit command reentry also verifies installed/runtime/publication identity; only an outstanding exact original claim release may mutate state. |
| Failure with valid checkpoint | Attempt stop; record actual `failed_stopped` or `stop_unconfirmed`, retain the original claim and both proofs. Resume only the same sealed child. |
| Malformed child status | Hash-bound parent routing plus exact original ownership still attempts stop. Preserve malformed bytes, claim and proofs; report stop confirmation/uncertainty without inventing a checkpoint or terminal success. Diagnose evidence rather than editing/resealing it. |

Read child `helper.log` for helper teardown truth and the controller's stderr for
pre-handoff failures; do not infer confirmed stop from a generic pending/invalid
status label. Missing/wrong-owner routing cannot authorize teardown of another
transaction. Never delete a claim, edit retained evidence, invoke historical
helpers directly, run original `finalize-current`, launch the original or
predecessor, or replay a snapshot to bypass the repair fence. Child success
publishes repair SHA/plist, fsyncs owned cleanup, retains original backup and both
proofs, persists terminal child status, then releases/fsyncs only the original
claim. Claim-release durability failure retains/restores visible original
ownership for explicit retry.

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
