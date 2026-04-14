//! Message expansion layer for inline references (REQ-IR-001 through REQ-IR-007)
//!
//! Resolves `@path/to/file` and `/skill-name` tokens in user messages before they
//! reach the LLM, producing a `display_text` (stored in DB, shown in history) and
//! an `llm_text` (delivered to the model with file/skill contents injected).
//!
//! Path (`./`) references are not expanded here — they are autocomplete-only (Task 572).

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
    /// If the message triggered a skill invocation, this contains the details.
    /// The chat handler uses this to persist as `MessageContent::Skill` instead
    /// of `MessageContent::User`.
    pub skill_invocation: Option<crate::skills::SkillInvocation>,
}

/// Errors produced during expansion (REQ-IR-007)
#[derive(Debug, Clone, PartialEq)]
pub enum ExpansionError {
    /// `@` reference points to a file that does not exist or cannot be read
    FileNotFound { path: String },
    /// `@` reference points to a binary file
    FileNotText { path: String },
    /// Skill was found but invocation failed (e.g., file read error)
    SkillInvocationFailed { name: String, error: String },
}

impl std::fmt::Display for ExpansionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileNotFound { path } => write!(f, "File not found: {path}"),
            Self::FileNotText { path } => {
                write!(f, "File is binary and cannot be included: {path}")
            }
            Self::SkillInvocationFailed { name, error } => {
                write!(f, "Skill '{name}' failed: {error}")
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
            Self::SkillInvocationFailed { .. } => "skill_invocation_failed",
        }
    }

    /// The reference token that caused the error (`@path` or `/skill-name`)
    pub fn reference(&self) -> String {
        match self {
            Self::FileNotFound { path } | Self::FileNotText { path } => format!("@{path}"),
            Self::SkillInvocationFailed { name, .. } => format!("/{name}"),
        }
    }
}

/// A reference found in user text (e.g., `@src/main.rs` or `/build`).
#[derive(Debug, Clone, PartialEq)]
struct InlineReference {
    /// The sigil character (`'@'`, `'/'`)
    sigil: char,
    /// The token after the sigil (e.g., `"src/main.rs"`, `"build"`)
    token: String,
    /// Byte range in the original text (sigil + token)
    span: std::ops::Range<usize>,
}

/// Scan `text` for inline references. A reference is a sigil character followed by
/// a non-empty token (runs until whitespace or end of string).
///
/// The sigil must be at the start of the text or preceded by whitespace.
/// This prevents matching email addresses (`user@domain`), embedded paths
/// (`foo/bar` when `/` is a sigil), etc.
fn tokenize_references(text: &str, sigils: &[char]) -> Vec<InlineReference> {
    let mut refs = Vec::new();

    for (i, ch) in text.char_indices() {
        if !sigils.contains(&ch) {
            continue;
        }

        // Must be at start of text or preceded by whitespace.
        if i > 0 {
            // Safety: `i` is from `char_indices()` on `text`, so it is a valid
            // char boundary. Slicing `text[..i]` is safe.
            #[allow(clippy::string_slice)]
            let prev_char = text[..i].chars().next_back().unwrap_or(ch);
            if !prev_char.is_whitespace() {
                continue;
            }
        }

        // Collect the token after the sigil.
        let token_start = i + ch.len_utf8();
        let mut token_end = token_start;
        // Safety: `token_start` is `i + ch.len_utf8()` where `i` is from
        // `char_indices()` on `text` and `ch` is the char at that index, so
        // `token_start` is a valid UTF-8 boundary. `token_end` is computed
        // from `char_indices()` on the same `text` slice.
        #[allow(clippy::string_slice)]
        for (j, c) in text[token_start..].char_indices() {
            if c.is_whitespace() {
                break;
            }
            token_end = token_start + j + c.len_utf8();
        }

        // Safety: `token_start` and `token_end` are from `char_indices()` on `text`.
        #[allow(clippy::string_slice)]
        let token = &text[token_start..token_end];
        if !token.is_empty() {
            refs.push(InlineReference {
                sigil: ch,
                token: token.to_string(),
                span: i..token_end,
            });
        }
    }

    refs
}

