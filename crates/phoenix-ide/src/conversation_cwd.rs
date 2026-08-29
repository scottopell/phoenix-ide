use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConversationCwdError {
    Empty,
    NotAcceptable(String),
    FilesystemRoot,
}

impl std::fmt::Display for ConversationCwdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConversationCwdError::Empty => write!(f, "Working directory cannot be empty"),
            ConversationCwdError::NotAcceptable(e) => {
                write!(f, "Working directory is not an acceptable directory: {e}")
            }
            ConversationCwdError::FilesystemRoot => write!(
                f,
                "Working directory cannot be the filesystem root; choose a project or home subdirectory"
            ),
        }
    }
}

impl std::error::Error for ConversationCwdError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidConversationCwd {
    raw: String,
    #[cfg(test)]
    canonical: PathBuf,
}

impl ValidConversationCwd {
    pub(crate) fn raw(&self) -> &str {
        &self.raw
    }

    pub(crate) fn as_path(&self) -> &Path {
        Path::new(&self.raw)
    }

    pub(crate) fn into_raw(self) -> String {
        self.raw
    }

    pub(crate) fn path_buf(&self) -> PathBuf {
        PathBuf::from(&self.raw)
    }

    #[cfg(test)]
    pub(crate) fn canonical(&self) -> &Path {
        &self.canonical
    }
}

