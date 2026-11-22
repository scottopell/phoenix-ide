//! `TaskSource` — the narrow seam between Phoenix's Explore→Work lifecycle and
//! the kind of file the agent points `propose_task` at (task 13009).
//!
//! Exactly two kinds, no more (KISS):
//!
//! - [`TaskSource::Taskmd`] — a filename that parses as `NNNNN-pX-status--slug.md`
//!   (taskmd 1.0). `id`, `priority`, `status`, and `slug` all come from the
//!   filename; on approval the worktree's temp branch is renamed to
//!   `task-{id}-{slug}` and the file's status segment is promoted to
//!   `in-progress` if it isn't already.
//!
//! - [`TaskSource::PlainMarkdown`] — any other `.md` file. Treated as a free-form
//!   task brief with no structured metadata: title from the body's first `# H1`
//!   (falling back to the file stem), `priority` defaults to `p2` (display-only),
//!   no status segment, **no status-rename on approval**, branch name derived
//!   from the file stem plus a short conversation-id uniquifier.
//!
//! A future explicit "task source = plain | taskmd | beads | …" backend would
//! slot in behind this same seam; that is deliberately out of scope for v1.

use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use taskmd_core::constants::Status;

/// Task priority — `p0` (highest) .. `p4` (lowest).
///
/// Newtype wrapper around `taskmd_core::constants::Priority` so the
/// authoritative enum is reused (no parallel set of variants), with serde
/// support added at the Phoenix-side type. The wire/persisted form is the
/// same lowercase string (`"p0"` .. `"p4"`) that taskmd filenames carry, so
/// existing DB rows round-trip unchanged; a payload with any other value
/// fails deserialisation loudly rather than being threaded through as bare
/// text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Priority(pub taskmd_core::constants::Priority);

impl Priority {
    pub const P0: Self = Priority(taskmd_core::constants::Priority::P0);
    pub const P1: Self = Priority(taskmd_core::constants::Priority::P1);
    pub const P2: Self = Priority(taskmd_core::constants::Priority::P2);
    pub const P3: Self = Priority(taskmd_core::constants::Priority::P3);
    pub const P4: Self = Priority(taskmd_core::constants::Priority::P4);

    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.0.as_str()
    }
}

impl Default for Priority {
    fn default() -> Self {
        Priority::P2
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl From<taskmd_core::constants::Priority> for Priority {
    fn from(p: taskmd_core::constants::Priority) -> Self {
        Priority(p)
    }
}

impl From<Priority> for taskmd_core::constants::Priority {
    fn from(p: Priority) -> Self {
        p.0
    }
}

impl Serialize for Priority {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for Priority {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        taskmd_core::constants::Priority::from_str(&s)
            .map(Priority)
            .map_err(serde::de::Error::custom)
    }
}

/// Classification of a task file referenced by `propose_task`, derived purely
/// from its filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskSource {
    /// Filename parsed as a taskmd 1.0 filename.
    Taskmd {
        id: String,
        /// `p0`..`p4`, from the filename.
        priority: Priority,
        status: Status,
        slug: String,
    },
    /// Any other markdown file — a plain task brief.
    PlainMarkdown {
        /// Filename with the `.md` extension stripped.
        stem: String,
    },
}

impl TaskSource {
    /// Classify a task-file *filename* (not a path). Returns `None` if the name
    /// is not a markdown file at all (no `.md` extension, or an empty stem).
    ///
    /// taskmd is tried first: if the name matches the taskmd pattern it is a
    /// [`TaskSource::Taskmd`] regardless of extension casing, otherwise any
    /// `.md` file is a [`TaskSource::PlainMarkdown`].
    #[must_use]
    pub fn detect(filename: &str) -> Option<Self> {
        if let Some(parsed) = taskmd_core::filename::parse_filename(filename) {
            return Some(Self::Taskmd {
                id: parsed.id,
                priority: Priority::from(parsed.priority),
                status: parsed.status,
                slug: parsed.slug,
            });
        }
        let path = Path::new(filename);
        let is_markdown = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md"));
        if !is_markdown {
            return None;
        }
        let stem = path.file_stem().and_then(|s| s.to_str())?;
        if stem.is_empty() {
            return None;
        }
        Some(Self::PlainMarkdown {
            stem: stem.to_string(),
        })
    }

