//! Effect execution for real filesystem operations

use super::types::PatchEffect;
use std::fs;
use std::io;
use std::path::Path;

/// Execute patch effects against the real filesystem
pub fn execute_effects(effects: &[PatchEffect]) -> Result<(), io::Error> {
    for effect in effects {
        match effect {
            PatchEffect::WriteFile { path, content } => {
                // Create parent directories if needed
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() && !parent.exists() {
                        fs::create_dir_all(parent)?;
                    }
                }
                fs::write(path, content)?;
            }
        }
    }
    Ok(())
}

/// Read file content, returning None if file doesn't exist
pub fn read_file_content(path: &Path) -> Result<Option<String>, io::Error> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_write_creates_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");

        execute_effects(&[PatchEffect::WriteFile {
            path: path.clone(),
            content: "hello world".to_string(),
        }])
        .unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[test]
    fn test_write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a/b/c/test.txt");

        execute_effects(&[PatchEffect::WriteFile {
            path: path.clone(),
            content: "nested".to_string(),
        }])
        .unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "nested");
    }

    #[test]
    fn test_read_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "content").unwrap();

        let content = read_file_content(&path).unwrap();
        assert_eq!(content, Some("content".to_string()));
    }

    #[test]
    fn test_read_nonexistent_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.txt");

        let content = read_file_content(&path).unwrap();
        assert_eq!(content, None);
    }
}
