//! System prompt construction with AGENTS.md discovery and skill catalog injection
//!
//! Discovers and loads guidance files (AGENTS.md, AGENT.md) from the working
//! directory up to the filesystem root, combining them into a system prompt.
//! Also scans for skill directories (any directory containing SKILL.md) and
//! injects a metadata catalog so the agent knows which skills are available.

use std::collections::HashSet;
use std::fmt::Write;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};

use phoenix_core::domain::project_instruction_bundle::{
    NewProjectInstructionBundle, ProjectGuidanceSnapshot, ProjectInstructionBundle,
    ProjectSkillSnapshot,
};
use sha2::{Digest, Sha256};

use crate::llm_language::{self, LlmLanguage};

/// Names of guidance files to look for, in order of preference
const GUIDANCE_FILE_NAMES: &[&str] = &["AGENTS.md", "AGENT.md"];

// `ModeContext` is a domain-vocabulary type embedded in `ConvState`; it now
// lives in phoenix-core. Re-export at the historical path.
pub use phoenix_core::domain::mode_context::ModeContext;
use phoenix_core::domain::sm_state::ExploreBashCapability;

// Skill discovery + metadata now live in the `phoenix-skills` crate. Re-export
// them at their historical `crate::system_prompt::…` paths so existing call
// sites (`api::handlers`, `tools::skill`, `message_expander`) resolve unchanged
// (move-down, re-export-up).
pub use crate::skills::{discover_skills, discover_skills_with_options, SkillSource};

/// A discovered guidance file with its path and content
#[derive(Debug, Clone)]
pub struct GuidanceFile {
    pub path: PathBuf,
    pub content: String,
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

/// Borrowed project-instruction data accepted by prompt construction.
///
/// Both newly discovered bundles and persisted bundles implement this view, so
/// callers can render an immutable snapshot without re-reading its source files.
pub trait ProjectInstructionView {
    fn guidance(&self) -> &[ProjectGuidanceSnapshot];
    fn skills(&self) -> &[ProjectSkillSnapshot];
}

impl ProjectInstructionView for NewProjectInstructionBundle {
    fn guidance(&self) -> &[ProjectGuidanceSnapshot] {
        &self.guidance
    }

    fn skills(&self) -> &[ProjectSkillSnapshot] {
        &self.skills
    }
}

impl ProjectInstructionView for ProjectInstructionBundle {
    fn guidance(&self) -> &[ProjectGuidanceSnapshot] {
        &self.guidance
    }

