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

**Always use `./dev.py` for development tasks.** It handles configuration automatically.

```bash
# Server management
./dev.py start         # Start server (release build, port 8000)
./dev.py start --debug # Start with debug build
./dev.py start --port 9000  # Use different port
./dev.py stop          # Stop server
./dev.py status        # Check if server is running
./dev.py restart       # Restart server

# Build & lint
./dev.py lint          # Run clippy + fmt check
./dev.py build         # Build project

# Task management
./dev.py tasks ready   # List tasks ready for implementation
./dev.py tasks close <id> [--wont-do]  # Close a task
```

### ⚠️ Do NOT start the server manually

Do NOT use `cargo run` directly. The server requires the LLM gateway configuration
which `./dev.py start` provides automatically from `/exe.dev/shelley.json`.

If you need to test with a running server:
1. `./dev.py status` - Check if already running
2. `./dev.py start` - Start if needed
3. `./dev.py restart` - Restart after code changes