    /// Display priority for the approval UI. taskmd files carry one in the
    /// filename; plain-markdown files default to `p2`.
    #[must_use]
    pub fn priority(&self) -> Priority {
        match self {
            Self::Taskmd { priority, .. } => *priority,
            Self::PlainMarkdown { .. } => Priority::P2,
        }
    }

    /// UI title: the body's first `# H1`, else a title-cased fallback derived
    /// from the slug (taskmd) or file stem (plain markdown).
    #[must_use]
    pub fn title(&self, body: &str) -> String {
        extract_h1(body).unwrap_or_else(|| match self {
            Self::Taskmd { slug, .. } => identifier_to_title(slug),
            Self::PlainMarkdown { stem } => identifier_to_title(stem),
        })
    }

    /// The git branch name for the Work-mode worktree, and the `task_id` value
    /// recorded on the conversation.
    ///
    /// - taskmd: `task-{id}-{slug}`; the `task_id` is the taskmd id.
    /// - plain markdown: `task-{sanitized-stem}-{conv-id-prefix}`. The
    ///   conversation-id prefix is the uniquifier — two conversations that
    ///   propose files with the same stem must not collide on the branch name
    ///   (the approval mutex only serializes; it does not uniquify). The
    ///   `task_id` is the sanitized stem (or the conv-id prefix when the stem
    ///   sanitizes to empty), kept non-empty for the conversation record.
    #[must_use]
    pub fn branch_and_id(&self, conv_id: &str) -> (String, String) {
        match self {
            Self::Taskmd { id, slug, .. } => (format!("task-{id}-{slug}"), id.clone()),
            Self::PlainMarkdown { stem } => {
                let conv_prefix: String = conv_id.chars().take(8).collect();
                let sanitized = slug_segment(stem);
                let id_segment = if sanitized.is_empty() {
                    conv_prefix.clone()
                } else {
                    sanitized
                };
                (format!("task-{id_segment}-{conv_prefix}"), id_segment)
            }
        }
    }
}

/// First ATX `# H1` heading in a markdown body, trimmed; `None` if there is none.
///
/// Lines inside fenced code blocks and indented code (4+ spaces) are not
/// headings. A closing `#` sequence (`# Title ##`) is not part of the title.
fn extract_h1(body: &str) -> Option<String> {
    let mut fence: Option<(char, usize)> = None;
    for line in body.lines() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let unindented = line.trim_start_matches(' ');
        if let Some((ch, len)) = fence {
            let run = unindented.chars().take_while(|&c| c == ch).count();
            if indent < 4 && run >= len && unindented.trim_start_matches(ch).trim().is_empty() {
                fence = None;
            }
            continue;
        }
        if indent >= 4 {
            continue;
        }
        if let Some(ch) = unindented.chars().next().filter(|c| matches!(c, '`' | '~')) {
            let run = unindented.chars().take_while(|&c| c == ch).count();
            if run >= 3 {
                fence = Some((ch, run));
                continue;
            }
        }
        let Some(rest) = unindented.strip_prefix('#') else {
            continue;
        };
        if !rest.starts_with([' ', '\t']) {
            continue;
        }
        let title = strip_closing_hashes(rest.trim());
        if !title.is_empty() {
            return Some(title.to_string());
        }
    }
    None
}

/// Drop an ATX closing sequence: trailing `#`s preceded by whitespace (or the
/// whole text when it is only `#`s).
fn strip_closing_hashes(text: &str) -> &str {
    let without = text.trim_end_matches('#');
    if without.len() == text.len() {
        text
    } else if without.is_empty() || without.ends_with([' ', '\t']) {
        without.trim_end()
    } else {
        text
    }
}

