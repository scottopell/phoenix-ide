//! Built-in skills compiled into the phoenix binary as an embedded directory
//! tree. At server startup, the tree is materialized to
//! `<HOME>/.phoenix-ide/builtin-skills/` so each built-in becomes a real
//! filesystem skill — same path semantics as user-installed skills, including
//! companion files (`references/*.md`, `scripts/`, etc.).
//!
//! ## Layout
//!
//! Each subdirectory under `src/skills/builtin/` is one skill. The directory
//! must contain `SKILL.md` with the standard frontmatter; any other files
//! (references, scripts, examples) are extracted alongside and visible to
//! the LLM via the existing skill-base-directory mechanism.
//!
//! ```
//! src/skills/builtin/
//!   caveman/
//!     SKILL.md
//!   allium/
//!     SKILL.md
//!     references/
//!       language-reference.md
//! ```
//!
//! ## Override
//!
//! A user-installed filesystem skill of the same name shadows the built-in.
//! The walk-up over `.claude/skills/` and `.agents/skills/` runs before the
//! built-in extract dir is scanned, and the existing name dedup keeps the
//! first-seen entry.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(rust_embed::RustEmbed)]
#[folder = "src/skills/builtin/"]
struct BuiltinAssets;

/// Subdirectory under the phoenix data dir where built-ins are extracted.
pub const EXTRACT_SUBDIR: &str = "builtin-skills";

/// Default extraction target: `<HOME>/.phoenix-ide/builtin-skills/`.
/// Returns `None` if `$HOME` is unset.
#[must_use]
pub fn default_extract_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|home| {
        PathBuf::from(home)
            .join(".phoenix-ide")
            .join(EXTRACT_SUBDIR)
    })
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
/// Overwrites existing files; does not delete files that are no longer in
/// the binary (rare, and a user can remove the target dir manually).
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
    for path in BuiltinAssets::iter() {
        let asset = BuiltinAssets::get(&path).expect("iterated asset must exist");
        let dest = target_dir.join(path.as_ref());
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Skip the write if content already matches — avoids touching mtimes
        // unnecessarily on every server restart.
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn skill_names_includes_caveman_and_allium() {
        let names = skill_names();
        assert!(names.contains(&"caveman".to_string()), "got {names:?}");
        assert!(names.contains(&"allium".to_string()), "got {names:?}");
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
        assert!(tmp.path().join("caveman/SKILL.md").is_file());
        assert!(tmp.path().join("allium/SKILL.md").is_file());
        assert!(tmp
            .path()
            .join("allium/references/language-reference.md")
            .is_file());
    }

    #[test]
    fn extract_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        extract_to(tmp.path()).unwrap();
        let mtime_first = std::fs::metadata(tmp.path().join("caveman/SKILL.md"))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        extract_to(tmp.path()).unwrap();
        let mtime_second = std::fs::metadata(tmp.path().join("caveman/SKILL.md"))
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
        let target = tmp.path().join("caveman/SKILL.md");
        std::fs::write(&target, "tampered content").unwrap();
        extract_to(tmp.path()).unwrap();
        let restored = std::fs::read_to_string(&target).unwrap();
        assert_ne!(
            restored, "tampered content",
            "extraction should restore tampered file"
        );
        assert!(restored.contains("Caveman Mode"));
    }
}
