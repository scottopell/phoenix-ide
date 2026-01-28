//! Pure effect interpreter for testing
//!
//! This module provides a pure interpreter that simulates filesystem
//! operations in memory, enabling property-based testing without
//! actual IO.

use super::types::PatchEffect;
use std::collections::HashMap;
use std::path::PathBuf;

/// Virtual filesystem state
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VirtualFs {
    files: HashMap<PathBuf, String>,
}

impl VirtualFs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a virtual filesystem with initial files
    pub fn with_files(files: impl IntoIterator<Item = (PathBuf, String)>) -> Self {
        Self {
            files: files.into_iter().collect(),
        }
    }

    /// Get file content
    pub fn get(&self, path: &PathBuf) -> Option<&String> {
        self.files.get(path)
    }

    /// Set file content
    pub fn set(&mut self, path: PathBuf, content: String) {
        self.files.insert(path, content);
    }

    /// Remove a file
    pub fn remove(&mut self, path: &PathBuf) -> Option<String> {
        self.files.remove(path)
    }

    /// Check if file exists
    pub fn exists(&self, path: &PathBuf) -> bool {
        self.files.contains_key(path)
    }

    /// Get all files
    pub fn files(&self) -> &HashMap<PathBuf, String> {
        &self.files
    }

    /// Interpret effects, returning the resulting filesystem state
    pub fn interpret(&mut self, effects: &[PatchEffect]) {
        for effect in effects {
            match effect {
                PatchEffect::WriteFile { path, content } => {
                    self.files.insert(path.clone(), content.clone());
                }
            }
        }
    }
}

/// Interpret effects starting from an initial state, returning final state
pub fn interpret_effects(
    initial: HashMap<PathBuf, String>,
    effects: &[PatchEffect],
) -> HashMap<PathBuf, String> {
    let mut fs = VirtualFs::with_files(initial);
    fs.interpret(effects);
    fs.files().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn test_write_new_file() {
        let mut fs = VirtualFs::new();
        fs.interpret(&[PatchEffect::WriteFile {
            path: path("test.txt"),
            content: "hello".to_string(),
        }]);

        assert_eq!(fs.get(&path("test.txt")), Some(&"hello".to_string()));
    }

    #[test]
    fn test_overwrite_existing() {
        let mut fs = VirtualFs::with_files([(path("test.txt"), "old".to_string())]);
        
        fs.interpret(&[PatchEffect::WriteFile {
            path: path("test.txt"),
            content: "new".to_string(),
        }]);

        assert_eq!(fs.get(&path("test.txt")), Some(&"new".to_string()));
    }

    #[test]
    fn test_multiple_effects() {
        let initial = HashMap::from([(path("a.txt"), "aaa".to_string())]);
        
        let final_state = interpret_effects(
            initial,
            &[
                PatchEffect::WriteFile {
                    path: path("a.txt"),
                    content: "AAA".to_string(),
                },
                PatchEffect::WriteFile {
                    path: path("b.txt"),
                    content: "BBB".to_string(),
                },
            ],
        );

        assert_eq!(final_state.get(&path("a.txt")), Some(&"AAA".to_string()));
        assert_eq!(final_state.get(&path("b.txt")), Some(&"BBB".to_string()));
    }
}
