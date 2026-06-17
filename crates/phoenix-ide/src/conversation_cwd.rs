use std::path::{Path, PathBuf};

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
) -> Result<ValidConversationCwd, String> {
    let raw = cwd.as_ref();
    let path = Path::new(raw);
    if raw.trim().is_empty() {
        return Err("Working directory cannot be empty".to_string());
    }
    if !path.is_absolute() {
        return Err("Working directory must be an absolute path".to_string());
    }

    let canonical = std::fs::canonicalize(path)
        .map_err(|e| format!("Working directory is not an acceptable directory: {e}"))?;
    if !canonical.is_dir() {
        return Err("Working directory is not an acceptable directory".to_string());
    }
    if canonical.parent().is_none() {
        return Err(
            "Working directory cannot be the filesystem root; choose a project or home subdirectory"
                .to_string(),
        );
    }

    #[cfg(not(test))]
    let _ = canonical;
    #[cfg(test)]
    let valid = ValidConversationCwd {
        raw: raw.to_string(),
        canonical,
    };
    #[cfg(not(test))]
    let valid = ValidConversationCwd {
        raw: raw.to_string(),
    };
    Ok(valid)
}

pub(crate) fn validate_conversation_cwd_for_runtime(
    conv_id: &str,
    cwd: &str,
) -> Result<ValidConversationCwd, String> {
    validate_conversation_cwd(cwd).map_err(|e| {
        tracing::error!(conv_id, cwd, error = %e, "Rejected invalid persisted conversation cwd");
        format!("Conversation '{conv_id}' has an invalid working directory: {e}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_filesystem_root() {
        let err = validate_conversation_cwd("/").expect_err("root rejected");
        assert!(err.contains("filesystem root"), "got: {err}");
    }

    #[test]
    fn rejects_parent_traversal_that_resolves_to_root() {
        let err = validate_conversation_cwd("/..").expect_err("canonical root rejected");
        assert!(err.contains("filesystem root"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_to_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let link = tmp.path().join("root-link");
        std::os::unix::fs::symlink("/", &link).expect("symlink");

        let err =
            validate_conversation_cwd(link.to_str().unwrap()).expect_err("root link rejected");
        assert!(err.contains("filesystem root"), "got: {err}");
    }

    #[test]
    fn accepts_deep_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let deep = tmp.path().join("project/src");
        std::fs::create_dir_all(&deep).expect("deep dir");

        let valid = validate_conversation_cwd(deep.to_str().unwrap()).expect("accepted");
        assert_eq!(valid.raw(), deep.to_str().unwrap());
        assert_eq!(valid.canonical(), deep.canonicalize().unwrap());
    }
}
