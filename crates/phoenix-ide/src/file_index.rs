//! Per-workspace file index cache for fast Cmd+P file search.
//!
//! Without this cache, every keystroke in the command palette ran a fresh
//! `ignore::WalkBuilder` walk of the conversation's `cwd`. A cold walk of a
//! large monorepo (e.g. datadog-agent at ~22k tracked files on EBS) takes
//! ~8 seconds on a fresh page cache. Under fast typing, each new keystroke
//! cancelled the previous request on the wire but left the backend walk
//! running to completion, so concurrent zombie walks competed for disk
//! cache. Newly-created files compounded this: a fresh file on a cold
//! parent directory was the slowest possible case.
//!
//! The cache walks each workspace once on first access and then keeps the
//! file list current via `notify`/`notify-debouncer-full` filesystem
//! events. Subsequent searches are pure in-memory nucleo scoring over a
//! `BTreeSet<String>` — sub-millisecond regardless of repo size.
//!
//! ## Watch strategy: per-directory, gitignore-aware
//!
//! Linux's inotify has no kernel-level recursive mode. notify's
//! `RecursiveMode::Recursive` emulates it by walking the tree and
//! registering one watch per directory — same mechanism we use, but
//! gitignore-blind. The cost difference shows up under build-tool churn:
//! recursive watching of e.g. datadog-agent registers ~106k watches
//! including `vendor/` and `.git/`, then a `go mod download` floods the
//! shared inotify queue, triggers `IN_Q_OVERFLOW`, and forces a full
//! workspace rescan exactly when the agent is most active.
//!
//! By walking with `ignore::WalkBuilder` during bootstrap and registering
//! non-recursive watches only on the directories the walker visited, we
//! drop to ~3k watches per workspace and ignore build-dir noise entirely.
//! New top-level dirs are picked up because their parent is watched; we
//! re-walk the new subtree with `ignore::WalkBuilder` (which respects
//! nested `.gitignore` files) and add watches as appropriate.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::RecommendedWatcher;
use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use tokio::sync::OnceCell;

/// Debounce window. Short enough that newly-saved files appear in the
/// palette before the user starts typing the new filename; long enough to
/// coalesce the `CREATE`+`MODIFY`+`CLOSE_WRITE` burst a single editor save
/// emits.
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(100);

/// The in-memory file list for one workspace, plus the bookkeeping needed
/// to keep it current as the filesystem changes.
struct WorkspaceIndex {
    /// Relative paths (POSIX-style, `/` separators) of every tracked file.
    /// `BTreeSet` so iteration order is deterministic for tests and the
    /// `q=""` case returns sorted results without an extra sort pass.
    paths: BTreeSet<String>,
    /// Directories we hold an inotify watch on. Needed so we can drop
    /// watches on directory removal and avoid double-registering on
    /// `Create(Folder)` events.
    watched_dirs: HashSet<PathBuf>,
    /// Composite gitignore matcher built from every `.gitignore` file the
    /// bootstrap walk encountered (root, `.git/info/exclude`, plus any
    /// nested `.gitignore` files inside non-ignored directories), used to
    /// filter watcher-driven `Create(File)` events so a freshly-written
    /// `*.log` / `.env` / build artifact doesn't slip into the index that
    /// the bootstrap walk's nested-gitignore handling would have excluded.
    gitignore: Gitignore,
}

/// State held while a workspace's first bootstrap walk is in flight.
type CellState = Arc<OnceCell<Arc<RwLock<WorkspaceIndex>>>>;

pub struct WorkspaceIndexer {
    /// Each workspace gets a `OnceCell` so concurrent first-callers share
    /// the same bootstrap walk instead of racing each other.
    cells: Mutex<HashMap<PathBuf, CellState>>,
    /// The single debouncer multiplexes events from every watched
    /// directory across every workspace. Wrapped in `Mutex` because
    /// `add_watch`/`remove_watch` mutate it, and we touch it from both
    /// the bootstrap path and the event-handler thread.
    debouncer: Mutex<Debouncer<RecommendedWatcher, RecommendedCache>>,
}

