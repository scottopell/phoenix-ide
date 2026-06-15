//! Centralized filesystem-environment resolution.
//!
//! `std::env::var("HOME")`, `CODEX_HOME`, and `std::env::temp_dir()` were read
//! inline at a dozen call sites, each with its own fallback (`/tmp`, `/root`,
//! `.`, …). [`PhoenixRuntimeEnvironment`] resolves all of them once at startup,
//! behind a typed surface, so every subsystem agrees on where Phoenix's data
//! lives.
//!
//! It is detected once at server startup — like [`crate::platform::PlatformCapability`]
//! — and threaded as an `Arc` through `AppState` / `RuntimeManager` /
//! `ToolContext`, so the OS environment is read exactly once per process.
//!
//! Construction:
//! - production: [`PhoenixRuntimeEnvironment::detect`] — reads the
//!   environment, applies the canonical fallback chain, logs **one**
//!   `tracing::warn!` if the temp-dir fallback fires.
//! - tests: [`PhoenixRuntimeEnvironment::with_root`] (behind the `test-support`
//!   feature) — no environment reads; everything is derived under a
//!   caller-supplied directory so tests need not mutate process env vars.
//!
//! This module is the **only** place permitted to read `HOME` / `CODEX_HOME`
//! / `temp_dir()` directly — see `ast-grep-rules/no-direct-home-reads.yml`.

use std::path::{Path, PathBuf};

/// Subdirectory under `$HOME` that holds all Phoenix data.
const PHOENIX_HOME_SUBDIR: &str = ".phoenix-ide";
/// Subdirectory under `temp_dir()` for Phoenix-namespaced scratch files.
const TMP_NAMESPACE: &str = "phoenix-ide";
/// Subdirectory under the Phoenix home where built-in skills are extracted.
/// `phoenix-skills` re-exports this as its canonical `EXTRACT_SUBDIR` so the
/// two never drift.
pub const BUILTIN_SKILLS_SUBDIR: &str = "builtin-skills";

/// Resolved filesystem-environment for one Phoenix process.
///
/// Cheap to clone (a handful of `PathBuf`s); typically wrapped in an `Arc`
/// and threaded through `AppState` / `RuntimeManager` / `ToolContext`.
#[derive(Debug, Clone)]
pub struct PhoenixRuntimeEnvironment {
    /// User home directory. Resolved once via the [`detect`] fallback chain;
    /// in tests this is the `with_root` argument.
    ///
    /// [`detect`]: PhoenixRuntimeEnvironment::detect
    home: PathBuf,
    /// `$CODEX_HOME` if set, else `home/.codex`.
    codex_home: PathBuf,
    /// `$PHOENIX_DATA_DIR` if set, else `phoenix_home`.
    data_dir: PathBuf,
    /// `$PHOENIX_DB_PATH` if set, else `phoenix_home/phoenix.db`.
    db_path: PathBuf,
    /// `temp_dir()/phoenix-ide` — root for Phoenix scratch namespaces.
    tmp_root: PathBuf,
}

impl PhoenixRuntimeEnvironment {
    /// Production constructor. Reads the environment and applies the
    /// canonical fallback chains:
    ///
    /// - `home`: `$HOME`, then `$USERPROFILE`, then `std::env::temp_dir()`.
    ///   The temp-dir fallback emits **one** `tracing::warn!` — that is the
    ///   only warning this type ever logs.
    /// - `codex_home`: `$CODEX_HOME`, else `home/.codex`.
    /// - `data_dir`: `$PHOENIX_DATA_DIR`, else `phoenix_home` (`home/.phoenix-ide`).
    /// - `db_path`: `$PHOENIX_DB_PATH`, else `phoenix_home/phoenix.db`.
    /// - `tmp_root`: `std::env::temp_dir()/phoenix-ide`.
    #[must_use]
    pub fn detect() -> Self {
        let (home, used_tmp_fallback) = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
            .map_or_else(
                || (std::env::temp_dir(), true),
                |v| (PathBuf::from(v), false),
            );

        if used_tmp_fallback {
            tracing::warn!(
                home = %home.display(),
                "neither $HOME nor $USERPROFILE set; using the system temp directory as the Phoenix home. Phoenix data (database, auth, terminal output) will live under there."
            );
        }

        let phoenix_home = home.join(PHOENIX_HOME_SUBDIR);
        let codex_home = std::env::var_os("CODEX_HOME")
            .filter(|v| !v.is_empty())
            .map_or_else(|| home.join(".codex"), PathBuf::from);
        let data_dir = std::env::var_os("PHOENIX_DATA_DIR")
            .filter(|v| !v.is_empty())
            .map_or_else(|| phoenix_home.clone(), PathBuf::from);
        let db_path = std::env::var_os("PHOENIX_DB_PATH")
            .filter(|v| !v.is_empty())
            .map_or_else(|| phoenix_home.join("phoenix.db"), PathBuf::from);
        let tmp_root = std::env::temp_dir().join(TMP_NAMESPACE);

        Self {
            home,
            codex_home,
            data_dir,
            db_path,
            tmp_root,
        }
    }

