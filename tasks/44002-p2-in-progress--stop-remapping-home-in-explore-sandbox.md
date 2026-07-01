# Stop remapping $HOME in the Explore bash sandbox

## Problem

The Explore-mode bash sandbox (`crates/phoenix-tools/src/bash/sandbox.rs`)
remaps `HOME` to a synthetic empty directory under a fresh-UUID scratch tree
that is deleted after each command. This is surprising and harmful:

- The user's real `~/.gitconfig`, `~/.cargo/config.toml`, `~/.npmrc`,
  `~/.ssh/...` are invisible to sandboxed bash — tools silently behave as
  unconfigured.
- `HOME` (and its aliases `PHOENIX_SANDBOX_HOME` / `PHOENIX_SANDBOX_SCRATCH`)
  churns on every `op="run"` (fresh UUID) and the old tree is deleted, so
  nothing written to `$HOME` survives to the next call.
- The synthetic home is inconsistent with the sandbox's own "broad read"
  threat model: `read_file` and `search` can already read `~/.ssh` etc.
  directly, so hiding `HOME` protects against config *auto-loading* but not
  against explicit reads — a half-measure that surprises without securing.

The separate `PHOENIX_SANDBOX_HOME` env var exists only to carry the synthetic
home path across the server→launcher-child IPC boundary and re-export it as
`HOME`. It has no other consumer. `PHOENIX_SANDBOX_SCRATCH` stays — it is
load-bearing IPC (the child needs the scratch path to grant it RW in the nono
capability set) and is harmless.

## Change

Stop faking `HOME`. Pass the **real** user home through instead. Remove
`PHOENIX_SANDBOX_HOME` entirely. Keep `PHOENIX_SANDBOX_SCRATCH` as-is.

### `crates/phoenix-tools/src/bash/sandbox.rs`

- Remove the `HOME_ENV` const (`"PHOENIX_SANDBOX_HOME"`).
- Remove the `sandbox_home` field from `ExploreReadOnlyPolicy`.
- In `discover()`: stop creating `sandbox_dir.join("home")` and stop
  `create_dir_all`-ing it. Get the real home from
  `PhoenixRuntimeEnvironment::home()` (the `runtime_env` is already
  constructed there) and store it as a `home` field.
- In `to_command_env()`: set `HOME` = real home (instead of
  `PHOENIX_SANDBOX_HOME` = synthetic home). This is the IPC channel — the
  launcher child reads it back in `from_env()`.
- In `from_env()`: read `HOME` (instead of `PHOENIX_SANDBOX_HOME`) to
  reconstruct the home path.
- In `apply_child_env()`: set `HOME` = `self.home` (the real home). Remove
  the `PHOENIX_SANDBOX_HOME` line.
- `capability_set()` is unchanged — it never referenced `sandbox_home`.

### `crates/phoenix-tools/src/bash.rs`

- Update `sandboxed_bash_description` (line ~237): remove `HOME` from the
  list of remapped vars. New text should mention `$PHOENIX_SANDBOX_SCRATCH`
  and `$TMPDIR` point at writable Phoenix-owned scratch/temp; `HOME` is the
  user's real home (read-only under the sandbox).

### `crates/phoenix-ide/tests/explore_sandbox.rs`

- `SandboxFixture`: replace `sandbox_home` with `home` (the real home path
  the test wants to pass through).
- `sandbox_run()`: replace `.env("PHOENIX_SANDBOX_HOME", ...)` with
  `.env("HOME", ...)`.
- Stop creating `scratch.join("home")` / `create_dir_all` for it.
- Env probe (line ~154): assert `$HOME` == the real home passed in, not the
  synthetic home. Remove the `psh=` (`$PHOENIX_SANDBOX_HOME`) check.
- Keep the `$PHOENIX_SANDBOX_SCRATCH` and `$TMPDIR` and `gh=unset` checks.

### `specs/bash/requirements.md` (REQ-BASH-012)

- Remove the bullet "a synthetic sandbox home under scratch, exposed as
  `PHOENIX_SANDBOX_HOME` and `HOME`".
- Replace with: `HOME` is the user's real home directory, passed through
  unchanged; the nono sandbox still blocks writes to it (only scratch and
  platform-temp are RW). This is consistent with the broad-read model.
- Remove the `PHOENIX_SANDBOX_HOME` mention from the env-var list.

### `specs/bash/design.md`

- Update the "Explore Read-Only Sandbox" section (line ~858): remove
  `PHOENIX_SANDBOX_HOME` and the synthetic-home description. State that
  `HOME` is the user's real home, passed through; `PHOENIX_SANDBOX_SCRATCH`
  names scratch; `TMPDIR` names platform temp.

## Security note

With real `HOME`, sandboxed bash can auto-load config from the real home
(e.g. git reads `~/.gitconfig`). This is a read, already permitted by the
broad-read model (`read_file`/`search` can read those paths directly). The
nono sandbox still blocks **writes** to the real home — only
`PHOENIX_SANDBOX_SCRATCH` and `TMPDIR` paths are RW. Network remains
blocked. Ambient credential env vars remain stripped (the `env_clear` +
allowlist rebuild is unchanged except for `HOME` now being the real value).

## Verification

- `cargo test -p phoenix-tools sandbox` — unit tests pass.
- `cargo test -p phoenix-ide --test explore_sandbox` — integration test
  passes with updated assertions.
- `./dev.py check` — clippy + fmt + tests + task validation.
- Manual: in an Explore conversation, `echo $HOME` shows the real home;
  `git config --global user.name` reads the real gitconfig; `echo > $HOME/x`
  fails (sandbox blocks the write).
