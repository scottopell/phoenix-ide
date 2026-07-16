//! Skill discovery, metadata, and invocation (REQ-SK-001 through REQ-SK-005).
//!
//! This crate owns the skill machinery shared across the user `/skill` path and
//! the LLM `skill` tool: walking the working-directory tree for filesystem
//! skills (`.claude/skills/`, `.agents/skills/`), extracting and discovering
//! built-in skills bundled with the binary, and turning a named skill into a
//! ready-to-inject [`SkillInvocation`] (frontmatter stripped, base directory
//! prepended, arguments substituted).
//!
//! Both the user `/skill` path and the LLM `skill` tool call [`invoke_skill`]
//! to produce identical output.

pub mod builtin;

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

// `SkillInvocation` is a domain-vocabulary type embedded in conversation
// events; it lives in phoenix-core. Re-export at the historical path.
pub use phoenix_core::domain::skill_invocation::SkillInvocation;

/// Where a skill came from. Filesystem skills come from user-installed
/// directories (`.claude/skills/`, `.agents/skills/`); built-in skills are
/// bundled with the phoenix binary and extracted to a real directory at
/// startup so they share filesystem semantics (companion files,
/// `Base directory` line, etc.).
#[derive(Debug, Clone)]
pub enum SkillSource {
    Filesystem {
        /// Absolute path to the SKILL.md file
        path: PathBuf,
        /// Discovery directory, e.g. ".claude/skills" or ".agents/skills"
        source_dir: String,
    },
    /// Skill is bundled with the phoenix binary. The path points at the
    /// extracted SKILL.md under `<HOME>/.phoenix-ide/builtin-skills/<name>/`.
    Builtin {
        /// Absolute path to the extracted SKILL.md file
        path: PathBuf,
    },
}

/// Metadata for a skill discovered (filesystem or built-in).
#[derive(Debug, Clone)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    /// Optional argument hint shown in autocomplete (from `argument-hint:` frontmatter field)
    pub argument_hint: Option<String>,
    pub source: SkillSource,
}

