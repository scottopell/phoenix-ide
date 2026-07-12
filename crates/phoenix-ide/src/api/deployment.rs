//! "About this deployment" endpoint.
//!
//! Serves `GET /api/deployment`: a read-only snapshot of build identity,
//! network binding + TLS posture, live resource usage, on-disk locations with
//! sizes, and the log sink. See `specs/deployment-info/`.
//!
//! Static facts (binding, TLS, the on-disk layout) are resolved once at startup
//! into [`DeploymentConfig`] and threaded through [`AppState`]. Sampled facts
//! (resource usage, sizes, `sampled_at`) are measured per request so a refresh
//! yields current values.

use super::AppState;
use crate::api::process_sample::{
    group_member_identities_for_sampling, process_identity_for_sampling,
    sample_process_observations, session_member_identities_for_sampling, ProcessIdentity,
    ProcessObservation,
};
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use phoenix_core::domain::db_schema::{ConvMode, Conversation};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use ts_rs::TS;

// ============================================================
// Captured static configuration (server-side, not on the wire)
// ============================================================

/// How a configured on-disk location should be sized.
#[derive(Clone, Copy, Debug)]
pub enum MeasureMode {
    /// Stat a single file.
    File,
    /// Recurse a directory known to be small/owned.
    RecurseSmall,
    /// Known-large (e.g. a binary cache); report the path but do not walk it.
    NoMeasure,
    /// A glob/pattern location (e.g. per-scope profile dirs). Always reported
    /// as not-measured — existence of the literal pattern path is meaningless.
    Pattern,
    /// The attachment store while attachment bytes live inside the database.
    InlineDb,
}

/// A configured on-disk location to report.
#[derive(Clone, Debug)]
pub struct DiskLocation {
    pub category: DiskCategory,
    pub label: String,
    pub path: PathBuf,
    pub mode: MeasureMode,
}

/// Static deployment facts resolved once at startup and threaded through
/// [`AppState`]. Sampled facts are computed per request in [`deployment_info`].
#[derive(Clone, Debug)]
pub struct DeploymentConfig {
    pub bind_address: SocketAddr,
    pub tls: TlsInfo,
    pub log: LogInfo,
    pub locations: Vec<DiskLocation>,
}

// ============================================================
// Wire types (exported to ui/src/generated/ via ts-rs)
// ============================================================

/// Snapshot returned by `GET /api/deployment`.
#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct DeploymentInfo {
    pub build: BuildInfo,
    pub network: NetworkInfo,
    pub log: LogInfo,
    /// Whether the requesting browser is on the server host, and so may use
    /// host-local actions like revealing a path in the OS file manager. False
    /// for any remote browser — the file-manager window opens on the server's
    /// desktop, which a remote user cannot see.
    pub local_access: bool,
    pub sampled_at: DateTime<Utc>,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct DeploymentDiskInfo {
    pub disk: Vec<DiskEntry>,
    pub managed_worktrees: Vec<ManagedWorktreeDiskEntry>,
    pub sampled_at: DateTime<Utc>,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BuildInfo {
    pub version: String,
    pub git_sha: String,
    pub started_at: Option<DateTime<Utc>>,
    pub uptime_seconds: u64,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct NetworkInfo {
    pub bind_address: String,
    pub socket_activated: bool,
    pub tls: TlsInfo,
}

/// TLS posture. Reused as the captured config value and on the wire.
#[derive(Serialize, TS, Clone, Debug)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct TlsInfo {
    pub enabled: bool,
    /// `"auto"` (self-signed) or `"manual"` (provided certs); `None` when disabled.
    pub mode: Option<String>,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    pub ca_cert_path: Option<String>,
    /// Host names the auto cert is generated for; empty otherwise.
    pub hosts: Vec<String>,
}

impl TlsInfo {
    /// TLS disabled — the server is serving plain HTTP.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            mode: None,
            cert_path: None,
            key_path: None,
            ca_cert_path: None,
            hosts: Vec::new(),
        }
    }
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct AboutResourcesSnapshot {
    pub sampled_at: DateTime<Utc>,
    pub host: HostResources,
    pub managed_total: ManagedResourceTotals,
    pub categories: Vec<ManagedResourceCategory>,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct HostResources {
    pub logical_cpu_count: Option<u32>,
    pub cpu_busy_percent: Option<f32>,
    pub cpu_system_percent: Option<f32>,
    pub cpu_idle_percent: Option<f32>,
    pub total_memory_bytes: Option<u64>,
    pub available_memory_bytes: Option<u64>,
    pub used_memory_bytes: Option<u64>,
    pub load_average_one: Option<f64>,
    pub load_average_five: Option<f64>,
    pub load_average_fifteen: Option<f64>,
}

#[derive(Serialize, TS, Clone, Default)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ManagedResourceTotals {
    pub cpu_percent: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub process_count: u32,
    pub deduplicated_pid_count: u32,
}

#[derive(Serialize, TS, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ManagedResourceCategoryKind {
    Api,
    Bash,
    Browser,
    TmuxTerminal,
    Mcp,
}

#[derive(Serialize, TS, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ManagedResourceAttribution {
    Available,
    Unavailable,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ManagedResourceCategory {
    pub kind: ManagedResourceCategoryKind,
    pub label: String,
    pub attribution: ManagedResourceAttribution,
    pub reason: Option<String>,
    pub totals: ManagedResourceTotals,
    pub processes: Vec<ManagedProcessRow>,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ManagedProcessRow {
    pub name: String,
    pub category: ManagedResourceCategoryKind,
    pub pid: u32,
    pub cpu_percent: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub thread_count: Option<u32>,
    pub cpu_time_seconds: Option<f64>,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct DiskEntry {
    pub category: DiskCategory,
    pub label: String,
    pub path: String,
    pub size: DiskSize,
}

#[derive(Serialize, TS, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum DiskCategory {
    Database,
    DataDirectory,
    ManagedWorktrees,
    PrContext,
    BrowserCache,
    BrowserProfiles,
    Tls,
    Skills,
    Credentials,
    Attachments,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ManagedWorktreeDiskEntry {
    pub path: String,
    pub size: DiskSize,
    pub repository: Option<String>,
    pub branch_name: Option<String>,
    pub disposition: ManagedWorktreeDisposition,
}

#[derive(Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ManagedWorktreeDisposition {
    Live {
        conversation_id: String,
        slug: Option<String>,
        title: Option<String>,
        state: String,
        archived: bool,
    },
    Leftover {
        source_conversation_id: String,
        source_state: String,
        archived: bool,
        cleanup_allowed: bool,
    },
}

#[derive(Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ManagedWorktreeCleanupRequest {
    pub path: String,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ManagedWorktreeCleanupResponse {
    pub path: String,
    pub removed: bool,
}

/// The four semantically-distinct outcomes of sizing a location. A bare
/// nullable number cannot tell these apart, so they are modelled as a tagged
/// union (correct-by-construction).
#[derive(Serialize, TS, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum DiskSize {
    /// Size was measured.
    Measured { bytes: u64 },
    /// Intentionally not walked (known-large directory).
    NotMeasured,
    /// Path does not exist on disk.
    Absent,
    /// Bytes live inside the `SQLite` database (attachment store placeholder).
    InlineDb,
}