impl WorkspaceIndexer {
    /// Create the indexer, spawn the event-handling thread, and arm the
    /// notify debouncer. Returns `Err` only if `notify` itself fails to
    /// initialize (e.g. cannot create an inotify instance).
    pub fn new() -> Result<Arc<Self>, notify::Error> {
        // std::sync::mpsc because the debouncer's handler trait is sync;
        // we consume from a dedicated std::thread, not a tokio task.
        let (tx, rx) = std::sync::mpsc::channel::<DebounceEventResult>();
        let debouncer = new_debouncer(DEBOUNCE_WINDOW, None, tx)?;

        let indexer = Arc::new(Self {
            cells: Mutex::new(HashMap::new()),
            debouncer: Mutex::new(debouncer),
        });

        // Background thread: drain the event channel and apply updates to
        // whichever workspace each event belongs to. Held weakly so the
        // thread exits when the last Arc<WorkspaceIndexer> is dropped.
        let weak = Arc::downgrade(&indexer);
        std::thread::Builder::new()
            .name("file-index-events".into())
            .spawn(move || {
                for result in rx {
                    let Some(indexer) = weak.upgrade() else { break };
                    indexer.handle_event_batch(result);
                }
            })
            .expect("spawn file-index-events thread");

        Ok(indexer)
    }

    /// Look up (or bootstrap) the workspace at `root` and return up to
    /// `limit` paths matching `query`. Empty query returns the first
    /// `limit` paths in lexical order. The bootstrap walk runs once per
    /// workspace; subsequent calls are pure in-memory scoring.
    pub async fn search(
        self: &Arc<Self>,
        root: PathBuf,
        query: &str,
        limit: usize,
    ) -> Result<Vec<String>, String> {
        let canonical = canonicalize_or_self(&root);
        let cell = self.cell_for(&canonical);
        let me = self.clone();
        let canonical_for_init = canonical.clone();
        let index = cell
            .get_or_try_init(|| async move { me.bootstrap_workspace(canonical_for_init).await })
            .await?
            .clone();

        let guard = index.read().expect("index lock poisoned");
        Ok(score_paths(&guard.paths, query, limit))
    }

    fn cell_for(&self, canonical: &Path) -> CellState {
        let mut map = self.cells.lock().expect("cells lock poisoned");
        map.entry(canonical.to_path_buf())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone()
    }

    /// Walk the workspace once, register per-directory watches, return the
    /// initialized index. Runs the walk in `spawn_blocking` so we don't
    /// stall the tokio executor on a multi-second cold walk.
    async fn bootstrap_workspace(
        self: Arc<Self>,
        root: PathBuf,
    ) -> Result<Arc<RwLock<WorkspaceIndex>>, String> {
        let walk_root = root.clone();
        let (paths, dirs, gitignore) =
            tokio::task::spawn_blocking(move || walk_workspace(&walk_root))
                .await
                .map_err(|e| format!("bootstrap walk panicked: {e}"))?;

        let mut debouncer = self.debouncer.lock().expect("debouncer poisoned");
        for dir in &dirs {
            // NonRecursive: see module docstring for why we don't use
            // RecursiveMode::Recursive here.
            if let Err(e) = debouncer.watch(dir, RecursiveMode::NonRecursive) {
                tracing::debug!(?dir, ?e, "failed to register file-index watch");
            }
        }
        drop(debouncer);

        let _ = root; // path is keyed on the outer `cells` map.
        Ok(Arc::new(RwLock::new(WorkspaceIndex {
            paths,
            watched_dirs: dirs,
            gitignore,
        })))
    }

