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
//! ## Actor model
//!
//! The indexer is built around two kinds of actors and a stateless
//! registry. The registry holds a `papaya::HashMap<PathBuf,
//! WorkspaceHandle>` keyed by canonical workspace root. Each handle wraps
//! an mpsc sender into a per-workspace actor task; the actor *owns* its
//! `paths`, `gitignore`, `watched_dirs`, and lifecycle state directly as
//! struct fields with no shared `Mutex`/`RwLock`. A single
//! `DebouncerActor` owns the underlying `notify` debouncer and a
//! refcounted subscription table mapping each watched directory to the
//! set of workspace actors interested in events under it. Two workspaces
//! whose trees overlap share one kernel inotify watch per directory;
//! when the last subscriber for a directory unsubscribes, the actor
//! calls `debouncer.unwatch(dir)`. This collapses the entire
//! shared-state coordination class (bootstrap publish race, watch
//! ownership, gitignore-file invalidation, pending-event replay) into
//! local actor logic and message passing.
//!
//! ## Hierarchical gitignore scoping (deferred)
//!
//! The current gitignore matcher is a flat composite: every `.gitignore`
//! / `.ignore` rule encountered during the bootstrap walk is folded
//! into a single `Gitignore` keyed at the workspace root. This mirrors
//! `ignore::WalkBuilder`'s output for the *initial* walk but is not a
//! perfect equivalent for event-time `Create(File)` filtering — a rule
//! in `<root>/sub/.gitignore` is treated as if it applied at the root.
//! In practice this is conservative (it can over-exclude under siblings
//! of the rule's intended scope, but never *under-*excludes), and a
//! proper hierarchical matcher is a deeper semantic fix tracked as
//! follow-up. Do not "fix" this by silently swapping in a per-directory
//! matcher; that's its own scope.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::RecommendedWatcher;
use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{
    new_debouncer, DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache,
};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use tokio::sync::{mpsc, oneshot};

/// Debounce window. Short enough that newly-saved files appear in the
/// palette before the user starts typing the new filename; long enough to
/// coalesce the `CREATE`+`MODIFY`+`CLOSE_WRITE` burst a single editor save
/// emits.
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(100);

/// Bounded buffer between the `DebouncerActor` and each `WorkspaceActor`.
/// Sized to absorb the largest burst we'd realistically see (a `cargo
/// build` finishing, dumping a few thousand events across `target/`)
/// without losing events. If a workspace's queue fills the actor is
/// likely wedged and we should drop the workspace; rescan via the
/// next search will recover.
const EVENT_CHANNEL_CAP: usize = 4096;

/// Bounded buffer for commands into a `WorkspaceActor`.
const COMMAND_CHANNEL_CAP: usize = 64;

/// Bounded buffer for `DebouncerActor` commands.
const DEBOUNCER_COMMAND_CAP: usize = 256;

type WorkspaceId = u64;

// ---------------------------------------------------------------------------
// Public registry
// ---------------------------------------------------------------------------

/// Registry of per-workspace indexers. The registry itself is lock-free
/// for reads (papaya) and holds no per-workspace state — each workspace
/// is an independently-running tokio task addressable through a
/// [`WorkspaceHandle`].
pub struct WorkspaceIndexer {
    handles: papaya::HashMap<PathBuf, WorkspaceHandle>,
    debouncer_tx: mpsc::Sender<DebouncerCommand>,
    next_workspace_id: AtomicU64,
}

impl WorkspaceIndexer {
    /// Spawn the debouncer actor and return an empty registry. Returns
    /// `Err` only if `notify` itself fails to initialize (e.g. cannot
    /// create an inotify instance).
    #[allow(clippy::unused_async)]
    pub async fn new() -> Result<Arc<Self>, String> {
        let (debouncer_tx, debouncer_rx) = mpsc::channel(DEBOUNCER_COMMAND_CAP);
        // Bridge thread holds a WeakSender derived inside spawn(); we
        // pass the strong sender in so it can derive the weak, but the
        // strong copy itself is dropped — the registry keeps the only
        // surviving strong reference (plus whatever it clones to
        // workspaces).
        DebouncerActor::spawn(&debouncer_tx, debouncer_rx)
            .map_err(|e| format!("notify init: {e}"))?;
        Ok(Arc::new(Self {
            handles: papaya::HashMap::new(),
            debouncer_tx,
            next_workspace_id: AtomicU64::new(1),
        }))
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
        let handle = self.handle_for(&canonical);
        handle.search(query.to_string(), limit).await
    }

    /// Get-or-create the handle for `canonical`. Spawns the workspace
    /// actor on first access. The papaya pin gives us a wait-free read
    /// path on the common case (handle already exists); only on a true
    /// first access do we go through the `get_or_insert_with` slow path.
    fn handle_for(self: &Arc<Self>, canonical: &Path) -> WorkspaceHandle {
        let pinned = self.handles.pin();
        if let Some(existing) = pinned.get(canonical) {
            return existing.clone();
        }
        // Concurrent first-callers race here. `get_or_insert_with` may
        // call the closure more than once under contention; the loser's
        // spawned actor sees its `WorkspaceHandle` (and thus mpsc
        // Sender) dropped and self-terminates cleanly at the next
        // `select!` tick.
        let canonical_buf = canonical.to_path_buf();
        pinned
            .get_or_insert_with(canonical_buf.clone(), || {
                let workspace_id = self.next_workspace_id.fetch_add(1, Ordering::Relaxed);
                WorkspaceActor::spawn(
                    workspace_id,
                    canonical_buf.clone(),
                    self.debouncer_tx.clone(),
                    self.clone(),
                )
            })
            .clone()
    }

