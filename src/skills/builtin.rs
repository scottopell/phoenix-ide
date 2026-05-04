//! Built-in skills compiled into the phoenix binary.
//!
//! Built-ins flow through the same discovery and invocation paths as
//! filesystem skills (see `specs/builtin-skills/` and `specs/skills/`). The
//! only divergence is the read step: built-in content lives as `&'static str`
//! rather than on disk.
//!
//! ## Override
//!
//! A filesystem skill of the same name shadows a built-in. The
//! `discover_skills` walk processes the filesystem first; built-ins are
//! appended last and the existing name dedup wins.
//!
//! ## Adding a built-in
//!
//! 1. Drop the body markdown in `src/skills/builtin/<name>.md`.
//! 2. Declare a `pub const` `BuiltinSkill` here using `include_str!`.
//! 3. Add `&YOUR_SKILL` to the `ALL` array.
//!
//! Frontmatter inside the `.md` is informational only — the registry holds
//! the canonical `name`/`description`/`argument_hint`.

/// A skill bundled with the phoenix binary.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinSkill {
    pub name: &'static str,
    pub description: &'static str,
    pub argument_hint: Option<&'static str>,
    pub content: &'static str,
}

pub const CAVEMAN: BuiltinSkill = BuiltinSkill {
    name: "caveman",
    description: "Talk like caveman. Drop articles and filler. Cuts ~75% of output tokens \
         without losing technical accuracy. Levels: lite | full | ultra | wenyan.",
    argument_hint: Some("[lite|full|ultra|wenyan]"),
    content: include_str!("builtin/caveman.md"),
};

pub const CAVEMAN_COMMIT: BuiltinSkill = BuiltinSkill {
    name: "caveman-commit",
    description:
        "Write a terse commit message. Conventional Commits, ≤50 char subject, why over what.",
    argument_hint: None,
    content: include_str!("builtin/caveman-commit.md"),
};

pub const CAVEMAN_REVIEW: BuiltinSkill = BuiltinSkill {
    name: "caveman-review",
    description:
        "One-line PR review comments. No throat-clearing. Severity emoji + file:line + fix.",
    argument_hint: None,
    content: include_str!("builtin/caveman-review.md"),
};

/// All built-in skills shipped with this build of phoenix.
pub const ALL: &[&BuiltinSkill] = &[&CAVEMAN, &CAVEMAN_COMMIT, &CAVEMAN_REVIEW];

/// Look up a built-in skill by name.
#[must_use]
pub fn find(name: &str) -> Option<&'static BuiltinSkill> {
    ALL.iter().copied().find(|s| s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lookup_hit() {
        let s = find("caveman").expect("caveman should be registered");
        assert_eq!(s.name, "caveman");
        assert!(!s.content.is_empty());
    }

    #[test]
    fn registry_lookup_miss() {
        assert!(find("nonexistent-builtin").is_none());
    }

    #[test]
    fn all_names_unique() {
        let mut names: Vec<&str> = ALL.iter().map(|s| s.name).collect();
        names.sort_unstable();
        let original_len = names.len();
        names.dedup();
        assert_eq!(names.len(), original_len, "built-in names must be unique");
    }

    #[test]
    fn all_have_nonempty_content() {
        for skill in ALL {
            assert!(
                !skill.content.is_empty(),
                "built-in {} has empty content",
                skill.name
            );
            assert!(
                !skill.description.is_empty(),
                "built-in {} has empty description",
                skill.name
            );
        }
    }
}
