# Centralize filesystem-environment access in `PhoenixRuntimeEnvironment`

## Problem

`std::env::var("HOME")` and friends are scattered across the codebase
with inconsistent and sometimes wrong fallbacks:

```bash
grep -rEn 'std::env::var(_os)?\("HOME"\)' crates/ | wc -l
# 16
```

Sample of fallback strategies in current code:

- `unwrap_or_else(|_| "/tmp".to_string())` (main.rs, mcp.rs)
- `unwrap_or_else(|_| "/root".to_string())` (command_tracker.rs, spawn.rs,
  monitor.rs) — plain wrong on macOS dev machines, CI runners,
  container hosts
- `unwrap_or_else(|_| ".".to_string())` (codex_credential.rs) — risk of
  writing secret-bearing files into CWD
- `unwrap_or_else(|_| PathBuf::from("/tmp"))` (browser/session.rs)
- `.ok().map(...)` (system_prompt.rs, skills/builtin.rs) — silently
  drop the path

Plus 5+ `std::env::temp_dir()` calls scattered with no audit trail of
which Phoenix subsystem owns which temp namespace, and CODEX_HOME /
implicit `.phoenix-ide/` path joins everywhere.

This was surfaced during PR #57 (Codex login): `default_phoenix_auth_path`
needed to fall back somewhere when HOME was unset, the existing
`default_auth_path` it wanted to mirror falls back to `"."`, and the
new code chose `$TMPDIR` with a `tracing::warn!` to flag the unusual
case. The reviewer correctly noted the inconsistency. We don't want
N more rounds of this — every new code path that needs a path picks
its own fallback, and the inconsistency compounds.

## Goal

A single typed entry point for filesystem-environment resolution that:

1. Resolves `~/.phoenix-ide/` once, with one canonical fallback rule,
   logged once at startup if the fallback fires.
2. Provides typed sub-paths for known Phoenix data: `db_path()`,
   `prod_log_path()`, `codex_auth_path()`, `worktrees_dir()`,
   `terminal_output_dir()`, `tls_dir()`, etc.
3. Handles `$TMPDIR` consistently — Phoenix-namespaced subdirs that
   don't collide with other apps' tmpfiles.
4. Lets tests inject a fake home via `PhoenixRuntimeEnvironment::with_root(tempdir)`
   so tests stop needing to mutate process env vars.
5. Is enforceable: an `ast-grep` rule in `./dev.py check` rejects any
   new `std::env::var("HOME")` outside `runtime_env.rs` itself.

## Sketch

```rust
// crates/phoenix-ide/src/runtime_env.rs
pub struct PhoenixRuntimeEnvironment {
    home: PathBuf,           // resolved once; fallback chain documented
    phoenix_home: PathBuf,   // home/.phoenix-ide
    tmp_root: PathBuf,       // env::temp_dir() / phoenix-ide
}

impl PhoenixRuntimeEnvironment {
    pub fn detect() -> Self;            // production constructor; logs once

    #[cfg(test)]
    pub fn with_root(root: &Path) -> Self;

    pub fn home(&self) -> &Path;
    pub fn phoenix_home(&self) -> &Path;
    pub fn db_path(&self) -> PathBuf;
    pub fn prod_log_path(&self) -> PathBuf;
    pub fn codex_auth_path(&self) -> PathBuf;
    pub fn codex_cli_auth_path(&self) -> PathBuf; // ~/.codex/auth.json
    pub fn worktrees_dir(&self) -> PathBuf;
    pub fn tls_dir(&self) -> PathBuf;
    pub fn terminal_output_dir(&self) -> PathBuf;
    pub fn tmp_subdir(&self, namespace: &str) -> Result<PathBuf, io::Error>;
}
```

Hold an `Arc<PhoenixRuntimeEnvironment>` on `AppState`; pass through to
subsystems that need paths. Constructor sites that today read HOME
inline get refactored to consult the Arc.

## ast-grep enforcement

Add to `./dev.py check`:

```yaml
# .ast-grep/no-direct-home-reads.yml
id: no-direct-home-reads
language: rust
rule:
  any:
    - pattern: std::env::var("HOME")
    - pattern: std::env::var_os("HOME")
    - pattern: std::env::var("CODEX_HOME")
    - pattern: std::env::var_os("CODEX_HOME")
    - pattern: std::env::temp_dir()
fix: |
  Use PhoenixRuntimeEnvironment instead. See crates/phoenix-ide/src/runtime_env.rs.
```

with allowlist for `runtime_env.rs` itself.

## Migration

Roughly the call sites surfaced today (May 2026):

- `src/main.rs:78` — db path
- `src/system_prompt.rs:410` — home_dir for prompt
- `src/git_ops.rs:484` — git index temp file
- `src/terminal/{command_tracker,spawn}.rs` — terminal output dir
- `src/skills/builtin.rs:46` — built-in skills directory
- `src/llm/codex_credential.rs:104,107,121,126` — auth paths (this PR)
- `src/tools/{bash_check,bash,tmux,mcp,browser/session}.rs` —
  scattered tmp/home reads
- `src/api/handlers.rs:{359,2577,3219}` — directory browser endpoints
- `src/bin/monitor.rs:42` — prod log path

About 16 HOME reads + 5 temp_dir calls. Each translates to a 1-2 line
change.

## Out of scope

- Migrating non-filesystem env vars (`OPENAI_API_KEY`, `LLM_GATEWAY`,
  etc.) — those stay in `LlmConfig::from_env`. This task is filesystem
  paths only.
- Changing fallback semantics for already-shipped code — match what
  each site does today, just route it through the helper.

## Acceptance

- [ ] `crates/phoenix-ide/src/runtime_env.rs` with the public surface above
- [ ] All 16 HOME reads + 5 temp_dir reads migrated
- [ ] ast-grep rule in `./dev.py check` rejects new direct reads outside
      `runtime_env.rs`
- [ ] Tests use `PhoenixRuntimeEnvironment::with_root(tempdir)` instead
      of mutating `HOME` (closes the env-mutation flakiness window)
- [ ] No behavior changes vs current — fallback paths preserved per call
      site, just centralized

## Notes

Surfaced during PR #57 review when `default_phoenix_auth_path` had to
pick its own fallback; the reviewer noted that `default_auth_path` falls
back differently. The right fix is structural (this task), not adding a
warning on each new site that needs a path.
