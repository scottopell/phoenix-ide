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

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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
    let Ok(temp) = TempDir::new() else {
        return SkillsView {
            dir: repo_root.to_path_buf(),
            _temp: None,
        };
    };

    let tree = tree_index(repo_root, reference);
    let symlinks = read_symlink_targets(repo_root, &tree);
    let mut catalog_roots = vec![".claude/skills".to_string(), ".agents/skills".to_string()];
    catalog_roots.extend(tree.keys().filter_map(|path| {
        let top = validated_tree_components(path)?.first()?.to_string();
        Some(format!("{top}/.claude/skills"))
    }));
    catalog_roots.extend(tree.keys().filter_map(|path| {
        let top = validated_tree_components(path)?.first()?.to_string();
        Some(format!("{top}/.agents/skills"))
    }));
    catalog_roots.sort();
    catalog_roots.dedup();

    let mut visited = HashSet::new();
    for logical_root in catalog_roots {
        let Some(physical_root) = resolve_tree_link_chain(&logical_root, &symlinks) else {
            continue;
        };
        materialize_skill_catalog(
            repo_root,
            reference,
            &tree,
            &symlinks,
            temp.path(),
            &logical_root,
            &physical_root,
            &mut visited,
        );
    }

    SkillsView {
        dir: temp.path().to_path_buf(),
        _temp: Some(temp),
    }
}

#[allow(clippy::too_many_arguments)]
fn materialize_skill_catalog(
    repo_root: &Path,
    reference: &str,
    tree: &HashMap<String, TreeEntry>,
    symlinks: &HashMap<String, String>,
    destination_root: &Path,
    logical_root: &str,
    physical_root: &str,
    visited: &mut HashSet<(String, String)>,
) {
    if !visited.insert((logical_root.to_string(), physical_root.to_string())) {
        return;
    }
    for child in tree_children(tree, physical_root) {
        let logical_skill = format!("{logical_root}/{child}");
        let physical_skill = format!("{physical_root}/{child}");
        let Some(physical_skill) = resolve_tree_link_chain(&physical_skill, symlinks) else {
            continue;
        };
        let metadata = format!("{physical_skill}/SKILL.md");
        let Some(metadata) = resolve_tree_link_chain(&metadata, symlinks) else {
            continue;
        };
        if tree.get(&metadata).is_some_and(|entry| !entry.symlink) {
            copy_tree_blob(
                repo_root,
                reference,
                &metadata,
                destination_root,
                &format!("{logical_skill}/SKILL.md"),
            );
            let logical_children = format!("{logical_skill}/skills");
            let physical_children = format!("{physical_skill}/skills");
            if let Some(physical_children) = resolve_tree_link_chain(&physical_children, symlinks) {
                materialize_skill_catalog(
                    repo_root,
                    reference,
                    tree,
                    symlinks,
                    destination_root,
                    &logical_children,
                    &physical_children,
                    visited,
                );
            }
        }
    }
}

fn tree_children(tree: &HashMap<String, TreeEntry>, directory: &str) -> Vec<String> {
    let prefix = format!("{directory}/");
    let mut children: Vec<String> = tree
        .keys()
        .filter_map(|path| path.strip_prefix(&prefix)?.split('/').next())
        .filter(|child| !child.is_empty())
        .map(str::to_string)
        .collect();
    children.sort();
    children.dedup();
    children
}

fn copy_tree_blob(
    repo_root: &Path,
    reference: &str,
    source: &str,
    destination_root: &Path,
    destination: &str,
) {
    if validated_tree_components(source).is_none() {
        return;
    }
    let Some(destination) = safe_tree_destination(destination_root, destination) else {
        return;
    };
    let spec = format!("{reference}:{source}");
    let Ok(bytes) = run_git_bytes(repo_root, &["cat-file", "blob", &spec]) else {
        return;
    };
    let Some(parent) = destination.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_ok() {
        let _ = std::fs::write(destination, bytes);
    }
}

#[derive(Clone)]
struct TreeEntry {
    oid: String,
    symlink: bool,
}

