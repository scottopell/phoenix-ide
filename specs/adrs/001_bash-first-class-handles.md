# ADR-001: Bash handles are first-class process-local entities with wait windows

- **Status:** Accepted
- **Date:** 2026-06-29
- **Affects:** REQ-BASH-001, REQ-BASH-002, REQ-BASH-003, REQ-BASH-005, REQ-BASH-009, REQ-BASH-014, REQ-BASH-WS-001, REQ-BASH-WS-002

## Context

The bash tool needs to support long-running commands without forcing the agent
into a binary choice between blocking for the entire duration or losing access
to the running process. The previous shapes in the design material all pushed
in that direction: a kill-on-timeout model, a detached background/PID model,
and a fire-and-forget `spawn` prior. Those shapes make the process less
addressable exactly when the agent still needs to inspect, wait, or terminate
it.

The design material also makes two boundary constraints explicit:

- handles are owned by the spawning `WorkScope`, not by a global process table
- handles are in-memory only and do not survive Phoenix restart; `tmux` is the
  separate persistence path for that use case

## Options considered

1. **Kill-on-timeout / fire-and-forget process control** — let the run call act
   as a timeout wrapper that kills the process when its wait window elapses, or
   detach it and hand back a PID plus temp-file path. This keeps the surface
   simple, but it forces the agent to choose between waiting all the way or
   losing the process, and it makes the timeout itself behave like a kill.
2. **First-class handle with a wait window** — let `wait_seconds` describe how
   long the call should block before returning a handle, while the process keeps
   running and remains addressable through `peek`, `wait`, and `kill`. This is
   the model described in the bash design material.
3. **Persistent cross-restart job state** — make handles durable across Phoenix
   restart. This would satisfy persistence needs, but it would move the bash
   tool away from the in-memory, process-local shape the design calls for and
   would overlap with the separate `tmux` path.

## Decision

Use first-class process-local handles with a wait window. `wait_seconds` is the
amount of time the call blocks before returning a handle, not a process-kill
mechanism. When the wait window elapses, the process keeps running and the same
handle remains the stable reference for later `peek`, `wait`, and `kill` calls.
The handle table is keyed by `WorkScope`, and the handles remain in memory only
for the Phoenix process lifetime.

This preserves the agent's ability to make an informed second decision after a
partial wait, keeps long-running work inspectable, and keeps restart persistence
out of scope for bash itself.

## Consequences

- **Positive:** the agent can inspect, wait again, or kill an in-flight command
  instead of losing it at the first timeout.
- **Positive:** the handle itself becomes the durable reference within a Phoenix
  process, which matches the command lifecycle the tool already exposes.
- **Positive:** restart persistence remains cleanly delegated to `tmux` instead
  of being mixed into bash.
- **Negative:** bash must maintain in-memory handle state for the Phoenix
  lifetime, so restart loses the table.
- **Negative:** the tool surface has to keep the wait-window wording explicit so
  the timeout/kill prior does not reappear in the description.
- **Neutral:** the same handle can be returned from multiple wait attempts until
  the process exits or is killed.

## References

- Related ADRs: ADR-002
- Feature spec: `specs/bash/requirements.md`
- Behavioural spec: `specs/bash/bash.allium`
- Executive summary: `specs/bash/executive.md`