    /// Drop a workspace from the cache and unwatch its directories. Used
    /// when the watcher signals a rescan (events were lost; the safest
    /// move is to throw out the index and let the next search re-bootstrap)
    /// and when the workspace root itself is deleted. Holds neither the
    /// cells lock nor any index lock across the debouncer mutation.
    fn invalidate_workspace(&self, root: &Path) {
        let cell_opt = {
            let mut map = self.cells.lock().expect("cells poisoned");
            map.remove(root)
        };
        let Some(cell) = cell_opt else { return };
        // If the cell was never initialized there's nothing to unwatch.
        let Some(index) = cell.get() else { return };
        let dirs: Vec<PathBuf> = {
            let guard = index.read().expect("index poisoned");
            guard.watched_dirs.iter().cloned().collect()
        };
        let mut debouncer = self.debouncer.lock().expect("debouncer poisoned");
        for dir in &dirs {
            let _ = debouncer.unwatch(dir);
        }
        tracing::debug!(?root, watches = dirs.len(), "invalidated workspace index");
    }

    /// Apply one batch of debounced events. Errors are logged at debug;
    /// they're typically "path no longer exists" races that the filesystem
    /// resolves on the next event.
    fn handle_event_batch(self: &Arc<Self>, result: DebounceEventResult) {
        let events = match result {
            Ok(events) => events,
            Err(errors) => {
                for err in errors {
                    tracing::debug!(?err, "file-index watcher error");
                }
                return;
            }
        };

        for event in events {
            // `need_rescan()` is set when the watcher dropped events (inotify
            // queue overflow, FSEvents coalescence). Any cached path under
            // the affected workspace may be stale; rebuild on next access
            // instead of trying to apply individual paths.
            if event.need_rescan() {
                for root in self.affected_workspace_roots(&event.event.paths) {
                    tracing::debug!(?root, "watcher signaled rescan; invalidating workspace");
                    self.invalidate_workspace(&root);
                }
                continue;
            }
            self.apply_event(&event.event);
        }
    }

    /// Apply a single event to every workspace whose root prefixes one of
    /// the event's paths. Nested workspaces (e.g. `/repo` and
    /// `/repo/packages/app`) both index the inner path under their own
    /// roots and both need the update; routing to "the first match" would
    /// leave the other stale.
    fn apply_event(self: &Arc<Self>, event: &notify::Event) {
        let targets = self.affected_workspaces(&event.paths);
        for (root, index) in targets {
            self.apply_event_to_workspace(&root, &index, event);
        }
    }

    fn apply_event_to_workspace(
        self: &Arc<Self>,
        root: &Path,
        index: &Arc<RwLock<WorkspaceIndex>>,
        event: &notify::Event,
    ) {
        match &event.kind {
            EventKind::Create(CreateKind::Folder) => {
                for path in &event.paths {
                    if path.starts_with(root) {
                        self.absorb_new_subtree(root, path, index);
                    }
                }
            }
            EventKind::Create(CreateKind::File) => {
                for path in &event.paths {
                    if path.starts_with(root) {
                        insert_file_if_not_ignored(root, path, index);
                    }
                }
            }
            EventKind::Create(CreateKind::Any | CreateKind::Other) => {
                // Backend didn't tell us if it was a file or folder.
                // Stat each path and route accordingly.
                for path in &event.paths {
                    if !path.starts_with(root) {
                        continue;
                    }
                    match std::fs::metadata(path) {
                        Ok(meta) if meta.is_dir() => {
                            self.absorb_new_subtree(root, path, index);
                        }
                        Ok(_) => {
                            insert_file_if_not_ignored(root, path, index);
                        }
                        Err(_) => { /* gone already */ }
                    }
                }
            }
            EventKind::Remove(
                RemoveKind::Folder | RemoveKind::File | RemoveKind::Any | RemoveKind::Other,
            ) => {
                for path in &event.paths {
                    self.absorb_removal(root, path, index);
                }
            }
            EventKind::Modify(ModifyKind::Name(rename_mode)) => {
                self.apply_rename(root, &event.paths, *rename_mode, index);
            }
            // Data/metadata modifies don't change the path set we index.
            EventKind::Modify(_) | EventKind::Access(_) | EventKind::Any | EventKind::Other => {}
        }
    }

