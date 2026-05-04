//! System prompt construction with AGENTS.md discovery and skill catalog injection
//!
//! Discovers and loads guidance files (AGENTS.md, AGENT.md) from the working
//! directory up to the filesystem root, combining them into a system prompt.
//! Also scans for skill directories (any directory containing SKILL.md) and
//! injects a metadata catalog so the agent knows which skills are available.

use std::collections::HashSet;
use std::fmt::Write;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Names of guidance files to look for, in order of preference
const GUIDANCE_FILE_NAMES: &[&str] = &["AGENTS.md", "AGENT.md"];

/// Base system prompt establishing the agent's role
const BASE_PROMPT: &str = r"You are a helpful AI assistant with access to tools for executing code, editing files, and searching codebases. Use tools when appropriate to accomplish tasks.

Be concise in your responses. When using tools, explain what you're doing briefly.";

/// Suffix added for sub-agent conversations
const SUB_AGENT_SUFFIX: &str = r"

You are a sub-agent working on a specific task. When you complete your task, call submit_result with your findings. If you encounter an unrecoverable error, call submit_error. Your conversation will end after calling either tool.";

/// Conversation mode context for system prompt injection.
/// Carries only the stable, display-oriented fields the prompt needs.
#[derive(Debug, Clone)]
pub enum ModeContext {
    /// Read-only git project. Agent can investigate and propose tasks.
    Explore,
    /// Isolated worktree with write access for an approved task.
    Work {
        branch_name: String,
        base_branch: String,
        worktree_path: String,
    },
    /// Direct mode: full tool access, no lifecycle ceremony.
    Direct,
    /// Branch mode: work directly on an existing branch. No task file.
    Branch {
        branch_name: String,
        base_branch: String,
        worktree_path: String,
    },
}

/// A discovered guidance file with its path and content
#[derive(Debug, Clone)]
pub struct GuidanceFile {
    pub path: PathBuf,
    pub content: String,
}

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
    pub fn display_location(&self) -> String {
        match &self.source {
            SkillSource::Filesystem { path, .. } => format!("(`{}`)", path.display()),
            SkillSource::Builtin { .. } => "(built-in)".to_string(),
        }
    }

    /// Path to the SKILL.md file for either source.
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
            use std::hash::{Hash, Hasher};
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
/// `<HOME>/.phoenix-ide/builtin-skills/`). Each immediate child directory
/// containing a `SKILL.md` becomes a `SkillMetadata` tagged with
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
    let Ok(entries) = std::fs::read_dir(builtin_dir) else {
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
pub fn discover_skills(working_dir: &Path) -> Vec<SkillMetadata> {
    let builtin_dir = crate::skills::builtin::default_extract_dir();
    discover_skills_with_options(working_dir, None, builtin_dir.as_deref())
}

/// Discovery with explicit overrides for both `$HOME` and the built-in
/// extract directory. Production goes through [`discover_skills`]; tests use
/// this entry point to inject deterministic locations.
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
        None => std::env::var("HOME").ok().map(PathBuf::from),
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

/// Discover guidance files from the working directory up to the root.
/// Returns files in order from root to cwd (more specific files last).
pub fn discover_guidance_files(working_dir: &Path) -> Vec<GuidanceFile> {
    let mut files = Vec::new();
    let mut current = Some(working_dir.to_path_buf());

    // Walk up the directory tree
    while let Some(dir) = current {
        for name in GUIDANCE_FILE_NAMES {
            let path = dir.join(name);
            if path.is_file() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    files.push(GuidanceFile {
                        path: path.clone(),
                        content,
                    });
                    // Only use one guidance file per directory (first match wins)
                    break;
                }
            }
        }
        current = dir.parent().map(Path::to_path_buf);
    }

    // Reverse so root files come first, cwd files last (more specific override)
    files.reverse();

    // Content-hash dedup: in a worktree, the same tracked AGENTS.md appears at both
    // the worktree path and the project root. Keep the first occurrence (root).
    let mut seen_hashes: HashSet<u64> = HashSet::new();
    files.retain(|f| {
        let mut hasher = std::hash::DefaultHasher::new();
        f.content.hash(&mut hasher);
        seen_hashes.insert(hasher.finish())
    });

    files
}