    fn skills(&self) -> &[ProjectSkillSnapshot] {
        &self.skills
    }
}

fn sha256_hex(content: &str) -> String {
    Sha256::digest(content.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn relative_display_path(base: &Path, target: &Path) -> String {
    let base_components: Vec<_> = base.components().collect();
    let target_components: Vec<_> = target.components().collect();
    let common = base_components
        .iter()
        .zip(&target_components)
        .take_while(|(left, right)| left == right)
        .count();

    let mut relative = PathBuf::new();
    for component in &base_components[common..] {
        if matches!(component, Component::Normal(_)) {
            relative.push("..");
        }
    }
    for component in &target_components[common..] {
        relative.push(component.as_os_str());
    }

    if relative.as_os_str().is_empty() {
        ".".to_string()
    } else {
        relative.to_string_lossy().into_owned()
    }
}

fn skill_source_label(working_dir: &Path, skill: &crate::skills::SkillMetadata) -> String {
    match &skill.source {
        SkillSource::Filesystem { path, .. } => {
            format!("(`{}`)", relative_display_path(working_dir, path))
        }
        SkillSource::Builtin { .. } => "(built-in)".to_string(),
    }
}

fn render_project_instructions(project: &impl ProjectInstructionView) -> String {
    let mut rendered = String::new();
    if !project.guidance().is_empty() {
        rendered.push_str("\n\n<project_guidance>\n");
        for (index, guidance) in project.guidance().iter().enumerate() {
            if index > 0 {
                rendered.push_str("\n---\n\n");
            }
            let _ = writeln!(rendered, "<!-- From: {} -->", guidance.relative_path);
            rendered.push_str(&guidance.content);
            if !guidance.content.ends_with('\n') {
                rendered.push('\n');
            }
        }
        rendered.push_str("</project_guidance>");
    }

    if !project.skills().is_empty() {
        rendered.push_str("\n\n<available_skills>\n");
        rendered.push_str("The following skills are available. Invoke them with the `skill` tool (e.g. skill(skill_name=\"build\")). Do not cat SKILL.md files directly.\n");
        for skill in project.skills() {
            let _ = writeln!(
                rendered,
                "\n- **{}** — {} {}",
                skill.name, skill.description, skill.source_label
            );
        }
        rendered.push_str("</available_skills>");
    }
    rendered
}

/// Discover and normalize the project-owned portion of the system prompt.
#[must_use]
#[allow(dead_code)] // Public extraction seam for project-bundle persistence integration.
pub fn discover_project_instruction_bundle(working_dir: &Path) -> NewProjectInstructionBundle {
    let builtin_dir = crate::skills::builtin::default_extract_dir();
    discover_project_instruction_bundle_with_options(working_dir, None, builtin_dir.as_deref())
}

/// Discovery variant with explicit skill roots for deterministic tests.
#[must_use]
pub fn discover_project_instruction_bundle_with_options(
    working_dir: &Path,
    home_override: Option<&Path>,
    builtin_dir: Option<&Path>,
) -> NewProjectInstructionBundle {
    let guidance = discover_guidance_files(working_dir)
        .into_iter()
        .map(|file| ProjectGuidanceSnapshot {
            relative_path: relative_display_path(working_dir, &file.path),
            content_hash: sha256_hex(&file.content),
            content: file.content,
        })
        .collect();

    let skills = discover_skills_with_options(working_dir, home_override, builtin_dir)
        .into_iter()
        .filter_map(|skill| {
            let source_label = skill_source_label(working_dir, &skill);
            let source_path = skill.skill_md_path().to_string_lossy().into_owned();
            let base_dir = skill.skill_dir();
            let raw = std::fs::read_to_string(skill.skill_md_path()).ok()?;
            let body = crate::skills::strip_skill_frontmatter(&raw);
            let content_hash = sha256_hex(&format!(
                "{}\0{}\0{:?}\0{source_label}\0{source_path}\0{base_dir}\0{body}",
                skill.name, skill.description, skill.argument_hint
            ));
            Some(ProjectSkillSnapshot {
                name: skill.name,
                description: skill.description,
                argument_hint: skill.argument_hint,
                source_label,
                body,
                base_dir,
                source_path,
                content_hash,
            })
        })
        .collect();

    let mut bundle = NewProjectInstructionBundle {
        estimated_tokens: 0,
        guidance,
        skills,
    };
    bundle.estimated_tokens = render_project_instructions(&bundle)
        .len()
        .div_ceil(4)
        .try_into()
        .unwrap_or(u64::MAX);
    bundle
}

/// Compute the next taskmd ID for this worktree, but only if the project
/// actually uses taskmd (signalled by a `_TEMPLATE.md` marker inside the
/// tasks directory). Returns `None` for plain-markdown task workflows so the
/// Explore prompt doesn't promise an ID convention the project isn't using.
///
/// Explore mode disables bash, so the agent can't run `taskmd next` itself —
/// we precompute the ID server-side and inject it into the prompt.
fn next_taskmd_id(working_dir: &Path, tasks_dir_name: &str) -> Option<String> {
    let tasks_dir = working_dir.join(tasks_dir_name);
    if !tasks_dir
        .join(taskmd_core::constants::TEMPLATE_FILENAME)
        .is_file()
    {
        return None;
    }
    Some(taskmd_core::ids::next_id(&tasks_dir))
}

pub fn snapshot_next_taskmd_id_hint(
    working_dir: &Path,
    tasks_dir_name: &str,
) -> Option<phoenix_core::domain::db_schema::NonEmptyString> {
    next_taskmd_id(working_dir, tasks_dir_name).and_then(|id| {
        phoenix_core::domain::db_schema::NonEmptyString::new(id)
            .map_err(|e| {
                tracing::warn!(error = %e, "computed empty taskmd ID hint; omitting Explore hint");
            })
            .ok()
    })
}

pub fn build_coordinator_system_prompt(language: LlmLanguage) -> String {
    let mut prompt = llm_language::coordinator_prompt(language).to_string();
    prompt.push_str("\n\n");
    prompt.push_str(llm_language::mermaid_rendering_hint(language));
    prompt
}

/// Build the complete system prompt for a conversation.
#[allow(dead_code)]
pub fn build_system_prompt(
    working_dir: &Path,
    tasks_dir_name: &str,
    is_sub_agent: bool,
    mode: Option<&ModeContext>,
    language: LlmLanguage,
    persona: Option<&str>,
    explore_bash: ExploreBashCapability,
) -> String {
    let builtin_dir = crate::skills::builtin::default_extract_dir();
    build_system_prompt_with_options(
        working_dir,
        tasks_dir_name,
        is_sub_agent,
        mode,
        None,
        builtin_dir.as_deref(),
        language,
        persona,
        explore_bash,
    )
}

/// System prompt build with explicit overrides for both `$HOME` and the
/// built-in extract directory. Tests pass `None` for `builtin_dir` to assert
/// filesystem-only behavior; production callers go through
/// [`build_system_prompt`] which uses the live extract location.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One match arm per ModeContext variant; splitting hurts readability
#[allow(dead_code)]
pub fn build_system_prompt_with_options(
    working_dir: &Path,
    tasks_dir_name: &str,
    is_sub_agent: bool,
    mode: Option<&ModeContext>,
    home_override: Option<&Path>,
    builtin_dir: Option<&Path>,
    language: LlmLanguage,
    persona: Option<&str>,
    explore_bash: ExploreBashCapability,
) -> String {
    let project =
        discover_project_instruction_bundle_with_options(working_dir, home_override, builtin_dir);
    build_system_prompt_with_project_instructions(
        working_dir,
        tasks_dir_name,
        is_sub_agent,
        mode,
        language,
        persona,
        explore_bash,
        &project,
    )
}

/// Build a prompt from immutable project instructions and live conversation state.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One match arm per ModeContext variant; splitting hurts readability
pub fn build_system_prompt_with_project_instructions(
    working_dir: &Path,
    tasks_dir_name: &str,
    is_sub_agent: bool,
    mode: Option<&ModeContext>,
    language: LlmLanguage,
    persona: Option<&str>,
    explore_bash: ExploreBashCapability,
    project: &impl ProjectInstructionView,
) -> String {
    // REQ-AG-006: a named agent's persona replaces the generic assistant
    // preamble at the head of the prompt. Everything below (guidance, skills,
    // mode context, sub-agent suffix) is appended regardless of persona.
    let mut prompt = match persona {
        Some(p) => String::from(p),
        None => String::from(llm_language::base_prompt(language)),
    };
    prompt.push_str("\n\n");
    prompt.push_str(llm_language::mermaid_rendering_hint(language));

    prompt.push_str(&render_project_instructions(project));

    // Worktree grounding when cwd is inside .phoenix/worktrees/. The Work and
    // Branch mode blocks below already state the worktree boundary (with the
    // concrete path), so only emit the generic note when no such block will —
    // i.e. for Explore (REQ-PROJ-028 gives Explore conversations a worktree
    // too) and the no-mode case.
    let mode_states_worktree_boundary = matches!(
        mode,
        Some(ModeContext::Work { .. } | ModeContext::Branch { .. })
    );
    if !mode_states_worktree_boundary {
        if let Some(repo_root) = crate::git_ops::repo_root_from_phoenix_worktree(working_dir) {
            let _ = write!(
                prompt,
                "\n\nYou are working in a git worktree. Your working directory is the worktree, \
                 not the main checkout at {}. Stay grounded here for file operations.",
                repo_root.display()
            );
        }
    }

    // Add mode context so the agent understands its capabilities
    if let Some(mode) = mode {
        match mode {
            ModeContext::Explore {
                next_taskmd_id_hint,
            } => {
                prompt.push_str(&llm_language::mode_explore(
                    language,
                    tasks_dir_name,
                    explore_bash,
                ));
                if let Some(next_id) = next_taskmd_id_hint {
                    prompt.push_str(&llm_language::next_taskmd_id_hint(
                        language,
                        tasks_dir_name,
                        next_id,
                    ));
                }
            }
            ModeContext::Work {
                branch_name,
                base_branch,
                worktree_path,
            } => {
                prompt.push_str(&llm_language::mode_work(
                    language,
                    branch_name,
                    base_branch,
                    worktree_path,
                ));
            }
            ModeContext::Direct => {
                prompt.push_str(llm_language::mode_direct(language));
            }
            ModeContext::Branch {
                branch_name,
                base_branch,
                worktree_path,
            } => {
                prompt.push_str(&llm_language::mode_branch(
                    language,
                    branch_name,
                    base_branch,
                    worktree_path,
                ));
            }
        }
    }

    // Add sub-agent suffix if applicable
    if is_sub_agent {
        prompt.push_str(llm_language::sub_agent_suffix(language));
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn coordinator_prompt_excludes_project_and_explore_guidance() {
        let prompt = build_coordinator_system_prompt(LlmLanguage::default());
        assert!(prompt.contains("You are Phoenix Coordinator"));
        assert!(!prompt.contains("taskmd"));
        assert!(!prompt.contains("available_skills"));
        assert!(!prompt.contains("propose_task"));
    }

    #[test]
    fn coordinator_prompt_uses_conversation_llm_language() {
        let prompt = build_coordinator_system_prompt(LlmLanguage::Caveman);
        assert!(prompt.contains("You Phoenix Coordinator"));
        assert!(!prompt.contains("You are Phoenix Coordinator"));
    }

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
        let prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            None,
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );

        assert!(prompt.contains("helpful AI assistant"));
        assert!(prompt.contains(llm_language::mermaid_rendering_hint(
            crate::llm_language::LlmLanguage::default()
        )));
        assert!(!prompt.contains("<project_guidance>"));
        assert!(!prompt.contains("sub-agent"));
    }

    #[test]
    fn caveman_language_swaps_the_base_prompt() {
        let temp = TempDir::new().unwrap();
        let native = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            None,
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::PhoenixNative,
            None,
            ExploreBashCapability::Unavailable,
        );
        let caveman = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            None,
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::Caveman,
            None,
            ExploreBashCapability::Unavailable,
        );
        assert!(native.contains("helpful AI assistant"));
        assert!(caveman.contains("smart caveman"));
        assert!(
            !caveman.contains("helpful AI assistant"),
            "caveman should not retain the phoenix-native opener"
        );
    }