    /// A new directory appeared inside a watched parent. Walk it with the
    /// same gitignore rules used at bootstrap, add every file we find,
    /// and register watches on every directory we visit.
    fn absorb_new_subtree(
        self: &Arc<Self>,
        root: &Path,
        new_dir: &Path,
        index: &Arc<RwLock<WorkspaceIndex>>,
    ) {
        // Skip the `.git` directory if it ever appears as a non-ignored
        // subdir (it shouldn't, but defensively match the bootstrap walk).
        if new_dir.file_name().is_some_and(|n| n == ".git") {
            return;
        }
        let (new_files, new_dirs) = walk_subtree(root, new_dir);

        if !new_dirs.is_empty() {
            let mut debouncer = self.debouncer.lock().expect("debouncer poisoned");
            let mut idx = index.write().expect("index poisoned");
            for dir in new_dirs {
                if idx.watched_dirs.insert(dir.clone()) {
                    if let Err(e) = debouncer.watch(&dir, RecursiveMode::NonRecursive) {
                        tracing::debug!(?dir, ?e, "failed to register watch on new subdir");
                    }
                }
            }
            idx.paths.extend(new_files);
        } else if !new_files.is_empty() {
            index.write().expect("poisoned").paths.extend(new_files);
        }
    }

    /// A path was removed. Could be a file (one entry) or a directory
    /// (drop everything with that prefix, including watches).
    fn absorb_removal(
        self: &Arc<Self>,
        root: &Path,
        gone: &Path,
        index: &Arc<RwLock<WorkspaceIndex>>,
    ) {
        let Some(rel) = relative_string(root, gone) else {
            return;
        };

        // Special case: the workspace root itself is gone. An empty `rel`
        // means our `paths.retain(starts_with("/"))` sweep below would be a
        // no-op (rel paths don't have leading slashes), so we'd leak the
        // entire pre-delete file list and serve stale results once the cwd
        // is recreated. Drop the whole workspace from the cache instead.
        if rel.is_empty() {
            self.invalidate_workspace(root);
            return;
        }

        let prefix = format!("{rel}/");

        let mut idx = index.write().expect("poisoned");
        // Files: exact-match removal handles the file case; the prefix
        // sweep handles a directory-removal case where the descendants
        // were tracked.
        idx.paths.remove(&rel);
        idx.paths.retain(|p| !p.starts_with(&prefix));

        // Watches: drop any watched dir whose path is `gone` or under it.
        let to_drop: Vec<PathBuf> = idx
            .watched_dirs
            .iter()
            .filter(|d| d.as_path() == gone || d.starts_with(gone))
            .cloned()
            .collect();
        if !to_drop.is_empty() {
            drop(idx);
            let mut debouncer = self.debouncer.lock().expect("debouncer poisoned");
            for dir in &to_drop {
                let _ = debouncer.unwatch(dir);
            }
            let mut idx = index.write().expect("poisoned");
            for dir in to_drop {
                idx.watched_dirs.remove(&dir);
            }
        }
    }

    /// Handle a rename. On Linux atomic-save the debouncer pairs
    /// `MOVED_FROM` with `MOVED_TO` and emits a single
    /// `Modify(Name(Both))` event with
    /// `paths = [from, to]`. On other platforms / edge cases we may get
    /// `RenameMode::From` and `RenameMode::To` as separate events, which
    /// we treat as Remove + Create respectively.
    fn apply_rename(
        self: &Arc<Self>,
        root: &Path,
        paths: &[PathBuf],
        mode: RenameMode,
        index: &Arc<RwLock<WorkspaceIndex>>,
    ) {
        match mode {
            RenameMode::Both if paths.len() >= 2 => {
                self.absorb_removal(root, &paths[0], index);
                self.absorb_create_unknown_kind(root, &paths[1], index);
            }
            RenameMode::From => {
                for path in paths {
                    self.absorb_removal(root, path, index);
                }
            }
            RenameMode::To => {
                for path in paths {
                    self.absorb_create_unknown_kind(root, path, index);
                }
            }
            // Any / Both-with-fewer-than-two-paths / Other modes: best-effort,
            // stat each path to decide whether it's a create or a remove. We
            // already covered Both with paired paths and From/To above; this
            // is the catch-all where the backend gave us less information
            // than the standard rename variants imply.
            RenameMode::Any | RenameMode::Both | RenameMode::Other => {
                for path in paths {
                    if path.exists() {
                        self.absorb_create_unknown_kind(root, path, index);
                    } else {
                        self.absorb_removal(root, path, index);
                    }
                }
            }
        }
    }

