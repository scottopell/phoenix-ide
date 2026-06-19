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
