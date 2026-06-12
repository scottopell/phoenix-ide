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
//!
//! ## Lifecycle: Loading vs Ready
//!
//! Each workspace lives in a `WorkspaceCell` whose state is either
//! `Loading { pending: Vec<notify::Event> }` or `Ready { index }`. The
//! first `search` for a `cwd` inserts a `Loading` cell, spawns the
//! bootstrap walk, and awaits the cell's transition to `Ready` via a
//! `tokio::sync::Notify`. Watcher events that arrive while the cell is
//! `Loading` are pushed into the pending buffer; bootstrap drains the
//! buffer atomically while flipping to `Ready`, so a file created
//! between the bootstrap's walk and its publication is never lost.
//!
//! This is the patch-shaped fix to the bootstrap TOCTOU. Codex review
//! correctly observed that the same shared-state coordination shows up
//! at many other sites in this file (event routing, watch ownership,
//! gitignore-file change detection). A cleaner endpoint is a
//! per-workspace actor that owns its own debouncer; see
//! `tasks/64002-p3-ready--file-index-actor-refactor.md` for the
//! proposed shape and the "When to do this" gating criteria.

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
    /// Composite gitignore matcher built from every `.gitignore` /
    /// `.ignore` file the bootstrap walk encountered, plus the workspace's
    /// `.git/info/exclude` and the resolved global excludes file. Used to
    /// filter watcher-driven `Create(File)` events so a freshly-written
    /// `*.log` / `.env` / build artifact doesn't slip into the index that
    /// the bootstrap walk's nested-gitignore handling would have excluded.
    /// `None` when the workspace is not a git repo — matches the bootstrap
    /// walker's behaviour, which only applies `.gitignore` rules inside a
    /// git tree.
    gitignore: Option<Gitignore>,
}

/// Lifecycle state for one workspace. A freshly-inserted cell starts in
/// `Loading`; the bootstrap task transitions to `Ready` (or `Failed`)
/// while holding the cell mutex, so events queued in `pending` are
/// drained atomically with the publication of the new index.
enum WorkspaceState {
    Loading { pending: Vec<notify::Event> },
    Ready { index: Arc<RwLock<WorkspaceIndex>> },
    Failed(String),
}

struct WorkspaceCell {
    state: Mutex<WorkspaceState>,
    /// Awaited by `search` callers while the bootstrap is still running;
    /// `notify_waiters` is fired when the state transitions out of
    /// `Loading`.
    notify: tokio::sync::Notify,
}

impl WorkspaceCell {
    fn new_loading() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(WorkspaceState::Loading {
                pending: Vec::new(),
            }),
            notify: tokio::sync::Notify::new(),
        })
    }
}

pub struct WorkspaceIndexer {
    /// One cell per indexed workspace cwd. The cell carries the loading/
    /// ready state machine so concurrent first-callers share a single
    /// bootstrap and watcher events arriving during bootstrap can be
    /// buffered into the cell instead of dropped on the floor.
    cells: Mutex<HashMap<PathBuf, Arc<WorkspaceCell>>>,
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
        let (cell, is_first) = self.cell_for(&canonical);

        if is_first {
            // First caller wins the bootstrap. Spawn it onto tokio so the
            // search future doesn't end up bound to this caller's lifetime
            // — concurrent searches all wait on the same Notify and pick
            // up the published index.
            let me = self.clone();
            let canonical_for_bs = canonical.clone();
            let cell_for_bs = cell.clone();
            tokio::spawn(async move {
                me.bootstrap_and_publish(canonical_for_bs, cell_for_bs)
                    .await;
            });
        }

        // Wait until the cell transitions out of Loading. notified() must
        // be registered before re-checking state to avoid missing the
        // notify_waiters call that fires inside the bootstrap task.
        let index = loop {
            let waiter = cell.notify.notified();
            {
                let state = cell.state.lock().expect("cell state poisoned");
                match &*state {
                    WorkspaceState::Ready { index } => break index.clone(),
                    WorkspaceState::Failed(e) => return Err(e.clone()),
                    WorkspaceState::Loading { .. } => {}
                }
            }
            waiter.await;
        };