fn tree_index(repo_root: &Path, reference: &str) -> HashMap<String, TreeEntry> {
    let Ok(listing) = run_git_bytes(repo_root, &["ls-tree", "-rz", "--full-tree", reference])
    else {
        return HashMap::new();
    };
    listing
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let record = std::str::from_utf8(record).ok()?;
            let (metadata, path) = record.split_once('\t')?;
            validated_tree_components(path)?;
            let mut fields = metadata.split_whitespace();
            let mode = fields.next()?;
            fields.next()?;
            let oid = fields.next()?.to_string();
            Some((
                path.to_string(),
                TreeEntry {
                    oid,
                    symlink: mode == "120000",
                },
            ))
        })
        .collect()
}

fn read_symlink_targets(
    repo_root: &Path,
    tree: &HashMap<String, TreeEntry>,
) -> HashMap<String, String> {
    let links: Vec<(&str, &str)> = tree
        .iter()
        .filter(|(_, entry)| entry.symlink)
        .map(|(path, entry)| (path.as_str(), entry.oid.as_str()))
        .collect();
    if links.is_empty() {
        return HashMap::new();
    }

    let Ok(mut child) = phoenix_core::git::command()
        .current_dir(repo_root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return HashMap::new();
    };
    let Some(mut stdin) = child.stdin.take() else {
        return HashMap::new();
    };
    let requests = links
        .iter()
        .map(|(_, oid)| *oid)
        .collect::<Vec<_>>()
        .join("\n");
    let writer = std::thread::spawn(move || writeln!(stdin, "{requests}").is_ok());
    let Ok(output) = child.wait_with_output() else {
        return HashMap::new();
    };
    if !writer.join().unwrap_or(false) {
        return HashMap::new();
    }
    if !output.status.success() {
        return HashMap::new();
    }

    parse_batch_blobs(&output.stdout, &links)
}

fn parse_batch_blobs(bytes: &[u8], links: &[(&str, &str)]) -> HashMap<String, String> {
    let mut cursor = 0;
    let mut targets = HashMap::new();
    for (path, _) in links {
        let Some(header_end) = bytes[cursor..].iter().position(|byte| *byte == b'\n') else {
            break;
        };
        let header_end = cursor + header_end;
        let Some(size) = std::str::from_utf8(&bytes[cursor..header_end])
            .ok()
            .and_then(|header| header.split_whitespace().nth(2))
            .and_then(|size| size.parse::<usize>().ok())
        else {
            break;
        };
        let content_start = header_end + 1;
        let content_end = content_start.saturating_add(size);
        if content_end >= bytes.len() {
            break;
        }
        if let Ok(target) = std::str::from_utf8(&bytes[content_start..content_end]) {
            targets.insert((*path).to_string(), target.to_string());
        }
        cursor = content_end + 1;
    }
    targets
}

fn validated_tree_components(path: &str) -> Option<Vec<&str>> {
    if path.is_empty() || path.starts_with('/') || path.contains(['\\', '\0']) {
        return None;
    }
    let components: Vec<&str> = path.split('/').collect();
    components
        .iter()
        .all(|part| !part.is_empty() && !matches!(*part, "." | ".."))
        .then_some(components)
}

fn safe_tree_destination(root: &Path, path: &str) -> Option<PathBuf> {
    let mut destination = root.to_path_buf();
    for component in validated_tree_components(path)? {
        destination.push(component);
    }
    Some(destination)
}

fn resolve_tree_link_chain(link_path: &str, symlinks: &HashMap<String, String>) -> Option<String> {
    let mut path = link_path.to_string();
    let mut visited = HashSet::new();
    loop {
        let components = validated_tree_components(&path)?;
        let Some((prefix_len, link, target)) = (1..=components.len()).find_map(|len| {
            let prefix = components[..len].join("/");
            symlinks
                .get(&prefix)
                .map(|target| (len, prefix, target.as_str()))
        }) else {
            return Some(path);
        };
        if !visited.insert(link.clone()) {
            return None;
        }
        let resolved = resolve_tree_link(&link, target)?;
        path = std::iter::once(resolved.as_str())
            .chain(components[prefix_len..].iter().copied())
            .collect::<Vec<_>>()
            .join("/");
    }
}

