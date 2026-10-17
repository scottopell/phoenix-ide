//! Standalone immutable `SQLite` fixture admission shared by observation and load measurement.

use std::path::{Path, PathBuf};

type Error = Box<dyn std::error::Error + Send + Sync>;

pub(crate) fn standalone_path(path: &Path) -> Result<PathBuf, Error> {
    if !path.is_absolute() {
        return Err("fixture database path must be absolute".into());
    }
    let canonical = path.canonicalize()?;
    if canonical.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some(".phoenix-ide" | "prod" | "production")
        )
    }) || canonical
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("prod"))
    {
        return Err("fixture admission refuses production database locations".into());
    }
    let metadata = std::fs::metadata(&canonical)?;
    if !metadata.is_file() {
        return Err("fixture database must be a regular file".into());
    }
    if !metadata.permissions().readonly() {
        return Err("fixture database must be chmod read-only".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err("fixture database must not be hard-linked".into());
        }
    }
    for base in [path, canonical.as_path()] {
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = base.as_os_str().to_os_string();
            sidecar.push(suffix);
            match std::fs::symlink_metadata(PathBuf::from(sidecar)) {
                Ok(_) => {
                    return Err(
                        "fixture database has SQLite sidecars; supply a standalone snapshot".into(),
                    )
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(canonical)
}