/// The deployment's active log sinks. Both are independent — logs fan out to
/// every enabled sink — so this mirrors the actual subscriber configuration
/// rather than picking one. Derived from [`crate::logging::LogConfig`].
#[derive(Serialize, TS, Clone, Debug)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct LogInfo {
    /// Logs are written to stdout (captured by the supervising process).
    pub stdout: bool,
    /// Absolute path of the process-owned log file, when file logging is active.
    pub file: Option<String>,
}

// ============================================================
// Handler
// ============================================================

/// `GET /api/deployment` — assemble and return a [`DeploymentInfo`] snapshot.
pub async fn deployment_info(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let cfg = &state.deployment;

    let build = BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_sha: env!("PHOENIX_GIT_SHA").to_string(),
        started_at: crate::hot_restart::started_at(),
        uptime_seconds: crate::hot_restart::uptime_secs(),
    };

    let network = NetworkInfo {
        bind_address: cfg.bind_address.to_string(),
        socket_activated: crate::hot_restart::is_socket_activated(),
        tls: cfg.tls.clone(),
    };

    Json(DeploymentInfo {
        build,
        network,
        log: cfg.log.clone(),
        local_access: super::local_reveal::client_is_local(peer.ip(), &headers),
        sampled_at: Utc::now(),
    })
}

pub async fn deployment_disk(State(state): State<AppState>) -> impl IntoResponse {
    Json(build_disk_info(&state).await)
}

pub async fn about_resources(State(state): State<AppState>) -> impl IntoResponse {
    Json(sample_about_resources(&state).await)
}

pub async fn cleanup_managed_worktree(
    State(state): State<AppState>,
    Json(request): Json<ManagedWorktreeCleanupRequest>,
) -> impl IntoResponse {
    match cleanup_managed_worktree_inner(&state, &request.path).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err((status, message)) => {
            (status, Json(serde_json::json!({ "error": message }))).into_response()
        }
    }
}

