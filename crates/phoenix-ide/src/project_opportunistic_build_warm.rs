#[cfg(target_os = "macos")]
use std::ffi::CString;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

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
        let Some(prepared) = prepare_candidate(source_root, dest_root, candidate.relative_path())
        else {
            continue;
        };
        let outcome = copier.clone_dir_best_effort(&prepared.src, &prepared.temp_dst);
        handle_clone_outcome(&prepared, outcome);
    }
}

struct PreparedCandidate {
    relative: PathBuf,
    src: PathBuf,
    dst: PathBuf,
    temp_dst: PathBuf,
}

fn prepare_candidate(
    source_root: &Path,
    dest_root: &Path,
    relative: &Path,
) -> Option<PreparedCandidate> {
    let src = source_root.join(relative);
    let dst = dest_root.join(relative);

    if !is_plain_directory(&src) {
        tracing::debug!(
            source = %src.display(),
            relative = %relative.display(),
            "build cache prewarm skipped: source directory missing or symlinked"
        );
        return None;
    }

    if contains_symlink(&src) {
        tracing::debug!(
            source = %src.display(),
            relative = %relative.display(),
            "build cache prewarm skipped: source tree contains symlink"
        );
        return None;
    }

    if dst.exists() {
        tracing::debug!(
            destination = %dst.display(),
            relative = %relative.display(),
            "build cache prewarm skipped: destination already exists"
        );
        return None;
    }

    if !destination_is_ignored(dest_root, relative, &dst) {
        return None;
    }

    let temp_dst = dest_root.join(temp_candidate_name(relative));
    if temp_dst.exists() {
        cleanup_path(&temp_dst, relative, "stale temporary prewarm path");
    }

    Some(PreparedCandidate {
        relative: relative.to_path_buf(),
        src,
        dst,
        temp_dst,
    })
}

fn destination_is_ignored(dest_root: &Path, relative: &Path, dst: &Path) -> bool {
    match is_ignored_in_dest_worktree(dest_root, relative) {
        Ok(true) => true,
        Ok(false) => {
            tracing::debug!(
                destination = %dst.display(),
                relative = %relative.display(),
                "build cache prewarm skipped: destination path is not ignored"
            );
            false
        }
        Err(reason) => {
            tracing::debug!(
                reason,
                destination = %dst.display(),
                relative = %relative.display(),
                "build cache prewarm skipped: could not verify ignore status"
            );
            false
        }
    }
}

fn handle_clone_outcome(prepared: &PreparedCandidate, outcome: WarmCopyOutcome) {
    match outcome {
        #[cfg(any(target_os = "macos", test))]
        WarmCopyOutcome::Cloned => install_cloned_candidate(prepared),
        WarmCopyOutcome::SkippedUnsupported { reason } => {
            cleanup_path(
                &prepared.temp_dst,
                &prepared.relative,
                "temporary prewarm path after unsupported clone",
            );
            tracing::debug!(
                reason,
                source = %prepared.src.display(),
                destination = %prepared.dst.display(),
                relative = %prepared.relative.display(),
                "build cache prewarm skipped: clone operation unsupported"
            );
        }
        #[cfg(any(target_os = "macos", test))]
        WarmCopyOutcome::Failed { reason } => {
            cleanup_path(
                &prepared.temp_dst,
                &prepared.relative,
                "partial temporary prewarm path after failed clone",
            );
            tracing::info!(
                reason,
                source = %prepared.src.display(),
                destination = %prepared.dst.display(),
                relative = %prepared.relative.display(),
                "build cache prewarm failed non-fatally"
            );
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn install_cloned_candidate(prepared: &PreparedCandidate) {
    if let Err(error) = install_temp_candidate(&prepared.temp_dst, &prepared.dst) {
        cleanup_path(
            &prepared.temp_dst,
            &prepared.relative,
            "temporary prewarm path after install failure",
        );
        tracing::info!(
            error = %error,
            source = %prepared.src.display(),
            destination = %prepared.dst.display(),
            relative = %prepared.relative.display(),
            "build cache prewarm failed non-fatally: destination install failed"
        );
        return;
    }
    tracing::info!(
        source = %prepared.src.display(),
        destination = %prepared.dst.display(),
        relative = %prepared.relative.display(),
        "build cache prewarm cloned allowlisted directory"
    );
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

fn is_plain_directory(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false)
}

fn contains_symlink(path: &Path) -> bool {
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            return true;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                return true;
            };
            let Ok(file_type) = entry.file_type() else {
                return true;
            };
            if file_type.is_symlink() {
                return true;
            }
            if file_type.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    false
}

fn is_ignored_in_dest_worktree(dest_root: &Path, relative: &Path) -> Result<bool, String> {
    match git_check_ignore(dest_root, relative) {
        Ok(true) => Ok(true),
        Ok(false) => git_check_ignore(dest_root, &directory_style_path(relative)),
        Err(reason) => Err(reason),
    }
}

fn git_check_ignore(dest_root: &Path, relative: &Path) -> Result<bool, String> {
    let output = std::process::Command::new("git")
        .arg("check-ignore")
        .arg("--quiet")
        .arg("--")
        .arg(relative)
        .current_dir(dest_root)
        .output()
        .map_err(|error| format!("failed to run git check-ignore: {error}"))?;

    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        Some(code) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("git check-ignore exited with status {code}")
            } else {
                stderr
            })
        }
        None => Err("git check-ignore terminated by signal".to_string()),
    }
}

