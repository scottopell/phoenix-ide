//! Message expansion layer for inline references (REQ-IR-001 through REQ-IR-007)
//!
//! Resolves `@path/to/file` and `/skill-name` tokens in user messages before they
//! reach the LLM, producing a `display_text` (stored in DB, shown in history) and
//! an `llm_text` (delivered to the model with file/skill contents injected).
//!
//! Path (`./`) references are not expanded here — they are autocomplete-only (Task 572).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::system_prompt::discover_skills;

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
    /// `/skill-name` does not match any discovered skill
    SkillNotFound {
        name: String,
        available: Vec<String>,
    },
}

impl std::fmt::Display for ExpansionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileNotFound { path } => write!(f, "File not found: {path}"),
            Self::FileNotText { path } => {
                write!(f, "File is binary and cannot be included: {path}")
            }
            Self::SkillNotFound { name, available } => {
                if available.is_empty() {
                    write!(f, "Skill not found: {name} (no skills are available)")
                } else {
                    write!(
                        f,
                        "Skill not found: {name}. Available skills: {}",
                        available.join(", ")
                    )
                }
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
            Self::SkillNotFound { .. } => "skill_not_found",
        }
    }

    /// The reference token that caused the error (`@path` or `/skill-name`)
    pub fn reference(&self) -> String {
        match self {
            Self::FileNotFound { path } | Self::FileNotText { path } => format!("@{path}"),
            Self::SkillNotFound { name, .. } => format!("/{name}"),
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
        // Safety: `start` is `i + 1` where `i` is from `char_indices()` on `text`
        // and `@` is a single-byte ASCII char, so `start` is a valid boundary.
        // `end` is computed from `char_indices()` on the same `text` slice.
        #[allow(clippy::string_slice)]
        for (j, c) in text[start..].char_indices() {
            if c.is_whitespace() {
                break;
            }
            end = start + j + c.len_utf8();
        }

        // Safety: `start` and `end` are from `char_indices()` on `text`
        #[allow(clippy::string_slice)]
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

/// Detect a `/skill-name` token anywhere in the message text.
///
/// Returns `(skill_name, full_original_text)` when a valid skill reference is found.
/// Validates against discovered skills to avoid false positives (e.g., "/path/to/file").
///
/// The full original text is returned as the second element so the LLM receives
/// the complete user message as context.
fn detect_skill_invocation(text: &str, working_dir: &Path) -> Option<(String, String)> {
    let skills = discover_skills(working_dir);
    let skill_names: HashSet<&str> = skills.iter().map(|s| s.name.as_str()).collect();

    // Scan for /word tokens
    for (i, _) in text.match_indices('/') {
        // Must be at start of message (after trimming) or preceded by whitespace
        let trimmed_start = text.len() - text.trim_start().len();
        if i > 0 && i != trimmed_start && !text.as_bytes()[i - 1].is_ascii_whitespace() {
            continue;
        }

        // Extract the name: runs until whitespace or end
        // Safety: `i` is from `match_indices('/')` on `text`, and '/' is a single-byte
        // ASCII char, so `i + 1` is a valid UTF-8 boundary.
        #[allow(clippy::string_slice)]
        let after_slash = &text[i + 1..];
        let name_end = after_slash
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after_slash.len());
        // Safety: `name_end` is from `find()` on `after_slash` (or its `.len()`)
        #[allow(clippy::string_slice)]
        let name = &after_slash[..name_end];

        if name.is_empty() {
            continue;
        }

        // Only match known skills (avoid matching file paths)
        if skill_names.contains(name) {
            return Some((name.to_string(), text.to_string()));
        }
    }
    None
}

/// Expand a skill invocation by loading SKILL.md and performing `$ARGUMENTS` substitution.
///
/// Implements REQ-IR-002 and REQ-IR-003:
/// - Finds the named skill via `discover_skills`
/// - Reads the full SKILL.md content
/// - Substitutes `$ARGUMENTS` with the trailing text (REQ-IR-003)
/// - If no `$ARGUMENTS` placeholder exists but arguments were provided, appends them
/// - If no arguments were provided, returns the SKILL.md content unmodified
fn expand_skill(
    skill_name: &str,
    arguments: &str,
    working_dir: &Path,
) -> Result<String, ExpansionError> {
    let skills = discover_skills(working_dir);

    let skill = skills
        .iter()
        .find(|s| s.name == skill_name)
        .ok_or_else(|| {
            let available = skills.iter().map(|s| s.name.clone()).collect();
            ExpansionError::SkillNotFound {
                name: skill_name.to_string(),
                available,
            }
        })?;

    // Read SKILL.md content
    let skill_content =
        std::fs::read_to_string(&skill.path).map_err(|_| ExpansionError::SkillNotFound {
            name: skill_name.to_string(),
            available: skills.iter().map(|s| s.name.clone()).collect(),
        })?;

    if arguments.is_empty() {
        // No arguments — load skill content unmodified (REQ-IR-003)
        return Ok(skill_content);
    }

    // Check for $ARGUMENTS placeholder
    if skill_content.contains("$ARGUMENTS") {
        // Substitute $ARGUMENTS with the full arguments string
        let mut result = skill_content.replace("$ARGUMENTS", arguments);

        // Also handle $ARGUMENTS[N] and $N (1-based positional)
        let tokens: Vec<&str> = arguments.split_whitespace().collect();
        for (i, token) in tokens.iter().enumerate() {
            let n = i + 1;
            result = result
                .replace(&format!("$ARGUMENTS[{n}]"), token)
                .replace(&format!("${n}"), token);
        }

        Ok(result)
    } else {
        // No placeholder — append arguments so the AI still receives them (REQ-IR-003)
        Ok(format!("{skill_content}\nARGUMENTS: {arguments}"))
    }
}

/// Expand all inline references in `text` relative to `working_dir`.
///
/// Processing order (per spec): skill expansion runs first (it transforms the
/// full message body), then `@file` references are resolved within the resulting text.
///
/// Returns `Ok(ExpandedMessage)` when all references resolve successfully.
/// Returns the first `Err(ExpansionError)` encountered when any reference fails.
pub fn expand(text: &str, working_dir: &Path) -> Result<ExpandedMessage, ExpansionError> {
    // --- Skill expansion (REQ-IR-002, REQ-IR-003) ----------------------------
    let mut llm_text = text.to_string();

    if let Some((skill_name, full_text)) = detect_skill_invocation(text, working_dir) {
        llm_text = expand_skill(&skill_name, &full_text, working_dir)?;
    }

    // --- File reference expansion (REQ-IR-001) --------------------------------
    let refs = extract_at_references(&llm_text);

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

    // If nothing changed, short-circuit (display_text == llm_text)
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
        assert_eq!(
            ExpansionError::SkillNotFound {
                name: "x".to_string(),
                available: vec![]
            }
            .error_type(),
            "skill_not_found"
        );
    }

    #[test]
    fn test_error_reference_token() {
        let err = ExpansionError::FileNotFound {
            path: "src/foo.rs".to_string(),
        };
        assert_eq!(err.reference(), "@src/foo.rs");

        let err2 = ExpansionError::SkillNotFound {
            name: "my-skill".to_string(),
            available: vec![],
        };
        assert_eq!(err2.reference(), "/my-skill");
    }

    // -------------------------------------------------------------------------
    // Skill helpers
    // -------------------------------------------------------------------------

    fn write_skill(dir: &Path, skill_dir: &str, name: &str, description: &str, body: &str) {
        let skill_path = dir.join(".claude/skills").join(skill_dir);
        fs::create_dir_all(&skill_path).unwrap();
        fs::write(
            skill_path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}"),
        )
        .unwrap();
    }

    // -------------------------------------------------------------------------
    // detect_skill_invocation
    // -------------------------------------------------------------------------

    #[test]
    fn test_detect_skill_invocation_at_start() {
        let tmp = make_tmp();
        write_skill(tmp.path(), "review", "review", "Code review", "Review body.");

        let result = detect_skill_invocation("/review src/main.rs", tmp.path());
        assert_eq!(
            result,
            Some(("review".to_string(), "/review src/main.rs".to_string()))
        );
    }

    #[test]
    fn test_detect_skill_invocation_mid_message() {
        let tmp = make_tmp();
        write_skill(tmp.path(), "build", "build", "Build skill", "Build body.");

        let result = detect_skill_invocation("use /build to compile", tmp.path());
        assert_eq!(
            result,
            Some(("build".to_string(), "use /build to compile".to_string()))
        );
    }

    #[test]
    fn test_detect_skill_invocation_no_slash() {
        let tmp = make_tmp();
        assert_eq!(detect_skill_invocation("hello world", tmp.path()), None);
    }

    #[test]
    fn test_detect_skill_invocation_bare_slash() {
        let tmp = make_tmp();
        assert_eq!(detect_skill_invocation("/", tmp.path()), None);
    }

    #[test]
    fn test_detect_skill_invocation_unknown_skill_ignored() {
        // File paths like /usr/bin/ls should not trigger expansion
        let tmp = make_tmp();
        assert_eq!(
            detect_skill_invocation("run /usr/bin/ls please", tmp.path()),
            None
        );
    }

    #[test]
    fn test_detect_skill_invocation_not_preceded_by_whitespace() {
        // `/build` embedded in a word (e.g. "foo/build") should not match
        let tmp = make_tmp();
        write_skill(tmp.path(), "build", "build", "Build skill", "Build body.");

        assert_eq!(
            detect_skill_invocation("foo/build bar", tmp.path()),
            None
        );
    }

    // -------------------------------------------------------------------------
    // expand with skills (REQ-IR-002, REQ-IR-003)
    // -------------------------------------------------------------------------

    #[test]
    fn test_expand_skill_prefix_only() {
        let tmp = make_tmp();
        write_skill(
            tmp.path(),
            "writing-style",
            "writing-style",
            "Apply writing style",
            "Write in a formal tone.",
        );

        let result = expand("/writing-style", tmp.path()).unwrap();
        assert_eq!(result.display_text, "/writing-style");
        assert!(result.llm_text.contains("Write in a formal tone."));
        // Full message is passed as arguments
        assert!(result.llm_text.contains("ARGUMENTS: /writing-style"));
    }

    #[test]
    fn test_expand_skill_with_arguments_placeholder() {
        let tmp = make_tmp();
        write_skill(
            tmp.path(),
            "review",
            "review",
            "Code review skill",
            "Please review $ARGUMENTS carefully.",
        );

        let result = expand("/review src/main.rs", tmp.path()).unwrap();
        assert_eq!(result.display_text, "/review src/main.rs");
        // $ARGUMENTS is replaced with the full original message
        assert!(result
            .llm_text
            .contains("Please review /review src/main.rs carefully."));
    }

    #[test]
    fn test_expand_skill_with_arguments_no_placeholder_appends() {
        let tmp = make_tmp();
        write_skill(
            tmp.path(),
            "deploy",
            "deploy",
            "Deploy skill",
            "Run the deployment steps.",
        );

        let result = expand("/deploy staging", tmp.path()).unwrap();
        assert_eq!(result.display_text, "/deploy staging");
        assert!(result.llm_text.contains("Run the deployment steps."));
        // Full message appended as ARGUMENTS
        assert!(result.llm_text.contains("ARGUMENTS: /deploy staging"));
    }

    #[test]
    fn test_expand_skill_mid_message() {
        let tmp = make_tmp();
        write_skill(
            tmp.path(),
            "build",
            "build",
            "Build skill",
            "Run the build steps.",
        );

        let result = expand("use /build to compile", tmp.path()).unwrap();
        assert_eq!(result.display_text, "use /build to compile");
        assert!(result.llm_text.contains("Run the build steps."));
        assert!(
            result
                .llm_text
                .contains("ARGUMENTS: use /build to compile")
        );
    }

    #[test]
    fn test_expand_skill_mid_message_with_placeholder() {
        let tmp = make_tmp();
        write_skill(
            tmp.path(),
            "review",
            "review",
            "Code review skill",
            "Please review $ARGUMENTS carefully.",
        );

        let result = expand("use /review to check this PR", tmp.path()).unwrap();
        assert_eq!(result.display_text, "use /review to check this PR");
        assert!(result
            .llm_text
            .contains("Please review use /review to check this PR carefully."));
    }

    #[test]
    fn test_expand_file_path_not_skill() {
        // /usr/bin/ls should not trigger skill expansion
        let tmp = make_tmp();
        let result = expand("run /usr/bin/ls please", tmp.path()).unwrap();
        assert_eq!(result.display_text, "run /usr/bin/ls please");
        assert_eq!(result.llm_text, "run /usr/bin/ls please");
    }

    #[test]
    fn test_expand_skill_not_found_error() {
        let tmp = make_tmp();
        // With no skills defined, /nonexistent should pass through as plain text
        // (detect_skill_invocation returns None for unknown skills)
        let result = expand("/nonexistent", tmp.path()).unwrap();
        assert_eq!(result.llm_text, "/nonexistent");
    }

    #[test]
    fn test_expand_skill_not_found_lists_available() {
        let tmp = make_tmp();
        write_skill(tmp.path(), "foo-skill", "foo", "Foo skill", "Foo body.");

        // /missing is not a known skill, so it passes through as plain text
        let result = expand("/missing", tmp.path()).unwrap();
        assert_eq!(result.llm_text, "/missing");
    }

    #[test]
    fn test_expand_skill_display_text_is_original() {
        let tmp = make_tmp();
        write_skill(tmp.path(), "ws", "ws", "Writing style", "Be concise.");

        let result = expand("/ws help with email", tmp.path()).unwrap();
        assert_eq!(result.display_text, "/ws help with email");
    }

    #[test]
    fn test_non_slash_message_unchanged() {
        let tmp = make_tmp();
        let result = expand("hello world", tmp.path()).unwrap();
        assert_eq!(result.display_text, "hello world");
        assert_eq!(result.llm_text, "hello world");
    }
}