    /// Drop a workspace from the registry. Used by tests and by the
    /// workspace actor itself when it decides to self-terminate (e.g.
    /// after a gitignore edit triggers a full rebuild).
    fn invalidate_workspace(self: &Arc<Self>, root: &Path) {
        let canonical = canonicalize_or_self(root);
        let pinned = self.handles.pin();
        if let Some(handle) = pinned.remove(&canonical) {
            // Best effort: tell the actor to shut down. The drop of the
            // last Sender to the actor will also cause it to exit; the
            // explicit message is just a hint to release watches sooner.
            let tx = handle.tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(WorkspaceCommand::Shutdown).await;
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Workspace handle (clone-friendly sender into a workspace actor)
// ---------------------------------------------------------------------------

/// Clone-friendly sender into a [`WorkspaceActor`]. Calls block (via
/// `await`) on a oneshot reply when the operation has a return value;
/// fire-and-forget operations (`invalidate`) return immediately once the
/// command is enqueued.
#[derive(Clone)]
struct WorkspaceHandle {
    tx: mpsc::Sender<WorkspaceCommand>,
}

impl WorkspaceHandle {
    async fn search(&self, query: String, limit: usize) -> Result<Vec<String>, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(WorkspaceCommand::Search {
                query,
                limit,
                reply: reply_tx,
            })
            .await
            .map_err(|_| "file-index actor exited".to_string())?;
        reply_rx
            .await
            .map_err(|_| "file-index actor dropped reply".to_string())?
    }
}

// ---------------------------------------------------------------------------
// Workspace actor
// ---------------------------------------------------------------------------

enum WorkspaceCommand {
    Search {
        query: String,
        limit: usize,
        reply: oneshot::Sender<Result<Vec<String>, String>>,
    },
    /// External invalidate. The registry sends this after removing the
    /// handle from the map; on receipt the actor releases its watches
    /// and exits.
    Shutdown,
}

/// All state a workspace actor owns. No `Mutex`/`RwLock` — the actor's
/// single-threaded select-loop is the synchronization point.
enum WorkspaceState {
    Loading,
    Ready {
        paths: BTreeSet<String>,
        gitignore: Option<Gitignore>,
        watched_dirs: HashSet<PathBuf>,
    },
    Failed(String),
}

struct WorkspaceActor {
    workspace_id: WorkspaceId,
    root: PathBuf,
    state: WorkspaceState,
    commands_rx: mpsc::Receiver<WorkspaceCommand>,
    events_rx: mpsc::Receiver<DebouncedEvent>,
    /// Held so the actor can subscribe to additional directories on a
    /// `Create(Folder)` event after bootstrap.
    events_tx: mpsc::Sender<DebouncedEvent>,
    debouncer_tx: mpsc::Sender<DebouncerCommand>,
    /// Weak ref to the registry. Used by the actor to self-invalidate
    /// (when a `.gitignore` edit or new subtree-imported `.gitignore`
    /// requires a full rebuild) without holding a strong cycle.
    registry: std::sync::Weak<WorkspaceIndexer>,
}

impl WorkspaceActor {
    /// Spawn the actor and return a handle. The bootstrap walk runs
    /// inside the actor's `run()` body — so `events_rx` already exists
    /// when watch registration completes, and events fired during
    /// bootstrap queue naturally in the channel without a separate
    /// pending buffer.
    fn spawn(
        workspace_id: WorkspaceId,
        root: PathBuf,
        debouncer_tx: mpsc::Sender<DebouncerCommand>,
        registry: Arc<WorkspaceIndexer>,
    ) -> WorkspaceHandle {
        let (commands_tx, commands_rx) = mpsc::channel(COMMAND_CHANNEL_CAP);
        let (events_tx, events_rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        let registry_weak = Arc::downgrade(&registry);
        // Drop our strong reference before moving into the task so the
        // actor doesn't keep the registry alive past its natural lifetime.
        drop(registry);

        let actor = Self {
            workspace_id,
            root,
            state: WorkspaceState::Loading,
            commands_rx,
            events_rx,
            events_tx,
            debouncer_tx,
            registry: registry_weak,
        };
        tokio::spawn(actor.run());

        WorkspaceHandle { tx: commands_tx }
    }

    async fn run(mut self) {
        // Bootstrap first. Events fired during bootstrap queue in
        // events_rx — we drain them after the state flip below, so no
        // separate pending buffer is required. The bootstrap walk runs
        // in spawn_blocking; a multi-second walk on a large monorepo
        // would otherwise stall the runtime worker this actor lives on.
        self.bootstrap().await;

        // Even after bootstrap, the registry might already have been
        // told to forget us (a sibling workspace caused an external
        // invalidate). In that case the next Search call would re-spawn
        // us on a fresh actor; here we drain any in-flight commands
        // until shutdown and exit.

        loop {
            tokio::select! {
                biased;
                cmd = self.commands_rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    if !self.handle_command(cmd) {
                        break;
                    }
                }
                event = self.events_rx.recv() => {
                    let Some(event) = event else {
                        // Debouncer dropped — no more events will arrive.
                        // Keep serving commands but treat the path set
                        // as frozen.
                        continue;
                    };
                    if let Err(e) = self.handle_event(event).await {
                        tracing::debug!(?e, root = ?self.root, "file-index event handling failed");
                    }
                }
            }
        }

        // Clean shutdown: tell the debouncer to release every directory
        // we subscribed to. The DebouncerActor refcounts and only
        // unwatches when no other workspace is using the directory.
        let _ = self
            .debouncer_tx
            .send(DebouncerCommand::UnsubscribeAll {
                workspace_id: self.workspace_id,
            })
            .await;
    }

    /// First-pass walk + watch registration + second-pass walk. State
    /// transitions to `Ready` or `Failed` on completion. Errors are
    /// non-fatal: the actor stays alive serving the `Failed` state so
    /// every search returns the same error string without re-walking.
    async fn bootstrap(&mut self) {
        let root = self.root.clone();
        let walk1 = tokio::task::spawn_blocking(move || walk_workspace(&root)).await;
        let (mut paths, dirs, gitignore) = match walk1 {
            Ok(triple) => triple,
            Err(e) => {
                self.state = WorkspaceState::Failed(format!("bootstrap walk panicked: {e}"));
                return;
            }
        };

        let mut watched_dirs = HashSet::new();
        let mut watch_failures = 0usize;
        for dir in &dirs {
            if self.subscribe_dir(dir.clone()).await {
                watched_dirs.insert(dir.clone());
            } else {
                watch_failures += 1;
            }
        }

        // Register a direct watch on `.git/info/` (the bootstrap walker
        // filter_entry skips `.git/`, so without an explicit watch
        // edits to `.git/info/exclude` would never produce events). For
        // linked worktrees the exclude lives at a different gitdir; the
        // resolution and watch registration happen in `bootstrap_excludes`.
        let info_dir = bootstrap_excludes_dir(&self.root);
        if let Some(info_dir) = info_dir {
            if info_dir.is_dir() && self.subscribe_dir(info_dir.clone()).await {
                watched_dirs.insert(info_dir);
            }
        }

        if watch_failures > 0 {
            tracing::warn!(
                root = ?self.root,
                failures = watch_failures,
                watched = watched_dirs.len(),
                "file-index: some directory watches could not be registered (likely fs.inotify.max_user_watches); changes under those dirs will not refresh the Cmd+P cache",
            );
        }

        let root2 = self.root.clone();
        let walk2 = tokio::task::spawn_blocking(move || walk_just_files(&root2)).await;
        match walk2 {
            Ok(extra) => paths.extend(extra),
            Err(e) => {
                tracing::debug!(?e, root = ?self.root, "second-pass walk panicked");
            }
        }

        self.state = WorkspaceState::Ready {
            paths,
            gitignore,
            watched_dirs,
        };
    }

    fn handle_command(&mut self, cmd: WorkspaceCommand) -> bool {
        match cmd {
            WorkspaceCommand::Search {
                query,
                limit,
                reply,
            } => {
                let result = match &self.state {
                    WorkspaceState::Ready { paths, .. } => Ok(score_paths(paths, &query, limit)),
                    WorkspaceState::Failed(e) => Err(e.clone()),
                    WorkspaceState::Loading => Err("workspace still loading".to_string()),
                };
                let _ = reply.send(result);
                true
            }
            WorkspaceCommand::Shutdown => false,
        }
    }

    async fn handle_event(&mut self, event: DebouncedEvent) -> Result<(), String> {
        // `need_rescan()` is set when the watcher dropped events
        // (inotify queue overflow, FSEvents coalescence). Drop the
        // workspace and let the next search re-bootstrap.
        if event.need_rescan() {
            tracing::debug!(root = ?self.root, "watcher signaled rescan; self-invalidating");
            self.request_self_invalidate();
            return Ok(());
        }

        // Any change to a gitignore-defining file invalidates the
        // cached path set AND the cached gitignore matcher; rebuild
        // from scratch.
        if event_touches_ignore_file(&event.event.paths) {
            tracing::debug!(root = ?self.root, "gitignore-defining file changed; self-invalidating");
            self.request_self_invalidate();
            return Ok(());
        }

        self.apply_event(&event.event).await;
        Ok(())
    }

    /// Ask the registry to forget us and shut down. If the weak ref is
    /// already dead the registry's dropped — we can just exit ourselves
    /// by closing our command channel. The Shutdown message we'll get
    /// back from the registry breaks the run loop cleanly.
    fn request_self_invalidate(&self) {
        if let Some(registry) = self.registry.upgrade() {
            let root = self.root.clone();
            tokio::spawn(async move {
                registry.invalidate_workspace(&root);
            });
        }
    }

    async fn apply_event(&mut self, event: &notify::Event) {
        let root = self.root.clone();
        match &event.kind {
            EventKind::Create(CreateKind::Folder) => {
                for path in &event.paths {
                    if path.starts_with(&root) {
                        self.absorb_new_subtree(path.clone()).await;
                    }
                }
            }
            EventKind::Create(CreateKind::File) => {
                for path in &event.paths {
                    if path.starts_with(&root) {
                        self.insert_file_if_not_ignored(path);
                    }
                }
            }
            EventKind::Create(CreateKind::Any | CreateKind::Other) => {
                for path in &event.paths {
                    if !path.starts_with(&root) {
                        continue;
                    }
                    match std::fs::metadata(path) {
                        Ok(meta) if meta.is_dir() => {
                            self.absorb_new_subtree(path.clone()).await;
                        }
                        Ok(_) => {
                            self.insert_file_if_not_ignored(path);
                        }
                        Err(_) => { /* gone already */ }
                    }
                }
            }
            EventKind::Remove(
                RemoveKind::Folder | RemoveKind::File | RemoveKind::Any | RemoveKind::Other,
            ) => {
                for path in &event.paths {
                    self.absorb_removal(path).await;
                }
            }
            EventKind::Modify(ModifyKind::Name(rename_mode)) => {
                self.apply_rename(&event.paths, *rename_mode).await;
            }
            EventKind::Modify(_) | EventKind::Access(_) | EventKind::Any | EventKind::Other => {}
        }
    }

    async fn absorb_new_subtree(&mut self, new_dir: PathBuf) {
        if new_dir.file_name().is_some_and(|n| n == ".git") {
            return;
        }

        // Gitignored directories created after bootstrap (e.g.
        // `target/`, `node_modules/`) are visible because their parent
        // is watched but the bootstrap walker would have excluded them.
        // Starting an `ignore::WalkBuilder` *at* an ignored directory
        // does not re-apply the ancestor rule, so we gate here.
        if let WorkspaceState::Ready {
            gitignore: Some(gi),
            ..
        } = &self.state
        {
            if gi.matched(&new_dir, true).is_ignore() {
                return;
            }
        }

        let walk_root = new_dir.clone();
        let root_clone = self.root.clone();
        let result = tokio::task::spawn_blocking(move || walk_subtree(&root_clone, &walk_root))
            .await
            .ok();
        let Some((new_files, new_dirs)) = result else {
            return;
        };

        let imports_ignore_rules = new_files
            .iter()
            .any(|f| f.ends_with("/.gitignore") || f.ends_with("/.ignore"));
        if imports_ignore_rules {
            tracing::debug!(
                root = ?self.root,
                ?new_dir,
                "subtree contained ignore file; self-invalidating"
            );
            self.request_self_invalidate();
            return;
        }

        let mut dirs_to_subscribe = Vec::new();
        if let WorkspaceState::Ready {
            paths,
            watched_dirs,
            ..
        } = &mut self.state
        {
            for dir in new_dirs {
                if !watched_dirs.contains(&dir) {
                    dirs_to_subscribe.push(dir);
                }
            }
            paths.extend(new_files);
        }

        for dir in dirs_to_subscribe {
            if self.subscribe_dir(dir.clone()).await {
                if let WorkspaceState::Ready { watched_dirs, .. } = &mut self.state {
                    watched_dirs.insert(dir);
                }
            }
        }
    }

    fn insert_file_if_not_ignored(&mut self, path: &Path) {
        let WorkspaceState::Ready {
            paths, gitignore, ..
        } = &mut self.state
        else {
            return;
        };
        let Some(rel) = relative_string(&self.root, path) else {
            return;
        };
        if gitignore
            .as_ref()
            .is_some_and(|gi| gi.matched(path, false).is_ignore())
        {
            return;
        }
        paths.insert(rel);
    }

    async fn absorb_removal(&mut self, gone: &Path) {
        let Some(rel) = relative_string(&self.root, gone) else {
            return;
        };

        if rel.is_empty() {
            // Workspace root itself removed. Self-invalidate so the
            // next search re-bootstraps a fresh tree.
            self.request_self_invalidate();
            return;
        }

        let prefix = format!("{rel}/");
        let mut dirs_to_drop: Vec<PathBuf> = Vec::new();
        if let WorkspaceState::Ready {
            paths,
            watched_dirs,
            ..
        } = &mut self.state
        {
            paths.remove(&rel);
            paths.retain(|p| !p.starts_with(&prefix));
            dirs_to_drop = watched_dirs
                .iter()
                .filter(|d| d.as_path() == gone || d.starts_with(gone))
                .cloned()
                .collect();
            for dir in &dirs_to_drop {
                watched_dirs.remove(dir);
            }
        }
        for dir in dirs_to_drop {
            self.unsubscribe_dir(dir).await;
        }
    }

    async fn apply_rename(&mut self, paths: &[PathBuf], mode: RenameMode) {
        match mode {
            RenameMode::Both if paths.len() >= 2 => {
                self.absorb_removal(&paths[0]).await;
                self.absorb_create_unknown_kind(&paths[1]).await;
            }
            RenameMode::From => {
                for path in paths {
                    self.absorb_removal(path).await;
                }
            }
            RenameMode::To => {
                for path in paths {
                    self.absorb_create_unknown_kind(path).await;
                }
            }
            RenameMode::Any | RenameMode::Both | RenameMode::Other => {
                for path in paths {
                    if path.exists() {
                        self.absorb_create_unknown_kind(path).await;
                    } else {
                        self.absorb_removal(path).await;
                    }
                }
            }
        }
    }

    async fn absorb_create_unknown_kind(&mut self, path: &Path) {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => self.absorb_new_subtree(path.to_path_buf()).await,
            Ok(_) => self.insert_file_if_not_ignored(path),
            Err(_) => {}
        }
    }

    /// Subscribe the workspace to events under `dir`. Returns `false`
    /// when the underlying notify backend refused the watch (typically
    /// `fs.inotify.max_user_watches` exhaustion); callers must not
    /// record refused dirs as "covered" or the subtree would silently
    /// lose updates.
    async fn subscribe_dir(&self, dir: PathBuf) -> bool {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .debouncer_tx
            .send(DebouncerCommand::Subscribe {
                dir: dir.clone(),
                workspace_id: self.workspace_id,
                sender: self.events_tx.clone(),
                reply: reply_tx,
            })
            .await
            .is_err()
        {
            tracing::debug!(?dir, "debouncer actor gone; cannot subscribe");
            return false;
        }
        match reply_rx.await {
            Ok(Ok(())) => true,
            Ok(Err(e)) => {
                tracing::debug!(?dir, ?e, "failed to register file-index watch");
                false
            }
            Err(_) => {
                tracing::debug!(?dir, "debouncer dropped subscribe reply");
                false
            }
        }
    }

    async fn unsubscribe_dir(&self, dir: PathBuf) {
        let _ = self
            .debouncer_tx
            .send(DebouncerCommand::Unsubscribe {
                dir,
                workspace_id: self.workspace_id,
            })
            .await;
    }
}

// ---------------------------------------------------------------------------
// Debouncer actor
// ---------------------------------------------------------------------------

/// Resolve the directory that should be watched for the workspace's
/// "info exclude" file. Uses `git rev-parse --git-path info/exclude`
/// to handle both regular repos and linked worktrees uniformly; falls
/// back to `<root>/.git/info` when git isn't available or refuses to
/// answer (e.g. when `root` isn't inside a git tree).
fn bootstrap_excludes_dir(root: &Path) -> Option<PathBuf> {
    if let Some(path) = git_resolve_info_exclude(root) {
        return path.parent().map(Path::to_path_buf);
    }
    Some(root.join(".git").join("info"))
}

enum DebouncerCommand {
    Subscribe {
        dir: PathBuf,
        workspace_id: WorkspaceId,
        sender: mpsc::Sender<DebouncedEvent>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Unsubscribe {
        dir: PathBuf,
        workspace_id: WorkspaceId,
    },
    UnsubscribeAll {
        workspace_id: WorkspaceId,
    },
    /// One batch from the debouncer. Pushed from the bridge thread.
    DebouncedBatch(DebounceEventResult),
}

struct DebouncerActor {
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    /// Refcounted subscription table: directory -> set of (`workspace_id`,
    /// sender). When the inner map empties we call `unwatch(dir)` so the
    /// kernel inotify watch is released.
    subscriptions: HashMap<PathBuf, HashMap<WorkspaceId, mpsc::Sender<DebouncedEvent>>>,
}

impl DebouncerActor {
    /// Spawn the actor. `inbound_tx` is the registry-side clone of the
    /// command channel — the bridge thread holds a `WeakSender` derived
    /// from it so debouncer batches travel through the same mpsc as
    /// subscribe/unsubscribe commands without keeping the channel alive
    /// past the registry's lifetime.
    ///
    /// Lifetime story: the actor exits when its `rx` returns None,
    /// which happens once every Sender clone is dropped. The registry
    /// holds one; each workspace actor holds one; the bridge thread
    /// upgrades a `WeakSender` per batch so it never keeps the channel
    /// alive on its own. When the registry and all workspaces drop,
    /// the next upgrade fails and the bridge thread exits, which
    /// allows the debouncer to drop and the actor task to terminate.
    fn spawn(
        inbound_tx: &mpsc::Sender<DebouncerCommand>,
        mut rx: mpsc::Receiver<DebouncerCommand>,
    ) -> Result<(), notify::Error> {
        let (bridge_tx, bridge_rx) = std::sync::mpsc::channel::<DebounceEventResult>();
        let debouncer = new_debouncer(DEBOUNCE_WINDOW, None, bridge_tx)?;

        let weak = inbound_tx.downgrade();
        std::thread::Builder::new()
            .name("file-index-debounce-bridge".into())
            .spawn(move || {
                for batch in bridge_rx {
                    let Some(strong) = weak.upgrade() else { break };
                    // blocking_send applies natural backpressure when
                    // the actor falls behind — events accumulate in
                    // the bounded channel and the bridge waits rather
                    // than dropping. A wedged workspace cannot block
                    // the bridge because dispatch_batch uses try_send
                    // per-workspace.
                    if strong
                        .blocking_send(DebouncerCommand::DebouncedBatch(batch))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(notify::Error::io)?;

        let mut actor = Self {
            debouncer,
            subscriptions: HashMap::new(),
        };

        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                actor.handle(cmd);
            }
        });

        Ok(())
    }

