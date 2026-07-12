#![allow(dead_code)]
//! The single source of truth for where a conversation's inline references
//! (`@file`, `./path`, `/skill`) resolve.
//!
//! `WorkingDir` reads a live filesystem directory; `GitTree` reads a branch's
//! committed tree via `git ls-tree` / `git cat-file` with no worktree required.
//!
//! The two cooperate so a candidate the composer offers always resolves when
//! the first message expands:
//!
//! - **Pre-create discovery** has no worktree yet, so it reads the chosen
//!   branch's committed tree through a `GitTree` built from
//!   [`crate::git_start::GitStartPoint`]. This is what lets the `/new`
//!   composer offer accurate suggestions before any worktree exists.
//! - **Create-time expansion** runs against the conversation's freshly-created
//!   worktree (`WorkingDir`), a clean checkout of that same committed tree —
//!   equivalent content (a clean checkout has no untracked files), plus the
//!   companion files and durable skill base directory a bare tree can't give.
//!
//! Both therefore resolve against the same branch ref; neither trusts the live
//! checkout, which is what closes the discovery-vs-expansion divergence.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tempfile::TempDir;

use crate::api::handlers::{fuzzy_score_path, search_files_in_root};
use crate::api::{FileSearchEntry, FileViewerKind};
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

    pub fn git_tree(repo_root: impl Into<PathBuf>, reference: impl Into<String>) -> Self {
        Self::GitTree {
            repo_root: repo_root.into(),
            reference: reference.into(),
        }
    }

    pub fn from_start_point(
        repo_root: impl Into<PathBuf>,
        start: &crate::git_start::GitStartPoint,
    ) -> Self {
        Self::git_tree(repo_root, start.tree_ref())
    }

    pub fn for_create(cwd: &str, mode: &str, base_branch: Option<&str>) -> Self {
        let cwd_path = PathBuf::from(cwd);
        if let Some(start) =
            crate::git_start::GitStartPoint::for_inline_discovery(&cwd_path, mode, base_branch)
        {
            if let Some(repo_root) = phoenix_core::git::detect_git_repo_root(&cwd_path) {
                return Self::from_start_point(repo_root, &start);
            }
        }
        Self::WorkingDir(cwd_path)
    }

    /// Fuzzy-search files at this root, returning paths relative to the root.
    pub fn list_files(&self, query: &str, limit: usize) -> Vec<FileSearchEntry> {
        match self {
            Self::WorkingDir(dir) => search_files_in_root(dir, query, limit),
            Self::GitTree {
                repo_root,
                reference,
            } => list_files_in_tree(repo_root, reference, query, limit),
        }
    }

    pub fn all_paths(&self) -> Vec<String> {
        match self {
            Self::WorkingDir(dir) => all_files_in_root(dir),
            Self::GitTree {
                repo_root,
                reference,
            } => tree_paths(repo_root, reference).iter().cloned().collect(),
        }
    }

    pub fn read_text(&self, rel: &str) -> Option<String> {
        match self.read_file(rel) {
            FileResolution::Text(text) => Some(text),
            FileResolution::Binary | FileResolution::NotFound => None,
        }
    }

    pub fn source_ref(&self) -> Option<&str> {
        match self {
            Self::WorkingDir(_) => None,
            Self::GitTree { reference, .. } => Some(reference),
        }
    }

    /// Resolve a single `@file` reference's content. `rel` is the reference
    /// token exactly as typed (e.g. `src/main.rs`).
    pub fn read_file(&self, rel: &str) -> FileResolution {
        match self {
            Self::WorkingDir(dir) => {
                let p = Path::new(rel);
                let full = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    dir.join(p)
                };
                if !full.exists() {
                    return FileResolution::NotFound;
                }
                match std::fs::read(&full) {
                    Ok(bytes) => bytes_to_resolution(bytes),
                    Err(_) => FileResolution::NotFound,
                }
            }
            Self::GitTree {
                repo_root,
                reference,
            } => {
                // Absolute paths have no meaning inside a tree; a ref-relative
                // path is what `git cat-file` expects.
                if Path::new(rel).is_absolute() {
                    return FileResolution::NotFound;
                }
                let spec = format!("{reference}:{rel}");
                // `cat-file blob` (not `-p`) refuses to dereference a tree: a
                // directory reference like `@src` errors out and resolves to
                // NotFound, matching the working-directory path (`fs::read` of a
                // dir fails) rather than expanding a pretty-printed tree listing.
                match run_git_bytes(repo_root, &["cat-file", "blob", &spec]) {
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
            Self::WorkingDir(dir) => SkillsView {
                dir: dir.clone(),
                _temp: None,
            },
            Self::GitTree {
                repo_root,
                reference,
            } => materialize_skill_files(repo_root, reference),
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

/// Process-global cache of a committed tree's full path listing, keyed by the
/// resolved commit SHA. `git ls-tree -r` enumerates the entire tree, which for
/// a large monorepo is expensive to run on every autocomplete keystroke; the
/// listing for a given commit is immutable, so caching by SHA collapses
/// repeated keystrokes (and the empty-query open) to a single enumeration per
/// ref content.
fn tree_listing_cache() -> &'static Mutex<HashMap<String, Arc<[String]>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<[String]>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The committed tree's path listing for `reference`, cached by resolved SHA.
///
/// Keying on the SHA (not the branch name) means a moved branch key-misses and
/// re-enumerates naturally, and two refs at the same commit share one entry.
/// Memory is bounded by dropping the cache wholesale once it accumulates a
/// handful of distinct trees — a session rarely discovers against many.
fn tree_paths(repo_root: &Path, reference: &str) -> Arc<[String]> {
    // Bound memory: drop the cache wholesale once it holds this many trees.
    const MAX_CACHED_TREES: usize = 8;

    let sha = run_git(
        repo_root,
        &["rev-parse", &format!("{reference}^{{commit}}")],
    )
    .map_or_else(|_| reference.to_string(), |s| s.trim().to_string());

    if let Some(hit) = tree_listing_cache().lock().unwrap().get(&sha) {
        return Arc::clone(hit);
    }

    let listing = run_git(repo_root, &["ls-tree", "-r", "--name-only", &sha]).unwrap_or_default();
    let paths: Arc<[String]> = listing
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();

    let mut cache = tree_listing_cache().lock().unwrap();
    if cache.len() >= MAX_CACHED_TREES {
        cache.clear();
    }
    cache.insert(sha, Arc::clone(&paths));
    paths
}

fn all_files_in_root(root: &Path) -> Vec<String> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    walker
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
        .filter_map(|entry| {
            entry
                .path()
                .strip_prefix(root)
                .ok()
                .map(|path| path.to_string_lossy().to_string())
        })
        .collect()
}

/// List files in a branch's committed tree, fuzzy-scored against `query` with
/// the same matcher the filesystem walk uses so ranking is identical.
fn list_files_in_tree(
    repo_root: &Path,
    reference: &str,
    query: &str,
    limit: usize,
) -> Vec<FileSearchEntry> {
    let paths = tree_paths(repo_root, reference);
    let q = query.to_lowercase();

    // Empty query: take the first `limit` paths in tree order, no scoring/sort —
    // the bounded fast path the filesystem walker also gets by stopping early.
    if q.is_empty() {
        return paths
            .iter()
            .take(limit)
            .map(|p| FileSearchEntry {
                path: p.clone(),
                viewer: FileViewerKind::for_path(Path::new(p)),
            })
            .collect();
    }

    let mut matcher = nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT);
    let mut buf: Vec<char> = Vec::new();
    let mut items: Vec<(i32, FileSearchEntry)> = Vec::new();
    for rel_path in paths.iter() {
        if let Some(score) = fuzzy_score_path(rel_path, &q, &mut matcher, &mut buf) {
            items.push((
                score,
                FileSearchEntry {
                    path: rel_path.clone(),
                    viewer: FileViewerKind::for_path(Path::new(rel_path)),
                },
            ));
        }
    }
    items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    items.truncate(limit);
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
    let Ok(temp) = TempDir::new() else {
        return SkillsView {
            dir: repo_root.to_path_buf(),
            _temp: None,
        };
    };

    // Reuse the cached full tree listing and pick out `SKILL.md` files under a
    // `.claude/skills` / `.agents/skills` directory at any depth. This matches
    // `discover_skills`' scan scope against a real working directory — the repo
    // root *and* immediate child projects (e.g. `service/.agents/skills/...`).
    // Filtering in Rust avoids fragile pathspec-wildcard semantics; deeper
    // matches that slip in are written but never surfaced, because
    // `discover_skills` only scans depth-1 children of the materialized root.
    for rel_path in tree_paths(repo_root, reference).iter() {
        if !rel_path.ends_with("SKILL.md") {
            continue;
        }
        if !(rel_path.contains(".claude/skills/") || rel_path.contains(".agents/skills/")) {
            continue;
        }
        let spec = format!("{reference}:{rel_path}");
        let Ok(bytes) = run_git_bytes(repo_root, &["cat-file", "blob", &spec]) else {
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

    SkillsView {
        dir: temp.path().to_path_buf(),
        _temp: Some(temp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = phoenix_core::git::command()
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
        assert!(
            paths.contains(&"committed.txt"),
            "committed file should list: {paths:?}"
        );
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
            FileResolution::Binary | FileResolution::NotFound => {
                panic!("expected committed.txt to resolve as text")
            }
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
            FileResolution::Binary | FileResolution::NotFound => {
                panic!("working dir should see untracked file")
            }
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
        assert!(
            skill_md.is_file(),
            "SKILL.md should be materialized from the ref tree"
        );
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

    #[test]
    fn git_tree_read_file_rejects_directory_reference() {
        // A path-like reference to a directory (`@sub`) must NOT expand the git
        // tree listing as text — it resolves to NotFound, same as the
        // working-directory path where reading a dir fails.
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(repo.path().join("sub")).unwrap();
        std::fs::write(repo.path().join("sub/inner.txt"), "leaf").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "init"]);

        let root = ResolutionRoot::GitTree {
            repo_root: repo.path().to_path_buf(),
            reference: "main".to_string(),
        };
        assert!(
            matches!(root.read_file("sub"), FileResolution::NotFound),
            "a directory reference must be NotFound, not a tree listing"
        );
        match root.read_file("sub/inner.txt") {
            FileResolution::Text(t) => assert_eq!(t, "leaf"),
            FileResolution::Binary | FileResolution::NotFound => {
                panic!("the blob under the dir should still resolve as text")
            }
        }
    }

    #[test]
    fn for_create_falls_back_to_remote_tracking_ref() {
        // A remote-only branch (selected in the picker before its local tracking
        // branch is materialized) resolves via `origin/<branch>`.
        let upstream = TempDir::new().unwrap();
        git(upstream.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(upstream.path().join("a.txt"), "x").unwrap();
        git(upstream.path(), &["add", "."]);
        git(upstream.path(), &["commit", "-qm", "init"]);
        git(upstream.path(), &["branch", "feature"]);

        let clone = TempDir::new().unwrap();
        git(clone.path(), &["init", "-q", "-b", "main"]);
        git(
            clone.path(),
            &["remote", "add", "origin", upstream.path().to_str().unwrap()],
        );
        git(clone.path(), &["fetch", "-q", "origin"]);

        // `feature` exists only as refs/remotes/origin/feature in the clone.
        assert_eq!(
            crate::git_start::resolve_tree_ref_without_fetch(clone.path(), "feature").as_deref(),
            Some("origin/feature"),
            "remote-only branch should resolve via origin/"
        );
        assert!(
            crate::git_start::resolve_tree_ref_without_fetch(clone.path(), "does-not-exist")
                .is_none(),
            "an unresolvable branch yields None"
        );
    }

    #[test]
    fn resolve_tree_ref_prefers_remote_for_unpinned_branch_behind_origin() {
        // A branch that is behind origin and NOT checked out in any worktree is
        // fast-forwarded by creation, so discovery resolves to the remote tip.
        let upstream = TempDir::new().unwrap();
        git(upstream.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(upstream.path().join("a.txt"), "1").unwrap();
        git(upstream.path(), &["add", "."]);
        git(upstream.path(), &["commit", "-qm", "c1"]);
        git(upstream.path(), &["branch", "feature"]); // feature at c1

        let clone = TempDir::new().unwrap();
        git(
            clone.path(),
            &["clone", "-q", upstream.path().to_str().unwrap(), "."],
        );
        // Local `feature` at c1, tracking origin/feature; clone stays on `main`,
        // so `feature` is not checked out in any worktree.
        git(clone.path(), &["branch", "feature", "origin/feature"]);

        // Advance upstream `feature` to c2 and fetch: local feature now behind.
        git(upstream.path(), &["checkout", "-q", "feature"]);
        std::fs::write(upstream.path().join("b.txt"), "2").unwrap();
        git(upstream.path(), &["add", "."]);
        git(upstream.path(), &["commit", "-qm", "c2"]);
        git(clone.path(), &["fetch", "-q", "origin"]);

        assert_eq!(
            crate::git_start::resolve_tree_ref_without_fetch(clone.path(), "feature").as_deref(),
            Some("origin/feature"),
            "unpinned branch behind origin → creation FFs to the remote tip"
        );
    }

    #[test]
    fn resolve_tree_ref_keeps_checked_out_branch_even_when_behind_origin() {
        // The base branch is checked out in the clone's primary worktree, so
        // creation cannot fast-forward it — the worktree is built from the stale
        // local tip, and discovery must match that (managed mode builds a temp
        // branch from this ref). Mirrors materialize_branch's worktree exception.
        let upstream = TempDir::new().unwrap();
        git(upstream.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(upstream.path().join("a.txt"), "1").unwrap();
        git(upstream.path(), &["add", "."]);
        git(upstream.path(), &["commit", "-qm", "c1"]);

        let clone = TempDir::new().unwrap();
        git(
            clone.path(),
            &["clone", "-q", upstream.path().to_str().unwrap(), "."],
        );

        // Advance upstream main and fetch: local main is behind origin/main, but
        // main is checked out in the clone's primary worktree.
        std::fs::write(upstream.path().join("b.txt"), "2").unwrap();
        git(upstream.path(), &["add", "."]);
        git(upstream.path(), &["commit", "-qm", "c2"]);
        git(clone.path(), &["fetch", "-q", "origin"]);

        assert_eq!(
            crate::git_start::resolve_tree_ref_without_fetch(clone.path(), "main").as_deref(),
            Some("main"),
            "a checked-out branch can't be fast-forwarded, so creation keeps the local tip"
        );
    }

    #[test]
    fn git_tree_skills_view_materializes_child_project_skill() {
        // discover_skills scans immediate child dirs (e.g. service/.agents/skills);
        // the GitTree materialization must include those so the composer matches
        // create-time worktree discovery.
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(repo.path().join("service/.agents/skills/review")).unwrap();
        std::fs::write(
            repo.path().join("service/.agents/skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review\n---\n\nbody",
        )
        .unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "init"]);

        let root = ResolutionRoot::GitTree {
            repo_root: repo.path().to_path_buf(),
            reference: "main".to_string(),
        };
        let view = root.skills_view();
        assert!(
            view.dir
                .join("service/.agents/skills/review/SKILL.md")
                .is_file(),
            "child project skill must be materialized"
        );
    }
}