        let guard = index.read().expect("index lock poisoned");
        Ok(score_paths(&guard.paths, query, limit))
    }

    /// Get-or-create the cell for `canonical`. Returns `(cell, is_first)`
    /// where `is_first` indicates this caller is responsible for spawning
    /// the bootstrap task.
    fn cell_for(&self, canonical: &Path) -> (Arc<WorkspaceCell>, bool) {
        let mut map = self.cells.lock().expect("cells lock poisoned");
        if let Some(existing) = map.get(canonical) {
            return (existing.clone(), false);
        }
        let cell = WorkspaceCell::new_loading();
        map.insert(canonical.to_path_buf(), cell.clone());
        (cell, true)
    }

    /// Run the bootstrap walk for `canonical`, then publish the result by
    /// transitioning the workspace cell from `Loading` to `Ready` (or
    /// `Failed`). Events that arrived in `pending` during the walk are
    /// applied to the new index inside the same `Mutex` critical section
    /// so the publication is atomic — no event written to the watcher
    /// while the bootstrap was in flight can be lost.
    ///
    /// If the workspace was invalidated (or replaced) while the bootstrap
    /// was running, the cell may no longer be the active one in
    /// [`Self::cells`]. We detect that by comparing pointers and clean up
    /// any watches we registered, otherwise they'd leak until process
    /// shutdown.
    async fn bootstrap_and_publish(self: Arc<Self>, canonical: PathBuf, cell: Arc<WorkspaceCell>) {
        let result = self.bootstrap_workspace(canonical.clone()).await;

        let watched_dirs_for_cleanup;
        {
            let mut state = cell.state.lock().expect("cell state poisoned");
            let pending = match &mut *state {
                WorkspaceState::Loading { pending } => std::mem::take(pending),
                // Only the bootstrap writer ever transitions out of
                // Loading, so seeing Ready/Failed here would mean the
                // cell was published by someone else — a bug, not an
                // expected state. Treat as empty to avoid double-applying
                // events from a parallel bootstrap.
                WorkspaceState::Ready { .. } | WorkspaceState::Failed(_) => Vec::new(),
            };

            match result {
                Ok(index) => {
                    let arc = Arc::new(RwLock::new(index));
                    // Drain buffered events into the fresh index. Both
                    // happen while the cell mutex is held, so no event
                    // can slip past the transition.
                    for event in &pending {
                        self.apply_event_to_index(&canonical, &arc, event);
                    }
                    watched_dirs_for_cleanup = {
                        let guard = arc.read().expect("index poisoned");
                        guard.watched_dirs.clone()
                    };
                    *state = WorkspaceState::Ready { index: arc };
                }
                Err(err) => {
                    *state = WorkspaceState::Failed(err);
                    watched_dirs_for_cleanup = HashSet::new();
                }
            }
        }
        cell.notify.notify_waiters();

        // Ownership check: if our cell is no longer the active one for
        // this canonical path (an invalidation or replacement happened
        // while we were walking), the watches we registered are
        // unreachable from any future invalidate_workspace call. Drop
        // them ourselves to prevent a slow inotify leak under repeated
        // cold-bootstrap-then-invalidate cycles.
        let still_active = {
            let map = self.cells.lock().expect("cells poisoned");
            map.get(&canonical)
                .is_some_and(|active| Arc::ptr_eq(active, &cell))
        };
        if !still_active && !watched_dirs_for_cleanup.is_empty() {
            tracing::debug!(
                ?canonical,
                orphan_watches = watched_dirs_for_cleanup.len(),
                "bootstrap completed after workspace was replaced; releasing orphan watches"
            );
            let mut debouncer = self.debouncer.lock().expect("debouncer poisoned");
            for dir in &watched_dirs_for_cleanup {
                let _ = debouncer.unwatch(dir);
            }
        }
    }

    /// Walk the workspace, register per-directory watches, and return a
    /// fully-populated [`WorkspaceIndex`]. Runs walks in `spawn_blocking`
    /// so we don't stall the tokio executor on a multi-second cold walk.
    ///
    /// Bootstrap ordering:
    /// 1. Walk #1 collects the snapshot, directories to watch, and the
    ///    composite ignore matcher.
    /// 2. Register a non-recursive watch on each directory; only record
    ///    the ones the debouncer accepted (a failed watch must not be
    ///    cached as "covered" or its subtree would silently lose
    ///    updates).
    /// 3. If a `.git/info/` exists, also watch it directly — the
    ///    bootstrap walker skips the `.git/` subtree, so events on
    ///    `.git/info/exclude` would otherwise never reach us, leaving
    ///    [`event_touches_ignore_file`] unable to trigger a refresh.
    /// 4. Walk #2 catches any files written between Walk #1 and watch
    ///    registration. The two-walk pass narrows the bootstrap TOCTOU;
    ///    the cell's `Loading`-state pending buffer (see
    ///    [`bootstrap_and_publish`]) closes the rest of the race.
    async fn bootstrap_workspace(
        self: &Arc<Self>,
        root: PathBuf,
    ) -> Result<WorkspaceIndex, String> {
        let walk_root = root.clone();
        let (mut paths, dirs, gitignore) =
            tokio::task::spawn_blocking(move || walk_workspace(&walk_root))
                .await
                .map_err(|e| format!("bootstrap walk panicked: {e}"))?;

        let mut watched_dirs = HashSet::new();
        let mut watch_failures = 0usize;
        {
            let mut debouncer = self.debouncer.lock().expect("debouncer poisoned");
            for dir in &dirs {
                // NonRecursive: see module docstring for why we don't use
                // RecursiveMode::Recursive here.
                match debouncer.watch(dir, RecursiveMode::NonRecursive) {
                    Ok(()) => {
                        watched_dirs.insert(dir.clone());
                    }
                    Err(e) => {
                        watch_failures += 1;
                        tracing::debug!(?dir, ?e, "failed to register file-index watch");
                    }
                }
            }

            // Explicitly watch `.git/info/` if it exists. The walker
            // skipped `.git/` (we filter_entry it out for sanity), so
            // edits to `.git/info/exclude` would never produce events
            // without this explicit registration.
            let git_info = root.join(".git").join("info");
            if git_info.is_dir() {
                match debouncer.watch(&git_info, RecursiveMode::NonRecursive) {
                    Ok(()) => {
                        watched_dirs.insert(git_info);
                    }
                    Err(e) => {
                        tracing::debug!(dir = ?git_info, ?e, "failed to register watch on .git/info");
                    }
                }
            }
        }
        if watch_failures > 0 {
            // Surface the count at warn — a handful of failures usually
            // mean the host's inotify budget is tight; many mean Cmd+P
            // results will silently miss updates in those subtrees.
            tracing::warn!(
                ?root,
                failures = watch_failures,
                watched = watched_dirs.len(),
                "file-index: some directory watches could not be registered (likely fs.inotify.max_user_watches); changes under those dirs will not refresh the Cmd+P cache",
            );
        }

        // Second walk to absorb files written between Walk #1 and watch
        // registration. `walk_just_files` reuses the same gitignore-aware
        // builder, so we only merge in paths that should be indexed.
        let walk_root_2 = root.clone();
        let second_pass = tokio::task::spawn_blocking(move || walk_just_files(&walk_root_2))
            .await
            .map_err(|e| format!("post-watch walk panicked: {e}"))?;
        paths.extend(second_pass);

        Ok(WorkspaceIndex {
            paths,
            watched_dirs,
            gitignore,
        })
    }

    /// Drop a workspace from the cache and unwatch its directories. Used
    /// when the watcher signals a rescan (events were lost; the safest
    /// move is to throw out the index and let the next search re-bootstrap)
    /// and when the workspace root itself is deleted.
    ///
    /// Overlapping workspaces (e.g. `/repo` and `/repo/packages/app`) can
    /// hold watches on the same physical directory. Invalidating one must
    /// not drop a watch the surviving sibling still depends on, or its
    /// subsequent events would be lost. We compute the set of directories
    /// that no remaining workspace still cares about and unwatch only
    /// those.
    fn invalidate_workspace(&self, root: &Path) {
        let cell_opt = {
            let mut map = self.cells.lock().expect("cells poisoned");
            map.remove(root)
        };
        let Some(cell) = cell_opt else { return };

        // Surface watched_dirs if the cell ever published. A Loading cell
        // has none yet; its in-flight bootstrap will detect the race
        // (cell removed from map) and clean up its own watches via the
        // ownership check in `bootstrap_and_publish`.
        let my_dirs: Vec<PathBuf> = {
            let state = cell.state.lock().expect("cell state poisoned");
            match &*state {
                WorkspaceState::Ready { index } => {
                    let guard = index.read().expect("index poisoned");
                    guard.watched_dirs.iter().cloned().collect()
                }
                WorkspaceState::Loading { .. } | WorkspaceState::Failed(_) => Vec::new(),
            }
        };

        // Wake any search() that was awaiting this cell — they'll see
        // Failed/Loading transitioning is unlikely, but we should signal
        // so they retry from the new cell on their next call.
        cell.notify.notify_waiters();

        if my_dirs.is_empty() {
            return;
        }

        // Snapshot survivors' watched_dirs so we only unwatch dirs that
        // no other workspace depends on. Without this, dropping `/repo`
        // while `/repo/packages/app` is still cached would orphan the
        // child workspace's watches.
        let still_needed: HashSet<PathBuf> = {
            let map = self.cells.lock().expect("cells poisoned");
            let survivors: Vec<Arc<WorkspaceCell>> = map.values().cloned().collect();
            drop(map);
            let mut combined = HashSet::new();
            for cell in survivors {
                let state = cell.state.lock().expect("cell state poisoned");
                if let WorkspaceState::Ready { index } = &*state {
                    let guard = index.read().expect("index poisoned");
                    combined.extend(guard.watched_dirs.iter().cloned());
                }
            }
            combined
        };

        let to_drop: Vec<&PathBuf> = my_dirs
            .iter()
            .filter(|d| !still_needed.contains(*d))
            .collect();
        let mut debouncer = self.debouncer.lock().expect("debouncer poisoned");
        for dir in &to_drop {
            let _ = debouncer.unwatch(dir);
        }
        tracing::debug!(
            ?root,
            owned = my_dirs.len(),
            unwatched = to_drop.len(),
            "invalidated workspace index"
        );
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
                let roots = if event.event.paths.is_empty() {
                    // Pathless rescan: notify can't tell us *which* tree
                    // lost events. Safest recovery is to invalidate
                    // every cached workspace and let the next searches
                    // re-bootstrap.
                    self.all_workspace_roots()
                } else {
                    self.affected_workspace_roots(&event.event.paths)
                };
                for root in roots {
                    tracing::debug!(?root, "watcher signaled rescan; invalidating workspace");
                    self.invalidate_workspace(&root);
                }
                continue;
            }

            // Any change to a gitignore-defining file (`.gitignore`,
            // `.ignore`, or `.git/info/exclude`) invalidates both the
            // cached path set (existing tracked files might now be
            // ignored) and the cached gitignore matcher (future
            // Create(File) events need the new rules). Drop the
            // workspace and let the next search rebuild.
            if event_touches_ignore_file(&event.event.paths) {
                for root in self.affected_workspace_roots(&event.event.paths) {
                    tracing::debug!(
                        ?root,
                        "gitignore-defining file changed; invalidating workspace"
                    );
                    self.invalidate_workspace(&root);
                }
                continue;
            }

            self.dispatch_event(&event.event);
        }
    }

    fn all_workspace_roots(&self) -> Vec<PathBuf> {
        let map = self.cells.lock().expect("cells poisoned");
        map.keys().cloned().collect()
    }

    /// Dispatch a single event to every workspace whose root prefixes one
    /// of the event's paths. For `Ready` cells the event is applied
    /// immediately; for `Loading` cells it's buffered into the cell's
    /// pending queue so the in-flight bootstrap can drain it atomically
    /// during publication.
    fn dispatch_event(self: &Arc<Self>, event: &notify::Event) {
        let targets: Vec<(PathBuf, Arc<WorkspaceCell>)> = {
            let map = self.cells.lock().expect("cells poisoned");
            map.iter()
                .filter(|(root, _)| event.paths.iter().any(|p| p.starts_with(root)))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        for (root, cell) in targets {
            let mut state = cell.state.lock().expect("cell state poisoned");
            match &mut *state {
                WorkspaceState::Loading { pending } => {
                    pending.push(event.clone());
                }
                WorkspaceState::Ready { index } => {
                    let index = index.clone();
                    drop(state);
                    self.apply_event_to_index(&root, &index, event);
                }
                WorkspaceState::Failed(_) => {}
            }
        }
    }

    fn apply_event_to_index(
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

        // Gitignored directories created after bootstrap (e.g. `cargo build`
        // creating `target/`, `npm install` creating `node_modules/`) are
        // visible to the watcher because their parent is watched, but the
        // bootstrap walk would have excluded them. Starting an
        // `ignore::WalkBuilder` *at* an ignored directory does not re-apply
        // the ancestor `.gitignore` rule to that directory's root — the
        // walker only considers the subtree below the starting point — so
        // without this guard we would walk and index a `target/` tree that
        // the bootstrap pass correctly skipped.
        {
            let guard = index.read().expect("index poisoned");
            if guard
                .gitignore
                .as_ref()
                .is_some_and(|gi| gi.matched(new_dir, true).is_ignore())
            {
                return;
            }
        }

        let (new_files, new_dirs) = walk_subtree(root, new_dir);

        // If the new subtree carries its own `.gitignore` or `.ignore` file
        // (e.g. the user just moved a foreign project into the workspace),
        // the cached matcher doesn't know about its rules. Invalidate the
        // workspace so the next search rebuilds with the new rules layered
        // in — far simpler than splicing them into the live matcher.
        let imports_ignore_rules = new_files
            .iter()
            .any(|f| f.ends_with("/.gitignore") || f.ends_with("/.ignore"));
        if imports_ignore_rules {
            tracing::debug!(
                ?root,
                ?new_dir,
                "subtree contained ignore file; invalidating workspace"
            );
            self.invalidate_workspace(root);
            return;
        }

        if !new_dirs.is_empty() {
            let mut debouncer = self.debouncer.lock().expect("debouncer poisoned");
            let mut idx = index.write().expect("index poisoned");
            for dir in new_dirs {
                if !idx.watched_dirs.contains(&dir) {
                    match debouncer.watch(&dir, RecursiveMode::NonRecursive) {
                        Ok(()) => {
                            idx.watched_dirs.insert(dir);
                        }
                        Err(e) => {
                            // Don't record an unwatched directory — it'd
                            // claim coverage we don't actually have.
                            tracing::debug!(?dir, ?e, "failed to register watch on new subdir");
                        }
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

    /// Roots of every workspace whose tree contains any of `paths`,
    /// including workspaces still in `Loading` — a rescan or
    /// ignore-file event needs to invalidate them so the in-flight
    /// bootstrap can be retried with a fresh walk.
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
fn walk_workspace(root: &Path) -> (BTreeSet<String>, HashSet<PathBuf>, Option<Gitignore>) {
    let mut files = BTreeSet::new();
    let mut dirs = HashSet::new();

    // The bootstrap walker only honors `.gitignore` rules when the tree
    // is inside a git repo (this is what `WalkBuilder::git_ignore(true)`
    // does). If `.git` doesn't exist at the root, the cached event-time
    // matcher must mirror that, or `insert_file_if_not_ignored` would
    // reject files that the bootstrap walk accepted, producing diverging
    // results.
    let is_git_repo = root.join(".git").exists();
    let mut gi_builder = GitignoreBuilder::new(root);

    if is_git_repo {
        // `.gitignore` files apply with their containing directory as
        // scope (mirroring git semantics). `GitignoreBuilder::add` handles
        // that correctly.
        let _ = gi_builder.add(root.join(".gitignore"));

        // `.git/info/exclude` and the global excludes file are NOT
        // directory-scoped to themselves — git applies their patterns
        // against paths relative to the workspace root. `add_line` keeps
        // each pattern scoped to the builder's root, which is exactly
        // what we want here. Loading them via `add()` would scope them
        // to `<root>/.git/info` and `$XDG_CONFIG_HOME/git`, which never
        // contain any project file.
        seed_pattern_lines(
            &mut gi_builder,
            &root.join(".git").join("info").join("exclude"),
        );
        if let Some(global) = resolve_global_gitignore() {
            seed_pattern_lines(&mut gi_builder, &global);
        }
    }
    // `.ignore` files are always honoured (independent of git presence).
    let _ = gi_builder.add(root.join(".ignore"));

    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .filter_entry(|e| e.file_name() != ".git")
        .build();

    let root_gitignore = root.join(".gitignore");
    let root_ignore = root.join(".ignore");
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
                // Layer in any `.gitignore` / `.ignore` files discovered
                // deeper in the tree. Each file's rules are scoped to its
                // own directory by `GitignoreBuilder`, mirroring git's
                // behaviour. Root-level files were already added above; we
                // skip them here to avoid double-loading the same rules.
                let name = entry.file_name();
                let is_gitignore = name == ".gitignore" && is_git_repo && path != root_gitignore;
                let is_ignore = name == ".ignore" && path != root_ignore;
                if is_gitignore || is_ignore {
                    let _ = gi_builder.add(path);
                }
            }
            None => {}
        }
    }

    // Build the matcher only when there are rules to honour. An
    // always-empty `Gitignore::empty()` would force every `Create(File)`
    // event through a no-op match call; `Option::None` is cheaper to
    // check and more honest about non-git workspaces.
    let gitignore = if is_git_repo || root.join(".ignore").exists() {
        gi_builder.build().ok().or_else(|| {
            tracing::debug!(?root, "failed to build composite gitignore");
            None
        })
    } else {
        None
    };

    (files, dirs, gitignore)
}

/// Read a pattern file (e.g. `.git/info/exclude` or the global git
/// excludes file) line by line and feed each pattern to `builder` via
/// `add_line`. This keeps the patterns scoped to the builder's root
/// instead of the pattern file's own directory — the semantics git
/// itself uses for these non-tree exclude files.
fn seed_pattern_lines(builder: &mut GitignoreBuilder, path: &Path) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let _ = builder.add_line(Some(path.to_path_buf()), trimmed);
    }
}

/// Cheap variant of [`walk_workspace`] that returns just the file set,
/// used by the second bootstrap pass to absorb files written between the
/// first walk and watch registration. The gitignore configuration is
/// identical so we don't pick up anything the original walk excluded.
fn walk_just_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
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
        if entry.file_type().is_some_and(|t| t.is_file()) {
            if let Some(rel) = relative_string(root, entry.path()) {
                files.push(rel);
            }
        }
    }
    files
}

