# Launchd socket activation on macOS (parity with systemd)

Primary file: `src/hot_restart.rs`. Likely also `Cargo.toml` and an operator
docs page.

## Summary

Extend the listener-acquisition path in `src/hot_restart.rs` so that on macOS,
phoenix-ide can adopt a TCP socket opened by `launchd` and passed in via
`launch_activate_socket()`, analogous to the existing systemd `LISTEN_FDS`
path. Today only systemd activation is implemented; macOS deployments always
fall through to the "dev mode" fresh-bind branch.

The immediate motivating use case is **dual-stack binding without changing the
bind call**: launchd's plist `<Sockets>` dict can open `[::]:<port>` with
`SockFamily=IPv4v6`, and phoenix-ide just inherits a socket that already
accepts both families. Today phoenix-ide binds `0.0.0.0:8031` (IPv4 only), so
clients resolving `<LocalHostName>.local` via mDNS — which returns both IPv6
and IPv4 records — try v6 first, fail (connection refused on `::1`/`fe80::1`,
hang on the en0 link-local v6), and either fall back slowly or time out. On
iOS Safari this manifests as "works via LAN IP, hangs via `.local` hostname."

## Context

`hot_restart.rs:46-69` (the `get_listener` function) currently does:

```rust
let mut listenfd = listenfd::ListenFd::from_env();
if listenfd.len() > 0 {
    // systemd path: take FD 3, wrap as TcpListener, set SOCKET_ACTIVATED
}
// otherwise: bind fresh
```

The `listenfd` crate only handles the systemd `LISTEN_FDS`/`LISTEN_PID`
contract. Launchd uses a different mechanism — there is no env var to detect.
The process must call Apple's `launch_activate_socket(name, &mut fds, &mut
count)` (declared in `<launch.h>`), passing the **socket name** from the plist
`<Sockets>` dict. The call returns 0 and populates `fds`/`count` if launchd
provided sockets under that name, or returns ESRCH if the process wasn't
launched by launchd / no socket was registered.

Detection of "are we launchd-activated?" is therefore by *attempt*, not by env
sniff: call `launch_activate_socket("Listeners", ...)`; if it succeeds with
count > 0, adopt those FDs.

### Surfaces involved

- `src/hot_restart.rs` — `get_listener` gains a third branch (macOS launchd),
  ordered before the systemd branch or with a platform `cfg`. `SOCKET_ACTIVATED`
  flag should be set for launchd activation too, so SIGHUP-exits-immediately
  behavior also kicks in under launchd `KeepAlive`.
- `Cargo.toml` — either pull in a crate that wraps `launch_activate_socket`
  (e.g. `raunch`), or add a small `#[cfg(target_os = "macos")]` FFI block. The
  FFI surface is tiny (one function); a crate dep is mostly about avoiding
  unsafe-block bookkeeping.
- Operator docs — a sample `.plist` snippet showing the `<Sockets>` dict with
  `SockFamily=IPv4v6` so users get dual-stack out of the box.

## Open design questions (resolve before implementing)

- **Socket name convention.** What string does phoenix-ide pass to
  `launch_activate_socket`? Suggest `"Listeners"` — that's the de facto
  convention in Apple's own examples and matches the plist key name users will
  copy/paste. Document it; users authoring plists need to match it exactly.
- **Crate vs raw FFI.** `raunch` is a thin wrapper but adds a dep that exists
  only to call one function on one OS. A `#[cfg(target_os = "macos")]` extern
  block is ~10 lines and avoids the dep. Recommend raw FFI — clearer and
  surface-matched to use site.
- **Detection order.** If `LISTEN_FDS` is set AND launchd also has a socket
  for us (shouldn't happen in practice, but theoretically possible under
  cross-init test harnesses), which wins? Recommend: probe systemd first
  (cheap env check), then launchd. The two are mutually exclusive on real
  deployments.
- **`is_socket_activated()` semantics.** Keep returning a single bool, OR
  expand to an enum `Activation::{None, Systemd, Launchd}`? The current SIGHUP
  handler only branches on yes/no, so bool is fine — but if future code needs
  to distinguish (e.g. logging, telemetry), an enum is cheap to add now. Lean
  enum.
- **Fallback when launchd activation fails.** If launchd is detected but the
  socket adoption errors, should we fall through to fresh-bind (current
  systemd code returns an error), or hard-fail? Recommend hard-fail with a
  clear error — silent fallback hides plist misconfiguration.

## Acceptance Criteria

- [ ] On macOS, launching phoenix-ide via a launchd plist that declares a
      `<Sockets>` entry with `SockFamily=IPv4v6`, `SockServiceName=8031`
      results in `get_listener` adopting the launchd-supplied FD rather than
      binding fresh. Verifiable via `lsof -p <pid> -iTCP` showing an IPv6
      socket family AND startup log line "Using launchd-provided TCP listener".
- [ ] Both `curl http://127.0.0.1:8031/` and `curl http://[::1]:8031/` succeed
      against a launchd-activated instance (single dual-stack listener).
- [ ] `is_socket_activated()` returns true under launchd activation, so
      SIGHUP triggers immediate exit (launchd `KeepAlive=true` restarts with
      the same socket).
- [ ] Non-activated `cargo run` / `./dev.py up` path is unchanged: still
      binds whatever address the caller passed.
- [ ] Sample plist snippet added to operator docs (likely a new section in
      `TLS.md` or a sibling `LAUNCHD.md`) showing the `<Sockets>` dict and
      the `SockFamily=IPv4v6` line, with a callout that this is what gives
      you `.local` hostname access from iOS.
- [ ] Linux build remains green (the macOS FFI block must be cfg-gated).

## Notes

- Apple's API reference: `launch_activate_socket(3)` — the entire useful
  surface is one function. Header: `<launch.h>`. The deprecated `launch_msg`
  API is what older docs reference; do not use it.
- `SockFamily=IPv4v6` on a macOS launchd socket creates an `AF_INET6` socket
  with `IPV6_V6ONLY=0`, which on Darwin is the same default the kernel uses
  for `::` binds. Equivalent end state, just owned by launchd.
- Worth confirming whether `configure_tcp_options` (keepalive/user_timeout)
  applies cleanly to a launchd-passed FD. The systemd path already does this
  successfully, so the answer is almost certainly yes, but flag it during
  implementation.
- Once this lands, the phoenix-bridge dual-purpose-launchd-agent pattern
  (used on the user's home setup) becomes the recommended way to expose
  phoenix-ide on a `.local` name to iOS / other LAN devices — no source
  patches, no v6 sidecar forwarder.