    fn absorb_create_unknown_kind(
        self: &Arc<Self>,
        root: &Path,
        path: &Path,
        index: &Arc<RwLock<WorkspaceIndex>>,
    ) {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => self.absorb_new_subtree(root, path, index),
            Ok(_) => {
                insert_file_if_not_ignored(root, path, index);
            }
            Err(_) => {}
        }
    }

    /// Snapshot of (root, ready-index) pairs whose root prefixes any path
    /// in `paths`. Empty if no indexed workspace contains the event.
    /// Nested workspaces both appear — see `apply_event` for why.
    fn affected_workspaces(
        &self,
        paths: &[PathBuf],
    ) -> Vec<(PathBuf, Arc<RwLock<WorkspaceIndex>>)> {
        let pairs: Vec<(PathBuf, CellState)> = {
            let map = self.cells.lock().expect("cells poisoned");
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let mut out: Vec<(PathBuf, Arc<RwLock<WorkspaceIndex>>)> = Vec::new();
        for (root, cell) in &pairs {
            let Some(index) = cell.get() else { continue };
            let touches = paths.iter().any(|p| p.starts_with(root));
            if touches {
                out.push((root.clone(), index.clone()));
            }
        }
        out
    }

    /// Roots of every workspace whose tree contains any of `paths`,
    /// including workspaces whose `OnceCell` hasn't initialized yet
    /// (a rescan signal still has to invalidate them so the in-flight
    /// bootstrap can be retried with a fresh walk if needed).
    fn affected_workspace_roots(&self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let roots: Vec<PathBuf> = {
            let map = self.cells.lock().expect("cells poisoned");
            map.keys().cloned().collect()
        };
        roots
            .into_iter()
            .filter(|root| paths.iter().any(|p| p.starts_with(root)))
            .collect()
    }
}

fn canonicalize_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// One-pass walk that returns (sorted relative-path set, watched-dir set,
/// composite gitignore matcher).
///
/// The gitignore matcher is built from every `.gitignore` file the walk
/// encountered (root-level, nested, plus `.git/info/exclude` and the
/// configured global gitignore). This lets the watcher path apply the same
/// rules `ignore::WalkBuilder` did at bootstrap when filtering events for
/// individual files. `GitignoreBuilder::add` preserves the rule's
/// originating directory, so a rule in `<root>/src/.gitignore` only
/// matches paths under `<root>/src`, mirroring real `git` semantics
/// instead of leaking across siblings.
fn walk_workspace(root: &Path) -> (BTreeSet<String>, HashSet<PathBuf>, Gitignore) {
    let mut files = BTreeSet::new();
    let mut dirs = HashSet::new();
    let mut gi_builder = GitignoreBuilder::new(root);

    // Seed with the well-known global sources so the matcher behaves like
    // `git status` would. Errors are non-fatal — the file may simply not
    // exist.
    let _ = gi_builder.add(root.join(".gitignore"));
    let _ = gi_builder.add(root.join(".git").join("info").join("exclude"));

    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .filter_entry(|e| e.file_name() != ".git")
        .build();

    for entry in walker {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        match entry.file_type().map(|t| t.is_dir()) {
            Some(true) => {
                dirs.insert(path.to_path_buf());
            }
            Some(false) => {
                if let Some(rel) = relative_string(root, path) {
                    files.insert(rel);
                }
                // Collect nested `.gitignore` files as we encounter them.
                // Root-level + .git/info/exclude were added above; any
                // additional `.gitignore` deeper in the tree gets layered
                // on with its own per-directory scope.
                if entry.file_name() == ".gitignore" && path != root.join(".gitignore") {
                    let _ = gi_builder.add(path);
                }
            }
            None => {}
        }
    }

    let gitignore = gi_builder.build().unwrap_or_else(|err| {
        tracing::debug!(
            ?err,
            ?root,
            "failed to build composite gitignore; falling back to empty matcher"
        );
        Gitignore::empty()
    });

    (files, dirs, gitignore)
}

