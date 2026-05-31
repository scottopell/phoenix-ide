//! Result of invoking a skill — shared by the user `/skill` path and the
//! LLM Skill tool so both produce identical output.

/// The result of invoking a skill.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillInvocation {
    /// The skill name (e.g., "build")
    pub name: String,
    /// The fully expanded skill body: frontmatter stripped, base directory
    /// prepended, arguments substituted (REQ-SK-001, REQ-SK-003, REQ-SK-004)
    pub body: String,
    /// Absolute path to the skill's directory
    pub skill_dir: String,
}
