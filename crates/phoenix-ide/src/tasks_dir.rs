//! Discovery of a project's taskmd tasks directory.
//!
//! taskmd 1.0 marks a tasks directory with a `_TEMPLATE.md` sentinel.
//! Phoenix-ide previously hard-coded the literal name `tasks/` everywhere;
//! task 13008 lifts that restriction. The Python taskmd CLI does the same
//! walk, but `taskmd-core` v1.0.0 does not expose a Rust helper — tracked
//! upstream at `scottopell/taskmd` (TODO: link issue once filed).
//!
//! Discovery strategy (matches the Python CLI):
//!
//! - Scan immediate children of the cwd. If any subdirectory contains a
//!   `_TEMPLATE.md`, that subdirectory is the tasks directory.
//! - When several candidates exist, prefer the literal `tasks` (the
//!   convention name) and otherwise pick the lexically-first directory so
//!   the result is deterministic.
//! - If nothing matches, fall back to the relative name `tasks` — this
//!   keeps existing repos working unchanged.
//!
//! Callers store the **relative name** on `ConvContext`. The absolute path
//! is `cwd.join(name)`; reformatting paths inside the worktree (commit
//! messages, system-prompt prose, `format!("{tasks_dir}/{filename}")`) all
//! need the bare name, not an absolute path.

use std::path::Path;

/// Conventional default when no `_TEMPLATE.md`-bearing directory exists.
pub const DEFAULT_TASKS_DIR_NAME: &str = "tasks";

/// Discover the tasks directory under `cwd` and return its relative name.
///
/// Returns the bare directory name (no slashes). Falls back to
/// [`DEFAULT_TASKS_DIR_NAME`] when nothing is found.
pub fn discover_tasks_dir_name(cwd: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(cwd) else {
        return DEFAULT_TASKS_DIR_NAME.to_string();
    };

    let mut candidates: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            if !path.join("_TEMPLATE.md").is_file() {
                return None;
            }
            path.file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
        })
        .collect();

    candidates.sort();
    if let Some(idx) = candidates
        .iter()
        .position(|name| name == DEFAULT_TASKS_DIR_NAME)
    {
        return candidates.swap_remove(idx);
    }
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| DEFAULT_TASKS_DIR_NAME.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_when_cwd_missing() {
        let missing = std::path::PathBuf::from("/nonexistent-phoenix-tasks-dir");
        assert_eq!(discover_tasks_dir_name(&missing), DEFAULT_TASKS_DIR_NAME);
    }

    #[test]
    fn default_when_no_template_anywhere() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::create_dir(tmp.path().join("tasks")).unwrap();
        // tasks/ exists but has no _TEMPLATE.md -> still default
        assert_eq!(discover_tasks_dir_name(tmp.path()), DEFAULT_TASKS_DIR_NAME);
    }

    #[test]
    fn finds_literal_tasks_dir() {
        let tmp = TempDir::new().unwrap();
        let tasks = tmp.path().join("tasks");
        std::fs::create_dir(&tasks).unwrap();
        std::fs::write(tasks.join("_TEMPLATE.md"), "# template\n").unwrap();
        assert_eq!(discover_tasks_dir_name(tmp.path()), "tasks");
    }

    #[test]
    fn finds_alternative_name() {
        let tmp = TempDir::new().unwrap();
        let taskmds = tmp.path().join("taskmds");
        std::fs::create_dir(&taskmds).unwrap();
        std::fs::write(taskmds.join("_TEMPLATE.md"), "# template\n").unwrap();
        assert_eq!(discover_tasks_dir_name(tmp.path()), "taskmds");
    }

    #[test]
    fn prefers_literal_tasks_when_multiple_candidates() {
        let tmp = TempDir::new().unwrap();
        for name in &["alpha-tasks", "tasks", "zeta-tasks"] {
            let dir = tmp.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("_TEMPLATE.md"), "# template\n").unwrap();
        }
        assert_eq!(discover_tasks_dir_name(tmp.path()), "tasks");
    }

    #[test]
    fn deterministic_pick_without_literal_tasks() {
        let tmp = TempDir::new().unwrap();
        for name in &["zeta-tasks", "alpha-tasks"] {
            let dir = tmp.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("_TEMPLATE.md"), "# template\n").unwrap();
        }
        assert_eq!(discover_tasks_dir_name(tmp.path()), "alpha-tasks");
    }

    #[test]
    fn ignores_files_named_template() {
        let tmp = TempDir::new().unwrap();
        // _TEMPLATE.md at the repo root, not inside a tasks-style subdir.
        std::fs::write(tmp.path().join("_TEMPLATE.md"), "# template\n").unwrap();
        assert_eq!(discover_tasks_dir_name(tmp.path()), DEFAULT_TASKS_DIR_NAME);
    }
}