impl SkillMetadata {
    /// On-disk directory containing this skill's `SKILL.md`. Both filesystem
    /// and built-in skills have a real path here — built-ins are extracted
    /// at startup so the LLM can read companion files (`references/*.md`,
    /// scripts, etc.) using the same `cat` / `read` workflow as user skills.
    #[must_use]
    pub fn skill_dir(&self) -> String {
        let path = match &self.source {
            SkillSource::Filesystem { path, .. } | SkillSource::Builtin { path } => path,
        };
        path.parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// Catalog display fragment shown after the description in the system
    /// prompt skill catalog. Filesystem entries render as ``(`/abs/path/SKILL.md`)``
    /// (matching the format documented in `specs/skills/skills.allium`); built-ins
    /// render as `(built-in)` so the LLM can distinguish phoenix-bundled skills
    /// from user-installed ones at a glance.
    #[must_use]
    pub fn display_location(&self) -> String {
        match &self.source {
            SkillSource::Filesystem { path, .. } => format!("(`{}`)", path.display()),
            SkillSource::Builtin { .. } => "(built-in)".to_string(),
        }
    }

    /// Path to the SKILL.md file for either source.
    #[must_use]
    pub fn skill_md_path(&self) -> &Path {
        match &self.source {
            SkillSource::Filesystem { path, .. } | SkillSource::Builtin { path } => path,
        }
    }
}

/// Parsed frontmatter fields from a SKILL.md file
struct SkillFrontmatter {
    name: String,
    description: String,
    argument_hint: Option<String>,
}

/// Parse `name`, `description`, and optional `argument-hint` from SKILL.md YAML frontmatter.
///
/// Expects the file to start with `---\n`, followed by `key: value` lines,
/// closed by `\n---\n`. Returns `None` if either required field is missing or the
/// frontmatter is malformed.
fn parse_skill_frontmatter(content: &str) -> Option<SkillFrontmatter> {
    let body = content.strip_prefix("---\n")?;
    let end = body.find("\n---\n").or_else(|| {
        // Handle frontmatter at end of file with no trailing newline after ---
        body.find("\n---").filter(|&i| i + 4 == body.len())
    })?;
    // Safety: `end` is from `find()` on `body`
    #[allow(clippy::string_slice)]
    let frontmatter = &body[..end];

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut argument_hint: Option<String> = None;

    for line in frontmatter.lines() {
        if let Some(val) = line.strip_prefix("name:") {
            name = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("description:") {
            description = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("argument-hint:") {
            let hint = val.trim().to_string();
            if !hint.is_empty() {
                argument_hint = Some(hint);
            }
        }
    }

    Some(SkillFrontmatter {
        name: name?,
        description: description?,
        argument_hint,
    })
}

/// Subdirectories to scan for skill directories at each level of the tree.
const SKILL_DIRS: &[&str] = &[".claude/skills", ".agents/skills"];

/// Collect skills from a single skills directory (e.g., `.claude/skills/`).
///
/// Scans immediate child directories for `SKILL.md` files. For each skill found,
/// also recursively scans a `skills/` subdirectory for namespaced sub-skills
/// (e.g., `allium/skills/distill/SKILL.md` becomes `allium:distill`).
fn collect_skills_from_dir(
    skills_dir: &Path,
    source: &str,
    namespace_prefix: &str,
    skills: &mut Vec<SkillMetadata>,
    seen_names: &mut HashSet<String>,
    seen_paths: &mut HashSet<PathBuf>,
    seen_content: &mut HashSet<u64>,
) {
    let Ok(entries) = std::fs::read_dir(skills_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        // Symlink dedup: canonicalize to detect duplicates
        let canonical = std::fs::canonicalize(&skill_md).unwrap_or_else(|_| skill_md.clone());
        if !seen_paths.insert(canonical) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&skill_md) else {
            continue;
        };
        // Content dedup: hash file content to catch copies
        let content_hash = {
            let mut hasher = std::hash::DefaultHasher::new();
            content.hash(&mut hasher);
            hasher.finish()
        };
        if !seen_content.insert(content_hash) {
            continue;
        }
        if let Some(fm) = parse_skill_frontmatter(&content) {
            // Build the full namespaced name (e.g., "allium:distill")
            let full_name = if namespace_prefix.is_empty() {
                fm.name.clone()
            } else {
                format!("{namespace_prefix}:{}", fm.name)
            };
            if seen_names.insert(full_name.clone()) {
                skills.push(SkillMetadata {
                    name: full_name.clone(),
                    description: fm.description,
                    argument_hint: fm.argument_hint,
                    source: SkillSource::Filesystem {
                        path: skill_md,
                        source_dir: source.to_string(),
                    },
                });
            }
            // Recurse into skills/ subdirectory for namespaced sub-skills
            let sub_skills_dir = entry.path().join("skills");
            if sub_skills_dir.is_dir() {
                collect_skills_from_dir(
                    &sub_skills_dir,
                    source,
                    &full_name,
                    skills,
                    seen_names,
                    seen_paths,
                    seen_content,
                );
            }
        }
    }
}

/// Collect built-in skills from the extract directory (e.g.
/// `<HOME>/.phoenix-ide/builtin-skills/`). Each current embedded skill
/// directory containing a `SKILL.md` becomes a `SkillMetadata` tagged with
/// `SkillSource::Builtin`.
///
/// Reuses the same dedup state as the filesystem walk: an entry is skipped
/// when its canonical path, content hash, or name was already seen by an
/// earlier source. This is what enforces the filesystem-shadows-builtin
/// override rule (REQ-BS-002).
fn collect_builtin_skills_from_dir(
    builtin_dir: &Path,
    skills: &mut Vec<SkillMetadata>,
    seen_names: &mut HashSet<String>,
    seen_paths: &mut HashSet<PathBuf>,
    seen_content: &mut HashSet<u64>,
) {
    let builtin_names: HashSet<String> = builtin::skill_names().into_iter().collect();
    let Ok(entries) = std::fs::read_dir(builtin_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let Some(dir_name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !builtin_names.contains(&dir_name) {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let canonical = std::fs::canonicalize(&skill_md).unwrap_or_else(|_| skill_md.clone());
        if !seen_paths.insert(canonical) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&skill_md) else {
            continue;
        };
        let content_hash = {
            let mut hasher = std::hash::DefaultHasher::new();
            content.hash(&mut hasher);
            hasher.finish()
        };
        if !seen_content.insert(content_hash) {
            continue;
        }
        if let Some(fm) = parse_skill_frontmatter(&content) {
            if fm.name != dir_name {
                continue;
            }
            if seen_names.insert(fm.name.clone()) {
                skills.push(SkillMetadata {
                    name: fm.name,
                    description: fm.description,
                    argument_hint: fm.argument_hint,
                    source: SkillSource::Builtin { path: skill_md },
                });
            }
        }
    }
}

/// Discover skills by walking from `working_dir` up to the filesystem root.
///
/// At each level, scans `SKILL_DIRS` (`.claude/skills/` and `.agents/skills/`)
/// for immediate child directories containing a `SKILL.md` file.
///
/// When the same skill name appears at multiple levels, the one closer to
/// `working_dir` wins (more specific overrides parent). Symlink dedup uses
/// `std::fs::canonicalize` so two paths resolving to the same file are
/// counted once (first discovered wins).
///
/// After the walk-up, explicitly scans `$HOME/.claude/skills/` and
/// `$HOME/.agents/skills/` in case `$HOME` is not an ancestor of `working_dir`.
/// Pass `home_override` to control which directory is treated as `$HOME`
/// (useful for testing without mutating process-global env vars).
///
/// Returns skills sorted by name for deterministic output.
#[must_use]
pub fn discover_skills(working_dir: &Path) -> Vec<SkillMetadata> {
    let builtin_dir = builtin::default_extract_dir();
    discover_skills_with_options(working_dir, None, builtin_dir.as_deref())
}

/// Discovery with explicit overrides for both `$HOME` and the built-in
/// extract directory. Production goes through [`discover_skills`]; tests use
/// this entry point to inject deterministic locations.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn discover_skills_with_options(
    working_dir: &Path,
    home_override: Option<&Path>,
    builtin_dir: Option<&Path>,
) -> Vec<SkillMetadata> {
    let mut skills: Vec<SkillMetadata> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new(); // canonical paths for symlink dedup
    let mut seen_content: HashSet<u64> = HashSet::new(); // content hash for copy dedup
    let mut scanned_dirs: HashSet<PathBuf> = HashSet::new(); // directories already scanned
    let mut current = Some(working_dir.to_path_buf());

    while let Some(dir) = current {
        for skill_subdir in SKILL_DIRS {
            let skills_dir = dir.join(skill_subdir);
            if !skills_dir.is_dir() {
                continue;
            }
            let canonical_dir =
                std::fs::canonicalize(&skills_dir).unwrap_or_else(|_| skills_dir.clone());
            if !scanned_dirs.insert(canonical_dir) {
                continue; // already scanned this directory
            }
            collect_skills_from_dir(
                &skills_dir,
                skill_subdir,
                "",
                &mut skills,
                &mut seen_names,
                &mut seen_paths,
                &mut seen_content,
            );
        }
        current = dir.parent().map(Path::to_path_buf);
    }

    // Scan immediate child directories of working_dir for skills.
    // Handles the "projects directory" case where CWD is a parent containing
    // multiple project subdirs, each with their own .claude/skills/.
    if let Ok(children) = std::fs::read_dir(working_dir) {
        for child in children.flatten() {
            if !child.path().is_dir() {
                continue;
            }
            for skill_subdir in SKILL_DIRS {
                let skills_dir = child.path().join(skill_subdir);
                if !skills_dir.is_dir() {
                    continue;
                }
                let canonical_dir =
                    std::fs::canonicalize(&skills_dir).unwrap_or_else(|_| skills_dir.clone());
                if !scanned_dirs.insert(canonical_dir) {
                    continue;
                }
                collect_skills_from_dir(
                    &skills_dir,
                    skill_subdir,
                    "",
                    &mut skills,
                    &mut seen_names,
                    &mut seen_paths,
                    &mut seen_content,
                );
            }
        }
    }

    // Explicitly check $HOME/.claude/skills/ and $HOME/.agents/skills/
    // in case $HOME is not an ancestor of working_dir (e.g., different mount).
    // Skip if the walk-up already passed through $HOME.
    let resolved_home = match home_override {
        Some(h) => Some(h.to_path_buf()),
        None => Some(
            phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()
                .home()
                .to_path_buf(),
        ),
    };
    if let Some(home) = resolved_home {
        for skill_subdir in SKILL_DIRS {
            let skills_dir = home.join(skill_subdir);
            if !skills_dir.is_dir() {
                continue;
            }
            let canonical_dir =
                std::fs::canonicalize(&skills_dir).unwrap_or_else(|_| skills_dir.clone());
            if !scanned_dirs.insert(canonical_dir) {
                continue; // walk-up already scanned this
            }
            collect_skills_from_dir(
                &skills_dir,
                skill_subdir,
                "",
                &mut skills,
                &mut seen_names,
                &mut seen_paths,
                &mut seen_content,
            );
        }
    }

    // Scan the built-in extract directory last. Existing name dedup means a
    // filesystem skill of the same name has already been collected and the
    // built-in is skipped — this is the documented override rule (REQ-BS-002).
    if let Some(bdir) = builtin_dir {
        if bdir.is_dir() {
            collect_builtin_skills_from_dir(
                bdir,
                &mut skills,
                &mut seen_names,
                &mut seen_paths,
                &mut seen_content,
            );
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Invoke a skill by name: look up in pre-discovered skills, read SKILL.md,
/// strip frontmatter, prepend base directory, substitute arguments.
///
/// This is the live-filesystem entry point used during discovery and by callers
/// that do not yet have a conversation snapshot. Established conversations use
/// [`invoke_captured_skill`] instead.
///
/// # Errors
///
/// Returns `Err` if the skill is not found or cannot be read from disk.
pub fn invoke_skill(
    skill_name: &str,
    arguments: &str,
    skills: &[SkillMetadata],
) -> Result<SkillInvocation, String> {
    let skill = skills
        .iter()
        .find(|s| s.name == skill_name)
        .ok_or_else(|| {
            let available: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
            format!(
                "Skill '{}' not found. Available: {}",
                skill_name,
                if available.is_empty() {
                    "none".to_string()
                } else {
                    available.join(", ")
                }
            )
        })?;

    // Both filesystem and built-in skills are real files on disk — built-ins
    // are extracted at server startup so the SKILL.md and any companion files
    // (`references/*.md`, scripts, etc.) live under
    // `<HOME>/.phoenix-ide/builtin-skills/<name>/`.
    let raw_content = std::fs::read_to_string(skill.skill_md_path())
        .map_err(|e| format!("Failed to read skill '{skill_name}': {e}"))?;

    let body = strip_skill_frontmatter(&raw_content);
    Ok(invoke_captured_skill(
        skill_name,
        arguments,
        &body,
        &skill.skill_dir(),
    ))
}

/// Render an invocation exclusively from captured snapshot values.
///
/// This deliberately performs no filesystem access. Companion files beneath
/// `base_dir` remain live and may be read later by ordinary file tools, while
/// the primary `SKILL.md` instruction body is exact for the active snapshot.
#[must_use]
pub fn invoke_captured_skill(
    skill_name: &str,
    arguments: &str,
    body: &str,
    base_dir: &str,
) -> SkillInvocation {
    let body_with_dir = format!("Base directory for this skill: {base_dir}\n\n{body}");
    SkillInvocation {
        name: skill_name.to_string(),
        body: substitute_arguments(&body_with_dir, arguments),
        skill_dir: base_dir.to_string(),
    }
}

/// Strip YAML frontmatter (--- delimited block at the top of the file).
/// Returns the body content after the closing ---.
#[must_use]
pub fn strip_skill_frontmatter(content: &str) -> String {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content.to_string();
    }
    // Safety: we checked that `trimmed` starts with "---" (3 bytes), so
    // slicing at byte offset 3 is a valid UTF-8 boundary.
    #[allow(clippy::string_slice)]
    let after_open = &trimmed[3..];
    if let Some(end_pos) = after_open.find("\n---") {
        // Skip past the closing "---" and any trailing newline
        // Safety: `end_pos` is from `find()` on `after_open`; adding 4
        // accounts for "\n---" (4 bytes). The result is a valid UTF-8 boundary.
        #[allow(clippy::string_slice)]
        let body_start_str = &after_open[end_pos + 4..];
        body_start_str.trim_start_matches('\n').to_string()
    } else {
        content.to_string()
    }
}

/// Substitute argument placeholders in the skill body.
/// Order: `$ARGUMENTS[N]` and `$N` first (to prevent `$ARGUMENTS` from
/// corrupting them), then `$ARGUMENTS`. If no placeholder exists, append
/// arguments.
fn substitute_arguments(body: &str, arguments: &str) -> String {
    if arguments.is_empty() {
        return body.to_string();
    }

    if body.contains("$ARGUMENTS") {
        let tokens: Vec<&str> = arguments.split_whitespace().collect();
        let mut result = body.to_string();

        // Positional first (prevents $ARGUMENTS from corrupting $ARGUMENTS[N])
        for (i, token) in tokens.iter().enumerate() {
            let n = i + 1;
            result = result
                .replace(&format!("$ARGUMENTS[{n}]"), token)
                .replace(&format!("${n}"), token);
        }

        // Then the full $ARGUMENTS
        result = result.replace("$ARGUMENTS", arguments);

        result
    } else {
        format!("{body}\nARGUMENTS: {arguments}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // -------------------------------------------------------------------------
    // strip_frontmatter
    // -------------------------------------------------------------------------

    #[test]
    fn test_strip_frontmatter_valid() {
        let content = "---\nname: build\ndescription: Build it\n---\n\n# Build\nRun cargo build.";
        let result = strip_skill_frontmatter(content);
        assert_eq!(result, "# Build\nRun cargo build.");
    }

    #[test]
    fn test_strip_frontmatter_no_frontmatter() {
        let content = "# Just markdown\nNo frontmatter here.";
        let result = strip_skill_frontmatter(content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_strip_frontmatter_incomplete() {
        // Opening --- but no closing ---
        let content = "---\nname: build\ndescription: Build it\n\n# Body";
        let result = strip_skill_frontmatter(content);
        // Should return original content since frontmatter is incomplete
        assert_eq!(result, content);
    }

    #[test]
    fn test_strip_frontmatter_empty_body() {
        let content = "---\nname: build\ndescription: Build it\n---\n";
        let result = strip_skill_frontmatter(content);
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_frontmatter_with_leading_whitespace() {
        let content = "  ---\nname: build\ndescription: Build it\n---\n\nBody here.";
        let result = strip_skill_frontmatter(content);
        assert_eq!(result, "Body here.");
    }

    // -------------------------------------------------------------------------
    // substitute_arguments
    // -------------------------------------------------------------------------

    #[test]
    fn test_substitute_arguments_full_replacement() {
        let body = "Review $ARGUMENTS carefully.";
        let result = substitute_arguments(body, "src/main.rs");
        assert_eq!(result, "Review src/main.rs carefully.");
    }

    #[test]
    fn test_substitute_arguments_positional() {
        let body = "Build $ARGUMENTS[1] in $ARGUMENTS[2] mode. Full: $ARGUMENTS";
        let result = substitute_arguments(body, "myapp release");
        assert_eq!(result, "Build myapp in release mode. Full: myapp release");
    }

    #[test]
    fn test_substitute_arguments_dollar_n_shorthand() {
        let body = "First: $1, second: $2";
        // $N shorthand requires $ARGUMENTS to be present somewhere for the
        // substitution branch to trigger. Test the full path:
        let body_with_args = "Full: $ARGUMENTS. First: $1, second: $2";
        let result = substitute_arguments(body_with_args, "foo bar");
        assert_eq!(result, "Full: foo bar. First: foo, second: bar");
        // Without $ARGUMENTS, falls through to append mode
        let result2 = substitute_arguments(body, "foo bar");
        assert_eq!(result2, "First: $1, second: $2\nARGUMENTS: foo bar");
    }

    #[test]
    fn test_substitute_arguments_no_placeholder() {
        let body = "Run the build steps.";
        let result = substitute_arguments(body, "staging");
        assert_eq!(result, "Run the build steps.\nARGUMENTS: staging");
    }

    #[test]
    fn test_substitute_arguments_no_args() {
        let body = "Run $ARGUMENTS if provided.";
        let result = substitute_arguments(body, "");
        assert_eq!(result, body);
    }

    // -------------------------------------------------------------------------
    // frontmatter parsing
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_frontmatter_valid() {
        let content = "---\nname: my-skill\ndescription: Does something useful\n---\n\n# Body\n";
        let result = parse_skill_frontmatter(content).unwrap();
        assert_eq!(result.name, "my-skill");
        assert_eq!(result.description, "Does something useful");
        assert_eq!(result.argument_hint, None);
    }

    #[test]
    fn test_parse_frontmatter_argument_hint() {
        let content =
            "---\nname: my-skill\ndescription: Does something useful\nargument-hint: <file>\n---\n\n# Body\n";
        let result = parse_skill_frontmatter(content).unwrap();
        assert_eq!(result.name, "my-skill");
        assert_eq!(result.argument_hint, Some("<file>".to_string()));
    }

    #[test]
    fn test_parse_frontmatter_missing_name() {
        let content = "---\ndescription: Does something useful\n---\n\n# Body\n";
        assert!(parse_skill_frontmatter(content).is_none());
    }

    #[test]
    fn test_parse_frontmatter_missing_description() {
        let content = "---\nname: my-skill\n---\n\n# Body\n";
        assert!(parse_skill_frontmatter(content).is_none());
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "# Just a markdown file\nNo frontmatter here.\n";
        assert!(parse_skill_frontmatter(content).is_none());
    }

    // -------------------------------------------------------------------------
    // discover_skills
    // -------------------------------------------------------------------------

    /// Write a skill under `{base}/{skills_subdir}/{skill_dir_name}/SKILL.md`.
    /// `skills_subdir` should be one of `SKILL_DIRS` (e.g. ".claude/skills").
    fn write_skill_meta(
        base: &Path,
        skills_subdir: &str,
        skill_dir_name: &str,
        name: &str,
        description: &str,
    ) {
        let skill_dir = base.join(skills_subdir).join(skill_dir_name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
        )
        .unwrap();
    }

    #[test]
    fn test_discover_skills_none() {
        let temp = TempDir::new().unwrap();
        let skills = discover_skills_with_options(temp.path(), Some(temp.path()), None);
        assert!(skills.is_empty());
    }

    #[test]
    fn test_discover_skills_found_claude_dir() {
        let temp = TempDir::new().unwrap();
        write_skill_meta(
            temp.path(),
            ".claude/skills",
            "my-skill",
            "my-skill",
            "Does something useful. Use when you need something.",
        );

        let skills = discover_skills_with_options(temp.path(), Some(temp.path()), None);
        let claude_skills: Vec<&SkillMetadata> =
            skills.iter().filter(|s| s.name == "my-skill").collect();
        assert_eq!(claude_skills.len(), 1);
        assert!(claude_skills[0]
            .description
            .contains("Does something useful"));
        match &claude_skills[0].source {
            SkillSource::Filesystem { path, source_dir } => {
                assert_eq!(
                    path,
                    &temp.path().join(".claude/skills/my-skill").join("SKILL.md")
                );
                assert_eq!(source_dir, ".claude/skills");
            }
            SkillSource::Builtin { .. } => panic!("expected Filesystem source"),
        }
    }

    #[test]
    fn test_discover_skills_found_agents_dir() {
        let temp = TempDir::new().unwrap();
        write_skill_meta(
            temp.path(),
            ".agents/skills",
            "my-skill",
            "my-skill",
            "An agents skill",
        );

        let skills = discover_skills_with_options(temp.path(), Some(temp.path()), None);
        let agent_skills: Vec<&SkillMetadata> =
            skills.iter().filter(|s| s.name == "my-skill").collect();
        assert_eq!(agent_skills.len(), 1);
        match &agent_skills[0].source {
            SkillSource::Filesystem { source_dir, .. } => {
                assert_eq!(source_dir, ".agents/skills");
            }
            SkillSource::Builtin { .. } => panic!("expected Filesystem source"),
        }
    }

    #[test]
    fn test_discover_skills_sorted_by_name() {
        let temp = TempDir::new().unwrap();
        write_skill_meta(
            temp.path(),
            ".claude/skills",
            "zzz-skill",
            "zzz-skill",
            "Last alphabetically",
        );
        write_skill_meta(
            temp.path(),
            ".claude/skills",
            "aaa-skill",
            "aaa-skill",
            "First alphabetically",
        );

        let skills = discover_skills_with_options(temp.path(), Some(temp.path()), None);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "aaa-skill");
        assert_eq!(skills[1].name, "zzz-skill");
    }

    #[test]
    fn test_discover_skills_dedup_cwd_wins() {
        let temp = TempDir::new().unwrap();
        let child = temp.path().join("project");
        fs::create_dir(&child).unwrap();

        // Parent has skill with one description
        write_skill_meta(
            temp.path(),
            ".claude/skills",
            "shared-skill",
            "shared-skill",
            "Parent description",
        );
        // Child has same skill name with different description
        write_skill_meta(
            &child,
            ".claude/skills",
            "shared-skill",
            "shared-skill",
            "Child description",
        );

        // Discover from child -- child should win
        let skills = discover_skills_with_options(&child, Some(temp.path()), None);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "Child description");
    }

    #[test]
    fn test_discover_skills_both_dirs_scanned() {
        let temp = TempDir::new().unwrap();
        write_skill_meta(
            temp.path(),
            ".claude/skills",
            "claude-skill",
            "claude-skill",
            "From .claude/skills",
        );
        write_skill_meta(
            temp.path(),
            ".agents/skills",
            "agents-skill",
            "agents-skill",
            "From .agents/skills",
        );

        let skills = discover_skills_with_options(temp.path(), Some(temp.path()), None);
        let agents = skills.iter().find(|s| s.name == "agents-skill").unwrap();
        let claude = skills.iter().find(|s| s.name == "claude-skill").unwrap();
        match &agents.source {
            SkillSource::Filesystem { source_dir, .. } => {
                assert_eq!(source_dir, ".agents/skills");
            }
            SkillSource::Builtin { .. } => panic!("expected Filesystem source"),
        }
        match &claude.source {
            SkillSource::Filesystem { source_dir, .. } => {
                assert_eq!(source_dir, ".claude/skills");
            }
            SkillSource::Builtin { .. } => panic!("expected Filesystem source"),
        }
    }

    #[test]
    fn test_discover_skills_claude_wins_over_agents_same_name() {
        let temp = TempDir::new().unwrap();
        // .claude/skills is scanned first, so it wins for same name
        write_skill_meta(
            temp.path(),
            ".claude/skills",
            "shared",
            "shared",
            "From claude",
        );
        write_skill_meta(
            temp.path(),
            ".agents/skills",
            "shared",
            "shared",
            "From agents",
        );

        let skills = discover_skills_with_options(temp.path(), Some(temp.path()), None);
        let shared = skills.iter().find(|s| s.name == "shared").unwrap();
        assert_eq!(shared.description, "From claude");
        match &shared.source {
            SkillSource::Filesystem { source_dir, .. } => {
                assert_eq!(source_dir, ".claude/skills");
            }
            SkillSource::Builtin { .. } => panic!("expected Filesystem source"),
        }
    }

    #[test]
    fn test_discover_skills_ignores_arbitrary_subdirs() {
        let temp = TempDir::new().unwrap();
        // A SKILL.md directly in a random subdir should NOT be found
        let random_dir = temp.path().join("random-dir");
        fs::create_dir_all(&random_dir).unwrap();
        fs::write(
            random_dir.join("SKILL.md"),
            "---\nname: stray\ndescription: Should not be found\n---\n",
        )
        .unwrap();

        let skills = discover_skills_with_options(temp.path(), Some(temp.path()), None);
        assert!(skills.is_empty());
    }

    #[test]
    fn test_discover_sub_skills_namespaced() {
        let temp = TempDir::new().unwrap();
        // Parent skill: allium
        write_skill_meta(
            temp.path(),
            ".agents/skills",
            "allium",
            "allium",
            "Allium parent skill",
        );
        // Sub-skills inside allium/skills/
        let sub_dir = temp.path().join(".agents/skills/allium/skills/distill");
        fs::create_dir_all(&sub_dir).unwrap();
        fs::write(
            sub_dir.join("SKILL.md"),
            "---\nname: distill\ndescription: Distill a spec from code\n---\n\n# distill\n",
        )
        .unwrap();

        let sub_dir2 = temp.path().join(".agents/skills/allium/skills/elicit");
        fs::create_dir_all(&sub_dir2).unwrap();
        fs::write(
            sub_dir2.join("SKILL.md"),
            "---\nname: elicit\ndescription: Elicit requirements\n---\n\n# elicit\n",
        )
        .unwrap();

        let skills = discover_skills_with_options(temp.path(), Some(temp.path()), None);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"allium"),
            "parent skill should be discovered"
        );
        assert!(
            names.contains(&"allium:distill"),
            "sub-skill should be namespaced: got {names:?}"
        );
        assert!(
            names.contains(&"allium:elicit"),
            "sub-skill should be namespaced: got {names:?}"
        );
        assert_eq!(skills.len(), 3);
    }

    #[test]
    fn test_discover_sub_skills_recursive_depth() {
        let temp = TempDir::new().unwrap();
        // a -> a/skills/b -> a/skills/b/skills/c
        write_skill_meta(temp.path(), ".claude/skills", "a", "a", "Skill A");

        let b_dir = temp.path().join(".claude/skills/a/skills/b");
        fs::create_dir_all(&b_dir).unwrap();
        fs::write(
            b_dir.join("SKILL.md"),
            "---\nname: b\ndescription: Skill B\n---\n",
        )
        .unwrap();

        let c_dir = temp.path().join(".claude/skills/a/skills/b/skills/c");
        fs::create_dir_all(&c_dir).unwrap();
        fs::write(
            c_dir.join("SKILL.md"),
            "---\nname: c\ndescription: Skill C\n---\n",
        )
        .unwrap();

        let skills = discover_skills_with_options(temp.path(), Some(temp.path()), None);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"a:b"));
        assert!(
            names.contains(&"a:b:c"),
            "deep nesting should work: got {names:?}"
        );
    }

    #[test]
    fn test_sub_skills_without_parent_skill_md_not_discovered() {
        // If a directory has skills/ but no SKILL.md, the sub-skills shouldn't be found
        // because the parent directory isn't recognized as a skill
        let temp = TempDir::new().unwrap();
        let parent_dir = temp.path().join(".claude/skills/notaskill");
        fs::create_dir_all(&parent_dir).unwrap();
        // No SKILL.md in notaskill/

        let sub_dir = parent_dir.join("skills/child");
        fs::create_dir_all(&sub_dir).unwrap();
        fs::write(
            sub_dir.join("SKILL.md"),
            "---\nname: child\ndescription: Orphan child\n---\n",
        )
        .unwrap();

        let skills = discover_skills_with_options(temp.path(), Some(temp.path()), None);
        assert!(
            skills.is_empty(),
            "sub-skills of non-skill dirs should not be found"
        );
    }

    // -------------------------------------------------------------------------
    // Built-in skill discovery (specs/builtin-skills/)
    // -------------------------------------------------------------------------

    /// Create a fake built-in extract directory at `<base>/builtin-skills/<name>/SKILL.md`
    /// with synthesized frontmatter, mirroring what `builtin::extract_to` produces
    /// at runtime.
    fn write_fake_builtin(base: &Path, name: &str, description: &str) -> PathBuf {
        let extract_dir = base.join("builtin-skills");
        let skill_dir = extract_dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\nbody\n"),
        )
        .unwrap();
        extract_dir
    }

    #[test]
    fn test_builtin_appears_when_no_filesystem_skill() {
        let temp = TempDir::new().unwrap();
        let extract_dir = write_fake_builtin(temp.path(), "spears", "Test spears");
        let skills =
            discover_skills_with_options(temp.path(), Some(temp.path()), Some(&extract_dir));
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "spears");
        match &skills[0].source {
            SkillSource::Builtin { path } => {
                assert!(path.starts_with(&extract_dir));
                assert!(path.ends_with("SKILL.md"));
            }
            SkillSource::Filesystem { .. } => panic!("expected Builtin source"),
        }
    }

    #[test]
    fn test_stale_extracted_builtin_is_ignored() {
        let temp = TempDir::new().unwrap();
        let extract_dir = write_fake_builtin(temp.path(), "spears", "Test spears");
        write_fake_builtin(temp.path(), "removed-skill", "Stale extracted skill");

        let skills =
            discover_skills_with_options(temp.path(), Some(temp.path()), Some(&extract_dir));
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["spears"]);
    }