pub(crate) fn normalize_product_creation_cwd_intent(
    cwd: impl AsRef<str>,
    home: &Path,
) -> Result<String, ConversationCwdError> {
    let raw = cwd.as_ref().trim();
    if raw.is_empty() {
        return Err(ConversationCwdError::Empty);
    }
    let path = Path::new(raw);
    if path.exists() {
        return validate_conversation_cwd(raw).map(ValidConversationCwd::into_raw);
    }
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(ConversationCwdError::NotAcceptable(
            "a new directory must use an absolute path without parent traversal".to_string(),
        ));
    }

    let ancestor = path
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(|| {
            ConversationCwdError::NotAcceptable("no existing parent directory".to_string())
        })?;
    let canonical_ancestor = std::fs::canonicalize(ancestor)
        .map_err(|error| ConversationCwdError::NotAcceptable(error.to_string()))?;
    if !canonical_ancestor.is_dir() {
        return Err(ConversationCwdError::NotAcceptable(
            "nearest existing parent is not a directory".to_string(),
        ));
    }
    let allowed = [home, Path::new("/tmp")].into_iter().any(|root| {
        std::fs::canonicalize(root)
            .is_ok_and(|canonical_root| canonical_ancestor.starts_with(canonical_root))
    });
    if !allowed {
        return Err(ConversationCwdError::NotAcceptable(
            "new directories must be under the server home directory or /tmp".to_string(),
        ));
    }
    let suffix = path.strip_prefix(ancestor).map_err(|error| {
        ConversationCwdError::NotAcceptable(format!("invalid new directory path: {error}"))
    })?;
    let normalized = canonical_ancestor.join(suffix);
    if normalized.parent().is_none() {
        return Err(ConversationCwdError::FilesystemRoot);
    }
    Ok(normalized.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn create_relative_directories_without_symlinks(
    canonical_ancestor: &Path,
    suffix: &Path,
) -> Result<(), ConversationCwdError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    let ancestor = CString::new(canonical_ancestor.as_os_str().as_bytes()).map_err(|_| {
        ConversationCwdError::NotAcceptable("directory path contains a NUL byte".to_string())
    })?;
    let fd = unsafe {
        libc::open(
            ancestor.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(ConversationCwdError::NotAcceptable(format!(
            "failed to open accepted directory root: {}",
            std::io::Error::last_os_error()
        )));
    }
    let mut current = unsafe { OwnedFd::from_raw_fd(fd) };
    for component in suffix.components() {
        let std::path::Component::Normal(segment) = component else {
            return Err(ConversationCwdError::NotAcceptable(
                "invalid selected directory segment".to_string(),
            ));
        };
        let segment = CString::new(segment.as_bytes()).map_err(|_| {
            ConversationCwdError::NotAcceptable("directory path contains a NUL byte".to_string())
        })?;
        let mut next = unsafe {
            libc::openat(
                current.as_raw_fd(),
                segment.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if next < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            let created = unsafe { libc::mkdirat(current.as_raw_fd(), segment.as_ptr(), 0o755) };
            if created < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
                return Err(ConversationCwdError::NotAcceptable(format!(
                    "failed to create directory after durable acceptance: {}",
                    std::io::Error::last_os_error()
                )));
            }
            next = unsafe {
                libc::openat(
                    current.as_raw_fd(),
                    segment.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
        }
        if next < 0 {
            return Err(ConversationCwdError::NotAcceptable(format!(
                "directory path contains an inaccessible or symlinked segment: {}",
                std::io::Error::last_os_error()
            )));
        }
        current = unsafe { OwnedFd::from_raw_fd(next) };
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_relative_directories_without_symlinks(
    _canonical_ancestor: &Path,
    _suffix: &Path,
) -> Result<(), ConversationCwdError> {
    Err(ConversationCwdError::NotAcceptable(
        "safe creation of missing selected directories is unsupported on this platform".to_string(),
    ))
}

pub(crate) fn ensure_product_creation_cwd(
    cwd: impl AsRef<str>,
    home: &Path,
) -> Result<ValidConversationCwd, ConversationCwdError> {
    let raw = cwd.as_ref();
    let path = Path::new(raw);
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(ConversationCwdError::NotAcceptable(
            "directory path contains a symlink".to_string(),
        ));
    }
    if path.exists() {
        let mut current = PathBuf::new();
        for component in path.components() {
            current.push(component);
            if current
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(ConversationCwdError::NotAcceptable(
                    "directory path contains a symlink".to_string(),
                ));
            }
        }
        return validate_conversation_cwd(raw);
    }
    let ancestor = path
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .ok_or_else(|| {
            ConversationCwdError::NotAcceptable("no existing parent directory".to_string())
        })?;
    let canonical_ancestor = std::fs::canonicalize(ancestor)
        .map_err(|error| ConversationCwdError::NotAcceptable(error.to_string()))?;
    let allowed = [home, Path::new("/tmp")].into_iter().any(|root| {
        std::fs::canonicalize(root)
            .is_ok_and(|canonical_root| canonical_ancestor.starts_with(canonical_root))
    });
    if !allowed {
        return Err(ConversationCwdError::NotAcceptable(
            "new directories must remain under the server home directory or /tmp".to_string(),
        ));
    }
    let suffix = path.strip_prefix(ancestor).map_err(|error| {
        ConversationCwdError::NotAcceptable(format!("invalid accepted directory path: {error}"))
    })?;
    if canonical_ancestor.join(suffix) != path {
        return Err(ConversationCwdError::NotAcceptable(
            "directory resolved outside the accepted canonical path".to_string(),
        ));
    }

    create_relative_directories_without_symlinks(&canonical_ancestor, suffix)?;
    validate_conversation_cwd(raw)
}

pub(crate) fn validate_conversation_cwd(
    cwd: impl AsRef<str>,
) -> Result<ValidConversationCwd, ConversationCwdError> {
    let raw = cwd.as_ref();
    let path = Path::new(raw);
    if raw.trim().is_empty() {
        return Err(ConversationCwdError::Empty);
    }

    let canonical = std::fs::canonicalize(path)
        .map_err(|e| ConversationCwdError::NotAcceptable(e.to_string()))?;
    if !canonical.is_dir() {
        return Err(ConversationCwdError::NotAcceptable(
            "path is not a directory".to_string(),
        ));
    }
    if canonical.parent().is_none() {
        return Err(ConversationCwdError::FilesystemRoot);
    }

    let raw = canonical.to_string_lossy().into_owned();
    #[cfg(test)]
    let valid = ValidConversationCwd { raw, canonical };
    #[cfg(not(test))]
    let valid = ValidConversationCwd { raw };
    Ok(valid)
}

pub(crate) fn validate_conversation_cwd_for_runtime(
    conv_id: &str,
    cwd: &str,
) -> Result<ValidConversationCwd, ConversationCwdError> {
    validate_conversation_cwd(cwd).map_err(|e| {
        tracing::error!(conv_id, cwd, error = %e, "Rejected invalid persisted conversation cwd");
        e
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_filesystem_root() {
        let err = validate_conversation_cwd("/").expect_err("root rejected");
        assert!(err.to_string().contains("filesystem root"), "got: {err}");
    }

    #[test]
    fn rejects_parent_traversal_that_resolves_to_root() {
        let err = validate_conversation_cwd("/..").expect_err("canonical root rejected");
        assert!(err.to_string().contains("filesystem root"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_to_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let link = tmp.path().join("root-link");
        std::os::unix::fs::symlink("/", &link).expect("symlink");

        let err =
            validate_conversation_cwd(link.to_str().unwrap()).expect_err("root link rejected");
        assert!(err.to_string().contains("filesystem root"), "got: {err}");
    }

    #[test]
    fn normalizes_missing_product_directory_without_creating_it() {
        let home = tempfile::tempdir().expect("home");
        let requested = home.path().join("new/project");

        let normalized =
            normalize_product_creation_cwd_intent(requested.to_string_lossy(), home.path())
                .expect("missing leaf is acceptable");

        assert_eq!(
            Path::new(&normalized),
            home.path()
                .canonicalize()
                .expect("canonical home")
                .join("new/project")
        );
        assert!(
            !requested.exists(),
            "acceptance validation must not mutate the filesystem"
        );
    }

    #[test]
    fn creates_product_directory_only_when_worker_ensures_it() {
        let home = tempfile::tempdir().expect("home");
        let requested = home.path().join("new/project");
        let normalized =
            normalize_product_creation_cwd_intent(requested.to_string_lossy(), home.path())
                .expect("missing leaf is acceptable");

        let valid = ensure_product_creation_cwd(&normalized, home.path())
            .expect("worker creates directory");
        let replayed = ensure_product_creation_cwd(&normalized, home.path())
            .expect("worker creation is idempotent");

        assert!(requested.is_dir());
        assert_eq!(valid.raw(), normalized);
        assert_eq!(replayed, valid);
    }

    #[cfg(unix)]
    #[test]
    fn worker_rejects_symlink_at_final_missing_component() {
        let home = tempfile::tempdir().expect("home");
        let target = tempfile::tempdir().expect("target");
        let requested = home.path().join("new-project");
        let normalized =
            normalize_product_creation_cwd_intent(requested.to_string_lossy(), home.path())
                .expect("missing leaf accepted");
        std::os::unix::fs::symlink(target.path(), &requested).expect("insert final symlink");

        let error = ensure_product_creation_cwd(&normalized, home.path())
            .expect_err("final symlink rejected");

        assert!(error.to_string().contains("symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn relative_creator_rejects_symlinked_segment_without_mutating_target() {
        let home = tempfile::tempdir().expect("home");
        let target = tempfile::tempdir().expect("target");
        std::os::unix::fs::symlink(target.path(), home.path().join("raced"))
            .expect("insert symlink");

        let error = create_relative_directories_without_symlinks(
            &home.path().canonicalize().expect("canonical home"),
            Path::new("raced/child"),
        )
        .expect_err("symlinked segment rejected");

        assert!(error.to_string().contains("symlinked segment"));
        assert!(!target.path().join("child").exists());
    }

    #[test]
    fn worker_accepts_existing_directory_outside_creation_roots() {
        let home = tempfile::tempdir().expect("home");
        let existing = std::env::current_dir().expect("current directory");

        let valid = ensure_product_creation_cwd(existing.to_string_lossy(), home.path())
            .expect("existing directories keep the general cwd contract");

        assert_eq!(
            valid.as_path(),
            existing.canonicalize().expect("canonical cwd")
        );
    }

    #[cfg(unix)]
    #[test]
    fn worker_rejects_intermediate_symlink_after_target_was_created() {
        let home = tempfile::tempdir().expect("home");
        let outside = tempfile::tempdir().expect("outside");
        let requested = home.path().join("new/project");
        let normalized =
            normalize_product_creation_cwd_intent(requested.to_string_lossy(), home.path())
                .expect("missing leaf accepted");
        std::os::unix::fs::symlink(outside.path(), home.path().join("new"))
            .expect("insert intermediate symlink");
        std::fs::create_dir(outside.path().join("project")).expect("raced target exists");

        let error = ensure_product_creation_cwd(&normalized, home.path())
            .expect_err("existing target reached through an intermediate symlink is rejected");

        assert!(error.to_string().contains("symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn worker_rejects_symlink_inserted_after_acceptance() {
        let home = tempfile::tempdir().expect("home");
        let outside = tempfile::tempdir().expect("outside");
        let requested = home.path().join("new/project");
        let normalized =
            normalize_product_creation_cwd_intent(requested.to_string_lossy(), home.path())
                .expect("missing leaf is acceptable");
        std::os::unix::fs::symlink(outside.path(), home.path().join("new"))
            .expect("insert symlink after acceptance");

        let error = ensure_product_creation_cwd(&normalized, home.path())
            .expect_err("symlink escape rejected");

        let message = error.to_string();
        assert!(
            message.contains("outside the accepted canonical path")
                || message.contains("must remain under the server home directory or /tmp"),
            "unexpected rejection: {message}"
        );
        assert!(
            !outside.path().join("project").exists(),
            "revalidation must reject before creating beneath the symlink"
        );
    }

    #[test]
    fn rejects_missing_product_directory_outside_allowed_roots() {
        let home = tempfile::tempdir().expect("home");
        let requested = std::env::current_dir()
            .expect("current directory")
            .join(format!("missing-product-cwd-{}", uuid::Uuid::new_v4()));

        let error = normalize_product_creation_cwd_intent(requested.to_string_lossy(), home.path())
            .expect_err("outside path rejected");

        assert!(error.to_string().contains("server home directory or /tmp"));
        assert!(!requested.exists());
    }

    #[test]
    fn accepts_deep_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let deep = tmp.path().join("project/src");
        std::fs::create_dir_all(&deep).expect("deep dir");

        let valid = validate_conversation_cwd(deep.to_str().unwrap()).expect("accepted");
        let canonical = deep.canonicalize().unwrap();
        assert_eq!(valid.raw(), canonical.to_str().unwrap());
        assert_eq!(valid.canonical(), canonical);
    }

    #[test]
    fn accepts_relative_directory_and_normalizes_to_absolute() {
        let valid = validate_conversation_cwd(".").expect("relative accepted");
        let canonical = std::env::current_dir()
            .expect("cwd")
            .canonicalize()
            .expect("canonical cwd");

        assert_eq!(valid.raw(), canonical.to_str().unwrap());
        assert_eq!(valid.canonical(), canonical);
    }
}
