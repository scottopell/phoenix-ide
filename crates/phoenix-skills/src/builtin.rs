//! Built-in skills compiled into the phoenix binary as an embedded directory
//! tree. At server startup, the tree is materialized to
//! `<HOME>/.phoenix-ide/builtin-skills/` so each built-in becomes a real
//! filesystem skill — same path semantics as user-installed skills, including
//! companion files (`references/*.md`, `scripts/`, etc.).
//!
//! ## Layout
//!
//! Each subdirectory under `src/builtin/` is one skill. The directory
//! must contain `SKILL.md` with the standard frontmatter; any other files
//! (references, scripts, examples) are extracted alongside and visible to
//! the LLM via the existing skill-base-directory mechanism.
//!
//! ```text
//! src/builtin/
//!   allium/
//!     SKILL.md
//!     references/
//!       language-reference.md
//!   spears/
//!     SKILL.md
//!     references/
//!     adrs/
//! ```
//!
//! ## Override
//!
//! A user-installed filesystem skill of the same name shadows the built-in.
//! The walk-up over `.claude/skills/` and `.agents/skills/` runs before the
//! built-in extract dir is scanned, and the existing name dedup keeps the
//! first-seen entry.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

#[derive(rust_embed::RustEmbed)]
#[folder = "src/builtin/"]
struct BuiltinAssets;

/// Subdirectory under the phoenix data dir where built-ins are extracted.
/// Single source of truth lives in `phoenix-core` so the deployment-info disk
/// row and the extraction target can never drift.
pub const EXTRACT_SUBDIR: &str = phoenix_core::runtime_env::BUILTIN_SKILLS_SUBDIR;

/// Default extraction target (`<home>/.phoenix-ide/builtin-skills/`), resolved
/// through [`PhoenixRuntimeEnvironment`]. `Option` is retained for callers that
/// treat a missing built-in directory as "no built-ins"; the environment always
/// resolves a home (falling back to the temp dir), so this is always `Some`.
#[must_use]
pub fn default_extract_dir() -> Option<PathBuf> {
    Some(phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect().builtin_skills_dir())
}

/// Names of built-in skills (top-level directories that contain `SKILL.md`).
/// Sorted, deterministic.
#[must_use]
pub fn skill_names() -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for path in BuiltinAssets::iter() {
        let Some(first) = path.split('/').next() else {
            continue;
        };
        if BuiltinAssets::get(&format!("{first}/SKILL.md")).is_some() {
            names.insert(first.to_string());
        }
    }
    names.into_iter().collect()
}

/// Extract every embedded built-in file to `target_dir/<skill>/<...>`.
/// Overwrites embedded files and removes every non-embedded file found under a
/// currently bundled skill directory. The target directory is Phoenix-owned;
/// user customizations and overrides must live in filesystem skill directories
/// such as `.claude/skills/` or `.agents/skills/`, not beside extracted
/// built-ins.
///
/// # Errors
///
/// Returns the first I/O error encountered while creating directories or
/// writing files.
///
/// # Panics
///
/// Panics if `BuiltinAssets::get` returns `None` for a path that
/// `BuiltinAssets::iter` just yielded — this would only occur with a
/// corrupt binary (the embed macro guarantees the iterator and lookup
/// share the same compile-time set).
pub fn extract_to(target_dir: &Path) -> std::io::Result<()> {
    ensure_real_directory_root(target_dir)?;
    prune_removed_builtin_files(target_dir)?;
    for path in BuiltinAssets::iter() {
        let asset = BuiltinAssets::get(&path).expect("iterated asset must exist");
        let dest = target_dir.join(path.as_ref());
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        ensure_regular_file_destination(&dest)?;
        let needs_write = match std::fs::read(&dest) {
            Ok(existing) => existing != asset.data.as_ref(),
            Err(_) => true,
        };
        if needs_write {
            std::fs::write(&dest, asset.data.as_ref())?;
        }
    }
    Ok(())
}

fn ensure_real_directory_root(root: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                std::fs::remove_file(root)?;
                std::fs::create_dir_all(root)?;
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(root)?;
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

fn ensure_regular_file_destination(dest: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(dest) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                std::fs::remove_file(dest)?;
            } else if metadata.is_dir() {
                std::fs::remove_dir_all(dest)?;
            } else if !metadata.is_file() {
                std::fs::remove_file(dest)?;
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

fn prune_removed_builtin_files(target_dir: &Path) -> io::Result<()> {
    let mut expected = BTreeSet::new();
    for path in BuiltinAssets::iter() {
        expected.insert(PathBuf::from(path.as_ref()));
    }

    for skill in skill_names() {
        let skill_dir = target_dir.join(&skill);
        let metadata = match std::fs::symlink_metadata(&skill_dir) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if metadata.file_type().is_symlink() {
            std::fs::remove_file(&skill_dir)?;
            continue;
        }
        if !metadata.is_dir() {
            std::fs::remove_file(&skill_dir)?;
            continue;
        }
        prune_dir(&skill_dir, Path::new(&skill), &expected)?;
    }
    Ok(())
}

fn prune_dir(dir: &Path, rel_dir: &Path, expected: &BTreeSet<PathBuf>) -> io::Result<bool> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel = rel_dir.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if prune_dir(&path, &rel, expected)? {
                std::fs::remove_dir(&path)?;
            }
        } else if !expected.contains(&rel) {
            std::fs::remove_file(&path)?;
        }
    }

    let is_empty = std::fs::read_dir(dir)?.next().is_none();
    Ok(is_empty && !expected_dir_has_descendants(rel_dir, expected))
}