    #[test]
    fn caveman_language_swaps_mode_blocks() {
        let temp = TempDir::new().unwrap();
        let mode = ModeContext::Work {
            branch_name: "task-1".into(),
            base_branch: "main".into(),
            worktree_path: "/wt".into(),
        };
        let caveman = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            Some(&mode),
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::Caveman,
            None,
            ExploreBashCapability::Unavailable,
        );
        assert!(caveman.contains("work cave"));
        // Phoenix-native phrasing must not bleed through.
        assert!(!caveman.contains("You are in Work mode"));
    }

    #[test]
    fn test_build_system_prompt_with_guidance() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("AGENTS.md"), "# Project Rules\nBe nice.").unwrap();

        let prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            None,
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );

        assert!(prompt.contains("<project_guidance>"));
        assert!(prompt.contains("# Project Rules"));
        assert!(prompt.contains("Be nice."));
        assert!(prompt.contains("</project_guidance>"));
    }

    #[test]
    fn discovered_bundle_is_normalized_and_deterministic() {
        let temp = TempDir::new().unwrap();
        let child = temp.path().join("project");
        fs::create_dir(&child).unwrap();
        fs::write(temp.path().join("AGENTS.md"), "parent guidance").unwrap();
        fs::write(child.join("AGENTS.md"), "project guidance").unwrap();
        write_skill(&child, ".claude/skills", "zeta", "zeta", "Zeta skill.");
        write_skill(&child, ".claude/skills", "alpha", "alpha", "Alpha skill.");

        let first = discover_project_instruction_bundle_with_options(&child, Some(&child), None);
        let second = discover_project_instruction_bundle_with_options(&child, Some(&child), None);

        assert_eq!(first, second);
        assert_eq!(
            first
                .guidance
                .iter()
                .map(|source| source.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["../AGENTS.md", "AGENTS.md"]
        );
        assert_eq!(
            first
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        assert_eq!(
            first.skills[0].source_label,
            "(`.claude/skills/alpha/SKILL.md`)"
        );
        assert!(first
            .guidance
            .iter()
            .all(|source| source.content_hash.len() == 64));
        assert!(first
            .skills
            .iter()
            .all(|skill| skill.content_hash.len() == 64));
        assert!(first.estimated_tokens > 0);
    }

    #[test]
    fn discovered_skill_body_and_hash_change_together() {
        let temp = TempDir::new().unwrap();
        let skill_dir = temp.path().join(".claude/skills/build");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: build\ndescription: Build\n---\n\ncaptured body",
        )
        .unwrap();
        let first =
            discover_project_instruction_bundle_with_options(temp.path(), Some(temp.path()), None);

        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: build\ndescription: Build\n---\n\nchanged body",
        )
        .unwrap();
        let second =
            discover_project_instruction_bundle_with_options(temp.path(), Some(temp.path()), None);

        assert_eq!(first.skills[0].body, "captured body");
        assert_eq!(second.skills[0].body, "changed body");
        assert_ne!(first.skills[0].content_hash, second.skills[0].content_hash);
        assert!(first.skills[0].source_path.ends_with("/build/SKILL.md"));
        assert!(first.skills[0].base_dir.ends_with("/build"));
    }

    #[test]
    fn provided_bundle_is_stable_while_live_mode_changes_render() {
        let temp = TempDir::new().unwrap();
        let guidance_path = temp.path().join("AGENTS.md");
        fs::write(&guidance_path, "original guidance").unwrap();
        write_skill(
            temp.path(),
            ".claude/skills",
            "build",
            "build",
            "Original skill.",
        );
        let bundle =
            discover_project_instruction_bundle_with_options(temp.path(), Some(temp.path()), None);
        let direct = build_system_prompt_with_project_instructions(
            temp.path(),
            "tasks",
            false,
            Some(&ModeContext::Direct),
            LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
            &bundle,
        );

        fs::write(&guidance_path, "mutated guidance").unwrap();
        write_skill(
            temp.path(),
            ".claude/skills",
            "build",
            "build",
            "Mutated skill.",
        );
        let direct_after_mutation = build_system_prompt_with_project_instructions(
            temp.path(),
            "tasks",
            false,
            Some(&ModeContext::Direct),
            LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
            &bundle,
        );
        assert_eq!(direct, direct_after_mutation);
        assert!(direct.contains("original guidance"));
        assert!(direct.contains("Original skill."));
        assert!(!direct.contains("mutated guidance"));

        let work_mode = ModeContext::Work {
            branch_name: "task-live-mode".into(),
            base_branch: "main".into(),
            worktree_path: temp.path().to_string_lossy().into_owned(),
        };
        let work = build_system_prompt_with_project_instructions(
            temp.path(),
            "tasks",
            false,
            Some(&work_mode),
            LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
            &bundle,
        );
        assert!(work.contains("task-live-mode"));
        assert!(work.contains("original guidance"));
        assert!(!work.contains("mutated guidance"));
        assert_ne!(direct, work);
    }

    #[tokio::test]
    async fn inherited_child_prompt_uses_parent_snapshot_after_mutation_and_restart() {
        let temp = TempDir::new().unwrap();
        let guidance_path = temp.path().join("AGENTS.md");
        fs::write(&guidance_path, "parent snapshot guidance").unwrap();
        write_skill(
            temp.path(),
            ".claude/skills",
            "build",
            "build",
            "Parent snapshot skill body.",
        );
        let snapshot =
            discover_project_instruction_bundle_with_options(temp.path(), Some(temp.path()), None);
        let db_path = temp.path().join("snapshot.db");
        let db = crate::db::Database::open(db_path.to_str().unwrap())
            .await
            .unwrap();
        crate::db::run_pending_migrations(db.pool()).await.unwrap();
        db.create_conversation(
            "parent",
            "parent",
            temp.path().to_str().unwrap(),
            true,
            None,
            None,
        )
        .await
        .unwrap();
        db.create_conversation(
            "child",
            "child",
            temp.path().to_str().unwrap(),
            false,
            Some("parent"),
            None,
        )
        .await
        .unwrap();
        db.initialize_project_instruction_bundle_if_absent("parent", &snapshot)
            .await
            .unwrap();

        fs::write(&guidance_path, "mutated guidance").unwrap();
        write_skill(
            temp.path(),
            ".claude/skills",
            "build",
            "build",
            "Mutated skill body.",
        );
        db.copy_active_project_instruction_bundle_to_child("parent", "child")
            .await
            .unwrap();
        drop(db);

        let reopened = crate::db::Database::open(db_path.to_str().unwrap())
            .await
            .unwrap();
        crate::db::run_pending_migrations(reopened.pool())
            .await
            .unwrap();
        let child = reopened
            .load_active_project_instruction_bundle("child")
            .await
            .unwrap()
            .unwrap();
        let prompt = build_system_prompt_with_project_instructions(
            temp.path(),
            "tasks",
            true,
            Some(&ModeContext::Direct),
            LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
            &child,
        );
        assert!(prompt.contains("parent snapshot guidance"));
        assert!(prompt.contains("Parent snapshot skill body."));
        assert!(!prompt.contains("mutated guidance"));
        assert!(!prompt.contains("Mutated skill body."));
    }

    #[test]
    fn test_build_system_prompt_sub_agent() {
        let temp = TempDir::new().unwrap();
        let prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            true,
            None,
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );

        assert!(prompt.contains("sub-agent"));
        assert!(prompt.contains("submit_result"));
    }

    #[test]
    fn test_persona_replaces_base_preamble_but_keeps_suffix() {
        // REQ-AG-006: persona stands in for the base preamble; the sub-agent
        // result-submission suffix is retained regardless.
        let temp = TempDir::new().unwrap();
        let persona = "You are a meticulous security reviewer.";
        let prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            true,
            None,
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            Some(persona),
            ExploreBashCapability::Unavailable,
        );

        assert!(
            prompt.starts_with(persona),
            "persona should lead the prompt"
        );
        assert!(
            !prompt.contains("helpful AI assistant"),
            "base preamble should be replaced by the persona"
        );
        assert!(
            prompt.contains("submit_result"),
            "sub-agent suffix must survive persona replacement"
        );
    }

    // -------------------------------------------------------------------------
    // Skill catalog injection (skill discovery itself is tested in phoenix-skills)
    // -------------------------------------------------------------------------

    /// Write a skill under `{base}/{skills_subdir}/{skill_dir_name}/SKILL.md`.
    /// `skills_subdir` is a skill discovery dir (e.g. ".claude/skills").
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
    fn test_build_system_prompt_with_skills() {
        let temp = TempDir::new().unwrap();
        write_skill(
            temp.path(),
            ".claude/skills",
            "deploy-skill",
            "deploy-skill",
            "Deploy the app. Use when deploying.",
        );

        let prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            None,
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("</available_skills>"));
        assert!(prompt.contains("**deploy-skill**"));
        assert!(prompt.contains("Deploy the app"));
        assert!(prompt.contains("SKILL.md"));
    }

    #[test]
    fn test_build_system_prompt_no_skills() {
        let temp = TempDir::new().unwrap();
        let prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            None,
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );

        assert!(!prompt.contains("<available_skills>"));
    }

    #[test]
    fn test_explore_mode_injects_next_taskmd_id_when_marker_present() {
        let temp = TempDir::new().unwrap();
        let tasks_dir = temp.path().join("tasks");
        fs::create_dir(&tasks_dir).unwrap();
        fs::write(
            tasks_dir.join(taskmd_core::constants::TEMPLATE_FILENAME),
            "# Task Title\n",
        )
        .unwrap();

        let mode = ModeContext::Explore {
            next_taskmd_id_hint: snapshot_next_taskmd_id_hint(temp.path(), "tasks")
                .map(|id| id.to_string()),
        };
        let prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            Some(&mode),
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );

        assert!(prompt.contains("Explore mode"));
        assert!(
            prompt.contains("next available taskmd ID for this worktree"),
            "expected next-id hint in prompt: {prompt}"
        );
        let expected_id = taskmd_core::ids::next_id(&tasks_dir);
        assert!(
            prompt.contains(&format!("`{expected_id}`")),
            "expected ID `{expected_id}` in prompt: {prompt}"
        );
    }

    #[test]
    fn test_explore_mode_omits_next_taskmd_id_when_marker_absent() {
        let temp = TempDir::new().unwrap();
        let tasks_dir = temp.path().join("tasks");
        fs::create_dir(&tasks_dir).unwrap();
        // No _TEMPLATE.md — plain-markdown workflow, not taskmd-managed.
        fs::write(tasks_dir.join("plan.md"), "# Plan\n").unwrap();

        let prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            Some(&ModeContext::Explore {
                next_taskmd_id_hint: None,
            }),
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );

        assert!(prompt.contains("Explore mode"));
        assert!(
            !prompt.contains("next available taskmd ID"),
            "next-id hint should be omitted when no _TEMPLATE.md marker: {prompt}"
        );
    }

    #[test]
    fn test_explore_mode_omits_next_taskmd_id_when_tasks_dir_absent() {
        let temp = TempDir::new().unwrap();
        // No tasks/ directory at all.
        let prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            Some(&ModeContext::Explore {
                next_taskmd_id_hint: None,
            }),
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );
        assert!(!prompt.contains("next available taskmd ID"));
    }

    #[test]
    fn test_next_taskmd_id_respects_custom_tasks_dir_name() {
        let temp = TempDir::new().unwrap();
        let tasks_dir = temp.path().join("task-archive");
        fs::create_dir(&tasks_dir).unwrap();
        fs::write(
            tasks_dir.join(taskmd_core::constants::TEMPLATE_FILENAME),
            "# Task Title\n",
        )
        .unwrap();

        let mode = ModeContext::Explore {
            next_taskmd_id_hint: snapshot_next_taskmd_id_hint(temp.path(), "task-archive")
                .map(|id| id.to_string()),
        };
        let prompt = build_system_prompt_with_options(
            temp.path(),
            "task-archive",
            false,
            Some(&mode),
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );

        let expected_id = taskmd_core::ids::next_id(&tasks_dir);
        assert!(prompt.contains(&format!("`{expected_id}`")));
        assert!(prompt.contains(&format!("`task-archive/{expected_id}")));
    }

    #[test]
    fn test_explore_mode_reuses_snapshotted_taskmd_id_after_file_creation() {
        let temp = TempDir::new().unwrap();
        let tasks_dir = temp.path().join("tasks");
        fs::create_dir(&tasks_dir).unwrap();
        fs::write(
            tasks_dir.join(taskmd_core::constants::TEMPLATE_FILENAME),
            "# Task Title\n",
        )
        .unwrap();

        let hinted_id = snapshot_next_taskmd_id_hint(temp.path(), "tasks")
            .expect("taskmd marker should produce a hint")
            .to_string();
        let mode = ModeContext::Explore {
            next_taskmd_id_hint: Some(hinted_id.clone()),
        };
        let first_prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            Some(&mode),
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );

        fs::write(
            tasks_dir.join(format!("{hinted_id}-p2-ready--draft.md")),
            "# Draft\n",
        )
        .unwrap();
        let recomputed_id = taskmd_core::ids::next_id(&tasks_dir);
        assert_ne!(hinted_id, recomputed_id);

        let second_prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            Some(&mode),
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );

        assert_eq!(first_prompt, second_prompt);
        assert!(second_prompt.contains(&format!("`{hinted_id}`")));
        assert!(!second_prompt.contains(&format!("`{recomputed_id}`")));
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
            "tasks",
            false,
            Some(&mode),
            Some(temp.path()),
            None,
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );

        assert!(prompt.contains("Work mode"));
        assert!(prompt.contains("task-42-fix-bug"));
        assert!(prompt.contains("/home/user/project/worktrees/abc123"));
        assert!(prompt.contains("MUST stay inside this worktree"));
        // The agent owns the task-status rename (taskmd files only); nothing
        // does it automatically.
        assert!(prompt.contains("mark it done yourself"));
        assert!(prompt.contains("Phoenix does not perform the merge"));
        // The Work block no longer hands out a taskmd ID prefix — task files
        // need not be taskmd files at all (task 13009).
        assert!(!prompt.contains("task ID prefix"));
    }

    // -------------------------------------------------------------------------
    // Built-in skill catalog rendering (specs/builtin-skills/)
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
    fn test_catalog_renders_builtin_with_marker_not_path() {
        let temp = TempDir::new().unwrap();
        let extract_dir = write_fake_builtin(temp.path(), "spears", "Built-in spears");
        let prompt = build_system_prompt_with_options(
            temp.path(),
            "tasks",
            false,
            None,
            Some(temp.path()),
            Some(&extract_dir),
            crate::llm_language::LlmLanguage::default(),
            None,
            ExploreBashCapability::Unavailable,
        );
        assert!(prompt.contains("**spears**"));
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
}
