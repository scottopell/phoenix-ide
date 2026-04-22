---
name: phoenix-task-tracking
description: Task tracking conventions for Phoenix IDE. Use when creating a task, updating task status, filing a bug found during work, or checking what tasks are ready to work on.
---

# Phoenix IDE Task Tracking

## Happy path: `taskmd new`

Always create tasks with `taskmd new`. It atomically allocates the next ID,
synthesizes the frontmatter, and writes the file. **Do not** write task files
directly, and **do not** use `taskmd next` + manual file writes — both risk ID
collisions and frontmatter drift.

```bash
echo 'Brief description of what needs doing and why.' \
  | taskmd new --slug fix-login --artifact src/auth.py --priority p1
```

Required:
- `--slug` — URL-safe slug (dirty input is normalized). E.g. `fix-login`.
- `--artifact` — the concrete output this task produces (file path, config
  change, commit). If you can't name one, the task probably shouldn't exist.
- stdin — the task body, non-empty. No frontmatter in the body; `taskmd`
  synthesizes it.

Optional:
- `--priority` — `p0` (critical) … `p4` (nice-to-have). Default `p2`.
- `--status` — default `ready`. Valid: `ready`, `in-progress`, `blocked`,
  `brainstorming`, `done`, `wont-do`.

Piping multi-line bodies:

```bash
cat <<'EOF' | taskmd new --slug ws-keepalive --artifact src/terminal/relay.rs --priority p1
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

### Required frontmatter

```yaml
---
created: YYYY-MM-DD
priority: p2
status: ready
artifact: src/auth.py
---
```

Filename must match frontmatter. `./dev.py check` enforces this.

## Updating a task

Transition status with `taskmd status`:

```bash
taskmd status 24691 in-progress
taskmd status 24691 done
```

The file is renamed and frontmatter updated in one step.

## Maintenance

```bash
ls tasks/*-ready--*.md       # List tasks ready to work on
./dev.py tasks validate      # Check all files for format errors
./dev.py tasks fix           # Auto-repair: inject missing 'created', rename to
                             # match frontmatter, migrate legacy ID formats,
                             # resolve duplicate IDs
```

`fix` cannot handle (requires human): missing `status`, missing `priority`,
missing `artifact`, invalid field values.

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