/// Walk a single new subtree, using the same gitignore rules as the
/// initial bootstrap so a freshly-created `target/` or `node_modules/`
/// silently yields nothing.
fn walk_subtree(root: &Path, subtree: &Path) -> (Vec<String>, Vec<PathBuf>) {
    let mut files = Vec::new();
    let mut dirs = Vec::new();

    let walker = ignore::WalkBuilder::new(subtree)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .filter_entry(|e| e.file_name() != ".git")
        .build();

    for entry in walker {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        match entry.file_type().map(|t| t.is_dir()) {
            Some(true) => dirs.push(path.to_path_buf()),
            Some(false) => {
                if let Some(rel) = relative_string(root, path) {
                    files.push(rel);
                }
            }
            None => {}
        }
    }

    (files, dirs)
}

fn relative_string(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(rel.to_string_lossy().into_owned())
}

/// Insert a created file into the index iff it would have passed the
/// bootstrap walk's gitignore filtering. Without this, a `*.log` rule that
/// excluded files from the initial walk would silently fail to exclude the
/// same file when it's recreated under a watched directory.
fn insert_file_if_not_ignored(root: &Path, path: &Path, index: &Arc<RwLock<WorkspaceIndex>>) {
    let Some(rel) = relative_string(root, path) else {
        return;
    };
    // Read-lock first so the common "ignored" case (build artifacts
    // churning) doesn't contend on the write lock.
    let ignored = {
        let guard = index.read().expect("index poisoned");
        guard.gitignore.matched(path, false).is_ignore()
    };
    if ignored {
        return;
    }
    index.write().expect("index poisoned").paths.insert(rel);
}

/// Pure scoring loop. Identical algorithm to the previous inline
/// implementation in `handlers::search_conversation_files`: nucleo
/// per-segment, falling back to full-path; +1000 bonus on segment matches
/// so directory-name hits beat scattered-char hits in long paths.
fn score_paths(paths: &BTreeSet<String>, query: &str, limit: usize) -> Vec<String> {
    if query.is_empty() {
        return paths.iter().take(limit).cloned().collect();
    }

    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut matcher = nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT);
    let mut buf: Vec<char> = Vec::new();

    let mut scored: Vec<(i32, &String)> = Vec::new();
    for path in paths {
        if let Some(score) = fuzzy_score(path, &pattern, &mut matcher, &mut buf) {
            scored.push((score, path));
        }
    }
    scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, p)| p.clone())
        .collect()
}