async fn cleanup_managed_worktree_inner(
    state: &AppState,
    path: &str,
) -> Result<ManagedWorktreeCleanupResponse, (StatusCode, String)> {
    let conversations = state
        .db
        .managed_worktree_conversations()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let owners: Vec<_> = conversations
        .iter()
        .filter(|conv| conv.conv_mode.worktree_path() == Some(path))
        .collect();
    if owners.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "worktree path is not known to Phoenix".to_string(),
        ));
    }
    let worktree = Path::new(path);
    let repo_root = exact_phoenix_worktree_root(worktree).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "worktree path is not a Phoenix-managed worktree path".to_string(),
        )
    })?;
    if owners
        .iter()
        .any(|conv| managed_worktree_scope_owner(conv, &conversations))
    {
        return Err((
            StatusCode::CONFLICT,
            "worktree is still owned by a live conversation".to_string(),
        ));
    }
    if !worktree.exists() {
        return Ok(ManagedWorktreeCleanupResponse {
            path: path.to_string(),
            removed: false,
        });
    }

    let branch_to_delete = cleanup_branch_for_leftover(owners[0]);
    let cleanup_repo_root = repo_root.clone();
    let cleanup_worktree = worktree.to_path_buf();
    let removed = tokio::task::spawn_blocking(move || {
        remove_leftover_worktree(&cleanup_repo_root, &cleanup_worktree, branch_to_delete)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(ManagedWorktreeCleanupResponse {
        path: path.to_string(),
        removed,
    })
}

fn exact_phoenix_worktree_root(path: &Path) -> Option<PathBuf> {
    match path.components().next_back()? {
        Component::Normal(name) if !name.is_empty() => {}
        Component::Prefix(_)
        | Component::RootDir
        | Component::CurDir
        | Component::ParentDir
        | Component::Normal(_) => return None,
    }
    let parent = path.parent()?;
    if parent.file_name()? != "worktrees" {
        return None;
    }
    let phoenix_dir = parent.parent()?;
    if phoenix_dir.file_name()? != ".phoenix" {
        return None;
    }
    phoenix_dir.parent().map(Path::to_path_buf)
}

fn remove_leftover_worktree(
    repo_root: &Path,
    worktree: &Path,
    branch_to_delete: Option<String>,
) -> Result<bool, String> {
    let worktree_str = worktree.to_string_lossy().to_string();
    let removed = match crate::git_ops::run_git(
        repo_root,
        &["worktree", "remove", &worktree_str, "--force"],
    ) {
        Ok(_) => true,
        Err(git_err) => {
            tracing::debug!(error = %git_err, path = %worktree.display(), "leftover worktree cleanup: git remove failed; trying filesystem removal");
            match std::fs::remove_dir_all(worktree) {
                Ok(()) => true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => return Err(e.to_string()),
            }
        }
    };
    if removed {
        let _ = crate::git_ops::run_git(repo_root, &["worktree", "prune"]);
    }

    if removed {
        if let Some(branch) = branch_to_delete {
            if crate::git_ops::find_branch_in_worktree_list(repo_root, &branch).is_none() {
                if let Err(e) = crate::git_ops::run_git(repo_root, &["branch", "-D", &branch]) {
                    tracing::debug!(error = %e, branch, "leftover worktree cleanup: branch delete failed after worktree removal");
                }
            }
        }
    }
    Ok(removed)
}

fn cleanup_branch_for_leftover(conv: &Conversation) -> Option<String> {
    match &conv.conv_mode {
        ConvMode::Work { branch_name, .. } => Some(branch_name.as_str().to_string()),
        ConvMode::Explore {
            worktree_path: Some(_),
            ..
        } => {
            let id_prefix: String = conv.id.chars().take(8).collect();
            Some(format!("task-pending-{id_prefix}"))
        }
        ConvMode::Explore {
            worktree_path: None,
            ..
        }
        | ConvMode::Branch { .. }
        | ConvMode::Direct => None,
    }
}

async fn build_disk_info(state: &AppState) -> DeploymentDiskInfo {
    let cfg = &state.deployment;
    let mut disk: Vec<DiskEntry> = cfg.locations.iter().map(measure_location).collect();
    disk.push(measure_location(&active_codex_credentials_location(
        &state.runtime_env,
    )));
    let (managed_entry, managed_worktrees) = managed_worktrees_disk(&state.db).await;
    disk.push(managed_entry);
    disk.push(pr_context_aggregate(&state.db).await);

    DeploymentDiskInfo {
        disk,
        managed_worktrees,
        sampled_at: Utc::now(),
    }
}

async fn managed_worktrees_disk(
    db: &crate::db::Database,
) -> (DiskEntry, Vec<ManagedWorktreeDiskEntry>) {
    const LABEL: &str = "Phoenix-managed worktrees";
    const PATTERN: &str = ".phoenix/worktrees/*";

    let paths = match db.managed_worktree_paths().await {
        Ok(paths) => paths,
        Err(e) => {
            tracing::debug!(error = %e, "managed worktree aggregate: failed to enumerate worktrees");
            return not_measured_managed_worktrees(LABEL, PATTERN);
        }
    };
    let conversations = match db.managed_worktree_conversations().await {
        Ok(conversations) => conversations,
        Err(e) => {
            tracing::debug!(error = %e, "managed worktree aggregate: failed to load worktree conversations");
            Vec::new()
        }
    };

    match tokio::task::spawn_blocking(move || {
        managed_worktrees_from_conversations(LABEL, PATTERN, &conversations, &paths)
    })
    .await
    {
        Ok(value) => value,
        Err(e) => {
            tracing::debug!(error = %e, "managed worktree aggregate: sizing task failed");
            not_measured_managed_worktrees(LABEL, PATTERN)
        }
    }
}

fn not_measured_managed_worktrees(
    label: &str,
    pattern: &str,
) -> (DiskEntry, Vec<ManagedWorktreeDiskEntry>) {
    (
        DiskEntry {
            category: DiskCategory::ManagedWorktrees,
            label: label.to_string(),
            path: pattern.to_string(),
            size: DiskSize::NotMeasured,
        },
        Vec::new(),
    )
}

fn managed_worktrees_from_conversations(
    label: &str,
    pattern: &str,
    conversations: &[Conversation],
    paths: &[String],
) -> (DiskEntry, Vec<ManagedWorktreeDiskEntry>) {
    let mut seen = BTreeSet::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut total: u64 = 0;
    let mut any = false;
    let by_path = worktree_conversation_index(conversations);
    let mut details = Vec::new();

    for path in paths {
        if !seen.insert(path.clone()) {
            continue;
        }
        let wt = Path::new(path);
        let repository = crate::git_ops::repo_root_from_phoenix_worktree(wt).map(|root| {
            if !roots.contains(&root) {
                roots.push(root.clone());
            }
            root.display().to_string()
        });
        let size = if wt.is_dir() {
            any = true;
            let bytes = dir_size(wt);
            total = total.saturating_add(bytes);
            DiskSize::Measured { bytes }
        } else {
            DiskSize::Absent
        };
        if let Some(convs) = by_path.get(path) {
            details.push(managed_worktree_detail(
                path,
                size,
                repository,
                convs,
                conversations,
            ));
        }
    }

    details.sort_by(|a, b| {
        disk_size_rank(&b.size)
            .cmp(&disk_size_rank(&a.size))
            .then_with(|| a.path.cmp(&b.path))
    });

    let path = aggregate_pattern_path(&roots, pattern);
    let size = if any {
        DiskSize::Measured { bytes: total }
    } else {
        DiskSize::Absent
    };

    (
        DiskEntry {
            category: DiskCategory::ManagedWorktrees,
            label: label.to_string(),
            path,
            size,
        },
        details,
    )
}

fn worktree_conversation_index(
    conversations: &[Conversation],
) -> BTreeMap<String, Vec<&Conversation>> {
    let mut by_path: BTreeMap<String, Vec<&Conversation>> = BTreeMap::new();
    for conv in conversations {
        if let Some(path) = conv.conv_mode.worktree_path() {
            by_path.entry(path.to_string()).or_default().push(conv);
        }
    }
    by_path
}

fn managed_worktree_detail(
    path: &str,
    size: DiskSize,
    repository: Option<String>,
    path_conversations: &[&Conversation],
    all_conversations: &[Conversation],
) -> ManagedWorktreeDiskEntry {
    let source = path_conversations[0];
    let live = path_conversations
        .iter()
        .copied()
        .find(|conv| managed_worktree_scope_owner(conv, all_conversations));
    let owner = live.unwrap_or(source);
    ManagedWorktreeDiskEntry {
        path: path.to_string(),
        size,
        repository,
        branch_name: owner.conv_mode.branch_name().map(str::to_string),
        disposition: match live {
            Some(conv) => ManagedWorktreeDisposition::Live {
                conversation_id: conv.id.clone(),
                slug: conv.slug.clone(),
                title: conv.title.clone(),
                state: conv_state_name(&conv.state).to_string(),
                archived: conv.archived,
            },
            None => ManagedWorktreeDisposition::Leftover {
                source_conversation_id: source.id.clone(),
                source_state: conv_state_name(&source.state).to_string(),
                archived: source.archived,
                cleanup_allowed: true,
            },
        },
    }
}

fn managed_worktree_scope_owner(conv: &Conversation, conversations: &[Conversation]) -> bool {
    use phoenix_core::domain::sm_state::ConvState;
    if conv.archived {
        return false;
    }
    match &conv.state {
        ConvState::ContextExhausted { .. } => match conv.continued_in_conv_id.as_deref() {
            Some(_) => continuation_chain_has_live_owner(conv, conversations),
            None => true,
        },
        ConvState::HandedOff { .. } => match conv.continued_in_conv_id.as_deref() {
            Some(_) => !continuation_chain_has_live_owner(conv, conversations),
            None => true,
        },
        ConvState::Completed { .. } | ConvState::Failed { .. } | ConvState::Terminal => false,
        ConvState::Idle
        | ConvState::LlmRequesting { .. }
        | ConvState::SeededLlmRequesting { .. }
        | ConvState::ToolExecuting { .. }
        | ConvState::CancellingTool { .. }
        | ConvState::AwaitingSubAgents { .. }
        | ConvState::CancellingSubAgents { .. }
        | ConvState::Error { .. }
        | ConvState::AwaitingRecovery { .. }
        | ConvState::AwaitingContinuation { .. }
        | ConvState::AwaitingTaskApproval { .. }
        | ConvState::AwaitingUserResponse { .. }
        | ConvState::AwaitingCommissionReviewApproval { .. } => true,
    }
}

fn continuation_chain_has_live_owner(conv: &Conversation, conversations: &[Conversation]) -> bool {
    let by_id: BTreeMap<&str, &Conversation> = conversations
        .iter()
        .map(|candidate| (candidate.id.as_str(), candidate))
        .collect();
    let mut next = conv.continued_in_conv_id.as_deref();
    let mut seen = BTreeSet::new();
    while let Some(id) = next {
        if !seen.insert(id) {
            return false;
        }
        let Some(member) = by_id.get(id).copied() else {
            return false;
        };
        if managed_worktree_single_node_owner(member) {
            return true;
        }
        next = member.continued_in_conv_id.as_deref();
    }
    false
}

fn managed_worktree_single_node_owner(conv: &Conversation) -> bool {
    use phoenix_core::domain::sm_state::ConvState;
    if conv.archived {
        return false;
    }
    match &conv.state {
        ConvState::ContextExhausted { .. } | ConvState::HandedOff { .. } => {
            conv.continued_in_conv_id.is_none()
        }
        ConvState::Completed { .. } | ConvState::Failed { .. } | ConvState::Terminal => false,
        ConvState::Idle
        | ConvState::LlmRequesting { .. }
        | ConvState::SeededLlmRequesting { .. }
        | ConvState::ToolExecuting { .. }
        | ConvState::CancellingTool { .. }
        | ConvState::AwaitingSubAgents { .. }
        | ConvState::CancellingSubAgents { .. }
        | ConvState::Error { .. }
        | ConvState::AwaitingRecovery { .. }
        | ConvState::AwaitingContinuation { .. }
        | ConvState::AwaitingTaskApproval { .. }
        | ConvState::AwaitingUserResponse { .. }
        | ConvState::AwaitingCommissionReviewApproval { .. } => true,
    }
}

fn conv_state_name(state: &phoenix_core::domain::sm_state::ConvState) -> &'static str {
    match state {
        phoenix_core::domain::sm_state::ConvState::Idle => "Idle",
        phoenix_core::domain::sm_state::ConvState::LlmRequesting { .. } => "LlmRequesting",
        phoenix_core::domain::sm_state::ConvState::SeededLlmRequesting { .. } => {
            "SeededLlmRequesting"
        }
        phoenix_core::domain::sm_state::ConvState::ToolExecuting { .. } => "ToolExecuting",
        phoenix_core::domain::sm_state::ConvState::CancellingTool { .. } => "CancellingTool",
        phoenix_core::domain::sm_state::ConvState::AwaitingSubAgents { .. } => "AwaitingSubAgents",
        phoenix_core::domain::sm_state::ConvState::CancellingSubAgents { .. } => {
            "CancellingSubAgents"
        }
        phoenix_core::domain::sm_state::ConvState::Completed { .. } => "Completed",
        phoenix_core::domain::sm_state::ConvState::Failed { .. } => "Failed",
        phoenix_core::domain::sm_state::ConvState::Error { .. } => "Error",
        phoenix_core::domain::sm_state::ConvState::AwaitingRecovery { .. } => "AwaitingRecovery",
        phoenix_core::domain::sm_state::ConvState::AwaitingContinuation { .. } => {
            "AwaitingContinuation"
        }
        phoenix_core::domain::sm_state::ConvState::AwaitingTaskApproval { .. } => {
            "AwaitingTaskApproval"
        }
        phoenix_core::domain::sm_state::ConvState::AwaitingUserResponse { .. } => {
            "AwaitingUserResponse"
        }
        phoenix_core::domain::sm_state::ConvState::AwaitingCommissionReviewApproval { .. } => {
            "AwaitingCommissionReviewApproval"
        }
        phoenix_core::domain::sm_state::ConvState::ContextExhausted { .. } => "ContextExhausted",
        phoenix_core::domain::sm_state::ConvState::HandedOff { .. } => "HandedOff",
        phoenix_core::domain::sm_state::ConvState::Terminal => "Terminal",
    }
}