/// Title-case a `-`/`_`-separated identifier (`fix-login_bug` → `Fix Login Bug`).
fn identifier_to_title(ident: &str) -> String {
    ident
        .split(['-', '_', ' '])
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Longest slug segment [`slug_segment`] emits; keeps a branch or file name
/// well under the 255-byte path-component limit.
pub const MAX_SLUG_SEGMENT_LEN: usize = 48;

/// Reduce an arbitrary string to a git-ref- and filename-safe lowercase
/// segment: `[a-z0-9]+` runs joined by single `-`, everything else dropped,
/// at most [`MAX_SLUG_SEGMENT_LEN`] bytes with no trailing `-`. Empty when the
/// input has no ASCII alphanumerics.
#[must_use]
pub fn slug_segment(stem: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in stem.chars() {
        if out.len() >= MAX_SLUG_SEGMENT_LEN {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_taskmd_filename() {
        let src = TaskSource::detect("12345-p1-ready--fix-the-login-bug.md").unwrap();
        match &src {
            TaskSource::Taskmd {
                id,
                priority,
                status,
                slug,
            } => {
                assert_eq!(id, "12345");
                assert_eq!(*priority, Priority::P1);
                assert_eq!(*status, Status::Ready);
                assert_eq!(slug, "fix-the-login-bug");
            }
            other @ TaskSource::PlainMarkdown { .. } => {
                panic!("expected Taskmd, got {other:?}")
            }
        }
        assert_eq!(src.priority(), Priority::P1);
        let (branch, id) = src.branch_and_id("conversation-abcdef");
        assert_eq!(branch, "task-12345-fix-the-login-bug");
        assert_eq!(id, "12345");
    }

    #[test]
    fn detects_plain_markdown() {
        let src = TaskSource::detect("plan.md").unwrap();
        assert_eq!(
            src,
            TaskSource::PlainMarkdown {
                stem: "plan".to_string()
            }
        );
        assert_eq!(src.priority(), Priority::P2);
        // README.md is a valid plain-markdown task brief.
        assert_eq!(
            TaskSource::detect("README.md"),
            Some(TaskSource::PlainMarkdown {
                stem: "README".to_string()
            })
        );
    }

    #[test]
    fn rejects_non_markdown() {
        assert_eq!(TaskSource::detect("notes.txt"), None);
        assert_eq!(TaskSource::detect("Makefile"), None);
        assert_eq!(TaskSource::detect(".md"), None);
        assert_eq!(TaskSource::detect(""), None);
        // Only `.md` — `.markdown` and other variants are not accepted.
        assert_eq!(TaskSource::detect("Design.markdown"), None);
    }

    #[test]
    fn plain_markdown_branch_is_conv_uniquified() {
        let a = TaskSource::detect("feature.md").unwrap();
        let b = TaskSource::detect("feature.md").unwrap();
        let (branch_a, id_a) = a.branch_and_id("conv-aaaaaaaa-1");
        let (branch_b, _) = b.branch_and_id("conv-bbbbbbbb-2");
        assert_eq!(branch_a, "task-feature-conv-aaa");
        assert_eq!(id_a, "feature");
        assert_ne!(
            branch_a, branch_b,
            "different conversations -> different branches"
        );
    }

    #[test]
    fn plain_markdown_branch_segment_sanitized() {
        let src = TaskSource::detect("My Big Plan! (v2).md").unwrap();
        let (branch, id) = src.branch_and_id("abcdefghij");
        assert_eq!(id, "my-big-plan-v2");
        assert_eq!(branch, "task-my-big-plan-v2-abcdefgh");
    }

    #[test]
    fn plain_markdown_empty_sanitized_stem_falls_back_to_conv_id() {
        let src = TaskSource::detect("！！！.md").unwrap();
        let (branch, id) = src.branch_and_id("abcdefghij-extra");
        assert_eq!(id, "abcdefgh");
        assert_eq!(branch, "task-abcdefgh-abcdefgh");
    }

    /// Priority round-trips through the same string form taskmd uses on disk;
    /// any other value fails deserialisation loudly. This is the
    /// correct-by-construction guard: an invalid priority cannot reach the
    /// approval UI as a bare String the way it could under task 13016.
    #[test]
    fn priority_round_trips_lowercase_string() {
        for (p, s) in [
            (Priority::P0, "p0"),
            (Priority::P1, "p1"),
            (Priority::P2, "p2"),
            (Priority::P3, "p3"),
            (Priority::P4, "p4"),
        ] {
            let v = serde_json::to_value(p).unwrap();
            assert_eq!(v, serde_json::Value::String(s.to_string()));
            let back: Priority = serde_json::from_value(v).unwrap();
            assert_eq!(back, p);
            assert_eq!(p.as_str(), s);
            assert_eq!(p.to_string(), s);
        }
    }

    #[test]
    fn priority_rejects_unknown_values() {
        let v = serde_json::Value::String("p9".to_string());
        assert!(
            serde_json::from_value::<Priority>(v).is_err(),
            "p9 must not deserialise into Priority"
        );
        let v = serde_json::Value::String(String::new());
        assert!(
            serde_json::from_value::<Priority>(v).is_err(),
            "empty string must not deserialise into Priority"
        );
    }

    #[test]
    fn title_prefers_h1_then_falls_back() {
        let taskmd = TaskSource::detect("12345-p2-ready--fix-the-login-bug.md").unwrap();
        assert_eq!(
            taskmd.title("# Repair the login flow\n\nbody"),
            "Repair the login flow"
        );
        assert_eq!(taskmd.title("no heading here"), "Fix The Login Bug");

        let plain = TaskSource::detect("migration_notes.md").unwrap();
        assert_eq!(plain.title("# DB migration plan"), "DB migration plan");
        assert_eq!(plain.title("(no heading)"), "Migration Notes");
    }

    #[test]
    fn title_ignores_hash_lines_in_fenced_code() {
        let body = "```sh\n# install deps\nmake\n```\n\n# Real Title";
        assert_eq!(extract_h1(body).as_deref(), Some("Real Title"));
        let tilde = "~~~~\n# nope\n~~~\n# still nope\n~~~~\n# Yes";
        assert_eq!(extract_h1(tilde).as_deref(), Some("Yes"));
        // Unclosed fence swallows the rest: no heading outside it.
        assert_eq!(extract_h1("```\n# x"), None);
    }

    #[test]
    fn title_ignores_indented_code_and_requires_space() {
        assert_eq!(extract_h1("    # code\n#nospace\n#Hashtag"), None);
        assert_eq!(extract_h1("   # Ok").as_deref(), Some("Ok"));
        assert_eq!(extract_h1("#\tTabbed").as_deref(), Some("Tabbed"));
    }

    #[test]
    fn title_strips_closing_hashes() {
        assert_eq!(extract_h1("# Title ##").as_deref(), Some("Title"));
        assert_eq!(extract_h1("# C#").as_deref(), Some("C#"));
        assert_eq!(extract_h1("# #"), None);
        assert_eq!(extract_h1("# Title"), Some("Title".to_string()));
    }

    #[test]
    fn plain_markdown_branch_is_bounded_and_nonempty() {
        let long = TaskSource::detect(&format!("{}.md", "a".repeat(300))).unwrap();
        let (branch, id) = long.branch_and_id("conv-12345678");
        assert!(branch.len() < 80, "{branch}");
        assert!(!id.ends_with('-'));
        let (branch, id) = TaskSource::detect("日本語.md")
            .unwrap()
            .branch_and_id("conv-12345678");
        assert_eq!(id, "conv-123");
        assert_eq!(branch, "task-conv-123-conv-123");
    }

    mod proptests {
        use super::super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn slug_segment_is_safe_and_bounded(s in "\\PC{0,400}") {
                let out = slug_segment(&s);
                prop_assert!(out.len() <= MAX_SLUG_SEGMENT_LEN);
                prop_assert!(out.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
                prop_assert!(!out.starts_with('-') && !out.ends_with('-') && !out.contains("--"));
            }

            #[test]
            fn slug_segment_is_idempotent(s in "\\PC{0,200}") {
                let once = slug_segment(&s);
                prop_assert_eq!(slug_segment(&once), once);
            }

            #[test]
            fn h1_never_comes_from_inside_a_fence(
                inner in "[ -~]{0,30}",
                title in "[A-Za-z][A-Za-z0-9 ]{0,20}",
            ) {
                let body = format!("```\n# {inner}\n```\n# {title}");
                prop_assert_eq!(extract_h1(&body), Some(title.trim().to_string()));
            }
        }
    }
}
