use std::path::{Component, Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;

const ALLOWLISTED_CACHE_DIRS: &[&str] = &[
    "target",
    "node_modules/.cache",
    ".next/cache",
    ".turbo",
    ".vite",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildWarmCandidate {
    relative_path: PathBuf,
}

impl BuildWarmCandidate {
    fn new(relative_path: &str) -> Option<Self> {
        let path = PathBuf::from(relative_path);
        is_safe_repo_relative_path(&path).then_some(Self {
            relative_path: path,
        })
    }

    pub(crate) fn relative_path(&self) -> &Path {
        &self.relative_path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WarmCopyOutcome {
    #[cfg(any(target_os = "macos", test))]
    Cloned,
    SkippedUnsupported {
        reason: String,
    },
    #[cfg(any(target_os = "macos", test))]
    Failed {
        reason: String,
    },
}

pub(crate) trait WarmCopier {
    fn clone_dir_best_effort(&self, src: &Path, dst: &Path) -> WarmCopyOutcome;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SystemWarmCopier;

impl WarmCopier for SystemWarmCopier {
    fn clone_dir_best_effort(&self, src: &Path, dst: &Path) -> WarmCopyOutcome {
        clone_dir_best_effort(src, dst)
    }
}

pub(crate) fn prewarm_project_build_caches(source_root: &Path, dest_root: &Path) {
    let copier = SystemWarmCopier;
    prewarm_project_build_caches_with_copier(source_root, dest_root, &copier);
}

pub(crate) fn prewarm_project_build_caches_with_copier(
    source_root: &Path,
    dest_root: &Path,
    copier: &dyn WarmCopier,
) {
    for candidate in allowlisted_candidates() {
        let relative = candidate.relative_path();
        let src = source_root.join(relative);
        let dst = dest_root.join(relative);

        if !src.is_dir() {
            tracing::debug!(
                source = %src.display(),
                relative = %relative.display(),
                "build cache prewarm skipped: source directory missing"
            );
            continue;
        }

        if dst.exists() {
            tracing::debug!(
                destination = %dst.display(),
                relative = %relative.display(),
                "build cache prewarm skipped: destination already exists"
            );
            continue;
        }

        if let Some(parent) = dst.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                tracing::info!(
                    error = %error,
                    destination = %dst.display(),
                    relative = %relative.display(),
                    "build cache prewarm failed non-fatally: destination parent unavailable"
                );
                continue;
            }
        }

        match copier.clone_dir_best_effort(&src, &dst) {
            #[cfg(any(target_os = "macos", test))]
            WarmCopyOutcome::Cloned => tracing::info!(
                source = %src.display(),
                destination = %dst.display(),
                relative = %relative.display(),
                "build cache prewarm cloned allowlisted directory"
            ),
            WarmCopyOutcome::SkippedUnsupported { reason } => tracing::debug!(
                reason,
                source = %src.display(),
                destination = %dst.display(),
                relative = %relative.display(),
                "build cache prewarm skipped: clone operation unsupported"
            ),
            #[cfg(any(target_os = "macos", test))]
            WarmCopyOutcome::Failed { reason } => tracing::info!(
                reason,
                source = %src.display(),
                destination = %dst.display(),
                relative = %relative.display(),
                "build cache prewarm failed non-fatally"
            ),
        }
    }
}

#[cfg(test)]
fn detect_existing_candidates(source_root: &Path) -> Vec<BuildWarmCandidate> {
    allowlisted_candidates()
        .into_iter()
        .filter(|candidate| source_root.join(candidate.relative_path()).is_dir())
        .collect()
}

fn allowlisted_candidates() -> Vec<BuildWarmCandidate> {
    ALLOWLISTED_CACHE_DIRS
        .iter()
        .filter_map(|relative| BuildWarmCandidate::new(relative))
        .collect()
}

fn is_safe_repo_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn clone_dir_best_effort(src: &Path, dst: &Path) -> WarmCopyOutcome {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/bin/cp")
            .arg("-c")
            .arg("-R")
            .arg(src)
            .arg(dst)
            .output();

        let output = match output {
            Ok(output) => output,
            Err(error) => {
                return WarmCopyOutcome::Failed {
                    reason: format!("failed to spawn /bin/cp: {error}"),
                };
            }
        };

        if output.status.success() {
            return WarmCopyOutcome::Cloned;
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let reason = if stderr.is_empty() {
            format!("/bin/cp exited with status {}", output.status)
        } else {
            stderr
        };
        let reason_lower = reason.to_ascii_lowercase();
        if reason_lower.contains("not supported")
            || reason_lower.contains("illegal option")
            || reason_lower.contains("invalid option")
        {
            WarmCopyOutcome::SkippedUnsupported { reason }
        } else {
            WarmCopyOutcome::Failed { reason }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (src, dst);
        WarmCopyOutcome::SkippedUnsupported {
            reason: "copy-on-write directory cloning is only enabled on macOS".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;

    #[test]
    fn detector_finds_only_existing_allowlisted_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::create_dir_all(root.join("node_modules/.cache/vite")).unwrap();
        fs::create_dir_all(root.join(".next/cache")).unwrap();
        fs::create_dir_all(root.join(".turbo")).unwrap();
        fs::create_dir_all(root.join(".vite")).unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::create_dir_all(root.join(".phoenix/worktrees")).unwrap();
        fs::create_dir_all(root.join("dist/cache")).unwrap();
        fs::write(root.join("package-lock.json"), "{}").unwrap();

        let mut found: Vec<_> = detect_existing_candidates(root)
            .into_iter()
            .map(|candidate| candidate.relative_path().to_string_lossy().to_string())
            .collect();
        found.sort();

        assert_eq!(
            found,
            vec![
                ".next/cache",
                ".turbo",
                ".vite",
                "node_modules/.cache",
                "target",
            ]
        );
    }

    #[test]
    fn detector_ignores_missing_allowlisted_directories() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("target")).unwrap();

        let found: Vec<_> = detect_existing_candidates(temp.path())
            .into_iter()
            .map(|candidate| candidate.relative_path().to_string_lossy().to_string())
            .collect();

        assert_eq!(found, vec!["target"]);
    }

    #[test]
    fn candidate_rejects_unsafe_relative_paths() {
        assert!(BuildWarmCandidate::new("target").is_some());
        assert!(BuildWarmCandidate::new("node_modules/.cache").is_some());
        assert!(BuildWarmCandidate::new("../target").is_none());
        assert!(BuildWarmCandidate::new("target/../.git").is_none());
        assert!(BuildWarmCandidate::new("/tmp/target").is_none());
        assert!(BuildWarmCandidate::new("").is_none());
    }

    #[test]
    fn prewarm_skips_existing_destination_without_overwriting() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        fs::create_dir_all(source.join("target")).unwrap();
        fs::create_dir_all(dest.join("target")).unwrap();
        fs::write(dest.join("target/owned.txt"), "keep").unwrap();

        let copier = RecordingCopier::new(WarmCopyOutcome::Cloned);
        prewarm_project_build_caches_with_copier(&source, &dest, &copier);

        assert!(copier.calls.borrow().is_empty());
        assert_eq!(
            fs::read_to_string(dest.join("target/owned.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn prewarm_failure_is_non_fatal_and_continues() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        fs::create_dir_all(source.join("target")).unwrap();
        fs::create_dir_all(source.join(".turbo")).unwrap();

        let copier = RecordingCopier::new(WarmCopyOutcome::Failed {
            reason: "boom".to_string(),
        });
        prewarm_project_build_caches_with_copier(&source, &dest, &copier);

        assert_eq!(copier.calls.borrow().len(), 2);
    }

    #[derive(Debug)]
    struct RecordingCopier {
        outcome: WarmCopyOutcome,
        calls: RefCell<Vec<(PathBuf, PathBuf)>>,
    }

    impl RecordingCopier {
        fn new(outcome: WarmCopyOutcome) -> Self {
            Self {
                outcome,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl WarmCopier for RecordingCopier {
        fn clone_dir_best_effort(&self, src: &Path, dst: &Path) -> WarmCopyOutcome {
            self.calls
                .borrow_mut()
                .push((src.to_path_buf(), dst.to_path_buf()));
            self.outcome.clone()
        }
    }
}