fn disk_size_rank(size: &DiskSize) -> (u8, u64) {
    match size {
        DiskSize::Measured { bytes } => (2, *bytes),
        DiskSize::NotMeasured | DiskSize::InlineDb => (1, 0),
        DiskSize::Absent => (0, 0),
    }
}

fn aggregate_pattern_path(roots: &[PathBuf], pattern: &str) -> String {
    match roots {
        [root] => root.join(pattern).display().to_string(),
        [first, rest @ ..] => format!(
            "{} (+{} more roots)",
            first.join(pattern).display(),
            rest.len()
        ),
        [] => pattern.to_string(),
    }
}

/// Aggregate the PR auto-fix context bundles across every DB-known managed
/// worktree into a single `DiskEntry`. These bundles are written under
/// `{worktree}/.phoenix/pr-context/`; worktrees are scattered under each
/// project's `{repo_root}/.phoenix/worktrees/`, so there is no single
/// startup-known path to size — the set is resolved per request from the DB.
///
/// Each `.phoenix/pr-context` directory is small and owned (capacity-bounded by
/// the capture-site retention), so summing them is a cheap bounded walk, not an
/// open-ended recursion. A failed DB query yields a `NotMeasured` row rather
/// than failing the whole snapshot.
async fn pr_context_aggregate(db: &crate::db::Database) -> DiskEntry {
    const LABEL: &str = "PR auto-fix context";
    const PATTERN: &str = ".phoenix/worktrees/*/.phoenix/pr-context";

    let paths = match db.managed_worktree_paths().await {
        Ok(paths) => paths,
        Err(e) => {
            tracing::debug!(error = %e, "PR context aggregate: failed to enumerate worktrees");
            return DiskEntry {
                category: DiskCategory::PrContext,
                label: LABEL.to_string(),
                path: PATTERN.to_string(),
                size: DiskSize::NotMeasured,
            };
        }
    };

    pr_context_entry_from_paths(LABEL, PATTERN, paths.iter().map(String::as_str))
}

