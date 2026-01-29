# Agent Instructions for phoenix-ide

## Task Tracking

Tasks are tracked in `tasks/NNN-slug.md` files with YAML frontmatter.

Use `./dev.py tasks ready` to see tasks available for implementation.

---

## ⚠️ CRITICAL: Issue Discovery Protocol

**When you encounter ANY issue during your work—whether or not it's related to your current task—you MUST take action.**

### The Rule

> **Finding a bug is not the end. Finding a bug is the beginning of a task.**

Do NOT:
- Note an issue and move on
- Say "this is unrelated to current changes" and ignore it
- Delete regression files to make tests pass
- Work around problems without documenting them

Do:
- **Immediately create a task** in `tasks/` documenting the issue
- Include reproduction steps, error messages, and context
- Assign appropriate priority (p1-p3)
- Then continue with your original work

### Why This Matters

Issues discovered incidentally are often forgotten. The cost of creating a task is ~30 seconds. The cost of rediscovering an issue later is much higher. When in doubt, create the task.

### Example

Bad:
```
"There's a flaky test here. This isn't related to our changes—let me run it again."
```

Good:
```
"There's a flaky test here. Creating tasks/009-fix-flaky-xyz-test.md before continuing."
```

---

## Development Commands

```bash
./dev.py lint          # Run clippy + fmt check
./dev.py build         # Build project
./dev.py tasks ready   # List tasks ready for implementation
./dev.py tasks close <id> [--wont-do]  # Close a task
```