    fn handle(&mut self, cmd: DebouncerCommand) {
        match cmd {
            DebouncerCommand::Subscribe {
                dir,
                workspace_id,
                sender,
                reply,
            } => {
                let result = self.subscribe(&dir, workspace_id, sender);
                let _ = reply.send(result);
            }
            DebouncerCommand::Unsubscribe { dir, workspace_id } => {
                self.unsubscribe(&dir, workspace_id);
            }
            DebouncerCommand::UnsubscribeAll { workspace_id } => {
                let dirs: Vec<PathBuf> = self.subscriptions.keys().cloned().collect();
                for dir in dirs {
                    self.unsubscribe(&dir, workspace_id);
                }
            }
            DebouncerCommand::DebouncedBatch(batch) => {
                self.dispatch_batch(batch);
            }
        }
    }

    fn subscribe(
        &mut self,
        dir: &Path,
        workspace_id: WorkspaceId,
        sender: mpsc::Sender<DebouncedEvent>,
    ) -> Result<(), String> {
        let entry = self.subscriptions.entry(dir.to_path_buf());
        let first_subscriber = matches!(entry, std::collections::hash_map::Entry::Vacant(_));
        let subs = entry.or_default();
        let prior = subs.insert(workspace_id, sender);

        if first_subscriber {
            if let Err(e) = self.debouncer.watch(dir, RecursiveMode::NonRecursive) {
                // Roll back the insertion so refcount stays honest.
                self.subscriptions.remove(dir);
                return Err(format!("{e}"));
            }
        } else if prior.is_none() {
            tracing::trace!(?dir, "additional file-index workspace subscribed");
        }
        Ok(())
    }