fn pr_context_entry_from_paths<'a>(
    label: &str,
    pattern: &str,
    paths: impl IntoIterator<Item = &'a str>,
) -> DiskEntry {
    let mut total: u64 = 0;
    let mut any = false;
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut seen = BTreeSet::new();

    for path in paths {
        if !seen.insert(path.to_string()) {
            continue;
        }
        let wt = Path::new(path);
        let ctx_dir = wt.join(".phoenix").join("pr-context");
        if let Some(root) = crate::git_ops::repo_root_from_phoenix_worktree(wt) {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        if !ctx_dir.is_dir() {
            continue;
        }
        any = true;
        total = total.saturating_add(dir_size(&ctx_dir));
    }

    let path = aggregate_pattern_path(&roots, pattern);
    let size = if any {
        DiskSize::Measured { bytes: total }
    } else {
        DiskSize::Absent
    };
    DiskEntry {
        category: DiskCategory::PrContext,
        label: label.to_string(),
        path,
        size,
    }
}

/// The codex credential location the process loads from right now: Phoenix's own
/// `~/.phoenix-ide/codex-auth.json`, or Codex CLI's `~/.codex/auth.json` under
/// `OPENAI_USE_CODEX_AUTH` piggyback mode; falls back to the canonical Phoenix
/// path (reported absent) when no credentials are present.
fn active_codex_credentials_location(
    runtime_env: &phoenix_core::runtime_env::PhoenixRuntimeEnvironment,
) -> DiskLocation {
    let path = absolutize(
        &phoenix_llm::codex_credential::resolve_active_auth_path(runtime_env)
            .unwrap_or_else(|| runtime_env.codex_auth_path()),
    );
    DiskLocation {
        category: DiskCategory::Credentials,
        label: "Codex credentials".to_string(),
        path,
        mode: MeasureMode::File,
    }
}

/// Make a path absolute for display without requiring it to exist or resolving
/// symlinks: a relative path is joined onto the process's current directory —
/// the same base the process resolves it against at startup. The deployment
/// wire contract specifies absolute `path` values.
pub fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

async fn sample_about_resources(state: &AppState) -> AboutResourcesSnapshot {
    sample_about_resources_inner(Some(state)).await
}

async fn sample_about_resources_inner(state: Option<&AppState>) -> AboutResourcesSnapshot {
    let api_identity = sysinfo::get_current_pid()
        .ok()
        .map(sysinfo::Pid::as_u32)
        .and_then(|pid| process_identity_for_sampling(pid).map(|identity| (pid, identity)));
    let api_identities = api_identity.into_iter().collect::<BTreeMap<_, _>>();
    let api_snapshot = if api_identities.is_empty() {
        BashPidSnapshot::Unavailable
    } else {
        BashPidSnapshot::Available(api_identities.clone())
    };
    let bash_pid_snapshot = match state {
        Some(state) => snapshot_bash_pids(state).await,
        None => BashPidSnapshot::Available(BTreeMap::new()),
    };

    let terminal_pid_snapshot = state.map(snapshot_terminal_pids);
    let mut all_identities = api_identities;
    if let BashPidSnapshot::Available(bash_identities) = &bash_pid_snapshot {
        all_identities.extend(bash_identities);
    }
    if let Some(BashPidSnapshot::Available(terminal_identities)) = &terminal_pid_snapshot {
        all_identities.extend(terminal_identities);
    }
    let all_pids = all_identities.keys().copied().collect::<BTreeSet<_>>();

    let (host, observed_rows) = tokio::join!(
        sample_host_resources(),
        sample_process_observations(&all_identities)
    );
    let observed_rows_by_pid: BTreeMap<u32, ProcessObservation> = observed_rows
        .into_iter()
        .map(|row| (row.pid, row))
        .collect();

    let mut categories = Vec::new();
    categories.push(build_api_category(&api_snapshot, &observed_rows_by_pid));

    if let Some(state) = state {
        categories.push(build_bash_category(
            &bash_pid_snapshot,
            &observed_rows_by_pid,
        ));

        categories.push(unavailable_category(
            ManagedResourceCategoryKind::Browser,
            "Browser",
            "browser sessions do not currently expose native process identity",
        ));
        categories.push(build_terminal_category(
            terminal_pid_snapshot
                .as_ref()
                .expect("terminal snapshot exists with app state"),
            &observed_rows_by_pid,
        ));

        let has_mcp = !state.mcp_manager.status().await.is_empty();
        if has_mcp {
            tracing::debug!(
                "about resources: MCP servers present but native process identity unavailable"
            );
        }
        categories.push(unavailable_category(
            ManagedResourceCategoryKind::Mcp,
            "MCP",
            if has_mcp {
                "MCP servers are configured or connected, but Phoenix does not currently surface native process identity for attribution"
            } else {
                "no MCP server identities available"
            },
        ));
    } else {
        categories.push(build_bash_category(
            &bash_pid_snapshot,
            &observed_rows_by_pid,
        ));
        categories.push(unavailable_category(
            ManagedResourceCategoryKind::Browser,
            "Browser",
            "browser sessions were not sampled in this context",
        ));
        categories.push(unavailable_category(
            ManagedResourceCategoryKind::TmuxTerminal,
            "tmux/terminal",
            "tmux and terminal resources were not sampled in this context",
        ));
        categories.push(unavailable_category(
            ManagedResourceCategoryKind::Mcp,
            "MCP",
            "MCP resources were not sampled in this context",
        ));
    }

    let managed_total = totals_from_observations(&all_pids, &observed_rows_by_pid);
    AboutResourcesSnapshot {
        sampled_at: Utc::now(),
        host,
        managed_total,
        categories,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum BashPidSnapshot {
    Available(BTreeMap<u32, ProcessIdentity>),
    Unavailable,
}

async fn snapshot_bash_pids(state: &AppState) -> BashPidSnapshot {
    let pgids = state
        .runtime
        .bash_handles()
        .snapshot_live_pgids()
        .await
        .into_iter()
        .collect::<BTreeSet<_>>();
    match group_member_identities_for_sampling(&pgids) {
        Some(member_identities) => BashPidSnapshot::Available(member_identities),
        None => BashPidSnapshot::Unavailable,
    }
}

fn snapshot_terminal_pids(state: &AppState) -> BashPidSnapshot {
    let session_ids = state
        .terminals
        .snapshot_shell_session_ids()
        .into_iter()
        .collect();
    match session_member_identities_for_sampling(&session_ids) {
        Some(member_identities) => BashPidSnapshot::Available(member_identities),
        None => BashPidSnapshot::Unavailable,
    }
}

fn build_api_category(
    snapshot: &BashPidSnapshot,
    observed_rows_by_pid: &BTreeMap<u32, ProcessObservation>,
) -> ManagedResourceCategory {
    match snapshot {
        BashPidSnapshot::Available(identities) => build_category_from_observations(
            ManagedResourceCategoryKind::Api,
            "API",
            ManagedResourceAttribution::Available,
            None,
            &identities.keys().copied().collect(),
            observed_rows_by_pid,
        ),
        BashPidSnapshot::Unavailable => unavailable_category(
            ManagedResourceCategoryKind::Api,
            "API",
            "Phoenix API native process identity unavailable",
        ),
    }
}

fn build_terminal_category(
    snapshot: &BashPidSnapshot,
    observed_rows_by_pid: &BTreeMap<u32, ProcessObservation>,
) -> ManagedResourceCategory {
    match snapshot {
        BashPidSnapshot::Available(identities) => build_category_from_observations(
            ManagedResourceCategoryKind::TmuxTerminal,
            "tmux/terminal",
            ManagedResourceAttribution::Available,
            Some("shell-mode terminals are attributed; tmux server identity remains unavailable".to_string()),
            &identities.keys().copied().collect(),
            observed_rows_by_pid,
        ),
        BashPidSnapshot::Unavailable => unavailable_category(
            ManagedResourceCategoryKind::TmuxTerminal,
            "tmux/terminal",
            "shell terminals exist, but native process enumeration failed; tmux server identity remains unavailable",
        ),
    }
}

fn build_bash_category(
    snapshot: &BashPidSnapshot,
    observed_rows_by_pid: &BTreeMap<u32, ProcessObservation>,
) -> ManagedResourceCategory {
    match snapshot {
        BashPidSnapshot::Available(identities) => build_category_from_observations(
            ManagedResourceCategoryKind::Bash,
            "Bash",
            ManagedResourceAttribution::Available,
            None,
            &identities.keys().copied().collect(),
            observed_rows_by_pid,
        ),
        BashPidSnapshot::Unavailable => unavailable_category(
            ManagedResourceCategoryKind::Bash,
            "Bash",
            "live bash process groups exist, but native process enumeration failed",
        ),
    }
}

#[derive(Default)]
struct HostCpuBreakdown {
    logical_cpu_count: Option<u32>,
    busy_percent: Option<f32>,
    system_percent: Option<f32>,
    idle_percent: Option<f32>,
}

async fn sample_host_cpu_breakdown() -> HostCpuBreakdown {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_cpu_all();
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sys.refresh_cpu_all();

    let logical_cpu_count =
        (!sys.cpus().is_empty()).then(|| u32::try_from(sys.cpus().len()).unwrap_or(u32::MAX));
    let busy = sys.global_cpu_usage().clamp(0.0, 100.0);

    HostCpuBreakdown {
        logical_cpu_count,
        busy_percent: Some(busy),
        system_percent: None,
        idle_percent: Some((100.0_f32 - busy).max(0.0)),
    }
}

fn build_category_from_observations(
    kind: ManagedResourceCategoryKind,
    label: &str,
    attribution: ManagedResourceAttribution,
    reason: Option<String>,
    pids: &BTreeSet<u32>,
    observed_rows_by_pid: &BTreeMap<u32, ProcessObservation>,
) -> ManagedResourceCategory {
    let processes: Vec<ManagedProcessRow> = pids
        .iter()
        .filter_map(|pid| observed_rows_by_pid.get(pid))
        .map(|row| ManagedProcessRow {
            name: row.name.clone(),
            category: kind,
            pid: row.pid,
            cpu_percent: row.cpu_percent,
            memory_bytes: row.memory_bytes,
            thread_count: row.thread_count,
            cpu_time_seconds: row.cpu_time_seconds,
        })
        .collect();
    let totals = totals_from_rows(&processes);
    ManagedResourceCategory {
        kind,
        label: label.to_string(),
        attribution,
        reason,
        totals,
        processes,
    }
}

fn unavailable_category(
    kind: ManagedResourceCategoryKind,
    label: &str,
    reason: &str,
) -> ManagedResourceCategory {
    tracing::debug!(category = ?kind, reason, "about resources: attribution unavailable");
    ManagedResourceCategory {
        kind,
        label: label.to_string(),
        attribution: ManagedResourceAttribution::Unavailable,
        reason: Some(reason.to_string()),
        totals: ManagedResourceTotals::default(),
        processes: Vec::new(),
    }
}

fn totals_from_observations(
    expected_pids: &BTreeSet<u32>,
    observed_rows_by_pid: &BTreeMap<u32, ProcessObservation>,
) -> ManagedResourceTotals {
    let processes: Vec<ManagedProcessRow> = expected_pids
        .iter()
        .filter_map(|pid| observed_rows_by_pid.get(pid))
        .map(|row| ManagedProcessRow {
            name: row.name.clone(),
            category: ManagedResourceCategoryKind::Api,
            pid: row.pid,
            cpu_percent: row.cpu_percent,
            memory_bytes: row.memory_bytes,
            thread_count: row.thread_count,
            cpu_time_seconds: row.cpu_time_seconds,
        })
        .collect();
    totals_from_rows(&processes)
}

fn totals_from_rows(processes: &[ManagedProcessRow]) -> ManagedResourceTotals {
    let mut cpu_total = 0.0_f32;
    let mut cpu_seen = false;
    let memory_bytes = if processes.is_empty() {
        None
    } else {
        processes.iter().try_fold(0_u64, |total, row| {
            row.memory_bytes.map(|memory| total.saturating_add(memory))
        })
    };
    for row in processes {
        if let Some(cpu) = row.cpu_percent {
            cpu_total += cpu;
            cpu_seen = true;
        }
    }
    ManagedResourceTotals {
        cpu_percent: cpu_seen.then_some(cpu_total),
        memory_bytes,
        process_count: u32::try_from(processes.len()).unwrap_or(u32::MAX),
        deduplicated_pid_count: u32::try_from(processes.len()).unwrap_or(u32::MAX),
    }
}

async fn sample_host_resources() -> HostResources {
    use sysinfo::System;

    let cpu = sample_host_cpu_breakdown().await;
    let mut sys = System::new();
    sys.refresh_memory();

    let total_memory_bytes = Some(sys.total_memory());
    let available_memory_bytes = Some(sys.available_memory());
    let used_memory_bytes = total_memory_bytes
        .zip(available_memory_bytes)
        .map(|(t, a)| t.saturating_sub(a));
    let load = System::load_average();
    HostResources {
        logical_cpu_count: cpu.logical_cpu_count,
        cpu_busy_percent: cpu.busy_percent,
        cpu_system_percent: cpu.system_percent,
        cpu_idle_percent: cpu.idle_percent,
        total_memory_bytes,
        available_memory_bytes,
        used_memory_bytes,
        load_average_one: Some(load.one),
        load_average_five: Some(load.five),
        load_average_fifteen: Some(load.fifteen),
    }
}

/// Size a single configured location per its [`MeasureMode`].
fn measure_location(loc: &DiskLocation) -> DiskEntry {
    let size = match loc.mode {
        MeasureMode::InlineDb => DiskSize::InlineDb,
        MeasureMode::Pattern => DiskSize::NotMeasured,
        _ if !loc.path.exists() => DiskSize::Absent,
        MeasureMode::File => std::fs::metadata(&loc.path)
            .map_or(DiskSize::Absent, |m| DiskSize::Measured { bytes: m.len() }),
        MeasureMode::RecurseSmall => DiskSize::Measured {
            bytes: dir_size(&loc.path),
        },
        MeasureMode::NoMeasure => DiskSize::NotMeasured,
    };
    DiskEntry {
        category: loc.category,
        label: loc.label.clone(),
        path: loc.path.display().to_string(),
        size,
    }
}

/// Recursively sum the byte sizes of regular files under `path`. Bounded to the
/// directory it is given — callers only pass directories classified as small,
/// never the known-large caches. An unreadable subtree contributes nothing
/// rather than aborting the walk.
///
/// Symlinks are never followed: `file_type()` reports the entry's own type (it
/// does not stat through links), so a symlinked directory is skipped rather
/// than recursed. This keeps the walk inside the intended subtree and immune to
/// symlink cycles or links pointing at large external trees.
fn dir_size(path: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total = total.saturating_add(meta.len());
                }
            }
        }
    }
    total
}

