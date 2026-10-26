Prevent terminal PTY descriptors surviving into unrelated executables.

## Confirmed mechanism and fix scope

`spawn_pty` allocates inheritable master/slave descriptors through `openpty`.
`terminal_ws::build_pty_io` also creates an inheritable relay master through
`libc::dup`. The terminal child closes only its newly allocated endpoints before
exec, leaving earlier terminals' descriptors eligible for inheritance.

REQ-TERM-024 requires atomic close-on-exec allocation for both PTY endpoints and
relay copies. Cover raw fork/exec inheritance with a deliberately inheritable
positive control, endpoint flags, relay flags, and existing usable-stdio/reclaim
tests. Do not introduce a global spawn lock or alter disconnect policy.

## Host investigation and authorized cleanup, 2026-10-26

Read-only enumeration found 47 durable Phoenix tmux servers with 364 panes,
plus three reachable temporary test servers and 62 servers whose `/private/tmp/ptt-*`
directories and sockets were deleted. Every one of those 62 had only one sleeping
zsh child, no child jobs, and only its original listening Unix socket (no clients).
Fresh per-process revalidation preceded SIGTERM to those exact servers; all 62
servers and 62 shells exited. Distinct open PTY slave devices fell from 508 to
446, and an immediately closed allocation probe succeeded. All 47 durable
servers and 364 panes remained reachable. Kernel limits were unchanged.

The 62 abandoned tests explain 62 occupied PTYs. Shared master holders are
additional retention evidence, not proof that every remaining PTY is leaked.
The production descriptor fix does not revoke descriptors already inherited by
existing processes; no existing user session is terminated by this work.

## Earlier investigation

Observed during context-capacity qualification: terminal handoff fixture fails at openpty with ENXIO before lifecycle assertions; an independent Python os.openpty call fails with the same host error. kern.tty.ptmx_max is 511. Read-only lsof inspection found many existing tmux processes holding multiple PTY masters, including masters shared across process families. This is evidence of exhaustion; it does not by itself prove a leak.

Trace crates/phoenix-terminal/src/spawn.rs: spawn_pty opens a PTY, forks, closes the new master in the child and execs the shell/tmux client. No FD_CLOEXEC setting appears in that module. Determine whether unrelated existing PTY masters can survive subsequent child execs and keep retired terminals allocated. Add a regression that inspects descriptor inheritance and proves master lifetime after the owning terminal closes; preserve intended stdin/stdout/stderr and supported terminal reclaim.

Do not terminate user sessions or raise host kernel limits as an automatic cleanup. Separate any product descriptor fix from explicitly authorized host-session cleanup. Current context-capacity work tracks this as an environment-blocked terminal gate and does not claim this suspected root cause is validated.

2026-10-25 host recovery: the user authorized easy recovery and asked to retain this as follow-up. Inventory found one unattached Phoenix tmux server with one idle zsh, no child jobs, and a deleted working directory. Closing only that orphan restored os.openpty; a bounded probe found just one free PTY pair afterward. The largest other server held 208 windows with three attached clients, so it was left untouched. Investigate retained windows as well as descriptor inheritance; neither test ownership nor a leak is established by the aggregate count. No host kernel setting or active user server was changed.

Subsequent same-day recovery found three more old unattached single-pane servers with idle shells, no child jobs and deleted temporary working directories. Fresh checks preceded closing only those three; all six server/shell processes disappeared and distinct live TTYs fell from 507 to 504. One allocation probe succeeded and immediately closed its descriptors. Active sessions and kernel limits remained untouched; this is not a guarantee of broad free capacity.

Separate terminal-test portability evidence: a macOS stdlib PTY reproduction shows that an interactive shell requesting exit can remain in kernel exit while unread master output drains. Reading the pending output lets it exit and be reaped. The natural-exit fixture drains its test output while awaiting the existing cleanup receipt; it does not attach, reconnect or invoke Close. This does not add a product detached-output reader. REQ-TERM-017 observes actual child exit; assess unread-output behavior separately before changing product reader ownership or claiming a descriptor-leak root cause.
