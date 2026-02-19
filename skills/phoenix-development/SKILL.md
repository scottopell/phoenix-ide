---
name: phoenix-development
description: Development workflow and code conventions for Phoenix IDE. Use when making code changes, running tests, starting or stopping the dev server, or following project conventions.
---

# Phoenix IDE Development

## Server Management

**Always use `./dev.py` — never `cargo run` directly.** It configures the LLM gateway automatically.

```bash
./dev.py up          # Build and start Phoenix + Vite dev servers
./dev.py down        # Stop all servers
./dev.py restart     # Rebuild Rust and restart Phoenix (Vite stays running)
./dev.py status      # Check what's running
./dev.py check       # clippy + fmt + tests + task validation
```

### Workflow

1. `./dev.py up` — start dev environment
2. Make changes
3. Rust change → `./dev.py restart` / UI change → nothing (Vite hot reloads)
4. `./dev.py check` — must pass before committing
5. `./dev.py down` — when done

Each git worktree gets unique ports and a separate database automatically.

## Testing

```bash
cargo test                    # All tests
cargo test state_machine      # Filter by module or test name
cargo test -- --nocapture     # See println! output
```

Property tests live in `**/proptests.rs`. Run with `cargo test proptests`.

## Code Conventions

### Module Organization

Use `foo.rs` + `foo/` subdirectory — NOT `foo/mod.rs`. Enforced by clippy.

```
✅  src/tools.rs + src/tools/bash.rs
❌  src/tools/mod.rs + src/tools/bash.rs
```

### Adding a New Tool

See `src/tools/think.rs` as the minimal example.

1. Create `src/tools/your_tool.rs` implementing the `Tool` trait
2. Register in `src/tools.rs` → `ToolRegistry::new_with_options()`
3. Add spec in `specs/your-tool/executive.md`

**Before modifying any existing tool**, read its spec in `specs/<tool>/executive.md`.

## Testing Conversations

Use `phoenix-client.py` to interact with the running server without a browser:

```bash
./phoenix-client.py "List files in this directory"
./phoenix-client.py -c <conversation-id> "Follow-up message"
```

Prefer this over browser automation for agent testing.
