//! The single source of truth for where a conversation's inline references
//! (`@file`, `./path`, `/skill`) resolve.
//!
//! A conversation resolves references against exactly one root. For a Direct
//! conversation that root is the live working directory; for a Branch/Managed
//! conversation it is a fresh worktree of the chosen branch — i.e. that
//! branch's *committed tree*. The composer's autocomplete (before the
//! conversation exists) and the create-time first-message expansion both
//! construct this same value via [`ResolutionRoot::for_create`] and consume it
//! through the same methods, so the candidate set offered to the user and the
//! set the first message expands against cannot diverge.
//!
//! `WorkingDir` reads the filesystem directly. `GitTree` reads a branch's
//! committed tree via `git ls-tree` / `git cat-file` — no worktree required,
//! which is what lets the `/new` composer offer accurate suggestions for a
//! branch workflow before any worktree has been created.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::api::handlers::{fuzzy_score_path, search_files_in_root};
use crate::api::FileSearchEntry;
use crate::git_ops::{run_git, run_git_bytes};

/// Outcome of resolving a single `@file` reference's content.
pub enum FileResolution {
    /// File exists and is UTF-8 text.
    Text(String),
    /// File exists but is binary (contains a NUL byte / invalid UTF-8).
    Binary,
    /// File does not exist at this root.
    NotFound,
}

/// Where inline references resolve for a conversation. See module docs.
pub enum ResolutionRoot {
    /// Resolve against a live filesystem directory (Direct mode — the agent
    /// runs here, so uncommitted/untracked state is correct to surface).
    WorkingDir(PathBuf),
    /// Resolve against a branch's committed tree (Branch/Managed mode — the
    /// exact content the about-to-be-created worktree will hold).
    GitTree {
        /// Repository root the `git` commands run against.
        repo_root: PathBuf,
        /// Branch/commit-ish whose committed tree is the resolution target.
        reference: String,
    },
}

impl ResolutionRoot {
    /// Resolve against a live working directory.
    pub fn working_dir(dir: impl Into<PathBuf>) -> Self {
        Self::WorkingDir(dir.into())
    }

    /// Construct the root a new conversation will resolve against, from the
    /// same creation parameters the create handler uses.
    ///
    /// `mode` is the resolved creation mode (`"direct"`, `"managed"`,
    /// `"branch"`). Branch/managed resolve against `base_branch`'s committed
    /// tree; everything else resolves against `cwd`. If a git mode is missing
    /// either the repo root or the branch, it degrades to `cwd` — there is no
    /// ref to resolve against yet, so the working directory is the best
    /// available (and what the user is looking at).
    pub fn for_create(cwd: &str, mode: &str, base_branch: Option<&str>) -> Self {
        let cwd_path = PathBuf::from(cwd);
        if matches!(mode, "branch" | "managed") {
            if let (Some(branch), Some(repo_root)) = (
                base_branch.filter(|b| !b.is_empty()),
                crate::db::detect_git_repo_root(&cwd_path),
            ) {
                return Self::GitTree {
                    repo_root: PathBuf::from(repo_root),
                    reference: branch.to_string(),
                };
            }
        }
        Self::WorkingDir(cwd_path)
    }

    /// Fuzzy-search files at this root, returning paths relative to the root.
    pub fn list_files(&self, query: &str, limit: usize) -> Vec<FileSearchEntry> {
        match self {
            Self::WorkingDir(dir) => search_files_in_root(dir, query, limit),
            Self::GitTree { repo_root, reference } => {
                list_files_in_tree(repo_root, reference, query, limit)
            }
        }
    }

    /// Resolve a single `@file` reference's content. `rel` is the reference
    /// token exactly as typed (e.g. `src/main.rs`).
    pub fn read_file(&self, rel: &str) -> FileResolution {
        match self {
            Self::WorkingDir(dir) => {
                let p = Path::new(rel);
                let full = if p.is_absolute() { p.to_path_buf() } else { dir.join(p) };
                if !full.exists() {
                    return FileResolution::NotFound;
                }
                match std::fs::read(&full) {
                    Ok(bytes) => bytes_to_resolution(bytes),
                    Err(_) => FileResolution::NotFound,
                }
            }
            Self::GitTree { repo_root, reference } => {
                // Absolute paths have no meaning inside a tree; a ref-relative
                // path is what `git cat-file` expects.
                if Path::new(rel).is_absolute() {
                    return FileResolution::NotFound;
                }
                let spec = format!("{reference}:{rel}");
                match run_git_bytes(repo_root, &["cat-file", "-p", &spec]) {
                    Ok(bytes) => bytes_to_resolution(bytes),
                    Err(_) => FileResolution::NotFound,
                }
            }
        }
    }

