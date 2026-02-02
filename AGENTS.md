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

## Code Conventions

### Module Organization

Use named module files (e.g., `foo.rs` + `foo/` subdirectory) instead of `foo/mod.rs`. This is enforced by the `mod_module_files` clippy lint.

✅ Correct:
```
src/
  tools.rs           # Module entry point
  tools/
    bash.rs          # Submodule
    patch.rs         # Submodule with its own children
    patch/
      planner.rs     # Nested submodule
```

❌ Wrong:
```
src/
  tools/
    mod.rs           # Legacy style - forbidden
    bash.rs
```

The modern style is preferred because:
- File names are more descriptive in editor tabs
- Easier to navigate in file trees
- Rust 2018 edition recommendation

---

## Development Commands

**Always use `./dev.py` for development tasks.** It handles LLM gateway configuration automatically.

```bash
# Server management
./dev.py up            # Build and start Phoenix + Vite dev servers
./dev.py down          # Stop all servers
./dev.py restart       # Rebuild Rust and restart Phoenix (Vite stays for hot reload)
./dev.py status        # Check what's running

# Code quality
./dev.py check         # Run clippy + fmt check + tests

# Task management
./dev.py tasks ready   # List tasks ready for implementation
./dev.py tasks close <id> [--wont-do]  # Close a task
```

### Development Workflow

1. **Start development:** `./dev.py up`
   - Builds Rust backend
   - Starts Phoenix server and Vite dev server
   - Ports are auto-assigned based on worktree path (supports multiple worktrees)
   - Each worktree gets its own database

2. **After Rust changes:** `./dev.py restart`
   - Rebuilds and restarts Phoenix
   - Vite keeps running (UI hot reloads automatically)

3. **After UI changes:** Nothing needed!
   - Vite hot reloads automatically

4. **Before committing:** `./dev.py check`

5. **When done:** `./dev.py down`

6. **Check configuration:** `./dev.py status`
   - Shows worktree hash, ports, database path

### Multi-Worktree Support

Each git worktree automatically gets:
- Unique ports (based on path hash)
- Separate database file
- Lock file to prevent conflicts

You can run multiple instances in different worktrees simultaneously.

### ⚠️ Do NOT start the server manually

Do NOT use `cargo run` or start the binary directly. The server requires the LLM gateway
configuration which `./dev.py up` provides automatically from `/exe.dev/shelley.json`.

If you see API key errors, you're not using dev.py.

---

## Production Deployment

```bash
# Build and deploy
./dev.py prod build [version]   # Build from git tag or HEAD
./dev.py prod deploy [version]  # Build + install systemd service + start
./dev.py prod status            # Show production service status  
./dev.py prod stop              # Stop production service
```

### How it works

- **Embedded UI**: React UI is embedded in the binary using `rust-embed`
- **Static binary**: Uses musl target for fully static ~9MB binary
- **Git worktree**: Builds in `~/.phoenix-ide-build` to avoid disturbing main worktree
- **Systemd service**: `phoenix-ide` runs on port 7331, database at `~/.phoenix-ide/prod.db`

### Example workflows

```bash
# Deploy current HEAD
./dev.py prod deploy

# Tag and deploy a release
git tag v0.1.0
./dev.py prod deploy v0.1.0

# Roll back
./dev.py prod deploy v0.0.9
```

Binaries are built on-demand from git tags—no need to store release artifacts.