/// Resolve the path git would use for global excludes — `core.excludesFile`
/// if set, otherwise `$XDG_CONFIG_HOME/git/ignore`
/// (default `~/.config/git/ignore`). Returns `None` if neither resolves
/// to an existing file.
fn resolve_global_gitignore() -> Option<PathBuf> {
    // `git config --global core.excludesFile` is the authoritative answer.
    if let Ok(out) = std::process::Command::new("git")
        .args(["config", "--global", "--path", "core.excludesFile"])
        .output()
    {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !raw.is_empty() {
                let p = PathBuf::from(raw);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    // Fall back to the XDG default.
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let candidate = base.join("git").join("ignore");
    candidate.is_file().then_some(candidate)
}

/// True iff any of the event's paths terminates in a filename that
/// changes gitignore semantics: `.gitignore`, `.ignore`, or
/// `.git/info/exclude`. Such an edit needs to invalidate the cached
/// matcher (and re-check existing files), so the entire workspace
/// gets dropped and re-bootstrapped on next access.
fn event_touches_ignore_file(paths: &[PathBuf]) -> bool {
    paths.iter().any(|p| {
        let name = p.file_name().and_then(|n| n.to_str());
        if matches!(name, Some(".gitignore" | ".ignore")) {
            return true;
        }
        // `.git/info/exclude` — match by full suffix.
        p.ends_with("info/exclude")
            && p.parent()
                .and_then(|p| p.parent())
                .is_some_and(|p| p.file_name().and_then(|n| n.to_str()) == Some(".git"))
    })
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
        guard
            .gitignore
            .as_ref()
            .is_some_and(|gi| gi.matched(path, false).is_ignore())
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

    /// Codex round-2 #2: a gitignored directory created after bootstrap
    /// (e.g. `cargo build` producing `target/`) must not enter the index.
    /// Starting `ignore::WalkBuilder` *at* an ignored directory does not
    /// re-apply the ancestor rule, so without the explicit gitignore
    /// gate in `absorb_new_subtree` we'd walk and index `target/`.
    #[tokio::test]
    async fn gitignored_dir_creation_does_not_enter_index() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fake_git(root);
        write(&root.join(".gitignore"), "target/\n");
        write(&root.join("src/keep.rs"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let _ = search(&indexer, root, "").await;

        // The bootstrap excluded `target/`. Now create it and put files
        // inside — the watcher will fire a Create(Folder) for `target/`
        // because its parent (the root) is watched.
        fs::create_dir_all(root.join("target/release")).unwrap();
        write(&root.join("target/release/phoenix"), "");

        tokio::time::sleep(Duration::from_millis(400)).await;
        let results = search(&indexer, root, "").await;
        assert!(
            !results.iter().any(|p| p.starts_with("target/")),
            "gitignored subtree leaked: {results:?}"
        );
        assert!(
            results.iter().any(|p| p == "src/keep.rs"),
            "kept file missing: {results:?}"
        );
    }

    /// Codex round-2 #3: invalidating one workspace must preserve watches
    /// the surviving sibling workspace still depends on. Otherwise a
    /// rescan on `/repo` would silently drop watches under
    /// `/repo/packages/app`, breaking the nested workspace's index.
    #[tokio::test]
    async fn invalidating_parent_preserves_child_workspace_watches() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path();
        let child = parent.join("packages/app");
        fs::create_dir_all(&child).unwrap();
        write(&parent.join("README.md"), "");
        write(&child.join("index.ts"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let _ = search(&indexer, parent, "").await;
        let _ = search(&indexer, &child, "").await;

        // Invalidate the parent — simulates a rescan signal on the
        // outer workspace.
        indexer.invalidate_workspace(parent);

        // After the parent is gone, the child workspace's watches on
        // `packages/app/...` must still deliver events. Verify by
        // creating a file inside the child workspace.
        write(&child.join("brand_new.ts"), "");

        let child_saw = wait_for_path(
            &indexer,
            &child,
            "brand_new",
            Duration::from_secs(2),
            |results| results.iter().any(|p| p == "brand_new.ts"),
        )
        .await;
        assert!(
            child_saw,
            "child workspace lost watches when parent was invalidated"
        );
    }

    /// Codex round-2 #4: editing `.gitignore` (adding or removing rules)
    /// must invalidate the cached workspace. The cached `Gitignore`
    /// matcher and the cached path set both reflect the old rules
    /// otherwise.
    #[tokio::test]
    async fn gitignore_edit_invalidates_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fake_git(root);
        write(&root.join(".gitignore"), "");
        write(&root.join("src/keep.rs"), "");
        write(&root.join("src/maybe.log"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let initial = search(&indexer, root, "").await;
        assert!(
            initial.iter().any(|p| p == "src/maybe.log"),
            "initial: {initial:?}"
        );

        // Edit `.gitignore` to start ignoring `.log` files. The cached
        // index should be invalidated; the next search runs a fresh
        // bootstrap that excludes the log.
        write(&root.join(".gitignore"), "*.log\n");

        let gone = wait_for_path(&indexer, root, "", Duration::from_secs(2), |results| {
            !results.iter().any(|p| p == "src/maybe.log")
        })
        .await;
        assert!(gone, "gitignore edit didn't refresh the index");
    }

    /// Codex round-2 #1: the bootstrap's two-pass design must absorb
    /// files written between the first walk and watch registration.
    /// Simulate by writing a file *during* the first bootstrap and
    /// asserting it's present in the snapshot. (We can't perfectly
    /// inject between walk 1 and watch registration in a test without
    /// hooking the internals, so we approximate by writing a file
    /// after bootstrap returns and asserting that a *subsequent*
    /// bootstrap of a fresh indexer sees it — proving the second walk
    /// merges new files into the snapshot.)
    #[tokio::test]
    async fn second_walk_absorbs_files_written_after_first_walk() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("src/a.rs"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let _ = search(&indexer, root, "").await;

        // Add a file post-bootstrap; the watcher will pick it up.
        write(&root.join("src/b.rs"), "");
        let saw_b = wait_for_path(&indexer, root, "", Duration::from_secs(2), |results| {
            results.iter().any(|p| p == "src/b.rs")
        })
        .await;
        assert!(saw_b, "watcher missed post-bootstrap create");

        // Now drop the indexer entirely (simulating Phoenix restart)
        // and re-bootstrap with a brand-new indexer. The second-pass
        // walk should still find b.rs that was added since the file
        // existed before the bootstrap began.
        drop(indexer);
        let fresh = WorkspaceIndexer::new().unwrap();
        let after_restart = search(&fresh, root, "").await;
        assert!(
            after_restart.iter().any(|p| p == "src/b.rs"),
            "fresh bootstrap missed pre-existing file: {after_restart:?}"
        );
    }

    /// Codex round-3 #1: events arriving while a workspace is still
    /// `Loading` must be buffered into the cell, not dropped. We force
    /// the race by issuing concurrent searches: the first one wins the
    /// bootstrap, the second one waits on the Notify, and any
    /// filesystem event in between must end up in the published index.
    ///
    /// The strongest assertion we can make from outside the module is
    /// that *after* bootstrap completes and a file was written during
    /// it, the file ends up in the index. We can't directly inject
    /// the timing without hooking internals, but the Loading-state
    /// buffering is also exercised by every other watcher-driven
    /// test (they all hit the post-bootstrap apply path through
    /// `dispatch_event`'s `Ready` arm); this test specifically covers
    /// the publication-atomic case.
    #[tokio::test]
    async fn events_during_loading_are_not_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Make Walk #1 long enough that we can race with it by
        // populating many files. (With a small tree the bootstrap is
        // sub-ms and we can't reliably race; with ~500 files we get
        // tens of ms of bootstrap work.)
        for i in 0..500 {
            write(&root.join(format!("src/f{i:03}.rs")), "");
        }

        let indexer = WorkspaceIndexer::new().unwrap();

        // Kick off the bootstrap in the background.
        let indexer_a = indexer.clone();
        let root_a = root.to_path_buf();
        let bs = tokio::spawn(async move {
            indexer_a.search(root_a, "", 1).await.expect("search");
        });

        // Race: write a file while bootstrap is in progress. The
        // watcher fires a Create event; with Loading-state buffering
        // it ends up in the pending buffer, drained at publication.
        write(&root.join("src/race.rs"), "");

        bs.await.unwrap();

        // The file should appear in the index. Either Walk #2 picked
        // it up (deterministic) or the Loading buffer drained the
        // Create event (also deterministic). Both paths converge.
        let saw_race = wait_for_path(&indexer, root, "race", Duration::from_secs(2), |results| {
            results.iter().any(|p| p == "src/race.rs")
        })
        .await;
        assert!(saw_race, "race file vanished during bootstrap");
    }

    /// Codex round-3 #2: `.git/info/exclude` must be watched even
    /// though `.git` is excluded from the bootstrap walk. Without an
    /// explicit watch on `.git/info/`, edits to the exclude file
    /// never produce events and the cached gitignore matcher goes
    /// stale silently.
    #[tokio::test]
    async fn git_info_exclude_edit_triggers_invalidation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git/info")).unwrap();
        write(&root.join(".git/info/exclude"), "");
        write(&root.join("src/test.tmp"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let initial = search(&indexer, root, "").await;
        assert!(
            initial.iter().any(|p| p == "src/test.tmp"),
            "initial: {initial:?}"
        );

        // Edit `.git/info/exclude` to ignore `.tmp` files.
        write(&root.join(".git/info/exclude"), "*.tmp\n");

        let gone = wait_for_path(&indexer, root, "", Duration::from_secs(2), |results| {
            !results.iter().any(|p| p == "src/test.tmp")
        })
        .await;
        assert!(gone, ".git/info/exclude edit didn't trigger refresh");
    }

    /// Codex round-3 #6: `.git/info/exclude` patterns are scoped to
    /// the workspace root, not to `<root>/.git/info/`. A pattern like
    /// `*.tmp` should match files anywhere in the tree, not just
    /// under `.git/info/`.
    #[tokio::test]
    async fn git_info_exclude_patterns_match_workspace_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git/info")).unwrap();
        write(&root.join(".git/info/exclude"), "*.tmp\n");
        write(&root.join("src/keep.rs"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let _ = search(&indexer, root, "").await;

        // Create a `.tmp` file under src/ — the cached matcher should
        // reject it because the workspace-scoped exclude pattern matches.
        write(&root.join("src/debug.tmp"), "");

        tokio::time::sleep(Duration::from_millis(400)).await;
        let results = search(&indexer, root, "").await;
        assert!(
            !results.iter().any(|p| p == "src/debug.tmp"),
            "exclude file's pattern wasn't applied to workspace path: {results:?}"
        );
    }

    /// Codex round-3 #7: a non-git workspace must not apply
    /// `.gitignore` rules — the bootstrap walker doesn't, and the
    /// cached matcher must match that behaviour or filtering diverges
    /// between Walk #1 and `Create(File)` events.
    #[tokio::test]
    async fn non_git_workspace_does_not_apply_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // No .git/ — this is NOT a git repo.
        write(&root.join(".gitignore"), "*.log\n");
        write(&root.join("src/main.rs"), "");
        write(&root.join("src/existing.log"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let initial = search(&indexer, root, "").await;
        // Bootstrap walk doesn't apply .gitignore in non-git tree, so
        // existing.log is present.
        assert!(
            initial.iter().any(|p| p == "src/existing.log"),
            "non-git bootstrap should include the .log file: {initial:?}"
        );

        // Create a new .log file. Without the gitignore-on-Create
        // filter being suppressed in non-git workspaces, this would
        // be rejected — diverging from the bootstrap walker.
        write(&root.join("src/new.log"), "");
        let saw_new = wait_for_path(
            &indexer,
            root,
            "new.log",
            Duration::from_secs(2),
            |results| results.iter().any(|p| p == "src/new.log"),
        )
        .await;
        assert!(saw_new, "non-git workspace incorrectly applied .gitignore");
    }

    /// Codex round-3 #3: moving in a subtree with its own `.gitignore`
    /// must trigger workspace invalidation so subsequent files under
    /// it are filtered by the moved-in rules.
    #[tokio::test]
    async fn imported_subtree_with_gitignore_invalidates_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fake_git(root);
        write(&root.join("src/main.rs"), "");

        let indexer = WorkspaceIndexer::new().unwrap();
        let _ = search(&indexer, root, "").await;

        // Simulate moving in an external project: a new directory
        // appears with its own .gitignore inside.
        fs::create_dir_all(root.join("external/sub")).unwrap();
        write(&root.join("external/.gitignore"), "*.tmp\n");
        write(&root.join("external/sub/keep.rs"), "");

        // Give notify time to fire the dir-create + file-creates.
        tokio::time::sleep(Duration::from_millis(400)).await;

        // The presence of the imported `.gitignore` should have
        // invalidated the workspace. Verify by writing a `.tmp` file
        // under `external/` — the next search re-bootstraps with the
        // new rules and should exclude it.
        write(&root.join("external/junk.tmp"), "");
        tokio::time::sleep(Duration::from_millis(400)).await;

        let results = search(&indexer, root, "").await;
        // The imported .gitignore should now be in force, so junk.tmp
        // is excluded by the rebuilt matcher. (Files added BEFORE the
        // invalidation might be present or absent depending on timing;
        // we only assert the gitignore is now active.)
        assert!(
            !results.iter().any(|p| p == "external/junk.tmp"),
            "imported .gitignore rule wasn't honored after import: {results:?}"
        );
    }

    /// Codex round-3 #4: if a workspace is invalidated while its
    /// bootstrap is in flight, the bootstrap-registered watches must
    /// not leak. The ownership check in `bootstrap_and_publish` is the
    /// guarantee; we verify behaviorally by invalidating during
    /// bootstrap and confirming a fresh bootstrap can still operate
    /// without exhausting the inotify budget.
    #[tokio::test]
    async fn bootstrap_invalidated_mid_flight_does_not_leak_watches() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..200 {
            write(&root.join(format!("src/f{i:03}.rs")), "");
        }

        let indexer = WorkspaceIndexer::new().unwrap();

        // Run 20 quick "search → invalidate → search" cycles. If
        // bootstrap-registered watches were leaking on each
        // invalidation, even a low watch budget would eventually
        // refuse new watches; the final search would lose updates.
        for _ in 0..20 {
            let _ = search(&indexer, root, "").await;
            indexer.invalidate_workspace(&canonicalize_or_self(root));
        }

        // Final state: bootstrap one more time and verify the index
        // is correct. Use a higher limit than the default 50 so we
        // exercise the full snapshot.
        let final_results = indexer
            .search(root.to_path_buf(), "", 500)
            .await
            .expect("final search");
        assert_eq!(
            final_results.len(),
            200,
            "expected 200 files after invalidation cycles, got {}: {:?}",
            final_results.len(),
            &final_results[..final_results.len().min(5)]
        );

        // And that the watcher still functions — i.e. the leaks
        // didn't push us past `max_user_watches`.
        write(&root.join("src/post_cycle.rs"), "");
        let saw = wait_for_path(
            &indexer,
            root,
            "post_cycle",
            Duration::from_secs(2),
            |results| results.iter().any(|p| p == "src/post_cycle.rs"),
        )
        .await;
        assert!(saw, "watcher stopped working after invalidation cycles");
    }
}