    /// A filesystem directory to run skill discovery against. For `WorkingDir`
    /// this is the directory itself. For `GitTree` it is a temporary
    /// materialization of the ref's `.claude/skills` / `.agents/skills`
    /// `SKILL.md` files, so the existing filesystem-based skill discovery and
    /// invocation run unchanged against the branch's committed skills.
    ///
    /// The returned [`SkillsView`] owns any temp directory; keep it alive for
    /// the duration of discovery *and* invocation (invocation reads `SKILL.md`
    /// back from the same paths).
    pub fn skills_view(&self) -> SkillsView {
        match self {
            Self::WorkingDir(dir) => SkillsView { dir: dir.clone(), _temp: None },
            Self::GitTree { repo_root, reference } => {
                materialize_skill_files(repo_root, reference)
            }
        }
    }
}

/// A directory to discover skills from, plus an optional temp guard that must
/// outlive both discovery and invocation.
pub struct SkillsView {
    pub dir: PathBuf,
    _temp: Option<TempDir>,
}

fn bytes_to_resolution(bytes: Vec<u8>) -> FileResolution {
    if bytes.contains(&0) {
        return FileResolution::Binary;
    }
    match String::from_utf8(bytes) {
        Ok(text) => FileResolution::Text(text),
        Err(_) => FileResolution::Binary,
    }
}

/// Extensions we treat as text when listing a git tree, where reading every
/// blob to sniff content would be prohibitively expensive. The `is_text_file`
/// flag is a cosmetic autocomplete hint, so an extension guess is sufficient.
fn looks_textual(path: &str) -> bool {
    !matches!(
        Path::new(path).extension().and_then(|e| e.to_str()),
        Some(
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "pdf" | "zip" | "gz" | "tar"
                | "wasm" | "so" | "dylib" | "dll" | "exe" | "bin" | "o" | "a" | "class" | "jar"
                | "mp3" | "mp4" | "mov" | "woff" | "woff2" | "ttf" | "otf"
        )
    )
}

/// List files in a branch's committed tree, fuzzy-scored against `query` with
/// the same matcher the filesystem walk uses so ranking is identical.
fn list_files_in_tree(
    repo_root: &Path,
    reference: &str,
    query: &str,
    limit: usize,
) -> Vec<FileSearchEntry> {
    let Ok(listing) = run_git(repo_root, &["ls-tree", "-r", "--name-only", reference]) else {
        return Vec::new();
    };
    let q = query.to_lowercase();
    let mut matcher = nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT);
    let mut buf: Vec<char> = Vec::new();
    let mut items: Vec<(i32, FileSearchEntry)> = Vec::new();

    for rel_path in listing.lines() {
        if rel_path.is_empty() {
            continue;
        }
        let entry = FileSearchEntry {
            path: rel_path.to_string(),
            is_text_file: looks_textual(rel_path),
        };
        if q.is_empty() {
            items.push((0, entry));
            if items.len() >= limit {
                break;
            }
        } else if let Some(score) = fuzzy_score_path(rel_path, &q, &mut matcher, &mut buf) {
            items.push((score, entry));
        }
    }

    if !q.is_empty() {
        items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
        items.truncate(limit);
    }
    items.into_iter().map(|(_, e)| e).collect()
}