/// Build the complete system prompt for a conversation.
pub fn build_system_prompt(
    working_dir: &Path,
    is_sub_agent: bool,
    mode: Option<&ModeContext>,
) -> String {
    let builtin_dir = crate::skills::builtin::default_extract_dir();
    build_system_prompt_with_options(
        working_dir,
        is_sub_agent,
        mode,
        None,
        builtin_dir.as_deref(),
    )
}

/// System prompt build with explicit overrides for both `$HOME` and the
/// built-in extract directory. Tests pass `None` for `builtin_dir` to assert
/// filesystem-only behavior; production callers go through
/// [`build_system_prompt`] which uses the live extract location.
#[allow(clippy::too_many_lines)] // One match arm per ModeContext variant; splitting hurts readability
pub fn build_system_prompt_with_options(
    working_dir: &Path,
    is_sub_agent: bool,
    mode: Option<&ModeContext>,
    home_override: Option<&Path>,
    builtin_dir: Option<&Path>,
) -> String {
    let mut prompt = String::from(BASE_PROMPT);

    // Add guidance from discovered files
    let guidance_files = discover_guidance_files(working_dir);
    if !guidance_files.is_empty() {
        prompt.push_str("\n\n<project_guidance>\n");

        for (i, file) in guidance_files.iter().enumerate() {
            if i > 0 {
                prompt.push_str("\n---\n\n");
            }
            // Include the relative path for context
            let display_path = file.path.display();
            let _ = writeln!(prompt, "<!-- From: {display_path} -->");
            prompt.push_str(&file.content);
            if !file.content.ends_with('\n') {
                prompt.push('\n');
            }
        }

        prompt.push_str("</project_guidance>");
    }

    // Inject skill catalog (metadata only — full instructions loaded on demand via bash)
    let skills = discover_skills_with_options(working_dir, home_override, builtin_dir);
    if !skills.is_empty() {
        prompt.push_str("\n\n<available_skills>\n");
        prompt.push_str("The following skills are available. Invoke them with the `skill` tool (e.g. skill(skill_name=\"build\")). Do not cat SKILL.md files directly.\n");
        for skill in &skills {
            let location = skill.display_location();
            let _ = writeln!(
                prompt,
                "\n- **{}** — {} {location}",
                skill.name, skill.description
            );
        }
        prompt.push_str("</available_skills>");
    }

    // Add worktree grounding when working_dir is inside a .phoenix/worktrees/ path
    let wd_str = working_dir.to_string_lossy();
    if let Some(pos) = wd_str.find("/.phoenix/worktrees/") {
        // Safety: `pos` is from `find()` on `wd_str`
        #[allow(clippy::string_slice)]
        let project_root = &wd_str[..pos];
        let _ = write!(
            prompt,
            "\n\nYou are working in a git worktree. Your working directory is the worktree, \
             not the main checkout at {project_root}. Stay grounded here for file operations."
        );
    }

    // Add mode context so the agent understands its capabilities
    if let Some(mode) = mode {
        match mode {
            ModeContext::Explore => {
                prompt.push_str(
                    "\n\nYou are in Explore mode. This conversation is read-only \
                     -- you can read files, search, analyze, and discuss the codebase, \
                     but you cannot modify files.\n\n\
                     When the user wants to make changes, use the propose_task tool to \
                     propose a plan. The user will review the plan and can approve, \
                     request revisions, or reject. On approval, an isolated workspace \
                     is created and you gain write access.\n\n\
                     Do not attempt to use bash for writes or the patch tool -- they \
                     are not available in this mode. If the user asks you to make a \
                     change directly, explain that you need to propose a task first.",
                );
            }
            ModeContext::Work {
                branch_name,
                base_branch,
                worktree_path,
            } => {
                let task_prefix = taskmd_core::ids::prefix_for(
                    &std::path::PathBuf::from(&worktree_path).join("tasks"),
                );
                let _ = write!(
                    prompt,
                    "\n\nYou are in Work mode on branch {branch_name}, targeting \
                     {base_branch}.\n\
                     Your working directory is {worktree_path}. All file edits and \
                     bash commands MUST stay inside this worktree. Do NOT modify \
                     files in the main checkout or repo root.\n\
                     Your task ID prefix is {task_prefix}. Task files in this worktree \
                     use IDs starting with {task_prefix} (e.g., {task_prefix}001).\n\
                     Use bash and the patch tool to make changes.\n\n\
                     When the work is complete, let the user know. They will initiate \
                     the merge to {base_branch} when ready. Task-file status renames \
                     are handled automatically during merge."
                );
            }
            ModeContext::Direct => {
                prompt.push_str(
                    "\n\nYou have full tool access. You are working directly in this directory \
                     with no plan/approve workflow or branch isolation. Changes happen on the \
                     current branch.",
                );
            }
            ModeContext::Branch {
                branch_name,
                base_branch,
                worktree_path,
            } => {
                let _ = write!(
                    prompt,
                    "\n\nYou are in Branch mode on existing branch {branch_name}, \
                     targeting {base_branch}.\n\
                     Your working directory is {worktree_path}. All file edits and \
                     bash commands MUST stay inside this worktree. Do NOT modify \
                     files in the main checkout or repo root.\n\
                     You are working directly on an existing branch -- there is no \
                     task file. Commit your changes directly to {branch_name}.\n\n\
                     When the work is complete, let the user know. They will handle \
                     merging or pushing when ready."
                );
            }
        }
    }

    // Add sub-agent suffix if applicable
    if is_sub_agent {
        prompt.push_str(SUB_AGENT_SUFFIX);
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_discover_no_files() {
        let temp = TempDir::new().unwrap();
        let files = discover_guidance_files(temp.path());
        assert!(files.is_empty());
    }

    #[test]
    fn test_discover_single_file() {
        let temp = TempDir::new().unwrap();
        let agents_path = temp.path().join("AGENTS.md");
        fs::write(&agents_path, "# Test guidance").unwrap();

        let files = discover_guidance_files(temp.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content, "# Test guidance");
    }

    #[test]
    fn test_agents_md_preferred_over_agent_md() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("AGENTS.md"), "agents content").unwrap();
        fs::write(temp.path().join("AGENT.md"), "agent content").unwrap();

        let files = discover_guidance_files(temp.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].content, "agents content");
    }

    #[test]
    fn test_discover_nested_files() {
        let temp = TempDir::new().unwrap();
        let subdir = temp.path().join("project");
        fs::create_dir(&subdir).unwrap();

        fs::write(temp.path().join("AGENTS.md"), "root guidance").unwrap();
        fs::write(subdir.join("AGENTS.md"), "project guidance").unwrap();

        let files = discover_guidance_files(&subdir);
        assert_eq!(files.len(), 2);
        // Root comes first
        assert_eq!(files[0].content, "root guidance");
        // Project-specific comes last (higher precedence)
        assert_eq!(files[1].content, "project guidance");
    }

    #[test]
    fn test_build_system_prompt_no_guidance() {
        let temp = TempDir::new().unwrap();
        // Use temp as home override to avoid $HOME skill contamination
        let prompt =
            build_system_prompt_with_options(temp.path(), false, None, Some(temp.path()), None);

        assert!(prompt.contains("helpful AI assistant"));
        assert!(!prompt.contains("<project_guidance>"));
        assert!(!prompt.contains("sub-agent"));
    }

    #[test]
    fn test_build_system_prompt_with_guidance() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("AGENTS.md"), "# Project Rules\nBe nice.").unwrap();

        let prompt =
            build_system_prompt_with_options(temp.path(), false, None, Some(temp.path()), None);

        assert!(prompt.contains("<project_guidance>"));
        assert!(prompt.contains("# Project Rules"));
        assert!(prompt.contains("Be nice."));
        assert!(prompt.contains("</project_guidance>"));
    }

    #[test]
    fn test_build_system_prompt_sub_agent() {
        let temp = TempDir::new().unwrap();
        let prompt =
            build_system_prompt_with_options(temp.path(), true, None, Some(temp.path()), None);

        assert!(prompt.contains("sub-agent"));
        assert!(prompt.contains("submit_result"));
    }

    // -------------------------------------------------------------------------
    // Skill discovery tests
    // -------------------------------------------------------------------------

    /// Write a skill under `{base}/{skills_subdir}/{skill_dir_name}/SKILL.md`.
    /// `skills_subdir` should be one of `SKILL_DIRS` (e.g. ".claude/skills").
    fn write_skill(
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

    #[test]
    fn test_discover_skills_none() {
        let temp = TempDir::new().unwrap();
        let skills = discover_skills_with_options(temp.path(), Some(temp.path()), None);
        assert!(skills.is_empty());
    }

    #[test]
    fn test_discover_skills_found_claude_dir() {
        let temp = TempDir::new().unwrap();
        write_skill(
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
        write_skill(
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
        write_skill(
            temp.path(),
            ".claude/skills",
            "zzz-skill",
            "zzz-skill",
            "Last alphabetically",
        );
        write_skill(
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
        write_skill(
            temp.path(),
            ".claude/skills",
            "shared-skill",
            "shared-skill",
            "Parent description",
        );
        // Child has same skill name with different description
        write_skill(
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
        write_skill(
            temp.path(),
            ".claude/skills",
            "claude-skill",
            "claude-skill",
            "From .claude/skills",
        );
        write_skill(
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
        write_skill(
            temp.path(),
            ".claude/skills",
            "shared",
            "shared",
            "From claude",
        );
        write_skill(
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
    fn test_build_system_prompt_with_skills() {
        let temp = TempDir::new().unwrap();
        write_skill(
            temp.path(),
            ".claude/skills",
            "deploy-skill",
            "deploy-skill",
            "Deploy the app. Use when deploying.",
        );

        let prompt =
            build_system_prompt_with_options(temp.path(), false, None, Some(temp.path()), None);

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("</available_skills>"));
        assert!(prompt.contains("**deploy-skill**"));
        assert!(prompt.contains("Deploy the app"));
        assert!(prompt.contains("SKILL.md"));
    }

    #[test]
    fn test_build_system_prompt_no_skills() {
        let temp = TempDir::new().unwrap();
        let prompt =
            build_system_prompt_with_options(temp.path(), false, None, Some(temp.path()), None);

        assert!(!prompt.contains("<available_skills>"));
    }

    #[test]
    fn test_discover_sub_skills_namespaced() {
        let temp = TempDir::new().unwrap();
        // Parent skill: allium
        write_skill(
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
        write_skill(temp.path(), ".claude/skills", "a", "a", "Skill A");

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

    #[test]
    fn test_work_mode_prompt_includes_worktree_boundary() {
        let temp = TempDir::new().unwrap();
        let mode = ModeContext::Work {
            branch_name: "task-42-fix-bug".to_string(),
            base_branch: "main".to_string(),
            worktree_path: "/home/user/project/worktrees/abc123".to_string(),
        };
        let prompt = build_system_prompt_with_options(
            temp.path(),
            false,
            Some(&mode),
            Some(temp.path()),
            None,
        );

        assert!(prompt.contains("Work mode"));
        assert!(prompt.contains("task-42-fix-bug"));
        assert!(prompt.contains("/home/user/project/worktrees/abc123"));
        assert!(prompt.contains("MUST stay inside this worktree"));
        assert!(prompt.contains("Task-file status renames"));
        assert!(prompt.contains("Your task ID prefix is"));
    }

    // -------------------------------------------------------------------------
    // Built-in skill discovery (specs/builtin-skills/)
    // -------------------------------------------------------------------------

    /// Create a fake built-in extract directory at `<base>/builtin-skills/<name>/SKILL.md`
    /// with synthesized frontmatter, mirroring what `crate::skills::builtin::extract_to`
    /// produces at runtime.
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
        let extract_dir = write_fake_builtin(temp.path(), "caveman", "Test caveman");
        let skills =
            discover_skills_with_options(temp.path(), Some(temp.path()), Some(&extract_dir));
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "caveman");
        match &skills[0].source {
            SkillSource::Builtin { path } => {
                assert!(path.starts_with(&extract_dir));
                assert!(path.ends_with("SKILL.md"));
            }
            SkillSource::Filesystem { .. } => panic!("expected Builtin source"),
        }
    }

    #[test]
    fn test_filesystem_skill_shadows_builtin_with_same_name() {
        let temp = TempDir::new().unwrap();
        write_skill(
            temp.path(),
            ".claude/skills",
            "caveman",
            "caveman",
            "User's own caveman skill",
        );
        let extract_dir = write_fake_builtin(temp.path(), "caveman", "Built-in caveman");
        let skills =
            discover_skills_with_options(temp.path(), Some(temp.path()), Some(&extract_dir));
        assert_eq!(skills.len(), 1, "exactly one caveman should be visible");
        match &skills[0].source {
            SkillSource::Filesystem { source_dir, .. } => {
                assert_eq!(source_dir, ".claude/skills");
            }
            SkillSource::Builtin { .. } => {
                panic!("filesystem caveman should shadow built-in (REQ-BS-002)")
            }
        }
        assert!(skills[0].description.contains("User's own"));
    }

    #[test]
    fn test_builtin_and_filesystem_coexist_when_names_differ() {
        let temp = TempDir::new().unwrap();
        write_skill(
            temp.path(),
            ".claude/skills",
            "build",
            "build",
            "Build the project",
        );
        let extract_dir = write_fake_builtin(temp.path(), "caveman", "Built-in caveman");
        let skills =
            discover_skills_with_options(temp.path(), Some(temp.path()), Some(&extract_dir));
        assert_eq!(skills.len(), 2);
        // Sorted: build < caveman
        assert_eq!(skills[0].name, "build");
        assert!(matches!(skills[0].source, SkillSource::Filesystem { .. }));
        assert_eq!(skills[1].name, "caveman");
        assert!(matches!(skills[1].source, SkillSource::Builtin { .. }));
    }

    #[test]
    fn test_catalog_renders_builtin_with_marker_not_path() {
        let temp = TempDir::new().unwrap();
        let extract_dir = write_fake_builtin(temp.path(), "caveman", "Built-in caveman");
        let prompt = build_system_prompt_with_options(
            temp.path(),
            false,
            None,
            Some(temp.path()),
            Some(&extract_dir),
        );
        assert!(prompt.contains("**caveman**"));
        // Built-ins use the (built-in) marker rather than exposing the extract path
        // to the LLM in the catalog (catalog stays terse — the path is still
        // resolvable via skill_dir if the skill is invoked).
        assert!(prompt.contains("(built-in)"));
        // The extract path should not leak into the catalog line for the built-in
        assert!(
            !prompt.contains(&format!("(`{}", extract_dir.display())),
            "extract path leaked into catalog: {prompt}"
        );
    }

    #[test]
    fn test_skill_dir_for_builtin_is_extracted_parent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("builtin-skills/caveman/SKILL.md");
        let bi = SkillMetadata {
            name: "caveman".to_string(),
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
    fn test_extracted_caveman_is_discoverable() {
        // End-to-end sanity: extract real built-ins and confirm they show up
        // in discovery via the production-shape entry point.
        let temp = TempDir::new().unwrap();
        let extract_dir = temp.path().join("builtin-skills");
        crate::skills::builtin::extract_to(&extract_dir).unwrap();
        let skills =
            discover_skills_with_options(temp.path(), Some(temp.path()), Some(&extract_dir));
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"caveman"),
            "extracted built-ins should include caveman; got {names:?}"
        );
        assert!(
            names.contains(&"allium"),
            "extracted built-ins should include allium; got {names:?}"
        );
    }
}
