# Implement OS-enforced read-only Explore bash with nono

## Goal

Give top-level Explore conversations a useful `bash` tool for local code investigation while preserving Explore's read-only promise through OS-level sandboxing.

The first-pass success case is an Explore agent running commands such as:

```bash
git blame path/to/file
git log
rg pattern
cat README.md
```

while source/worktree mutations and network access are denied by the sandbox.

## First-pass product scope

Explore mode with a working sandbox should expose:

- read-only Phoenix tools (`think`, `read_file`, `search`, `keyword_search`, `read_image`)
- `bash`, but only through a `nono`-enforced sandbox
- scoped `patch` for task proposal drafts
- `propose_task`
- parent coordination tools needed for normal Explore planning, excluding browser tools for this slice

Explore mode should omit for this first pass:

- browser tools
- `tmux` / `tmux_run`
- MCP sandboxing
- Explore sub-agent bash unless it falls out trivially from the same launcher without expanding scope

If `nono`/OS sandbox support is unavailable or cannot enforce the required policy, Phoenix must fail closed: Explore mode does not expose `bash`.

## Sandbox behavior

For top-level Explore `bash`:

Allowed:

```bash
git blame src/file.rs
git log
rg "foo"
cat README.md
TMPDIR=$PHOENIX_SANDBOX_SCRATCH mktemp
echo "# Task" > <discovered-task-dir>/34001-p2-ready--thing.md
```

Denied:

```bash
echo x > src/file.rs
git checkout -- src/file.rs
git reset --hard
git fetch
curl https://example.com
npm install
cargo check # if it writes target/ in the repo/worktree
```

Policy:

- repo/worktree root: read-only
- task proposal directories: read-write
- Phoenix sandbox scratch directory: read-write
- network: blocked
- environment: provide scratch-backed `TMPDIR`, `HOME`, and `PHOENIX_SANDBOX_SCRATCH`; avoid passing secrets unless explicitly required by existing non-sandboxed behavior and justified

## Task directory discovery

Do not hard-code only `tasks/`.

Use the imported `taskmd-core` crate's upgraded auto-discovery behavior (recently upgraded to 1.3.0) to discover valid task proposal directories. The OS sandbox should grant write access to every discovered taskmd directory needed for proposal drafting.

Keep semantic validation in Phoenix/taskmd paths:

- OS sandbox grants directory-level write access.
- Existing scoped `patch`/`propose_task` validation remains responsible for taskmd filename/source correctness.
- Bash may write task files in discovered task directories, but `propose_task` remains the approval gateway.

## nono integration direction

Use `nono` as much as practical rather than Phoenix's current shallow platform probing:

- replace/wrap `PlatformCapability::detect()` with `nono::Sandbox::support_info()` and richer backend details
- target macOS and Linux as first-class supported environments
- first development step: verify this current macOS environment can build/use the needed `nono` sandbox path

Do not call `nono::Sandbox::apply()` in the long-running Phoenix server process. It is irreversible for the current process. Apply sandboxing only at a child-process boundary, e.g. a Phoenix-owned sandbox launcher/helper that applies `nono` and then execs `bash -c <cmd>`.

## Implementation plan

1. Verify local development environment suitability for `nono` on current macOS:
   - dependency/build compatibility
   - sandbox support detection
   - minimal child-process sandbox smoke test
2. Introduce an explicit sandbox policy type for tool execution, e.g. `ExploreReadOnly` with repo root, discovered task dirs, scratch dir, and blocked network.
3. Add a shared sandboxed process-launch path for Explore bash.
4. Wire top-level Explore `bash` through the sandbox launcher while preserving existing bash handle semantics:
   - stdout/stderr capture
   - `wait_seconds=0` handle creation
   - `peek`/`wait`
   - process-group kill
   - cancellation
   - non-zero command exits as normal command outcomes
5. Adjust Explore registry construction:
   - sandbox available: include sandboxed bash, scoped patch, propose_task, read-only/planning tools; omit browser and tmux
   - sandbox unavailable: omit bash
6. Add scratch directory and environment wiring.
7. Add tests for supported and unsupported sandbox paths, including write denial, task-dir write allowance, network denial, and registry contents.
8. Update specs for projects/bash to describe `nono`-backed read-only Explore bash and first-pass exclusions.

## Acceptance criteria

- On a supported macOS/Linux host, top-level Explore mode exposes `bash`.
- Explore `bash` can run `git blame` against repo files.
- Explore `bash` cannot mutate source/worktree files.
- Explore `bash` can write task proposal files in taskmd-discovered task directories.
- Explore `bash` can write only to Phoenix scratch outside task directories.
- Explore `bash` cannot use network.
- On unsupported hosts, Explore mode does not expose `bash`.
- Browser tools are omitted from first-pass Explore-with-sandbox registry.
- `tmux` and `tmux_run` remain omitted from first-pass Explore-with-sandbox registry.
- Existing Work/Direct bash behavior remains unchanged.
