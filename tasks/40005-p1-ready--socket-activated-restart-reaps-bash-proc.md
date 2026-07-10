# Reap live Bash process groups during socket-activated restart

## Problem

A Bash tool command can survive a completed Phoenix conversation and a production restart. A confirmed incident left `cargo test -q stream_query_prefers_after_event_sequence_and_ignores_legacy_after_sequence -- --nocapture` and its test child running for more than two days, consuming build artifacts until the host had only 476 MB free.

The command was launched by conversation `6622f7e4-07f4-4edd-9de0-32bd84f8f04c` / sub-agent `62337b8e-5f8f-4dbc-b98e-54f0f8bedd02` in work scope `7a602323-7882-4489-a2ad-a0c4182128f9`. The surviving Cargo process remained process-group leader (`PID/PGID 63752`), so the existing process-group kill mechanism would have terminated it if invoked.

## Root cause

Production uses launchd socket activation. In `hot_restart::shutdown_signal`, socket-activated SIGHUP calls `std::process::exit(0)` immediately. This bypasses Rust destructors and the `shutdown_kill_tree(&bash_handles_for_shutdown)` call after server shutdown in `main`. The old Phoenix process exits, its Bash child is reparented to PID 1, and launchd starts the replacement server with an empty in-memory handle registry that can no longer discover the orphan.

Evidence:

- `hot_restart::shutdown_signal` exits directly for socket-activated SIGHUP.
- `main` invokes `shutdown_kill_tree` only after the HTTP/TLS server returns.
- launchd plist supplies the production listener through socket activation.
- production restarted after the orphan was launched; the child survived with PPID 1 and its original process group intact.

## Required behavior

Socket-activated restart must preserve listener handoff while still running bounded child-process cleanup before the old Phoenix process exits. No shutdown path may bypass the Bash kill-tree pass.

## Acceptance criteria

- [ ] Socket-activated SIGHUP returns control to the normal bounded shutdown sequence instead of calling `std::process::exit` from the signal handler.
- [ ] Every graceful production shutdown/restart path invokes `shutdown_kill_tree` before process exit, for both HTTP and HTTPS serving paths.
- [ ] Listener continuity under launchd/systemd socket activation remains intact.
- [ ] An integration test launches a real long-running Bash process group, triggers the socket-activated restart path, and proves the group is gone before old-process exit.
- [ ] Tests cover SIGTERM/SIGINT, non-socket SIGHUP, and socket-activated SIGHUP shutdown decisions without sending real process-exit calls from testable logic.
- [ ] Shutdown logs make the kill-tree pass and any signal failures observable.
- [ ] The Bash and hot-restart specs accurately describe the bounded cleanup ordering.
