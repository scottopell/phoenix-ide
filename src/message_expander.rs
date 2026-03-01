//! Message expansion layer for inline references (REQ-IR-001, REQ-IR-006, REQ-IR-007)
//!
//! Resolves `@path/to/file` tokens in user messages before they reach the LLM,
//! producing a `display_text` (stored in DB, shown in history) and an `llm_text`
//! (delivered to the model with file contents injected).
//!
//! Only `@` file references are handled here. Skill (`/`) and path (`./`)
//! references are tracked in Tasks 570 and 572 respectively.

use std::path::{Path, PathBuf};

/// The result of expanding a user message.
///
/// `display_text` is the original shorthand typed by the user — it is what gets
/// stored in the DB and shown in conversation history.  `llm_text` is the fully
/// resolved form delivered to the model.
#[derive(Debug, Clone)]
pub struct ExpandedMessage {
    /// Original user text — stored and displayed (REQ-IR-006)
    pub display_text: String,
    /// Fully resolved text delivered to the LLM (REQ-IR-001)
    pub llm_text: String,
}

/// Errors produced during expansion (REQ-IR-007)
#[derive(Debug, Clone, PartialEq)]
pub enum ExpansionError {
    /// `@` reference points to a file that does not exist or cannot be read
    FileNotFound { path: String },
    /// `@` reference points to a binary file
    FileNotText { path: String },
}

impl std::fmt::Display for ExpansionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileNotFound { path } => write!(f, "File not found: {path}"),
            Self::FileNotText { path } => {
                write!(f, "File is binary and cannot be included: {path}")
            }
        }
    }
}

impl ExpansionError {
    /// Short machine-readable type string for the frontend
    pub fn error_type(&self) -> &'static str {
        match self {
            Self::FileNotFound { .. } => "file_not_found",
            Self::FileNotText { .. } => "file_not_text",
        }
    }

    /// The `@…` reference token that caused the error
    pub fn reference(&self) -> String {
        match self {
            Self::FileNotFound { path } | Self::FileNotText { path } => format!("@{path}"),
        }
    }
}

/// Scan `text` for `@token` patterns and return each matched token's path
/// (without the leading `@`).
///
/// A token starts after `@` and runs until the next whitespace or end of string.
/// Tokens that contain no path characters (e.g. a bare `@`) are ignored.
fn extract_at_references(text: &str) -> Vec<String> {
    let mut refs = Vec::new();

    for (i, ch) in text.char_indices() {
        if ch != '@' {
            continue;
        }

        // Collect the token after `@`
        let start = i + 1;
        let mut end = start;
        for (j, c) in text[start..].char_indices() {
            if c.is_whitespace() {
                break;
            }
            end = start + j + c.len_utf8();
        }

        let token = &text[start..end];
        if !token.is_empty() {
            refs.push(token.to_string());
        }
    }

    refs
}

/// Determine whether `content` is valid UTF-8 text (no null bytes).
fn is_text_content(content: &[u8]) -> bool {
    !content.contains(&0) && std::str::from_utf8(content).is_ok()
}

/// Expand all `@path` references in `text` relative to `working_dir`.
///
/// Returns `Ok(ExpandedMessage)` when all references resolve successfully.
/// Returns the first `Err(ExpansionError)` encountered when any reference fails.
///
/// If the message contains no `@` references the function short-circuits without
/// reading any files.
pub fn expand(text: &str, working_dir: &Path) -> Result<ExpandedMessage, ExpansionError> {
    let refs = extract_at_references(text);

    if refs.is_empty() {
        return Ok(ExpandedMessage {
            display_text: text.to_string(),
            llm_text: text.to_string(),
        });
    }

    let mut llm_text = text.to_string();

    for ref_path in refs {
        let full_path = resolve_path(&ref_path, working_dir);

        // Validate existence
        if !full_path.exists() {
            return Err(ExpansionError::FileNotFound {
                path: ref_path.clone(),
            });
        }

        // Read contents
        let content = std::fs::read(&full_path).map_err(|_| ExpansionError::FileNotFound {
            path: ref_path.clone(),
        })?;

        // Reject binary files
        if !is_text_content(&content) {
            return Err(ExpansionError::FileNotText {
                path: ref_path.clone(),
            });
        }

        let file_text = String::from_utf8(content).map_err(|_| ExpansionError::FileNotText {
            path: ref_path.clone(),
        })?;

        // Replace `@ref_path` token with structured block
        let token = format!("@{ref_path}");
        let block = format!("<file path=\"{ref_path}\">\n{file_text}\n</file>");
        llm_text = llm_text.replace(&token, &block);
    }

    Ok(ExpandedMessage {
        display_text: text.to_string(),
        llm_text,
    })
}