/// Known file extensions that indicate an @ token is an intentional file reference.
/// A token like @AGENTS.md is a file reference; @username is not.
const PATH_LIKE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "md", "json", "yaml", "yml", "toml", "txt", "css",
    "scss", "html", "htm", "sh", "bash", "sql", "xml", "allium", "cfg", "conf", "env", "lock",
    "mod", "sum", "c", "h", "cpp", "hpp", "java", "kt", "swift", "rb", "php", "ex", "exs", "hs",
    "ml", "zig", "scala", "proto", "graphql", "vue", "svelte", "csv", "log",
];

/// Determine whether a token after @ looks like an intentional file path reference.
/// Returns true if the token contains `/` (path separator) or ends with a known
/// file extension. Returns false for bare words like "username" or "param".
fn looks_like_file_path(token: &str) -> bool {
    // Contains a path separator -- clearly a file path
    if token.contains('/') {
        return true;
    }
    // Check for known file extension
    if let Some(ext) = token.rsplit('.').next() {
        if ext != token && PATH_LIKE_EXTENSIONS.contains(&ext) {
            return true;
        }
    }
    false
}

/// Determine whether `content` is valid UTF-8 text (no null bytes).
fn is_text_content(content: &[u8]) -> bool {
    !content.contains(&0) && std::str::from_utf8(content).is_ok()
}