fn directory_style_path(relative: &Path) -> PathBuf {
    let mut path = relative.as_os_str().to_os_string();
    path.push(std::path::MAIN_SEPARATOR.to_string());
    PathBuf::from(path)
}

fn temp_candidate_name(relative: &Path) -> String {
    let mut name = String::from(".phoenix-prewarm-");
    for (idx, component) in relative.components().enumerate() {
        if idx > 0 {
            name.push('-');
        }
        name.push_str(&component.as_os_str().to_string_lossy());
    }
    name
}

#[cfg(any(target_os = "macos", test))]
fn install_temp_candidate(temp_dst: &Path, dst: &Path) -> Result<(), String> {
    let parent = dst
        .parent()
        .ok_or_else(|| format!("destination has no parent: {}", dst.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create destination parent: {error}"))?;
    std::fs::rename(temp_dst, dst)
        .map_err(|error| format!("failed to rename clone into place: {error}"))
}

fn cleanup_path(path: &Path, relative: &Path, reason: &str) {
    if !path.exists() {
        return;
    }
    if let Err(error) = std::fs::remove_dir_all(path) {
        tracing::debug!(
            error = %error,
            path = %path.display(),
            relative = %relative.display(),
            reason,
            "build cache prewarm cleanup failed"
        );
    }
}

fn clone_dir_best_effort(src: &Path, dst: &Path) -> WarmCopyOutcome {
    #[cfg(target_os = "macos")]
    {
        match clone_dir_recursive(src, dst) {
            Ok(()) => WarmCopyOutcome::Cloned,
            Err(CloneDirError::Unsupported(reason)) => {
                WarmCopyOutcome::SkippedUnsupported { reason }
            }
            Err(CloneDirError::Failed(reason)) => WarmCopyOutcome::Failed { reason },
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

#[cfg(target_os = "macos")]
#[derive(Debug)]
enum CloneDirError {
    Unsupported(String),
    Failed(String),
}

#[cfg(target_os = "macos")]
fn clone_dir_recursive(src: &Path, dst: &Path) -> Result<(), CloneDirError> {
    std::fs::create_dir(dst).map_err(|error| {
        CloneDirError::Failed(format!("failed to create cloned directory root: {error}"))
    })?;

    let result = clone_dir_children(src, dst);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(dst);
    }
    result
}

#[cfg(target_os = "macos")]
fn clone_dir_children(src: &Path, dst: &Path) -> Result<(), CloneDirError> {
    let entries = std::fs::read_dir(src).map_err(|error| {
        CloneDirError::Failed(format!("failed to read source directory: {error}"))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            CloneDirError::Failed(format!("failed to read source directory entry: {error}"))
        })?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| {
            CloneDirError::Failed(format!("failed to read source file type: {error}"))
        })?;

        if file_type.is_symlink() {
            return Err(CloneDirError::Failed(format!(
                "source tree contains symlink: {}",
                src_path.display()
            )));
        }
        if file_type.is_dir() {
            std::fs::create_dir(&dst_path).map_err(|error| {
                CloneDirError::Failed(format!("failed to create cloned subdirectory: {error}"))
            })?;
            clone_dir_children(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            clone_file(&src_path, &dst_path)?;
        } else {
            return Err(CloneDirError::Failed(format!(
                "source tree contains unsupported file type: {}",
                src_path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn clone_file(src: &Path, dst: &Path) -> Result<(), CloneDirError> {
    let src_c = cstring_path(src)?;
    let dst_c = cstring_path(dst)?;
    let rc = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
    if rc == 0 {
        return Ok(());
    }

    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOTSUP | libc::EXDEV | libc::ENOSYS) => {
            Err(CloneDirError::Unsupported(format!(
                "clonefile unsupported for {} -> {}: {error}",
                src.display(),
                dst.display()
            )))
        }
        _ => Err(CloneDirError::Failed(format!(
            "clonefile failed for {} -> {}: {error}",
            src.display(),
            dst.display()
        ))),
    }
}

#[cfg(target_os = "macos")]
fn cstring_path(path: &Path) -> Result<CString, CloneDirError> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        CloneDirError::Failed(format!(
            "path contains interior nul byte: {}",
            path.display()
        ))
    })
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
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dest).unwrap();
        init_git_repo_with_ignore(
            &dest,
            "target/\nnode_modules/.cache/\n.next/cache/\n.turbo/\n.vite/\n",
        );
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
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dest).unwrap();
        init_git_repo_with_ignore(&dest, "target/\n.turbo/\n");
        fs::create_dir_all(source.join("target")).unwrap();
        fs::create_dir_all(source.join(".turbo")).unwrap();

        let copier = RecordingCopier::new(WarmCopyOutcome::Failed {
            reason: "boom".to_string(),
        });
        prewarm_project_build_caches_with_copier(&source, &dest, &copier);

        assert_eq!(copier.calls.borrow().len(), 2);
        assert!(!dest.join("target").exists());
        assert!(!dest.join(".turbo").exists());
    }

    #[test]
    fn prewarm_unsupported_clone_does_not_create_nested_parents() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dest).unwrap();
        init_git_repo_with_ignore(&dest, "node_modules/.cache/\n");
        fs::create_dir_all(source.join("node_modules/.cache")).unwrap();

        let copier = RecordingCopier::new(WarmCopyOutcome::SkippedUnsupported {
            reason: "unsupported".to_string(),
        });
        prewarm_project_build_caches_with_copier(&source, &dest, &copier);

        assert_eq!(copier.calls.borrow().len(), 1);
        assert!(!dest.join("node_modules").exists());
    }

    #[test]
    fn prewarm_skips_symlinked_source_directory() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        let shared = temp.path().join("shared-target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::create_dir_all(&shared).unwrap();
        init_git_repo_with_ignore(&dest, "target/\n");
        make_symlink(&shared, &source.join("target"));

        let copier = RecordingCopier::new(WarmCopyOutcome::Cloned);
        prewarm_project_build_caches_with_copier(&source, &dest, &copier);

        assert!(copier.calls.borrow().is_empty());
        assert!(!dest.join("target").exists());
    }

    #[test]
    fn prewarm_skips_source_tree_containing_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        let shared = temp.path().join("shared-cache-file");
        fs::create_dir_all(source.join("target")).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(&shared, "shared").unwrap();
        init_git_repo_with_ignore(&dest, "target/\n");
        make_symlink(&shared, &source.join("target/link"));

        let copier = RecordingCopier::new(WarmCopyOutcome::Cloned);
        prewarm_project_build_caches_with_copier(&source, &dest, &copier);

        assert!(copier.calls.borrow().is_empty());
        assert!(!dest.join("target").exists());
    }

    #[test]
    fn prewarm_skips_unignored_destination_path() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        fs::create_dir_all(source.join("target")).unwrap();
        fs::create_dir_all(&dest).unwrap();
        init_git_repo_with_ignore(&dest, "");

        let copier = RecordingCopier::new(WarmCopyOutcome::Cloned);
        prewarm_project_build_caches_with_copier(&source, &dest, &copier);

        assert!(copier.calls.borrow().is_empty());
        assert!(!dest.join("target").exists());
    }

    #[test]
    fn prewarm_installs_nested_candidate_only_after_success() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        fs::create_dir_all(source.join("node_modules/.cache")).unwrap();
        fs::write(source.join("node_modules/.cache/cache.txt"), "warm").unwrap();
        fs::create_dir_all(&dest).unwrap();
        init_git_repo_with_ignore(&dest, "node_modules/.cache/\n");

        let copier = RecordingCopier::copy_then_clone();
        prewarm_project_build_caches_with_copier(&source, &dest, &copier);

        assert_eq!(copier.calls.borrow().len(), 1);
        assert_eq!(
            fs::read_to_string(dest.join("node_modules/.cache/cache.txt")).unwrap(),
            "warm"
        );
        assert!(!dest.join(".phoenix-prewarm-node_modules-.cache").exists());
    }

    #[test]
    fn prewarm_removes_partial_temp_cache_after_failure() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        fs::create_dir_all(source.join("target")).unwrap();
        fs::create_dir_all(&dest).unwrap();
        init_git_repo_with_ignore(&dest, "target/\n");

        let copier = RecordingCopier::create_then_fail();
        prewarm_project_build_caches_with_copier(&source, &dest, &copier);

        assert_eq!(copier.calls.borrow().len(), 1);
        assert!(!dest.join("target").exists());
        assert!(!dest.join(".phoenix-prewarm-target").exists());
    }

    fn init_git_repo_with_ignore(root: &Path, ignore: &str) {
        run(root, &["git", "init", "--quiet", "--initial-branch=main"]);
        fs::write(root.join(".gitignore"), ignore).unwrap();
    }

    fn run(cwd: &Path, args: &[&str]) {
        let status = std::process::Command::new(args[0])
            .args(&args[1..])
            .current_dir(cwd)
            .status()
            .unwrap_or_else(|error| panic!("failed to run {args:?}: {error}"));
        assert!(status.success(), "command failed: {args:?}");
    }

    #[cfg(unix)]
    fn make_symlink(src: &Path, dst: &Path) {
        std::os::unix::fs::symlink(src, dst).unwrap();
    }

    #[cfg(windows)]
    fn make_symlink(src: &Path, dst: &Path) {
        if src.is_dir() {
            std::os::windows::fs::symlink_dir(src, dst).unwrap();
        } else {
            std::os::windows::fs::symlink_file(src, dst).unwrap();
        }
    }

    fn copy_tree_for_test(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let file_type = entry.file_type().unwrap();
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if file_type.is_dir() {
                copy_tree_for_test(&src_path, &dst_path);
            } else if file_type.is_file() {
                fs::copy(&src_path, &dst_path).unwrap();
            }
        }
    }

    #[derive(Debug, Clone)]
    enum RecordingCopierBehavior {
        Outcome(WarmCopyOutcome),
        CreateThenFail,
        CopyThenClone,
    }

    #[derive(Debug)]
    struct RecordingCopier {
        behavior: RecordingCopierBehavior,
        calls: RefCell<Vec<(PathBuf, PathBuf)>>,
    }

    impl RecordingCopier {
        fn new(outcome: WarmCopyOutcome) -> Self {
            Self {
                behavior: RecordingCopierBehavior::Outcome(outcome),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn create_then_fail() -> Self {
            Self {
                behavior: RecordingCopierBehavior::CreateThenFail,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn copy_then_clone() -> Self {
            Self {
                behavior: RecordingCopierBehavior::CopyThenClone,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl WarmCopier for RecordingCopier {
        fn clone_dir_best_effort(&self, src: &Path, dst: &Path) -> WarmCopyOutcome {
            self.calls
                .borrow_mut()
                .push((src.to_path_buf(), dst.to_path_buf()));
            match &self.behavior {
                RecordingCopierBehavior::Outcome(outcome) => outcome.clone(),
                RecordingCopierBehavior::CreateThenFail => {
                    fs::create_dir_all(dst).unwrap();
                    fs::write(dst.join("partial.txt"), "partial").unwrap();
                    WarmCopyOutcome::Failed {
                        reason: "boom".to_string(),
                    }
                }
                RecordingCopierBehavior::CopyThenClone => {
                    copy_tree_for_test(src, dst);
                    WarmCopyOutcome::Cloned
                }
            }
        }
    }
}