/// Resolve a reference path to an absolute filesystem path.
///
/// Absolute paths are used as-is; relative paths are joined to `working_dir`.
fn resolve_path(ref_path: &str, working_dir: &Path) -> PathBuf {
    let p = Path::new(ref_path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        working_dir.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_tmp() -> TempDir {
        TempDir::new().unwrap()
    }

    // -------------------------------------------------------------------------
    // extract_at_references
    // -------------------------------------------------------------------------

    #[test]
    fn test_extract_no_refs() {
        assert!(extract_at_references("hello world").is_empty());
    }

    #[test]
    fn test_extract_single_ref() {
        let refs = extract_at_references("look at @src/main.rs please");
        assert_eq!(refs, vec!["src/main.rs"]);
    }

    #[test]
    fn test_extract_multiple_refs() {
        let refs = extract_at_references("@a.rs and @b.rs");
        assert_eq!(refs, vec!["a.rs", "b.rs"]);
    }

    #[test]
    fn test_extract_bare_at_ignored() {
        // `@` with no following token is not a reference
        let refs = extract_at_references("send @ me");
        assert!(refs.is_empty());
    }

    #[test]
    fn test_extract_ref_at_end_of_string() {
        let refs = extract_at_references("see @foo.rs");
        assert_eq!(refs, vec!["foo.rs"]);
    }

    // -------------------------------------------------------------------------
    // expand — success path
    // -------------------------------------------------------------------------

    #[test]
    fn test_expand_no_refs_passthrough() {
        let tmp = make_tmp();
        let result = expand("hello world", tmp.path()).unwrap();
        assert_eq!(result.display_text, "hello world");
        assert_eq!(result.llm_text, "hello world");
    }

    #[test]
    fn test_expand_single_file_ref() {
        let tmp = make_tmp();
        fs::write(tmp.path().join("hello.txt"), "contents here").unwrap();

        let result = expand("check @hello.txt please", tmp.path()).unwrap();
        assert_eq!(result.display_text, "check @hello.txt please");
        assert!(result.llm_text.contains("<file path=\"hello.txt\">"));
        assert!(result.llm_text.contains("contents here"));
        assert!(result.llm_text.contains("</file>"));
    }

    #[test]
    fn test_expand_multiple_refs() {
        let tmp = make_tmp();
        fs::write(tmp.path().join("a.txt"), "aaa").unwrap();
        fs::write(tmp.path().join("b.txt"), "bbb").unwrap();

        let result = expand("@a.txt and @b.txt", tmp.path()).unwrap();
        assert!(result.llm_text.contains("<file path=\"a.txt\">"));
        assert!(result.llm_text.contains("<file path=\"b.txt\">"));
        assert!(result.llm_text.contains("aaa"));
        assert!(result.llm_text.contains("bbb"));
    }

    #[test]
    fn test_expand_display_text_unchanged() {
        let tmp = make_tmp();
        fs::write(tmp.path().join("f.txt"), "x").unwrap();

        let result = expand("see @f.txt", tmp.path()).unwrap();
        // display_text is exactly what the user typed
        assert_eq!(result.display_text, "see @f.txt");
    }

    // -------------------------------------------------------------------------
    // expand — error path (REQ-IR-007)
    // -------------------------------------------------------------------------

    #[test]
    fn test_expand_missing_file_error() {
        let tmp = make_tmp();
        let err = expand("check @missing.rs", tmp.path()).unwrap_err();
        assert_eq!(
            err,
            ExpansionError::FileNotFound {
                path: "missing.rs".to_string()
            }
        );
    }

    #[test]
    fn test_expand_binary_file_error() {
        let tmp = make_tmp();
        // Write a file with a null byte — triggers binary detection
        fs::write(tmp.path().join("bin.dat"), b"hello\x00world").unwrap();

        let err = expand("check @bin.dat", tmp.path()).unwrap_err();
        assert_eq!(
            err,
            ExpansionError::FileNotText {
                path: "bin.dat".to_string()
            }
        );
    }

    #[test]
    fn test_error_type_strings() {
        assert_eq!(
            ExpansionError::FileNotFound {
                path: "x".to_string()
            }
            .error_type(),
            "file_not_found"
        );
        assert_eq!(
            ExpansionError::FileNotText {
                path: "x".to_string()
            }
            .error_type(),
            "file_not_text"
        );
    }

    #[test]
    fn test_error_reference_token() {
        let err = ExpansionError::FileNotFound {
            path: "src/foo.rs".to_string(),
        };
        assert_eq!(err.reference(), "@src/foo.rs");
    }
}