/// Expand all inline references in `text` relative to `working_dir`.
///
/// Tokenizes the ORIGINAL text once for both `@` and `/` sigils, then:
/// 1. Checks for skill invocations (`/` sigil, validated against discovered skills).
///    Skill expansion replaces the entire message, so it takes priority and file
///    references in the original text are not expanded.
/// 2. If no skill matched, expands `@file` references by inlining file contents.
///
/// Tokenizing the original text (not skill-expanded text) prevents skill output
/// from accidentally introducing `@` tokens that trigger file expansion.
///
/// Returns `Ok(ExpandedMessage)` when all references resolve successfully.
/// Returns the first `Err(ExpansionError)` encountered when any reference fails.
pub fn expand(text: &str, working_dir: &Path) -> Result<ExpandedMessage, ExpansionError> {
    let refs = tokenize_references(text, &['/', '@']);

    // --- Skill expansion (REQ-IR-002, REQ-IR-003) ----------------------------
    // Check for skill invocation first. Skill expansion replaces the entire
    // message, so it takes priority over file references.
    if let Some(skill_ref) = refs.iter().find(|r| r.sigil == '/') {
        let skills = discover_skills(working_dir);
        if skills.iter().any(|s| s.name == skill_ref.token) {
            // Safety: `skill_ref.span.end` is produced by the tokenizer from
            // `char_indices()` on `text`, so it is always a valid UTF-8
            // boundary.
            #[allow(clippy::string_slice)]
            let arguments = text[skill_ref.span.end..].trim_start();
            match crate::skills::invoke_skill(&skill_ref.token, arguments, &skills) {
                Ok(invocation) => {
                    return Ok(ExpandedMessage {
                        display_text: text.to_string(),
                        llm_text: invocation.body.clone(),
                        skill_invocation: Some(invocation),
                    });
                }
                Err(e) => {
                    return Err(ExpansionError::SkillInvocationFailed {
                        name: skill_ref.token.clone(),
                        error: e,
                    });
                }
            }
        }
    }

    // --- File reference expansion (REQ-IR-001, REQ-IR-007) ---------------------
    let mut llm_text = text.to_string();
    let file_refs: Vec<_> = refs.iter().filter(|r| r.sigil == '@').collect();

    for file_ref in file_refs {
        // ClassifyAtReference: only treat path-like tokens as file references.
        // Bare words (@username, @param) pass through as literal text.
        if !looks_like_file_path(&file_ref.token) {
            continue;
        }

        let full_path = resolve_path(&file_ref.token, working_dir);

        // Validate existence
        if !full_path.exists() {
            return Err(ExpansionError::FileNotFound {
                path: file_ref.token.clone(),
            });
        }

        // Read contents
        let content = std::fs::read(&full_path).map_err(|_| ExpansionError::FileNotFound {
            path: file_ref.token.clone(),
        })?;

        // Reject binary files
        if !is_text_content(&content) {
            return Err(ExpansionError::FileNotText {
                path: file_ref.token.clone(),
            });
        }

        let file_text = String::from_utf8(content).map_err(|_| ExpansionError::FileNotText {
            path: file_ref.token.clone(),
        })?;

        // Replace `@ref_path` token with structured block
        let token = format!("@{}", file_ref.token);
        let block = format!("<file path=\"{}\">\n{file_text}\n</file>", file_ref.token);
        llm_text = llm_text.replace(&token, &block);
    }

    Ok(ExpandedMessage {
        display_text: text.to_string(),
        llm_text,
        skill_invocation: None,
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
    // tokenize_references — @ sigil
    // -------------------------------------------------------------------------

    #[test]
    fn test_tokenize_no_refs() {
        assert!(tokenize_references("hello world", &['@']).is_empty());
    }

    #[test]
    fn test_tokenize_single_at_ref() {
        let refs = tokenize_references("look at @src/main.rs please", &['@']);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].sigil, '@');
        assert_eq!(refs[0].token, "src/main.rs");
    }

    #[test]
    fn test_tokenize_multiple_at_refs() {
        let refs = tokenize_references("@a.rs and @b.rs", &['@']);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].token, "a.rs");
        assert_eq!(refs[1].token, "b.rs");
    }

    #[test]
    fn test_tokenize_bare_at_ignored() {
        // `@` with no following token is not a reference
        let refs = tokenize_references("send @ me", &['@']);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_tokenize_at_ref_at_end_of_string() {
        let refs = tokenize_references("see @foo.rs", &['@']);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].token, "foo.rs");
    }

    #[test]
    fn test_tokenize_email_not_treated_as_ref() {
        // @ embedded in an email address should not be treated as a file reference
        let refs = tokenize_references("contact user@example.com for help", &['@']);
        assert!(
            refs.is_empty(),
            "email @ should not be a reference: {refs:?}"
        );
    }

    #[test]
    fn test_tokenize_at_ref_after_newline() {
        // @ at start of a new line (preceded by \n) is a valid reference
        let refs = tokenize_references("check this:\n@src/main.rs", &['@']);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].token, "src/main.rs");
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
        assert!(result.skill_invocation.is_none());
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
        assert!(result.skill_invocation.is_none());
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
        // Write a file with a null byte -- triggers binary detection.
        // Uses .txt extension so ClassifyAtReference treats it as a file reference.
        fs::write(tmp.path().join("bin.txt"), b"hello\x00world").unwrap();

        let err = expand("check @bin.txt", tmp.path()).unwrap_err();
        assert_eq!(
            err,
            ExpansionError::FileNotText {
                path: "bin.txt".to_string()
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
            ExpansionError::SkillInvocationFailed {
                name: "x".to_string(),
                error: "oops".to_string()
            }
            .error_type(),
            "skill_invocation_failed"
        );
    }

    #[test]
    fn test_error_reference_token() {
        let err = ExpansionError::FileNotFound {
            path: "src/foo.rs".to_string(),
        };
        assert_eq!(err.reference(), "@src/foo.rs");

        let err2 = ExpansionError::SkillInvocationFailed {
            name: "my-skill".to_string(),
            error: "read failed".to_string(),
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
    // tokenize_references — / sigil
    // -------------------------------------------------------------------------

    #[test]
    fn test_tokenize_slash_at_start() {
        let refs = tokenize_references("/review src/main.rs", &['/']);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].sigil, '/');
        assert_eq!(refs[0].token, "review");
    }

    #[test]
    fn test_tokenize_slash_mid_message() {
        let refs = tokenize_references("use /build to compile", &['/']);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].sigil, '/');
        assert_eq!(refs[0].token, "build");
    }

    #[test]
    fn test_tokenize_no_slash() {
        let refs = tokenize_references("hello world", &['/']);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_tokenize_bare_slash() {
        // "/" at end-of-string has no token after it
        let refs = tokenize_references("/", &['/']);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_tokenize_slash_not_preceded_by_whitespace() {
        // `/build` embedded in a word (e.g. "foo/build") should not match
        let refs = tokenize_references("foo/build bar", &['/']);
        assert!(refs.is_empty());
    }

    // -------------------------------------------------------------------------
    // tokenize_references — mixed sigils
    // -------------------------------------------------------------------------

    #[test]
    fn test_tokenize_mixed_sigils() {
        let refs = tokenize_references("use /build on @src/main.rs", &['/', '@']);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].sigil, '/');
        assert_eq!(refs[0].token, "build");
        assert_eq!(refs[1].sigil, '@');
        assert_eq!(refs[1].token, "src/main.rs");
    }

    #[test]
    fn test_tokenize_span_correctness() {
        let text = "look at @foo.rs please";
        let refs = tokenize_references(text, &['@']);
        assert_eq!(refs.len(), 1);
        // Safety: span indices come from the tokenizer which operates on `text`.
        #[allow(clippy::string_slice)]
        let spanned = &text[refs[0].span.clone()];
        assert_eq!(spanned, "@foo.rs");
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
        // No text after the token — arguments is empty, so append fallback does not fire
        assert!(!result.llm_text.contains("ARGUMENTS:"));
        // Skill invocation is populated
        let invocation = result.skill_invocation.as_ref().unwrap();
        assert_eq!(invocation.name, "writing-style");
        assert!(invocation.body.contains("Write in a formal tone."));
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
        // $ARGUMENTS is replaced with only the text after the skill token
        assert!(result
            .llm_text
            .contains("Please review src/main.rs carefully."));
        let invocation = result.skill_invocation.as_ref().unwrap();
        assert_eq!(invocation.name, "review");
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
        // Only the text after the skill token is appended as ARGUMENTS
        assert!(result.llm_text.contains("ARGUMENTS: staging"));
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
        assert!(result.llm_text.contains("ARGUMENTS: to compile"));
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
            .contains("Please review to check this PR carefully."));
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
        // (tokenizer finds it but expand validates against known skills)
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
        assert!(result.skill_invocation.is_none());
    }

    // --- ClassifyAtReference / AtTokenPassThrough (REQ-IR-007) ----

    #[test]
    fn test_bare_at_word_passes_through() {
        let tmp = make_tmp();
        let result = expand("hello @username how are you", tmp.path()).unwrap();
        assert_eq!(result.llm_text, "hello @username how are you");
    }

    #[test]
    fn test_at_param_passes_through() {
        let tmp = make_tmp();
        let result = expand("use @param annotation", tmp.path()).unwrap();
        assert_eq!(result.llm_text, "use @param annotation");
    }

    #[test]
    fn test_at_with_extension_treated_as_file_ref() {
        let tmp = make_tmp();
        // This has .md extension -- looks like a file, should try to resolve
        let result = expand("check @MISSING.md please", tmp.path());
        assert!(result.is_err()); // FileNotFound because the file doesn't exist
    }

    #[test]
    fn test_at_with_slash_treated_as_file_ref() {
        let tmp = make_tmp();
        // Contains / -- looks like a path, should try to resolve
        let result = expand("check @src/main.rs please", tmp.path());
        assert!(result.is_err()); // FileNotFound
    }

    #[test]
    fn test_at_existing_file_with_extension_expands() {
        let tmp = make_tmp();
        fs::write(tmp.path().join("test.txt"), "file content").unwrap();
        let result = expand("see @test.txt here", tmp.path()).unwrap();
        assert!(result.llm_text.contains("file content"));
        assert!(result.llm_text.contains("<file"));
    }

    #[test]
    fn test_looks_like_file_path_function() {
        assert!(looks_like_file_path("src/main.rs"));
        assert!(looks_like_file_path("AGENTS.md"));
        assert!(looks_like_file_path("config.toml"));
        assert!(looks_like_file_path("test.txt"));
        assert!(!looks_like_file_path("username"));
        assert!(!looks_like_file_path("param"));
        assert!(!looks_like_file_path("override"));
        assert!(!looks_like_file_path("TODO"));
    }
}