fn resolve_tree_link(link_path: &str, target: &str) -> Option<String> {
    let parent = Path::new(link_path).parent()?;
    let mut resolved = Vec::new();
    for component in parent.join(target).components() {
        match component {
            std::path::Component::Normal(part) => resolved.push(part.to_str()?.to_string()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop()?;
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return None,
        }
    }
    (!resolved.is_empty()).then(|| resolved.join("/"))
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

    fn symlink_dir(target: &str, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(target, link).unwrap();
    }

    fn assert_tree_skills_match_checkout(repo: &Path, expected: &str) {
        let checkout_names: Vec<String> = crate::system_prompt::discover_skills(repo)
            .into_iter()
            .filter(|skill| skill.name == expected)
            .map(|skill| skill.name)
            .collect();
        let root = ResolutionRoot::git_tree(repo, "main");
        let view = root.skills_view();
        let tree_names: Vec<String> = crate::system_prompt::discover_skills(&view.dir)
            .into_iter()
            .filter(|skill| skill.name == expected)
            .map(|skill| skill.name)
            .collect();
        assert_eq!(tree_names, checkout_names);
        assert_eq!(tree_names, [expected]);
    }

    #[test]
    fn git_tree_skills_view_matches_checkout_for_repo_relative_symlinked_skill() {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(repo.path().join("skills/phoenix-development")).unwrap();
        std::fs::write(
            repo.path().join("skills/phoenix-development/SKILL.md"),
            "---\nname: phoenix-development\ndescription: Develop Phoenix\n---\n\nbody",
        )
        .unwrap();
        std::fs::create_dir_all(repo.path().join("skills/phoenix-development/skills/review"))
            .unwrap();
        std::fs::write(
            repo.path()
                .join("skills/phoenix-development/skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review Phoenix\n---\n\nbody",
        )
        .unwrap();
        std::fs::create_dir_all(repo.path().join(".agents/skills")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            "../../skills/phoenix-development",
            repo.path().join(".agents/skills/phoenix-development"),
        )
        .unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(
            "../../skills/phoenix-development",
            repo.path().join(".agents/skills/phoenix-development"),
        )
        .unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "init"]);

        let checkout_names: Vec<String> = crate::system_prompt::discover_skills(repo.path())
            .into_iter()
            .filter(|skill| skill.name.starts_with("phoenix-development"))
            .map(|skill| skill.name)
            .collect();
        let root = ResolutionRoot::git_tree(repo.path(), "main");
        let view = root.skills_view();
        let tree_names: Vec<String> = crate::system_prompt::discover_skills(&view.dir)
            .into_iter()
            .filter(|skill| skill.name.starts_with("phoenix-development"))
            .map(|skill| skill.name)
            .collect();

        assert_eq!(tree_names, checkout_names);
        assert_eq!(
            tree_names,
            ["phoenix-development", "phoenix-development:review"]
        );
    }

    #[test]
    fn git_tree_skills_view_matches_checkout_for_symlinked_catalog_root() {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
        std::fs::create_dir_all(repo.path().join("skills/review")).unwrap();
        std::fs::write(
            repo.path().join("skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review\n---\n\nbody",
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("../skills", repo.path().join(".agents/skills")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir("../skills", repo.path().join(".agents/skills")).unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "init"]);

        let checkout_names: Vec<String> = crate::system_prompt::discover_skills(repo.path())
            .into_iter()
            .filter(|skill| skill.name == "review")
            .map(|skill| skill.name)
            .collect();
        let root = ResolutionRoot::git_tree(repo.path(), "main");
        let view = root.skills_view();
        let tree_names: Vec<String> = crate::system_prompt::discover_skills(&view.dir)
            .into_iter()
            .filter(|skill| skill.name == "review")
            .map(|skill| skill.name)
            .collect();

        assert_eq!(tree_names, checkout_names);
        assert_eq!(tree_names, ["review"]);
    }

    #[test]
    fn git_tree_skills_view_follows_symlinked_entries_under_catalog_link() {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
        std::fs::create_dir_all(repo.path().join("catalog")).unwrap();
        std::fs::create_dir_all(repo.path().join("real/review")).unwrap();
        std::fs::write(
            repo.path().join("real/review/SKILL.md"),
            "---\nname: review\ndescription: Review\n---\n\nbody",
        )
        .unwrap();
        symlink_dir("../catalog", &repo.path().join(".agents/skills"));
        symlink_dir("../real/review", &repo.path().join("catalog/review"));
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "init"]);

        assert_tree_skills_match_checkout(repo.path(), "review");
    }

    #[test]
    fn git_tree_skills_view_follows_symlinked_catalog_parent() {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(repo.path().join("agent-config/skills/review")).unwrap();
        std::fs::write(
            repo.path().join("agent-config/skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review\n---\n\nbody",
        )
        .unwrap();
        symlink_dir("agent-config", &repo.path().join(".agents"));
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "init"]);

        assert_tree_skills_match_checkout(repo.path(), "review");
    }

    #[test]
    fn tree_link_resolution_rejects_paths_outside_committed_tree() {
        assert_eq!(
            resolve_tree_link(
                ".agents/skills/phoenix-development",
                "../../skills/phoenix-development"
            ),
            Some("skills/phoenix-development".to_string())
        );
        assert_eq!(
            resolve_tree_link(".agents/skills/escape", "../../../outside"),
            None
        );
        assert_eq!(resolve_tree_link(".agents/skills/escape", "/outside"), None);

        let chained = HashMap::from([
            (
                ".agents/skills/phoenix-development".to_string(),
                "../../aliases/development/current".to_string(),
            ),
            (
                "aliases/development".to_string(),
                "../skills/phoenix-development".to_string(),
            ),
        ]);
        assert_eq!(
            resolve_tree_link_chain(".agents/skills/phoenix-development", &chained),
            Some("skills/phoenix-development/current".to_string())
        );
        let cyclic = HashMap::from([
            (".agents/skills/a".to_string(), "b".to_string()),
            (".agents/skills/b".to_string(), "a".to_string()),
        ]);
        assert_eq!(resolve_tree_link_chain(".agents/skills/a", &cyclic), None);

        let intermediate_cycle = HashMap::from([
            (
                ".agents/skills/a".to_string(),
                "../../aliases/a/current".to_string(),
            ),
            ("aliases/a".to_string(), "b".to_string()),
            ("aliases/b".to_string(), "a".to_string()),
        ]);
        assert_eq!(
            resolve_tree_link_chain(".agents/skills/a", &intermediate_cycle),
            None
        );
    }

    #[test]
    fn tree_destinations_reject_non_posix_and_traversing_paths() {
        let root = Path::new("/safe/root");
        assert_eq!(
            safe_tree_destination(root, ".agents/skills/review/SKILL.md"),
            Some(root.join(".agents/skills/review/SKILL.md"))
        );
        for unsafe_path in [
            "../outside/SKILL.md",
            ".agents/../outside/SKILL.md",
            ".agents//skills/review/SKILL.md",
            ".agents/./skills/review/SKILL.md",
            "/outside/SKILL.md",
            "C:\\outside\\SKILL.md",
            ".agents\\skills\\review\\SKILL.md",
            ".agents/skills/review/SKILL.md\0outside",
        ] {
            assert_eq!(
                safe_tree_destination(root, unsafe_path),
                None,
                "unsafe committed-tree path must be rejected: {unsafe_path:?}"
            );
        }
    }

    #[test]
    fn git_tree_skills_view_ignores_broken_and_escaping_symlinks() {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(repo.path().join(".agents/skills")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("../../missing", repo.path().join(".agents/skills/broken"))
                .unwrap();
            std::os::unix::fs::symlink(
                "../../../outside",
                repo.path().join(".agents/skills/escape"),
            )
            .unwrap();
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(
                "../../missing",
                repo.path().join(".agents/skills/broken"),
            )
            .unwrap();
            std::os::windows::fs::symlink_dir(
                "../../../outside",
                repo.path().join(".agents/skills/escape"),
            )
            .unwrap();
        }
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "init"]);

        let root = ResolutionRoot::git_tree(repo.path(), "main");
        let view = root.skills_view();
        assert!(!view.dir.join(".agents/skills/broken").exists());
        assert!(!view.dir.join(".agents/skills/escape").exists());
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
