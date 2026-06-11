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