fn expected_dir_has_descendants(rel_dir: &Path, expected: &BTreeSet<PathBuf>) -> bool {
    expected.iter().any(|path| {
        path.parent()
            .is_some_and(|parent| parent == rel_dir || parent.starts_with(rel_dir))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn skill_names_includes_spears_and_allium_only() {
        let names = skill_names();
        assert_eq!(names, vec!["allium".to_string(), "spears".to_string()]);
    }

    #[test]
    fn skill_names_excludes_nested_skill_md() {
        // Allium has references/ but no nested SKILL.md, so allium:foo
        // should not appear at this layer (it would only appear if we
        // grew sub-skills; currently we don't ship any).
        let names = skill_names();
        for name in &names {
            assert!(
                !name.contains('/'),
                "skill name should not contain '/': {name}"
            );
        }
    }

    #[test]
    fn extract_writes_skill_md_and_companions() {
        let tmp = TempDir::new().unwrap();
        extract_to(tmp.path()).expect("extraction should succeed");
        assert!(tmp.path().join("allium/SKILL.md").is_file());
        assert!(tmp
            .path()
            .join("allium/references/language-reference.md")
            .is_file());
        assert!(tmp.path().join("spears/SKILL.md").is_file());
        assert!(tmp.path().join("spears/references/discovery.md").is_file());
        assert!(tmp.path().join("spears/adrs/_TEMPLATE.md").is_file());
    }

    #[test]
    fn extract_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        extract_to(tmp.path()).unwrap();
        let mtime_first = std::fs::metadata(tmp.path().join("spears/SKILL.md"))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        extract_to(tmp.path()).unwrap();
        let mtime_second = std::fs::metadata(tmp.path().join("spears/SKILL.md"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            mtime_first, mtime_second,
            "second extract should not rewrite unchanged file"
        );
    }

    #[test]
    fn extract_overwrites_modified_file() {
        let tmp = TempDir::new().unwrap();
        extract_to(tmp.path()).unwrap();
        let target = tmp.path().join("spears/SKILL.md");
        std::fs::write(&target, "tampered content").unwrap();
        extract_to(tmp.path()).unwrap();
        let restored = std::fs::read_to_string(&target).unwrap();
        assert_ne!(
            restored, "tampered content",
            "extraction should restore tampered file"
        );
        assert!(restored.contains("spEARS"));
    }

    #[test]
    fn extract_prunes_removed_builtin_files() {
        let tmp = TempDir::new().unwrap();
        let stale = tmp.path().join("spears/references/discover.md");
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(&stale, "old v1 guidance").unwrap();

        extract_to(tmp.path()).unwrap();

        assert!(
            !stale.exists(),
            "removed built-in companion should be pruned"
        );
        assert!(tmp.path().join("spears/references/discovery.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn extract_prunes_stale_symlink_companion() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let stale = tmp.path().join("spears/references/discover.md");
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        symlink(tmp.path().join("outside.md"), &stale).unwrap();

        extract_to(tmp.path()).unwrap();

        assert!(
            std::fs::symlink_metadata(&stale).is_err(),
            "stale symlink companion should be pruned"
        );
        assert!(tmp.path().join("spears/references/discovery.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn extract_replaces_embedded_file_symlink_destination() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let outside = tmp.path().join("outside.md");
        let dest = tmp.path().join("spears/SKILL.md");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&outside, "do not overwrite").unwrap();
        symlink(&outside, &dest).unwrap();

        extract_to(tmp.path()).unwrap();

        let metadata = std::fs::symlink_metadata(&dest).unwrap();
        assert!(
            metadata.is_file(),
            "embedded asset destination should be a regular file"
        );
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "do not overwrite"
        );
        assert!(std::fs::read_to_string(&dest)
            .unwrap()
            .contains("name: spears"));
    }

    #[cfg(unix)]
    #[test]
    fn extract_replaces_top_level_skill_symlink_without_following() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let outside = tmp.path().join("outside-skill");
        let outside_file = outside.join("keep.md");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(&outside_file, "do not delete").unwrap();
        symlink(&outside, tmp.path().join("spears")).unwrap();

        extract_to(tmp.path()).unwrap();

        assert_eq!(
            std::fs::read_to_string(&outside_file).unwrap(),
            "do not delete"
        );
        let skill_metadata = std::fs::symlink_metadata(tmp.path().join("spears")).unwrap();
        assert!(
            skill_metadata.is_dir(),
            "top-level skill path should be a real directory"
        );
        assert!(tmp.path().join("spears/SKILL.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn extract_replaces_symlinked_extract_root_without_following() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let outside = tmp.path().join("outside-root");
        let outside_file = outside.join("spears/unrelated.md");
        let extract_root = tmp.path().join("extract-root");
        std::fs::create_dir_all(outside_file.parent().unwrap()).unwrap();
        std::fs::write(&outside_file, "do not delete").unwrap();
        symlink(&outside, &extract_root).unwrap();

        extract_to(&extract_root).unwrap();

        assert_eq!(
            std::fs::read_to_string(&outside_file).unwrap(),
            "do not delete"
        );
        let root_metadata = std::fs::symlink_metadata(&extract_root).unwrap();
        assert!(
            root_metadata.is_dir(),
            "extract root should be a real directory"
        );
        assert!(extract_root.join("spears/SKILL.md").is_file());
    }
}