#[cfg(test)]
impl DeploymentConfig {
    /// Minimal config for handler tests.
    pub fn for_tests() -> Self {
        Self {
            bind_address: SocketAddr::from(([127, 0, 0, 1], 0)),
            tls: TlsInfo::disabled(),
            log: LogInfo {
                stdout: true,
                file: None,
            },
            locations: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::db_schema::NonEmptyString;
    use std::fs;

    fn loc(path: PathBuf, mode: MeasureMode) -> DiskLocation {
        DiskLocation {
            category: DiskCategory::DataDirectory,
            label: "x".to_string(),
            path,
            mode,
        }
    }

    fn conversation(
        id: &str,
        path: &str,
        state: phoenix_core::domain::sm_state::ConvState,
    ) -> Conversation {
        let now = Utc::now();
        Conversation {
            id: id.to_string(),
            slug: Some(format!("slug-{id}")),
            title: Some(format!("Title {id}")),
            cwd: "/tmp".to_string(),
            parent_conversation_id: None,
            user_initiated: true,
            state,
            state_updated_at: now,
            created_at: now,
            updated_at: now,
            archived: false,
            transcript_generation: 1,
            model: None,
            project_id: None,
            conv_mode: ConvMode::Work {
                branch_name: NonEmptyString::new(format!("branch-{id}")).unwrap(),
                worktree_path: NonEmptyString::new(path.to_string()).unwrap(),
                base_branch: NonEmptyString::new("main").unwrap(),
                task_id: NonEmptyString::new(id.to_string()).unwrap(),
                task_title: NonEmptyString::new(format!("Task {id}")).unwrap(),
            },
            desired_base_branch: None,
            message_count: 0,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,
            chain_name: None,
            llm_language: phoenix_core::llm_language::LlmLanguage::default(),
            spawned_from_conversation_id: None,
        }
    }

    #[test]
    fn file_mode_measures_byte_length() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.bin");
        fs::write(&file, b"hello").unwrap();
        assert_eq!(
            measure_location(&loc(file, MeasureMode::File)).size,
            DiskSize::Measured { bytes: 5 }
        );
    }

    #[test]
    fn missing_file_is_absent_not_zero() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            measure_location(&loc(dir.path().join("nope"), MeasureMode::File)).size,
            DiskSize::Absent
        );
    }

    #[test]
    fn recurse_small_sums_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), b"123").unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("b"), b"4567").unwrap();
        assert_eq!(
            measure_location(&loc(dir.path().to_path_buf(), MeasureMode::RecurseSmall)).size,
            DiskSize::Measured { bytes: 7 }
        );
    }

    #[test]
    fn no_measure_existing_dir_reports_not_measured() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            measure_location(&loc(dir.path().to_path_buf(), MeasureMode::NoMeasure)).size,
            DiskSize::NotMeasured
        );
    }

    #[test]
    fn no_measure_absent_path_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            measure_location(&loc(dir.path().join("gone"), MeasureMode::NoMeasure)).size,
            DiskSize::Absent
        );
    }

    #[test]
    fn pattern_is_not_measured_even_when_path_is_a_glob() {
        // The literal glob never exists on disk, but a Pattern row must still
        // report not_measured (a pointer to where bytes live), never absent.
        let entry = measure_location(&loc(
            PathBuf::from("/tmp/phoenix-chrome-*"),
            MeasureMode::Pattern,
        ));
        assert_eq!(entry.size, DiskSize::NotMeasured);
        assert_eq!(entry.path, "/tmp/phoenix-chrome-*");
    }

    #[test]
    fn dir_size_does_not_follow_symlinked_dirs() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("real"), b"12345").unwrap();
        // A symlink pointing back at the parent would cause an unbounded walk
        // if followed; it must contribute nothing and not loop.
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
        assert_eq!(
            measure_location(&loc(dir.path().to_path_buf(), MeasureMode::RecurseSmall)).size,
            DiskSize::Measured { bytes: 5 }
        );
    }

    #[test]
    fn inline_db_is_inline_regardless_of_path() {
        let entry = measure_location(&loc(
            PathBuf::from("/does/not/exist/phoenix.db"),
            MeasureMode::InlineDb,
        ));
        assert_eq!(entry.size, DiskSize::InlineDb);
        assert_eq!(entry.path, "/does/not/exist/phoenix.db");
    }

    #[test]
    fn exact_phoenix_worktree_root_accepts_only_worktree_root() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = repo.join(".phoenix").join("worktrees").join("conv-1");

        assert_eq!(exact_phoenix_worktree_root(&wt), Some(repo));
    }

    #[test]
    fn exact_phoenix_worktree_root_rejects_descendant_path() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let descendant = repo
            .join(".phoenix")
            .join("worktrees")
            .join("conv-1")
            .join("src");

        assert_eq!(exact_phoenix_worktree_root(&descendant), None);
    }

    #[test]
    fn exact_phoenix_worktree_root_rejects_parent_dir_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let parent_leaf = repo.join(".phoenix").join("worktrees").join("..");

        assert_eq!(exact_phoenix_worktree_root(&parent_leaf), None);
    }

    #[test]
    fn remove_leftover_worktree_treats_missing_fallback_as_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let missing = repo.join(".phoenix").join("worktrees").join("gone");

        assert_eq!(remove_leftover_worktree(&repo, &missing, None), Ok(false));
    }

    #[test]
    fn managed_worktrees_aggregate_dedupes_and_sums_existing_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = repo.join(".phoenix").join("worktrees").join("conv-1");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join("a.bin"), b"12345").unwrap();
        fs::create_dir_all(wt.join("nested")).unwrap();
        fs::write(wt.join("nested").join("b.bin"), b"123").unwrap();

        let wt_str = wt.to_string_lossy().to_string();
        let (entry, details) = managed_worktrees_from_conversations(
            "Managed worktrees",
            ".phoenix/worktrees/*",
            &[],
            &[wt_str.clone(), wt_str.clone()],
        );

        assert_eq!(entry.size, DiskSize::Measured { bytes: 8 });
        assert!(details.is_empty());
        assert_eq!(
            entry.path,
            repo.join(".phoenix/worktrees/*").display().to_string()
        );
    }

    #[test]
    fn managed_worktrees_aggregate_reports_absent_when_no_known_path_exists() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = repo.join(".phoenix").join("worktrees").join("missing");
        let wt_str = wt.to_string_lossy().to_string();

        let (entry, _) = managed_worktrees_from_conversations(
            "Managed worktrees",
            ".phoenix/worktrees/*",
            &[],
            &[wt_str],
        );

        assert_eq!(entry.size, DiskSize::Absent);
        assert_eq!(
            entry.path,
            repo.join(".phoenix/worktrees/*").display().to_string()
        );
    }

    #[test]
    fn managed_worktrees_aggregate_mentions_multiple_roots() {
        let dir = tempfile::tempdir().unwrap();
        let repo_a = dir.path().join("repo-a");
        let repo_b = dir.path().join("repo-b");
        let wt_a = repo_a.join(".phoenix").join("worktrees").join("a");
        let wt_b = repo_b.join(".phoenix").join("worktrees").join("b");
        fs::create_dir_all(&wt_a).unwrap();
        fs::create_dir_all(&wt_b).unwrap();
        fs::write(wt_a.join("a.bin"), b"1").unwrap();
        fs::write(wt_b.join("b.bin"), b"22").unwrap();
        let wt_a = wt_a.to_string_lossy().to_string();
        let wt_b = wt_b.to_string_lossy().to_string();

        let (entry, _) = managed_worktrees_from_conversations(
            "Managed worktrees",
            ".phoenix/worktrees/*",
            &[],
            &[wt_a, wt_b],
        );

        assert_eq!(entry.size, DiskSize::Measured { bytes: 3 });
        assert!(entry.path.ends_with(".phoenix/worktrees/* (+1 more roots)"));
    }

    #[test]
    fn context_exhausted_uncontinued_worktree_is_live_not_leftover() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = repo.join(".phoenix").join("worktrees").join("ctx");
        fs::create_dir_all(&wt).unwrap();
        let wt = wt.to_string_lossy().to_string();
        let conversations = vec![conversation(
            "ctx",
            &wt,
            phoenix_core::domain::sm_state::ConvState::ContextExhausted {
                summary: "summary".to_string(),
            },
        )];

        let (_, details) = managed_worktrees_from_conversations(
            "Managed worktrees",
            ".phoenix/worktrees/*",
            &conversations,
            &[wt],
        );

        assert!(matches!(
            details[0].disposition,
            ManagedWorktreeDisposition::Live { .. }
        ));
    }

    #[test]
    fn handed_off_dead_end_protector_is_live_not_leftover() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = repo.join(".phoenix").join("worktrees").join("handoff");
        fs::create_dir_all(&wt).unwrap();
        let wt = wt.to_string_lossy().to_string();
        let mut source = conversation(
            "source",
            &wt,
            phoenix_core::domain::sm_state::ConvState::HandedOff {
                successor_conv_id: "successor".to_string(),
            },
        );
        source.continued_in_conv_id = Some("successor".to_string());
        let mut successor = conversation(
            "successor",
            &wt,
            phoenix_core::domain::sm_state::ConvState::Terminal,
        );
        successor.archived = true;
        let conversations = vec![source, successor];

        let (_, details) = managed_worktrees_from_conversations(
            "Managed worktrees",
            ".phoenix/worktrees/*",
            &conversations,
            &[wt],
        );

        assert!(matches!(
            details[0].disposition,
            ManagedWorktreeDisposition::Live { .. }
        ));
    }

    #[test]
    fn cleanup_branch_for_leftover_deletes_explore_temp_branch() {
        let mut conv = conversation(
            "abcdef123456",
            "/repo/.phoenix/worktrees/abcdef123456",
            phoenix_core::domain::sm_state::ConvState::Terminal,
        );
        conv.conv_mode = ConvMode::Explore {
            worktree_path: Some(
                NonEmptyString::new("/repo/.phoenix/worktrees/abcdef123456").unwrap(),
            ),
            next_taskmd_id_hint: None,
        };

        assert_eq!(
            cleanup_branch_for_leftover(&conv),
            Some("task-pending-abcdef12".to_string())
        );
    }

    #[test]
    fn cleanup_branch_for_leftover_preserves_branch_mode_branch() {
        let mut conv = conversation(
            "branchy",
            "/repo/.phoenix/worktrees/branchy",
            phoenix_core::domain::sm_state::ConvState::Terminal,
        );
        conv.conv_mode = ConvMode::Branch {
            branch_name: NonEmptyString::new("user-branch").unwrap(),
            worktree_path: NonEmptyString::new("/repo/.phoenix/worktrees/branchy").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
        };

        assert_eq!(cleanup_branch_for_leftover(&conv), None);
    }

    #[test]
    fn pr_context_aggregate_dedupes_worktree_paths() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = repo.join(".phoenix").join("worktrees").join("conv-1");
        let ctx = wt.join(".phoenix").join("pr-context");
        fs::create_dir_all(&ctx).unwrap();
        fs::write(ctx.join("context.json"), b"1234").unwrap();
        let wt_str = wt.to_string_lossy().to_string();

        let entry = pr_context_entry_from_paths(
            "PR auto-fix context",
            ".phoenix/worktrees/*/.phoenix/pr-context",
            [wt_str.as_str(), wt_str.as_str()],
        );

        assert_eq!(entry.size, DiskSize::Measured { bytes: 4 });
        assert_eq!(
            entry.path,
            repo.join(".phoenix/worktrees/*/.phoenix/pr-context")
                .display()
                .to_string()
        );
    }

    #[test]
    fn absolutize_leaves_absolute_paths_unchanged() {
        let p = "/var/lib/phoenix-ide/prod.db";
        assert_eq!(absolutize(Path::new(p)), PathBuf::from(p));
    }

    #[test]
    fn absolutize_joins_relative_paths_onto_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let abs = absolutize(Path::new("phoenix.db"));
        assert!(abs.is_absolute());
        assert_eq!(abs, cwd.join("phoenix.db"));
    }

    #[test]
    fn unavailable_api_snapshot_preserves_capability_failure() {
        let category = build_api_category(&BashPidSnapshot::Unavailable, &BTreeMap::new());

        assert_eq!(
            category.attribution,
            ManagedResourceAttribution::Unavailable
        );
        assert!(category.reason.is_some());
        assert!(category.processes.is_empty());
    }

    #[test]
    fn unavailable_bash_snapshot_preserves_capability_failure() {
        let category = build_bash_category(&BashPidSnapshot::Unavailable, &BTreeMap::new());

        assert_eq!(
            category.attribution,
            ManagedResourceAttribution::Unavailable
        );
        assert!(category.reason.is_some());
        assert!(category.processes.is_empty());
        assert_eq!(category.totals.cpu_percent, None);
        assert_eq!(category.totals.memory_bytes, None);
        assert_eq!(category.totals.process_count, 0);
        assert_eq!(category.totals.deduplicated_pid_count, 0);
    }

    #[test]
    fn unavailable_category_carries_reason_and_zero_totals() {
        let category = unavailable_category(
            ManagedResourceCategoryKind::Browser,
            "Browser",
            "native process identity unavailable",
        );

        assert_eq!(category.kind, ManagedResourceCategoryKind::Browser);
        assert_eq!(
            category.attribution,
            ManagedResourceAttribution::Unavailable
        );
        assert_eq!(
            category.reason.as_deref(),
            Some("native process identity unavailable")
        );
        assert_eq!(category.totals.process_count, 0);
        assert_eq!(category.totals.deduplicated_pid_count, 0);
        assert!(category.totals.cpu_percent.is_none());
        assert!(category.totals.memory_bytes.is_none());
        assert!(category.processes.is_empty());
    }

    #[test]
    fn totals_require_memory_for_every_process() {
        let complete = ManagedProcessRow {
            name: "api".to_string(),
            category: ManagedResourceCategoryKind::Api,
            pid: 1,
            cpu_percent: Some(1.0),
            memory_bytes: Some(1024),
            thread_count: None,
            cpu_time_seconds: None,
        };
        let missing = ManagedProcessRow {
            name: "bash".to_string(),
            category: ManagedResourceCategoryKind::Bash,
            pid: 2,
            cpu_percent: Some(2.0),
            memory_bytes: None,
            thread_count: None,
            cpu_time_seconds: None,
        };

        assert_eq!(
            totals_from_rows(std::slice::from_ref(&complete)).memory_bytes,
            Some(1024)
        );
        assert_eq!(totals_from_rows(&[complete, missing]).memory_bytes, None);
    }

    #[test]
    fn managed_total_requires_memory_for_every_deduplicated_process() {
        let expected_pids = BTreeSet::from([1, 2]);
        let observations = BTreeMap::from([
            (
                1,
                ProcessObservation {
                    pid: 1,
                    name: "api".to_string(),
                    cpu_percent: Some(1.0),
                    memory_bytes: Some(1024),
                    thread_count: None,
                    cpu_time_seconds: None,
                },
            ),
            (
                2,
                ProcessObservation {
                    pid: 2,
                    name: "bash".to_string(),
                    cpu_percent: Some(2.0),
                    memory_bytes: None,
                    thread_count: None,
                    cpu_time_seconds: None,
                },
            ),
        ]);

        let totals = totals_from_observations(&expected_pids, &observations);

        assert_eq!(totals.memory_bytes, None);
        assert_eq!(totals.process_count, 2);
        assert_eq!(totals.deduplicated_pid_count, 2);
    }

    #[test]
    fn managed_process_row_category_round_trips_requested_kind() {
        let row = ManagedProcessRow {
            name: "bash".to_string(),
            category: ManagedResourceCategoryKind::Bash,
            pid: 42,
            cpu_percent: Some(12.5),
            memory_bytes: Some(1024),
            thread_count: Some(3),
            cpu_time_seconds: Some(1.5),
        };

        assert_eq!(row.category, ManagedResourceCategoryKind::Bash);
    }

    #[tokio::test]
    async fn host_cpu_breakdown_preserves_idle_plus_busy_budget() {
        let cpu = sample_host_cpu_breakdown().await;

        if let (Some(busy), Some(idle)) = (cpu.busy_percent, cpu.idle_percent) {
            assert!(
                (busy + idle) <= 100.5,
                "busy + idle should stay near 100%, got {}",
                busy + idle
            );
            assert!(busy >= 0.0);
            assert!(idle >= 0.0);
        }
    }
}