    #[test]
    fn test_filesystem_skill_shadows_builtin_with_same_name() {
        let temp = TempDir::new().unwrap();
        write_skill_meta(
            temp.path(),
            ".claude/skills",
            "spears",
            "spears",
            "User's own spears skill",
        );
        let extract_dir = write_fake_builtin(temp.path(), "spears", "Built-in spears");
        let skills =
            discover_skills_with_options(temp.path(), Some(temp.path()), Some(&extract_dir));
        assert_eq!(skills.len(), 1, "exactly one spears should be visible");
        match &skills[0].source {
            SkillSource::Filesystem { source_dir, .. } => {
                assert_eq!(source_dir, ".claude/skills");
            }
            SkillSource::Builtin { .. } => {
                panic!("filesystem spears should shadow built-in (REQ-BS-002)")
            }
        }
        assert!(skills[0].description.contains("User's own"));
    }

    #[test]
    fn test_builtin_and_filesystem_coexist_when_names_differ() {
        let temp = TempDir::new().unwrap();
        write_skill_meta(
            temp.path(),
            ".claude/skills",
            "build",
            "build",
            "Build the project",
        );
        let extract_dir = write_fake_builtin(temp.path(), "spears", "Built-in spears");
        let skills =
            discover_skills_with_options(temp.path(), Some(temp.path()), Some(&extract_dir));
        assert_eq!(skills.len(), 2);
        // Sorted: build < spears
        assert_eq!(skills[0].name, "build");
        assert!(matches!(skills[0].source, SkillSource::Filesystem { .. }));
        assert_eq!(skills[1].name, "spears");
        assert!(matches!(skills[1].source, SkillSource::Builtin { .. }));
    }