/// Materialize a ref's `.claude/skills` / `.agents/skills` `SKILL.md` files
/// into a temp directory at their original relative paths, so the existing
/// filesystem skill discovery (which expects `<root>/.claude/skills/...`)
/// works unchanged. Only `SKILL.md` files are written — discovery and
/// invocation read nothing else from a skill directory.
fn materialize_skill_files(repo_root: &Path, reference: &str) -> SkillsView {
    // On any failure, fall back to an empty temp dir: discovery still surfaces
    // global (`$HOME`) and built-in skills, just no repo-local ones.
    let temp = match TempDir::new() {
        Ok(t) => t,
        Err(_) => return SkillsView { dir: repo_root.to_path_buf(), _temp: None },
    };

    let listing = run_git(
        repo_root,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            reference,
            "--",
            ".claude/skills",
            ".agents/skills",
        ],
    )
    .unwrap_or_default();

    for rel_path in listing.lines() {
        if !rel_path.ends_with("SKILL.md") {
            continue;
        }
        let spec = format!("{reference}:{rel_path}");
        let Ok(bytes) = run_git_bytes(repo_root, &["cat-file", "-p", &spec]) else {
            continue;
        };
        let dest = temp.path().join(rel_path);
        if let Some(parent) = dest.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                continue;
            }
        }
        let _ = std::fs::write(&dest, bytes);
    }

    SkillsView { dir: temp.path().to_path_buf(), _temp: Some(temp) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            // Disable commit signing — the host may set commit.gpgsign globally,
            // which would fail in the sandbox without a signing key.
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
            .env("GIT_CONFIG_VALUE_0", "false")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Build a repo whose committed tree differs from the working directory:
    /// `committed.txt` is committed, `untracked.txt` exists only in the working
    /// dir. Returns the repo dir.
    fn repo_with_divergent_worktree() -> TempDir {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(repo.path().join("committed.txt"), "hello from commit").unwrap();
        std::fs::create_dir_all(repo.path().join(".claude/skills/greet")).unwrap();
        std::fs::write(
            repo.path().join(".claude/skills/greet/SKILL.md"),
            "---\nname: greet\ndescription: Greet someone\n---\n\nSay hello.",
        )
        .unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "initial"]);
        // Working-dir-only file: present on disk, absent from the committed tree.
        std::fs::write(repo.path().join("untracked.txt"), "only on disk").unwrap();
        repo
    }

    #[test]
    fn git_tree_lists_committed_files_not_untracked() {
        let repo = repo_with_divergent_worktree();
        let root = ResolutionRoot::GitTree {
            repo_root: repo.path().to_path_buf(),
            reference: "main".to_string(),
        };
        let files = root.list_files("txt", 50);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"committed.txt"), "committed file should list: {paths:?}");
        assert!(
            !paths.contains(&"untracked.txt"),
            "untracked working-dir file must NOT list (it won't be in the worktree): {paths:?}"
        );
    }

    #[test]
    fn git_tree_reads_committed_content_and_misses_untracked() {
        let repo = repo_with_divergent_worktree();
        let root = ResolutionRoot::GitTree {
            repo_root: repo.path().to_path_buf(),
            reference: "main".to_string(),
        };
        match root.read_file("committed.txt") {
            FileResolution::Text(t) => assert_eq!(t, "hello from commit"),
            _ => panic!("expected committed.txt to resolve as text"),
        }
        assert!(
            matches!(root.read_file("untracked.txt"), FileResolution::NotFound),
            "untracked file must be NotFound against the committed tree"
        );
    }

    #[test]
    fn working_dir_sees_untracked_files() {
        let repo = repo_with_divergent_worktree();
        let root = ResolutionRoot::working_dir(repo.path());
        match root.read_file("untracked.txt") {
            FileResolution::Text(t) => assert_eq!(t, "only on disk"),
            _ => panic!("working dir should see untracked file"),
        }
    }

    #[test]
    fn git_tree_skills_view_materializes_committed_skill() {
        let repo = repo_with_divergent_worktree();
        let root = ResolutionRoot::GitTree {
            repo_root: repo.path().to_path_buf(),
            reference: "main".to_string(),
        };
        let view = root.skills_view();
        let skill_md = view.dir.join(".claude/skills/greet/SKILL.md");
        assert!(skill_md.is_file(), "SKILL.md should be materialized from the ref tree");
        let body = std::fs::read_to_string(&skill_md).unwrap();
        assert!(body.contains("name: greet"));
    }

    #[test]
    fn for_create_direct_is_working_dir() {
        let root = ResolutionRoot::for_create("/tmp/x", "direct", None);
        assert!(matches!(root, ResolutionRoot::WorkingDir(_)));
    }

    #[test]
    fn for_create_branch_without_repo_degrades_to_working_dir() {
        // /tmp is (almost certainly) not a git repo, so a branch mode with no
        // resolvable repo root falls back to the working directory.
        let root = ResolutionRoot::for_create("/tmp", "branch", Some("main"));
        assert!(matches!(root, ResolutionRoot::WorkingDir(_)));
    }
}