    /// Deterministic constructor: derive everything under `root`, reading no
    /// environment variables. `home == root`, `codex_home == root/.codex`,
    /// `data_dir == phoenix_home`, `db_path == phoenix_home/phoenix.db`,
    /// `tmp_root == root/tmp`. Used by tests so they need not mutate process
    /// env vars.
    ///
    /// Behind the `test-support` feature so dependent crates can use it in
    /// their own test builds (a `#[cfg(test)]` item is invisible across a
    /// crate boundary).
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_root(root: &Path) -> Self {
        let home = root.to_path_buf();
        let phoenix_home = home.join(PHOENIX_HOME_SUBDIR);
        Self {
            codex_home: home.join(".codex"),
            data_dir: phoenix_home.clone(),
            db_path: phoenix_home.join("phoenix.db"),
            tmp_root: home.join("tmp"),
            home,
        }
    }

    /// User home directory.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `home/.phoenix-ide` — root of all Phoenix on-disk state.
    #[must_use]
    pub fn phoenix_home(&self) -> PathBuf {
        self.home.join(PHOENIX_HOME_SUBDIR)
    }

    /// `$CODEX_HOME` (or `home/.codex`) — the Codex CLI's home directory.
    #[must_use]
    pub fn codex_home(&self) -> &Path {
        &self.codex_home
    }

    /// `$PHOENIX_DATA_DIR` (or `phoenix_home`) — mutable Phoenix data root.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// `SQLite` database path (`$PHOENIX_DB_PATH` or `phoenix_home/phoenix.db`).
    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.db_path.clone()
    }

    /// Whether this looks like a production deployment. Mirrors the legacy
    /// `main.rs` heuristic: the resolved db path contains the substring
    /// `"prod"` (the prod deploy writes to `~/.phoenix-ide/prod.db`).
    #[must_use]
    pub fn is_production(&self) -> bool {
        self.db_path.to_string_lossy().contains("prod")
    }

    /// `phoenix_home/prod.log` — the production log file.
    #[must_use]
    pub fn prod_log_path(&self) -> PathBuf {
        self.phoenix_home().join("prod.log")
    }

    /// `phoenix_home/codex-auth.json` — where Phoenix stores the ChatGPT/Codex
    /// OAuth credential it captured during in-app login.
    #[must_use]
    pub fn codex_auth_path(&self) -> PathBuf {
        self.phoenix_home().join("codex-auth.json")
    }

    /// `codex_home/auth.json` — the Codex *CLI's* own credential file.
    #[must_use]
    pub fn codex_cli_auth_path(&self) -> PathBuf {
        self.codex_home().join("auth.json")
    }

    /// `phoenix_home/terminal-output` — root of saved terminal command output.
    #[must_use]
    pub fn terminal_output_dir(&self) -> PathBuf {
        self.phoenix_home().join("terminal-output")
    }

    /// `phoenix_home/builtin-skills` — where built-in skills are extracted
    /// (matches `phoenix_skills::builtin::EXTRACT_SUBDIR`).
    #[must_use]
    pub fn builtin_skills_dir(&self) -> PathBuf {
        self.phoenix_home().join(BUILTIN_SKILLS_SUBDIR)
    }

    /// `data_dir/tmux-sockets` — directory holding per-conversation tmux
    /// server sockets.
    #[must_use]
    pub fn tmux_socket_dir(&self) -> PathBuf {
        self.data_dir().join("tmux-sockets")
    }

    /// `temp_dir()/phoenix-ide` — the root for Phoenix scratch namespaces.
    /// Prefer [`tmp_subdir`](Self::tmp_subdir), which validates the namespace
    /// and creates the directory; this accessor is for callers that only need
    /// the path (e.g. to build a child path they create themselves).
    #[must_use]
    pub fn tmp_root(&self) -> &Path {
        &self.tmp_root
    }

    /// `home/.cache/phoenix-ide/chromium` — Chrome-for-Testing download cache.
    #[must_use]
    pub fn chromium_cache_dir(&self) -> PathBuf {
        self.home
            .join(".cache")
            .join("phoenix-ide")
            .join("chromium")
    }

    /// `tmp_root/namespace`, created (`mkdir -p`) and returned. Use this
    /// instead of `std::env::temp_dir().join(...)` so Phoenix scratch files
    /// never collide with other apps' tmpfiles and the namespace is
    /// auditable.
    ///
    /// `namespace` must be a single relative path component (no separators,
    /// no `..`, not absolute) so the result cannot escape `tmp_root`;
    /// anything else is an [`io::ErrorKind::InvalidInput`] error.
    ///
    /// [`io::ErrorKind::InvalidInput`]: std::io::ErrorKind::InvalidInput
    pub fn tmp_subdir(&self, namespace: &str) -> std::io::Result<PathBuf> {
        if !is_single_path_component(namespace) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("tmp_subdir namespace must be a single relative path component, got {namespace:?}"),
            ));
        }
        let dir = self.tmp_root.join(namespace);
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

