---
name: phoenix-development
description: Development workflow and conventions for the Phoenix IDE project. Covers server management, code quality, task tracking, and production deployment.
---

# Phoenix IDE Development

This skill provides the workflow and conventions for developing Phoenix IDE.

## Server Management

**Always use `./dev.py` for development tasks.** It handles LLM gateway configuration automatically.

```bash
./dev.py up            # Build and start Phoenix + Vite dev servers
./dev.py down          # Stop all servers
./dev.py restart       # Rebuild Rust and restart Phoenix (Vite stays)
./dev.py status        # Check what's running
./dev.py check         # Run clippy + fmt check + tests
```

### Development Workflow

1. **Start development:** `./dev.py up`
2. **After Rust changes:** `./dev.py restart`
3. **After UI changes:** Nothing needed (Vite hot reloads)
4. **Before committing:** `./dev.py check`
5. **When done:** `./dev.py down`

### ⚠️ Do NOT start the server manually

Do NOT use `cargo run` or start the binary directly. The server requires LLM gateway configuration which `./dev.py up` provides automatically.

## Production Deployment

```bash
./dev.py prod deploy [version]  # Build + install systemd service + start
./dev.py prod status            # Show production service status
./dev.py prod stop              # Stop production service
```

### Deployment Process

1. Stop the service first if replacing: `sudo systemctl stop phoenix-ide`
2. Deploy: `./dev.py prod deploy`
3. Verify: `./dev.py prod status`

The deploy builds from HEAD by default. Specify a git tag for tagged releases.

## Code Conventions

### Module Organization

Use named module files instead of `mod.rs`:

✅ Correct:
```
src/
  tools.rs           # Module entry point
  tools/
    bash.rs          # Submodule
```

❌ Wrong:
```
src/
  tools/
    mod.rs           # Forbidden
```

## Task Tracking

Tasks are tracked in `tasks/NNN-slug.md` files with YAML frontmatter.

```bash
./dev.py tasks ready              # List tasks ready for implementation
./dev.py tasks close <id>         # Close a task as done
./dev.py tasks close <id> --wont-do  # Close as won't-do
```

### Issue Discovery Protocol

**When you encounter ANY issue during work, create a task immediately.**

Do NOT:
- Note an issue and move on
- Say "this is unrelated" and ignore it
- Work around problems without documenting

Do:
- Create a task in `tasks/` documenting the issue
- Include reproduction steps and context
- Then continue with original work