    #[test]
    fn test_skill_dir_for_builtin_is_extracted_parent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("builtin-skills/spears/SKILL.md");
        let bi = SkillMetadata {
            name: "spears".to_string(),
            description: "x".to_string(),
            argument_hint: None,
            source: SkillSource::Builtin { path: path.clone() },
        };
        assert_eq!(
            bi.skill_dir(),
            path.parent().unwrap().to_string_lossy().to_string()
        );
        assert_eq!(bi.display_location(), "(built-in)");
        assert_eq!(bi.skill_md_path(), path.as_path());
    }

    #[test]
    fn test_extracted_builtin_skills_are_discoverable() {
        // End-to-end sanity: extract real built-ins and confirm they show up
        // in discovery via the production-shape entry point.
        let temp = TempDir::new().unwrap();
        let extract_dir = temp.path().join("builtin-skills");
        builtin::extract_to(&extract_dir).unwrap();
        let skills =
            discover_skills_with_options(temp.path(), Some(temp.path()), Some(&extract_dir));
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["allium", "spears"]);
    }

    #[test]
    fn test_builtin_spears_description_is_parseable() {
        let temp = TempDir::new().unwrap();
        let extract_dir = temp.path().join("builtin-skills");
        builtin::extract_to(&extract_dir).unwrap();
        let skills =
            discover_skills_with_options(temp.path(), Some(temp.path()), Some(&extract_dir));
        let spears = skills.iter().find(|s| s.name == "spears").unwrap();
        assert_ne!(spears.description, ">-");
        assert!(spears.description.contains("spEARS"));
    }

    // -------------------------------------------------------------------------
    // invoke_skill (integration with temp dir)
    // -------------------------------------------------------------------------

    fn write_skill(
        dir: &std::path::Path,
        skill_dir: &str,
        name: &str,
        description: &str,
        body: &str,
    ) {
        let skill_path = dir.join(".claude/skills").join(skill_dir);
        fs::create_dir_all(&skill_path).unwrap();
        fs::write(
            skill_path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}"),
        )
        .unwrap();
    }

    #[test]
    fn test_invoke_skill_found() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "build",
            "build",
            "Build the project",
            "Run cargo build.",
        );

        let skills = discover_skills(tmp.path());
        let result = invoke_skill("build", "", &skills).unwrap();
        assert_eq!(result.name, "build");
        assert!(result.body.contains("Base directory for this skill:"));
        assert!(result.body.contains("Run cargo build."));
        assert!(!result.body.contains("---"));
        assert!(!result.skill_dir.is_empty());
    }

    #[test]
    fn test_invoke_skill_with_arguments() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "deploy",
            "deploy",
            "Deploy the app",
            "Deploy to $ARGUMENTS environment.",
        );

        let skills = discover_skills(tmp.path());
        let result = invoke_skill("deploy", "staging", &skills).unwrap();
        assert!(result.body.contains("Deploy to staging environment."));
    }

    #[test]
    fn test_invoke_skill_not_found() {
        let tmp = TempDir::new().unwrap();
        let skills = discover_skills(tmp.path());
        let err = invoke_skill("nonexistent", "", &skills).unwrap_err();
        assert!(err.contains("not found"));
        assert!(err.contains("nonexistent"));
    }

    #[test]
    fn test_invoke_skill_not_found_lists_available() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "build", "build", "Build it", "body");
        write_skill(tmp.path(), "lint", "lint", "Lint it", "body");

        let skills = discover_skills(tmp.path());
        let err = invoke_skill("deploy", "", &skills).unwrap_err();
        assert!(err.contains("deploy"));
        assert!(err.contains("build"));
        assert!(err.contains("lint"));
    }

    #[test]
    fn test_invoke_skill_builtin_reads_from_extracted_dir() {
        // Extract built-ins into an isolated tempdir, point discovery at it,
        // and pin $HOME so a developer's ~/.claude/skills/spears/ can't
        // shadow the built-in.
        let tmp = TempDir::new().unwrap();
        let extract_dir = tmp.path().join("builtin-skills");
        builtin::extract_to(&extract_dir).unwrap();

        let skills = discover_skills_with_options(tmp.path(), Some(tmp.path()), Some(&extract_dir));
        let result = invoke_skill("spears", "", &skills).unwrap();
        assert_eq!(result.name, "spears");
        assert!(
            result.skill_dir.starts_with(extract_dir.to_str().unwrap()),
            "built-in skill_dir should point at the extract directory, got {}",
            result.skill_dir
        );
        // Body should contain content from the vendored markdown
        assert!(result.body.contains("spEARS"));
        // Frontmatter is stripped
        assert!(!result.body.starts_with("---"));
    }

    #[test]
    fn test_invoke_skill_builtin_can_read_companion_files() {
        // Verifies the whole point of the disk-extraction design: a built-in
        // skill's companion files (e.g. allium/references/language-reference.md)
        // are real files the LLM can read.
        let tmp = TempDir::new().unwrap();
        let extract_dir = tmp.path().join("builtin-skills");
        builtin::extract_to(&extract_dir).unwrap();

        let companion = extract_dir
            .join("allium")
            .join("references")
            .join("language-reference.md");
        assert!(
            companion.is_file(),
            "allium reference should be on disk after extraction: {}",
            companion.display()
        );
        let content = std::fs::read_to_string(&companion).unwrap();
        assert!(!content.is_empty());
    }

    #[test]
    fn test_invoke_skill_no_args_appended_when_no_placeholder() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "simple",
            "simple",
            "Simple skill",
            "Do the thing.",
        );

        let skills = discover_skills(tmp.path());
        let result = invoke_skill("simple", "extra args", &skills).unwrap();
        assert!(result.body.contains("ARGUMENTS: extra args"));
    }
}