/// True iff `s` is exactly one normal path component — no `/` or `\`, not
/// `.`/`..`, not absolute, not empty. Used to keep [`PhoenixRuntimeEnvironment::tmp_subdir`]
/// from escaping its temp root.
fn is_single_path_component(s: &str) -> bool {
    use std::path::Component;
    let mut components = Path::new(s).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_root_derives_sub_paths() {
        let root = std::path::Path::new("/tmp/phoenix-test-root");
        let env = PhoenixRuntimeEnvironment::with_root(root);

        assert_eq!(env.home(), root);
        assert_eq!(env.phoenix_home(), root.join(".phoenix-ide"));
        assert_eq!(env.db_path(), root.join(".phoenix-ide/phoenix.db"));
        assert_eq!(env.codex_home(), root.join(".codex"));
        assert_eq!(env.codex_cli_auth_path(), root.join(".codex/auth.json"));
        assert_eq!(
            env.codex_auth_path(),
            root.join(".phoenix-ide/codex-auth.json")
        );
        assert_eq!(env.prod_log_path(), root.join(".phoenix-ide/prod.log"));
        assert_eq!(
            env.terminal_output_dir(),
            root.join(".phoenix-ide/terminal-output")
        );
        assert_eq!(
            env.builtin_skills_dir(),
            root.join(".phoenix-ide").join(BUILTIN_SKILLS_SUBDIR)
        );
        assert_eq!(
            env.tmux_socket_dir(),
            root.join(".phoenix-ide/tmux-sockets")
        );
        assert_eq!(
            env.chromium_cache_dir(),
            root.join(".cache/phoenix-ide/chromium")
        );
    }

    #[test]
    fn tmp_subdir_creates_the_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env = PhoenixRuntimeEnvironment::with_root(tmp.path());
        let sub = env.tmp_subdir("git-index").unwrap();
        assert!(sub.is_dir());
        assert_eq!(sub, tmp.path().join("tmp").join("git-index"));
        // Idempotent.
        let sub2 = env.tmp_subdir("git-index").unwrap();
        assert_eq!(sub, sub2);
        assert!(sub2.is_dir());
    }

    #[test]
    fn tmp_subdir_rejects_escaping_namespaces() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env = PhoenixRuntimeEnvironment::with_root(tmp.path());
        for bad in ["..", ".", "", "a/b", "/etc", "../sibling", "a/../b"] {
            let err = env.tmp_subdir(bad).unwrap_err();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::InvalidInput,
                "namespace {bad:?}"
            );
        }
        assert!(env.tmp_subdir("ok").unwrap().is_dir());
    }

    #[test]
    fn is_production_reflects_db_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        // with_root → db is phoenix.db, not prod.db.
        let dev = PhoenixRuntimeEnvironment::with_root(tmp.path());
        assert!(!dev.is_production());

        // A hand-built env with a prod db path is "production".
        let prod = PhoenixRuntimeEnvironment {
            home: tmp.path().to_path_buf(),
            codex_home: tmp.path().join(".codex"),
            data_dir: tmp.path().join(".phoenix-ide"),
            db_path: tmp.path().join(".phoenix-ide/prod.db"),
            tmp_root: tmp.path().join("tmp"),
        };
        assert!(prod.is_production());
    }
}
