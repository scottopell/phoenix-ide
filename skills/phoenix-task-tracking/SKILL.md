---
name: phoenix-task-tracking
description: Task tracking conventions for Phoenix IDE. Use when creating a task, updating task status, filing a bug found during work, or checking what tasks are ready to work on.
---

# Phoenix IDE Task Tracking

## Happy path: `taskmd new`

Always create tasks with `taskmd new`. It atomically allocates the next ID,
formats the filename, and writes the file with the body from stdin. **Do not**
write task files directly, and **do not** use `taskmd next` + manual file writes
— both risk ID collisions.

```bash
echo 'Brief description of what needs doing and why.' \
  | taskmd new --slug fix-login --priority p1
```

Required:
- `--slug` — URL-safe slug (dirty input is normalized). E.g. `fix-login`.
- stdin — the task body, non-empty. Free-form markdown — **no frontmatter**;
  the filename is the sole source of task metadata.

Optional:
- `--priority` — `p0` (critical) … `p4` (nice-to-have). Default `p2`.
- `--status` — default `ready`. Valid: `ready`, `in-progress`, `blocked`,
  `brainstorming`, `done`, `wont-do`.

Piping multi-line bodies:

```bash
cat <<'EOF' | taskmd new --slug ws-keepalive --priority p1
# Title

## Summary
What needs to happen.

## Acceptance Criteria
- [ ] ...
EOF
```

## File format (reference — produced by `taskmd new`)

```
tasks/NNNNN-pX-status--slug.md
```

Example: `tasks/24691-p1-ready--terminal-ws-keepalive-reap-stale-sessions.md`

| Segment | Meaning |
|---------|---------|
| `NNNNN` | 5-digit task ID (`taskmd` allocates; don't hand-craft) |
| `pX` | Priority `p0`–`p4` |
| `status` | `ready`, `in-progress`, `blocked`, `brainstorming`, `done`, `wont-do` |
| `slug` | Short hyphenated description |

The **filename is the sole source of task metadata** — there is no frontmatter.
The body is free-form markdown. `./dev.py check` (the `task validation` lane,
also `./dev.py tasks validate`) enforces the filename pattern and the absence of
duplicate IDs.

## Updating a task

Transition status with `taskmd status`:

```bash
taskmd status 24691 in-progress
taskmd status 24691 done
```

This renames the file — the `status` segment of the filename changes — and that's
the whole operation; there is nothing else to keep in sync.

## Maintenance

```bash
ls tasks/*-ready--*.md       # List tasks ready to work on
./dev.py tasks validate      # Check all files for filename-format errors
./dev.py tasks fix           # Auto-repair: migrate legacy ID formats,
                             # rename files to match the pattern, resolve
                             # duplicate IDs
```

`fix` cannot handle (requires human): a filename that doesn't parse at all, or
genuinely ambiguous duplicates.

## Issue Discovery Protocol

> **Finding a bug or problem is the start of a task, not an observation to note and move on.**

When you encounter ANY issue during work — even if unrelated to your current task:

1. Create a task with `taskmd new`, including what the problem is, how to
   reproduce it, and relevant context.
2. Then continue with your original work.

**Do NOT:**
- Note an issue and move on without filing it.
- Say "this is unrelated" and ignore it.
- Work around a problem silently.