    fn unsubscribe(&mut self, dir: &Path, workspace_id: WorkspaceId) {
        let Some(subs) = self.subscriptions.get_mut(dir) else {
            return;
        };
        subs.remove(&workspace_id);
        if subs.is_empty() {
            self.subscriptions.remove(dir);
            let _ = self.debouncer.unwatch(dir);
        }
    }

    fn dispatch_batch(&mut self, batch: DebounceEventResult) {
        let events = match batch {
            Ok(events) => events,
            Err(errors) => {
                for err in errors {
                    tracing::debug!(?err, "file-index watcher error");
                }
                return;
            }
        };

        // Snapshot before send — a slow receiver should not stall the
        // whole debouncer loop. We use try_send; a full queue means
        // that workspace is wedged and we drop the event rather than
        // blocking every other workspace.
        for event in events {
            // Route by directory containment: an event's paths live
            // *under* one or more subscribed directories. The direct
            // hit (event's path's parent) is fast; we also consider
            // ancestor directories so an event under a watched root
            // reaches every workspace whose root prefixes the path.
            let event_paths = &event.event.paths;
            let mut routed: HashSet<WorkspaceId> = HashSet::new();
            for path in event_paths {
                for (subscribed_dir, subs) in &self.subscriptions {
                    if path.starts_with(subscribed_dir) {
                        for (wid, sender) in subs {
                            if routed.insert(*wid) {
                                let _ = sender.try_send(event.clone());
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Free functions: walking, gitignore resolution, scoring
// ---------------------------------------------------------------------------

fn canonicalize_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Run `git -C <root> rev-parse --show-toplevel` and return the toplevel
/// directory if it differs from `root`. Used to detect that the
/// conversation cwd is a subdirectory of a repo so parent `.gitignore`
/// rules apply.
fn git_toplevel(root: &Path) -> Option<PathBuf> {
    if which::which("git").is_err() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = std::str::from_utf8(&out.stdout).ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    let toplevel = PathBuf::from(raw);
    Some(canonicalize_or_self(&toplevel))
}

/// Run `git -C <root> rev-parse --git-path info/exclude` to resolve the
/// per-checkout exclude path. Works for both regular repos
/// (`<toplevel>/.git/info/exclude`) and linked worktrees
/// (`<common_gitdir>/info/exclude`).
fn git_resolve_info_exclude(root: &Path) -> Option<PathBuf> {
    if which::which("git").is_err() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--git-path", "info/exclude"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = std::str::from_utf8(&out.stdout).ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    let mut path = PathBuf::from(raw);
    if !path.is_absolute() {
        path = root.join(path);
    }
    Some(path)
}

/// True if `root/.git` exists as a directory OR as a gitlink file
/// pointing at an alternative gitdir.
fn root_is_in_git_tree(root: &Path) -> bool {
    let git_path = root.join(".git");
    if git_path.is_dir() {
        return true;
    }
    // Linked worktree: `.git` is a regular file containing
    // `gitdir: <path>`. Treat as a git tree even without resolving the
    // file, since the walker's behaviour is conditioned on git_ignore=true
    // finding *any* git context.
    if git_path.is_file() {
        return true;
    }
    false
}

/// One-pass walk that returns (sorted relative-path set, watched-dir set,
/// composite gitignore matcher).
///
/// The gitignore matcher is built from every `.gitignore` file the walk
/// encountered (root-level, nested, plus the resolved per-checkout
/// `info/exclude` and the global excludes file). `GitignoreBuilder::add`
/// preserves the rule's originating directory, so a rule in
/// `<root>/src/.gitignore` only matches paths under `<root>/src`. When
/// the conversation cwd is a *subdirectory* of a repo, the bootstrap
/// uses the repo toplevel as the matcher root so parent `.gitignore`
/// rules apply correctly.
fn walk_workspace(root: &Path) -> (BTreeSet<String>, HashSet<PathBuf>, Option<Gitignore>) {
    let mut files = BTreeSet::new();
    let mut dirs = HashSet::new();

    let is_git_tree = root_is_in_git_tree(root);

    // If the cwd is a subdirectory of a repo (toplevel differs from
    // root), build the matcher anchored at the toplevel so parent
    // `.gitignore` rules apply. Without this, a cwd of
    // `/repo/packages/app` with a `dist/` rule in `/repo/.gitignore`
    // would silently miss the rule because `root.join(".git")` doesn't
    // exist. `git rev-parse --show-toplevel` works whether or not
    // `root` itself contains a `.git` entry, so we don't need to gate
    // it on `is_git_tree`.
    let toplevel = git_toplevel(root);
    let matcher_root = toplevel.clone().unwrap_or_else(|| root.to_path_buf());
    let in_git_repo = is_git_tree || toplevel.is_some();

    let mut gi_builder = GitignoreBuilder::new(&matcher_root);

    if in_git_repo {
        // Pull in the repo-level .gitignore if it exists.
        let _ = gi_builder.add(matcher_root.join(".gitignore"));

        // `info/exclude` and the global excludes file are NOT
        // directory-scoped to themselves — git applies their patterns
        // against paths relative to the workspace root. `add_line` keeps
        // each pattern scoped to the builder's root, which is what we
        // want here. Loading them via `add()` would scope them to the
        // gitdir, which never contains any project file.
        //
        // Resolution goes through `git rev-parse --git-path info/exclude`,
        // which handles both regular repos and linked worktrees (where
        // `.git` is a file pointing at a separate gitdir).
        if let Some(exclude_path) = git_resolve_info_exclude(root) {
            seed_pattern_lines(&mut gi_builder, &exclude_path);
        } else {
            // Fallback heuristic when `which git` is missing or
            // rev-parse failed: the colocated `.git/info/exclude`.
            seed_pattern_lines(
                &mut gi_builder,
                &root.join(".git").join("info").join("exclude"),
            );
        }
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
                let name = entry.file_name();
                let is_gitignore = name == ".gitignore" && in_git_repo && path != root_gitignore;
                let is_ignore = name == ".ignore" && path != root_ignore;
                if is_gitignore || is_ignore {
                    let _ = gi_builder.add(path);
                }
            }
            None => {}
        }
    }

    let gitignore = if in_git_repo || root.join(".ignore").exists() {
        gi_builder.build().ok().or_else(|| {
            tracing::debug!(?root, "failed to build composite gitignore");
            None
        })
    } else {
        None
    };

    (files, dirs, gitignore)
}

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

fn resolve_global_gitignore() -> Option<PathBuf> {
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
    let base = std::env::var_os("XDG_CONFIG_HOME").map_or_else(
        || {
            phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()
                .home()
                .join(".config")
        },
        PathBuf::from,
    );
    let candidate = base.join("git").join("ignore");
    candidate.is_file().then_some(candidate)
}

fn event_touches_ignore_file(paths: &[PathBuf]) -> bool {
    paths.iter().any(|p| {
        let name = p.file_name().and_then(|n| n.to_str());
        if matches!(name, Some(".gitignore" | ".ignore")) {
            return true;
        }
        // `.git/info/exclude` (regular repos) and `<gitdir>/info/exclude`
        // (linked worktrees) both terminate in `info/exclude`; match on
        // the suffix and accept both shapes.
        p.ends_with("info/exclude")
    })
}

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

    async fn search(indexer: &Arc<WorkspaceIndexer>, root: &Path, q: &str) -> Vec<String> {
        indexer
            .search(root.to_path_buf(), q, 50)
            .await
            .expect("search")
    }

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

        let indexer = WorkspaceIndexer::new().await.unwrap();
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

        let indexer = WorkspaceIndexer::new().await.unwrap();
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

        let indexer = WorkspaceIndexer::new().await.unwrap();
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

        let indexer = WorkspaceIndexer::new().await.unwrap();
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

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let _ = search(&indexer, root, "").await;

        fs::create_dir_all(root.join("src/new_module")).unwrap();
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

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let first = Instant::now();
        let _ = search(&indexer, root, "f100").await;
        let _bootstrap = first.elapsed();

        let second = Instant::now();
        let _ = search(&indexer, root, "f150").await;
        let cached = second.elapsed();

        assert!(
            cached < Duration::from_millis(25),
            "cached search took {cached:?}, expected sub-25ms"
        );
    }

    #[tokio::test]
    async fn gitignored_create_file_does_not_enter_index() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fake_git(root);
        write(&root.join(".gitignore"), "*.log\n");
        write(&root.join("src/main.rs"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let _ = search(&indexer, root, "").await;

        write(&root.join("src/debug.log"), "noise");

        tokio::time::sleep(Duration::from_millis(400)).await;
        let results = search(&indexer, root, "debug").await;
        assert!(
            !results.iter().any(|p| p == "src/debug.log"),
            "gitignored file leaked into index: {results:?}"
        );
    }

    #[tokio::test]
    async fn nested_workspaces_both_see_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path();
        let child = parent.join("packages/app");
        fs::create_dir_all(&child).unwrap();
        write(&parent.join("README.md"), "");
        write(&child.join("index.ts"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
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

    #[tokio::test]
    async fn root_removal_purges_cached_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("workspace");
        fs::create_dir_all(&root).unwrap();
        write(&root.join("src/a.rs"), "");
        write(&root.join("src/b.rs"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let primed = search(&indexer, &root, "").await;
        assert!(primed.iter().any(|p| p == "src/a.rs"));

        fs::remove_dir_all(&root).unwrap();
        fs::create_dir_all(&root).unwrap();
        write(&root.join("src/c.rs"), "");

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

    #[tokio::test]
    async fn gitignored_dir_creation_does_not_enter_index() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fake_git(root);
        write(&root.join(".gitignore"), "target/\n");
        write(&root.join("src/keep.rs"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let _ = search(&indexer, root, "").await;

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

    #[tokio::test]
    async fn invalidating_parent_preserves_child_workspace_watches() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path();
        let child = parent.join("packages/app");
        fs::create_dir_all(&child).unwrap();
        write(&parent.join("README.md"), "");
        write(&child.join("index.ts"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let _ = search(&indexer, parent, "").await;
        let _ = search(&indexer, &child, "").await;

        // Invalidate the parent — simulates a rescan signal on the
        // outer workspace.
        indexer.invalidate_workspace(parent);

        // Give the actor a moment to drop its subscriptions.
        tokio::time::sleep(Duration::from_millis(150)).await;

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

    #[tokio::test]
    async fn gitignore_edit_invalidates_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fake_git(root);
        write(&root.join(".gitignore"), "");
        write(&root.join("src/keep.rs"), "");
        write(&root.join("src/maybe.log"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let initial = search(&indexer, root, "").await;
        assert!(
            initial.iter().any(|p| p == "src/maybe.log"),
            "initial: {initial:?}"
        );

        write(&root.join(".gitignore"), "*.log\n");

        let gone = wait_for_path(&indexer, root, "", Duration::from_secs(2), |results| {
            !results.iter().any(|p| p == "src/maybe.log")
        })
        .await;
        assert!(gone, "gitignore edit didn't refresh the index");
    }

    #[tokio::test]
    async fn second_walk_absorbs_files_written_after_first_walk() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("src/a.rs"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let _ = search(&indexer, root, "").await;

        write(&root.join("src/b.rs"), "");
        let saw_b = wait_for_path(&indexer, root, "", Duration::from_secs(2), |results| {
            results.iter().any(|p| p == "src/b.rs")
        })
        .await;
        assert!(saw_b, "watcher missed post-bootstrap create");

        drop(indexer);
        let fresh = WorkspaceIndexer::new().await.unwrap();
        let after_restart = search(&fresh, root, "").await;
        assert!(
            after_restart.iter().any(|p| p == "src/b.rs"),
            "fresh bootstrap missed pre-existing file: {after_restart:?}"
        );
    }

    #[tokio::test]
    async fn events_during_loading_are_not_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..500 {
            write(&root.join(format!("src/f{i:03}.rs")), "");
        }

        let indexer = WorkspaceIndexer::new().await.unwrap();

        let indexer_a = indexer.clone();
        let root_a = root.to_path_buf();
        let bs = tokio::spawn(async move {
            indexer_a.search(root_a, "", 1).await.expect("search");
        });

        write(&root.join("src/race.rs"), "");

        bs.await.unwrap();

        let saw_race = wait_for_path(&indexer, root, "race", Duration::from_secs(2), |results| {
            results.iter().any(|p| p == "src/race.rs")
        })
        .await;
        assert!(saw_race, "race file vanished during bootstrap");
    }

    #[tokio::test]
    async fn git_info_exclude_edit_triggers_invalidation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git/info")).unwrap();
        write(&root.join(".git/info/exclude"), "");
        write(&root.join("src/test.tmp"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let initial = search(&indexer, root, "").await;
        assert!(
            initial.iter().any(|p| p == "src/test.tmp"),
            "initial: {initial:?}"
        );

        write(&root.join(".git/info/exclude"), "*.tmp\n");

        let gone = wait_for_path(&indexer, root, "", Duration::from_secs(2), |results| {
            !results.iter().any(|p| p == "src/test.tmp")
        })
        .await;
        assert!(gone, ".git/info/exclude edit didn't trigger refresh");
    }

    #[tokio::test]
    async fn git_info_exclude_patterns_match_workspace_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git/info")).unwrap();
        write(&root.join(".git/info/exclude"), "*.tmp\n");
        write(&root.join("src/keep.rs"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let _ = search(&indexer, root, "").await;

        write(&root.join("src/debug.tmp"), "");

        tokio::time::sleep(Duration::from_millis(400)).await;
        let results = search(&indexer, root, "").await;
        assert!(
            !results.iter().any(|p| p == "src/debug.tmp"),
            "exclude file's pattern wasn't applied to workspace path: {results:?}"
        );
    }

    #[tokio::test]
    async fn non_git_workspace_does_not_apply_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join(".gitignore"), "*.log\n");
        write(&root.join("src/main.rs"), "");
        write(&root.join("src/existing.log"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let initial = search(&indexer, root, "").await;
        assert!(
            initial.iter().any(|p| p == "src/existing.log"),
            "non-git bootstrap should include the .log file: {initial:?}"
        );

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

    #[tokio::test]
    async fn imported_subtree_with_gitignore_invalidates_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fake_git(root);
        write(&root.join("src/main.rs"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let _ = search(&indexer, root, "").await;

        fs::create_dir_all(root.join("external/sub")).unwrap();
        write(&root.join("external/.gitignore"), "*.tmp\n");
        write(&root.join("external/sub/keep.rs"), "");

        tokio::time::sleep(Duration::from_millis(400)).await;

        write(&root.join("external/junk.tmp"), "");
        tokio::time::sleep(Duration::from_millis(400)).await;

        let results = search(&indexer, root, "").await;
        assert!(
            !results.iter().any(|p| p == "external/junk.tmp"),
            "imported .gitignore rule wasn't honored after import: {results:?}"
        );
    }

    #[tokio::test]
    async fn bootstrap_invalidated_mid_flight_does_not_leak_watches() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..200 {
            write(&root.join(format!("src/f{i:03}.rs")), "");
        }

        let indexer = WorkspaceIndexer::new().await.unwrap();

        for _ in 0..20 {
            let _ = search(&indexer, root, "").await;
            indexer.invalidate_workspace(&canonicalize_or_self(root));
        }

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

    // -------------------------------------------------------------------
    // Regression tests for round-4 review findings
    // -------------------------------------------------------------------

    /// Bug #1 — nested cwd parent gitignore. When the conversation cwd
    /// is a subdirectory of a repo, `root.join(".git").exists()`
    /// returns false because `.git` lives at the repo toplevel. The
    /// bootstrap must consult `git rev-parse --show-toplevel` to find
    /// the actual toplevel and anchor the gitignore matcher there, so
    /// parent rules still apply.
    #[tokio::test]
    async fn nested_cwd_inherits_parent_gitignore() {
        if which::which("git").is_err() {
            eprintln!("skipping: git not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let toplevel = tmp.path();
        // Make `toplevel` a real git repo so `git rev-parse` resolves.
        let init = std::process::Command::new("git")
            .arg("-C")
            .arg(toplevel)
            .args(["init", "-q"])
            .output()
            .unwrap();
        assert!(init.status.success(), "git init failed");

        write(&toplevel.join(".gitignore"), "dist/\n");
        let cwd = toplevel.join("packages/app");
        fs::create_dir_all(&cwd).unwrap();
        write(&cwd.join("src/main.ts"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let _ = search(&indexer, &cwd, "").await;

        // Create a `dist/` file under the nested cwd. The parent
        // `.gitignore` excludes it — the cached matcher must too.
        fs::create_dir_all(cwd.join("dist")).unwrap();
        write(&cwd.join("dist/x.js"), "");

        tokio::time::sleep(Duration::from_millis(400)).await;
        let results = search(&indexer, &cwd, "").await;
        assert!(
            !results.iter().any(|p| p.starts_with("dist/")),
            "parent .gitignore not honored from nested cwd: {results:?}"
        );
    }

    /// Bug #3 — events arriving during bootstrap (especially events on
    /// gitignore-defining files that would have triggered the old code
    /// to take a shared lock from inside a worker that already held one)
    /// must not deadlock. Wrap the entire test body in `tokio::time::timeout`
    /// so a regression would manifest as a missed deadline.
    #[tokio::test]
    async fn bootstrap_with_concurrent_gitignore_event_does_not_deadlock() {
        let work = async {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            fake_git(root);
            write(&root.join(".gitignore"), "*.log\n");
            for i in 0..400 {
                write(&root.join(format!("src/f{i:03}.rs")), "");
            }

            let indexer = WorkspaceIndexer::new().await.unwrap();

            // Kick off the bootstrap.
            let indexer_a = indexer.clone();
            let root_a = root.to_path_buf();
            let bs =
                tokio::spawn(async move { indexer_a.search(root_a, "", 1).await.expect("search") });

            // During bootstrap, create a new subdirectory containing
            // a `.gitignore` file. In the old code this triggered an
            // invalidation that took a lock the bootstrap thread held.
            fs::create_dir_all(root.join("external/sub")).unwrap();
            write(&root.join("external/.gitignore"), "*.tmp\n");

            bs.await.unwrap();

            // Settle: invalidation should have triggered, follow-up
            // search must complete.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = search(&indexer, root, "").await;
        };

        tokio::time::timeout(Duration::from_secs(5), work)
            .await
            .expect("bootstrap+gitignore-event deadlocked");
    }

    /// Bug #7 — linked worktrees. In a linked git worktree `.git` is a
    /// file (gitlink) pointing at a separate gitdir, and the local
    /// exclude lives at `<gitdir>/info/exclude`, NOT `root/.git/info/exclude`.
    /// `git rev-parse --git-path info/exclude` resolves both shapes.
    #[tokio::test]
    async fn linked_worktree_honors_gitdir_info_exclude() {
        if which::which("git").is_err() {
            eprintln!("skipping: git not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let main_repo = tmp.path().join("main");
        let linked_wt = tmp.path().join("wt");
        fs::create_dir_all(&main_repo).unwrap();

        let run = |args: &[&str], cwd: &Path| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .output()
                .expect("git invocation");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
            out
        };

        // Set up a real repo with one commit so `worktree add` works.
        run(&["init", "-q", "-b", "main"], &main_repo);
        // Configure identity so commit doesn't refuse.
        run(
            &["config", "user.email", "file-index-test@example.com"],
            &main_repo,
        );
        run(&["config", "user.name", "file index test"], &main_repo);
        write(&main_repo.join("README.md"), "# main\n");
        run(&["add", "README.md"], &main_repo);
        run(&["commit", "-q", "-m", "initial"], &main_repo);

        // Add a linked worktree at /tmp/.../wt. Use a feature branch
        // so we don't try to check out `main` twice.
        run(
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat",
                linked_wt.to_str().unwrap(),
            ],
            &main_repo,
        );
        assert!(
            linked_wt.join(".git").is_file(),
            "linked worktree's .git should be a gitlink file"
        );

        // Find the common gitdir (the location `info/exclude` lives in
        // for a linked worktree's per-checkout excludes is its own
        // worktree gitdir, but the common gitdir holds the shared
        // info/exclude). Use `git rev-parse --git-path info/exclude`
        // from inside the worktree — this is exactly what the bootstrap
        // does and what we're regression-testing.
        let exclude_path = std::process::Command::new("git")
            .arg("-C")
            .arg(&linked_wt)
            .args(["rev-parse", "--git-path", "info/exclude"])
            .output()
            .expect("rev-parse");
        let raw = String::from_utf8_lossy(&exclude_path.stdout)
            .trim()
            .to_string();
        let mut exclude = PathBuf::from(raw);
        if !exclude.is_absolute() {
            exclude = linked_wt.join(exclude);
        }
        fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        write(&exclude, "secret.txt\n");

        // Stage a `secret.txt` inside the linked worktree.
        write(&linked_wt.join("src/keep.rs"), "");

        let indexer = WorkspaceIndexer::new().await.unwrap();
        let _ = search(&indexer, &linked_wt, "").await;

        // Create `secret.txt` via the watcher path. The cached
        // matcher should reject it because info/exclude from the
        // resolved gitdir included `secret.txt`.
        write(&linked_wt.join("secret.txt"), "shh");
        tokio::time::sleep(Duration::from_millis(400)).await;

        let results = search(&indexer, &linked_wt, "").await;
        assert!(
            !results.iter().any(|p| p == "secret.txt"),
            "linked-worktree info/exclude wasn't applied: {results:?}"
        );
    }
}