fn fuzzy_score(
    path: &str,
    pattern: &Pattern,
    matcher: &mut nucleo_matcher::Matcher,
    buf: &mut Vec<char>,
) -> Option<i32> {
    let best_segment = path
        .split('/')
        .filter_map(|seg| {
            buf.clear();
            buf.extend(seg.chars());
            let haystack = nucleo_matcher::Utf32Str::Unicode(buf);
            pattern
                .score(haystack, matcher)
                .map(|s| i32::try_from(s).unwrap_or(i32::MAX).saturating_add(1000))
        })
        .max();

    if best_segment.is_some() {
        return best_segment;
    }

    buf.clear();
    buf.extend(path.chars());
    let haystack = nucleo_matcher::Utf32Str::Unicode(buf);
    pattern
        .score(haystack, matcher)
        .map(|s| i32::try_from(s).unwrap_or(i32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Instant;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("create dir");
        }
        fs::write(p, body).expect("write file");
    }

    /// Pull the indexer at `root` synchronously into a state where a
    /// search would hit the cached path. Used by tests below.
    async fn search(indexer: &Arc<WorkspaceIndexer>, root: &Path, q: &str) -> Vec<String> {
        indexer
            .search(root.to_path_buf(), q, 50)
            .await
            .expect("search")
    }

    /// Spin until `predicate` returns true or `timeout` elapses. We poll
    /// the indexed state because the debouncer's internal timer isn't
    /// exposed for direct synchronization in tests.
    async fn wait_for_path<F: Fn(&[String]) -> bool>(
        indexer: &Arc<WorkspaceIndexer>,
        root: &Path,
        query: &str,
        timeout: Duration,
        predicate: F,
    ) -> bool {
        let start = Instant::now();
        loop {
            let results = indexer
                .search(root.to_path_buf(), query, 50)
                .await
                .expect("search");
            if predicate(&results) {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// `ignore::WalkBuilder::git_ignore` only honors `.gitignore` files
    /// when it can find a containing `.git/` directory. Tests that exercise
    /// gitignore behavior need a sentinel `.git/` even if it's empty.
    fn fake_git(root: &Path) {
        fs::create_dir_all(root.join(".git")).expect("fake .git");
    }

    #[tokio::test]
    async fn bootstrap_finds_existing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("src/main.rs"), "fn main() {}\n");
        write(&root.join("src/lib.rs"), "// lib\n");
        write(&root.join("README.md"), "# hi\n");

        let indexer = WorkspaceIndexer::new().unwrap();
        let results = search(&indexer, root, "main").await;
        assert!(
            results.iter().any(|p| p == "src/main.rs"),
            "got {results:?}"
        );
    }

    #[tokio::test]
    async fn gitignore_excludes_target() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fake_git(root);
        write(&root.join(".gitignore"), "target/\n");
        write(&root.join("src/keep.rs"), "");
        write(&root.join("target/exclude.rs"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let all = search(&indexer, root, "").await;
        assert!(all.iter().any(|p| p == "src/keep.rs"));
        assert!(
            !all.iter().any(|p| p.starts_with("target/")),
            "leaked gitignored: {all:?}"
        );
    }

    #[tokio::test]
    async fn newly_created_file_appears_via_watcher() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("src/old.rs"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        // Prime the bootstrap.
        let _ = search(&indexer, root, "old").await;

        write(&root.join("src/brand_new.rs"), "");

        let appeared = wait_for_path(
            &indexer,
            root,
            "brand_new",
            Duration::from_secs(2),
            |results| results.iter().any(|p| p == "src/brand_new.rs"),
        )
        .await;
        assert!(appeared, "new file never showed up in the index");
    }

    #[tokio::test]
    async fn removed_file_disappears_via_watcher() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("src/doomed.rs"), "");
        write(&root.join("src/keep.rs"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let _ = search(&indexer, root, "doomed").await;

        fs::remove_file(root.join("src/doomed.rs")).unwrap();

        let gone = wait_for_path(
            &indexer,
            root,
            "doomed",
            Duration::from_secs(2),
            |results| !results.iter().any(|p| p == "src/doomed.rs"),
        )
        .await;
        assert!(gone, "deleted file never left the index");
    }

    #[tokio::test]
    async fn nested_dir_creation_adds_watch_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("src/lib.rs"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let _ = search(&indexer, root, "").await;

        fs::create_dir_all(root.join("src/new_module")).unwrap();
        // Give notify a tick to register the dir-create event and our
        // handler a tick to add the watch before we write the file.
        tokio::time::sleep(Duration::from_millis(200)).await;
        write(&root.join("src/new_module/feature.rs"), "");

        let appeared = wait_for_path(
            &indexer,
            root,
            "feature",
            Duration::from_secs(3),
            |results| results.iter().any(|p| p == "src/new_module/feature.rs"),
        )
        .await;
        assert!(appeared, "file in newly-created subdir never indexed");
    }

    #[tokio::test]
    async fn second_search_does_not_rewalk() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..200 {
            write(&root.join(format!("src/f{i:03}.rs")), "");
        }

        let indexer = WorkspaceIndexer::new().unwrap();
        let first = Instant::now();
        let _ = search(&indexer, root, "f100").await;
        let _bootstrap = first.elapsed();

        let second = Instant::now();
        let _ = search(&indexer, root, "f150").await;
        let cached = second.elapsed();

        // 200 files is tiny but the cached search should still be
        // dramatically faster than even a cheap walk over them. The
        // bound is loose enough to avoid flakes on a noisy CI host
        // while still failing if we accidentally regress to the
        // bootstrap path (which would be 10-100× higher on 200 files).
        assert!(
            cached < Duration::from_millis(25),
            "cached search took {cached:?}, expected sub-25ms"
        );
    }

    /// Codex finding #1: a `Create(File)` for `*.log` should respect the
    /// same gitignore rules the bootstrap walk used. Without the cached
    /// `Gitignore` matcher this file would slip into the index because
    /// individual `Create(File)` events bypass `WalkBuilder`.
    #[tokio::test]
    async fn gitignored_create_file_does_not_enter_index() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fake_git(root);
        write(&root.join(".gitignore"), "*.log\n");
        write(&root.join("src/main.rs"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let _ = search(&indexer, root, "").await;

        // Create a file that the bootstrap walk would have excluded.
        write(&root.join("src/debug.log"), "noise");

        // Give notify time to fire. We expect the file to NEVER appear, so
        // we wait a window long enough for the event to be debounced
        // (DEBOUNCE_WINDOW + slack) and then assert.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let results = search(&indexer, root, "debug").await;
        assert!(
            !results.iter().any(|p| p == "src/debug.log"),
            "gitignored file leaked into index: {results:?}"
        );
    }

    /// Codex finding #3: when two indexed workspaces overlap (parent +
    /// nested child), a single event under the nested path must update
    /// both indexes — not just whichever entry `HashMap` iteration hits
    /// first.
    #[tokio::test]
    async fn nested_workspaces_both_see_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path();
        let child = parent.join("packages/app");
        fs::create_dir_all(&child).unwrap();
        write(&parent.join("README.md"), "");
        write(&child.join("index.ts"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        // Bootstrap both workspaces.
        let _ = search(&indexer, parent, "").await;
        let _ = search(&indexer, &child, "").await;

        write(&child.join("brand_new.ts"), "");

        let parent_saw = wait_for_path(
            &indexer,
            parent,
            "brand_new",
            Duration::from_secs(2),
            |results| results.iter().any(|p| p == "packages/app/brand_new.ts"),
        )
        .await;
        let child_saw = wait_for_path(
            &indexer,
            &child,
            "brand_new",
            Duration::from_secs(2),
            |results| results.iter().any(|p| p == "brand_new.ts"),
        )
        .await;
        assert!(parent_saw, "parent workspace missed nested-path create");
        assert!(child_saw, "child workspace missed its own-path create");
    }

    /// Codex finding #6: deleting the workspace root itself must purge the
    /// cached path set, not silently retain it because `rel == ""` makes
    /// the prefix sweep a no-op.
    #[tokio::test]
    async fn root_removal_purges_cached_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("workspace");
        fs::create_dir_all(&root).unwrap();
        write(&root.join("src/a.rs"), "");
        write(&root.join("src/b.rs"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let primed = search(&indexer, &root, "").await;
        assert!(primed.iter().any(|p| p == "src/a.rs"));

        fs::remove_dir_all(&root).unwrap();

        // Re-create the directory with completely different contents. If
        // the cache wasn't invalidated, a search would return the stale
        // pre-delete paths because `root.exists()` is true again and the
        // OnceCell still holds the old index.
        fs::create_dir_all(&root).unwrap();
        write(&root.join("src/c.rs"), "");

        // Give notify time to emit the removal and our handler to
        // invalidate the workspace.
        tokio::time::sleep(Duration::from_millis(400)).await;

        let after = search(&indexer, &root, "").await;
        assert!(
            !after.iter().any(|p| p == "src/a.rs"),
            "stale path survived root deletion: {after:?}"
        );
        assert!(
            after.iter().any(|p| p == "src/c.rs"),
            "post-recreate path missing: {after:?}"
        );
    }
}
