---
name: phoenix-task-tracking
description: Task tracking conventions for Phoenix IDE. Covers file format, status/priority values, CLI commands, and the issue discovery protocol agents must follow.
---

# Phoenix IDE Task Tracking

## File Format

```
tasks/NNN-pX-status-slug.md
```

Examples:
```
tasks/042-p1-ready-fix-executor-deadlock.md
tasks/103-p2-in-progress-add-browser-click.md
tasks/210-p0-done-critical-data-loss-fix.md
```

### Fields

| Field | Values |
|-------|--------|
| `NNN` | Zero-padded task number (001, 042, 103…) |
| `pX` | Priority: `p0` (critical) → `p4` (nice-to-have) |
| `status` | `ready`, `in-progress`, `pending`, `blocked`, `done`, `wont-do`, `brainstorming` |
| `slug` | Short hyphenated description |

### Required frontmatter

Every task file must have:

```yaml
---
created: YYYY-MM-DD
priority: p2
status: ready
---
```

**The filename must match the frontmatter.** `./dev.py check` enforces this.

## Commands

```bash
ls tasks/*-ready-*.md       # List tasks ready to work on
./dev.py tasks validate     # Check all files for format errors
./dev.py tasks fix          # Auto-fix: inject missing 'created', rename to match frontmatter
```

`fix` handles:
- Missing `created` field (inferred from git log → file mtime → today)
- Filename out of sync with frontmatter status/priority

`fix` cannot handle (requires human): missing `status`, missing `priority`, invalid field values.

## Issue Discovery Protocol

> **Finding a bug or problem is the start of a task, not an observation to note and move on.**

When you encounter ANY issue during work — even if unrelated to your current task:

1. **Create a task file** in `tasks/` documenting the issue
2. Include: what the problem is, how to reproduce it, relevant context
3. Then continue with your original work

**Do NOT:**
- Note an issue and move on without filing it
- Say "this is unrelated" and ignore it
- Work around a problem silently

## Creating a task

Use the next available number and follow the format:

```bash
# Find the highest current number
ls tasks/*.md | sort | tail -5

# Create the file
cat > tasks/NNN-pX-ready-short-description.md << 'EOF'
---
created: YYYY-MM-DD
priority: pX
status: ready
---

# Title

## Summary
What needs to be done.

## Context
Why this task exists.

## Acceptance Criteria
- [ ] ...
EOF
```
