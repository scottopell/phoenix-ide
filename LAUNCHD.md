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

Paired activation snapshots the stopped legacy database through SQLite's backup
API only after bounded `lsof` exclusivity proof. It requires an existing
migration ledger at version 69 or earlier and no ProductConversation tables.
The private transaction directory, backup, proof, and active claim are retained
for recovery. Candidate health failure restores the verified matching database,
binary and plist before predecessor startup. If stop, ownership, snapshot or
restore proof fails, service remains stopped with rollback failure; do not delete
that transaction or restart blindly. Inspect the proof, run SQLite integrity
checks offline, and restore the matching predecessor binary, plist, and database
before starting it.
