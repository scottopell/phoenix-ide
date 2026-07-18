//! Published release discovery and in-app production update handoff.
//!
//! The native deployment status file is the sole transaction-state authority.
//! This module resolves an immutable release preview and launches the selected
//! release's deployment controller; it never replaces or restarts Phoenix.

use super::{local_reveal::client_is_local, AppState};
use axum::{
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, OnceLock, RwLock},
    time::{Duration, Instant},
};
use ts_rs::TS;
use uuid::Uuid;

const REPOSITORY: &str = "scottopell/phoenix-ide";
const USER_AGENT: &str = "phoenix-ide-release-updates";
const PREVIEW_CACHE_TTL: Duration = Duration::from_secs(300);
static PREVIEW_CACHE: OnceLock<RwLock<Option<(Instant, ReleasePreview)>>> = OnceLock::new();
static GITHUB_REQUEST_GATE: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
static SYSTEMD_AUTHORITY_CACHE: OnceLock<RwLock<Option<(Instant, bool)>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ReleaseUpdateBackend {
    Launchd,
    Systemd,
    BareLinux,
    Unsupported,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ReleasePreview {
    Available {
        tag: String,
        version: String,
        commit: String,
        asset_name: String,
        asset_sha256: String,
        release_url: String,
        notes: String,
        newer_than_current: bool,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Debug, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ReleaseUpdateAuthority {
    Allowed,
    RemoteBrowser,
    NotProduction,
    UnsupportedHost,
    MissingPrerequisite { reason: String },
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ReleaseTransactionStatus {
    None,
    Present {
        transaction_id: String,
        state: String,
        source_commit: Option<String>,
        release_tag: Option<String>,
        expected_version: Option<String>,
        expected_git_sha: Option<String>,
        created_at: Option<String>,
        updated_at: Option<String>,
        failure: Option<String>,
        rollback_failure: Option<String>,
        stale: bool,
    },
    Unreadable {
        reason: String,
    },
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ReleaseUpdateSnapshot {
    pub backend: ReleaseUpdateBackend,
    pub current_version: String,
    pub current_git_sha: String,
    pub preview: ReleasePreview,
    pub authority: ReleaseUpdateAuthority,
    pub transaction: ReleaseTransactionStatus,
    pub sampled_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ApproveReleaseUpdateRequest {
    pub tag: String,
    pub commit: String,
    pub asset_name: String,
    pub asset_sha256: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ApproveReleaseUpdateResponse {
    pub transaction_id: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    prerelease: bool,
}

#[derive(Deserialize)]
struct GithubCommit {
    sha: String,
}

#[derive(Default, Deserialize)]
pub struct SnapshotQuery {
    #[serde(default)]
    refresh: bool,
}

fn backend(_state: &AppState) -> ReleaseUpdateBackend {
    if cfg!(target_os = "macos") {
        ReleaseUpdateBackend::Launchd
    } else if cfg!(target_os = "linux") {
        let pid_one = Command::new("ps")
            .args(["-p", "1", "-o", "comm="])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
        if pid_one.as_deref() == Some("systemd") {
            ReleaseUpdateBackend::Systemd
        } else {
            ReleaseUpdateBackend::BareLinux
        }
    } else {
        ReleaseUpdateBackend::Unsupported
    }
}

fn asset_name() -> Result<String, String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => {
            return Err(format!(
                "published releases do not support architecture {other}"
            ))
        }
    };
    let target = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-musl",
        other => {
            return Err(format!(
                "published releases do not support platform {other}"
            ))
        }
    };
    Ok(format!("phoenix_ide-{arch}-{target}"))
}

fn github_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())
}

async fn response_text(response: reqwest::Response, description: &str) -> Result<String, String> {
    let status = response.status();
    let text = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!("{description} failed with HTTP {status}"));
    }
    Ok(text)
}

fn version_triplet(version: &str) -> Option<(u64, u64, u64)> {
    let mut fields = version.trim_start_matches('v').split('.');
    let triplet = (
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
    );
    fields.next().is_none().then_some(triplet)
}

async fn discover_release() -> Result<ReleasePreview, String> {
    let _permit = GITHUB_REQUEST_GATE
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .acquire()
        .await
        .map_err(|_| "release discovery request gate closed".to_string())?;
    let asset = asset_name()?;
    let client = github_client()?;
    let release: GithubRelease = client
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/releases/latest"
        ))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    if release.prerelease {
        return Err("GitHub latest release is a prerelease".to_string());
    }
    let commit: GithubCommit = client
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/commits/{}",
            release.tag_name
        ))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    if commit.sha.len() != 40 || !commit.sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("release tag did not resolve to a full commit".to_string());
    }
    let sums = response_text(
        client
            .get(format!(
                "https://github.com/{REPOSITORY}/releases/download/{}/SHA256SUMS",
                release.tag_name
            ))
            .send()
            .await
            .map_err(|error| error.to_string())?,
        "release checksum discovery",
    )
    .await?;
    let checksum = sums
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?, fields.next()?.trim_start_matches('*')))
        })
        .find_map(|(checksum, name)| (name == asset).then(|| checksum.to_string()))
        .ok_or_else(|| format!("release {} has no checksum for {asset}", release.tag_name))?;
    if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "release {} has a malformed asset checksum",
            release.tag_name
        ));
    }
    let current = version_triplet(env!("CARGO_PKG_VERSION"))
        .ok_or_else(|| "running Phoenix version is not semantic x.y.z".to_string())?;
    let version = release.tag_name.trim_start_matches('v').to_string();
    let released = version_triplet(&version)
        .ok_or_else(|| format!("release {} is not semantic x.y.z", release.tag_name))?;
    Ok(ReleasePreview::Available {
        newer_than_current: released > current,
        version,
        tag: release.tag_name,
        commit: commit.sha.to_ascii_lowercase(),
        asset_name: asset,
        asset_sha256: checksum.to_ascii_lowercase(),
        release_url: release.html_url,
        notes: release.body.unwrap_or_default(),
    })
}

async fn cached_preview(refresh: bool) -> Result<ReleasePreview, String> {
    let cache = PREVIEW_CACHE.get_or_init(|| RwLock::new(None));
    if !refresh {
        if let Some((sampled, preview)) = cache.read().expect("preview cache poisoned").as_ref() {
            if sampled.elapsed() < PREVIEW_CACHE_TTL {
                return Ok(preview.clone());
            }
        }
    }
    let preview = discover_release()
        .await
        .unwrap_or_else(|reason| ReleasePreview::Unavailable { reason });
    *cache.write().expect("preview cache poisoned") = Some((Instant::now(), preview.clone()));
    Ok(preview)
}

fn status_path(state: &AppState, backend: ReleaseUpdateBackend) -> Option<PathBuf> {
    match backend {
        ReleaseUpdateBackend::Systemd => {
            Some(PathBuf::from("/var/lib/phoenix-ide-deploy/status.json"))
        }
        ReleaseUpdateBackend::Launchd | ReleaseUpdateBackend::BareLinux => Some(
            state
                .runtime_env
                .home()
                .join(".phoenix-ide/deploy/status.json"),
        ),
        ReleaseUpdateBackend::Unsupported => None,
    }
}

#[allow(clippy::too_many_lines)]
fn read_status(state: &AppState, backend: ReleaseUpdateBackend) -> ReleaseTransactionStatus {
    let Some(path) = status_path(state, backend) else {
        return ReleaseTransactionStatus::None;
    };
    let text = if matches!(backend, ReleaseUpdateBackend::Systemd) {
        match Command::new("sudo").args(["-n", "cat"]).arg(&path).output() {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).into_owned()
            }
            Ok(output)
                if String::from_utf8_lossy(&output.stderr)
                    .contains("No such file or directory") =>
            {
                return ReleaseTransactionStatus::None;
            }
            Ok(output) => {
                return ReleaseTransactionStatus::Unreadable {
                    reason: format!(
                        "cannot read durable systemd deployment status: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                };
            }
            Err(error) => {
                return ReleaseTransactionStatus::Unreadable {
                    reason: format!("cannot read durable systemd deployment status: {error}"),
                };
            }
        }
    } else {
        match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ReleaseTransactionStatus::None;
            }
            Err(error) => {
                return ReleaseTransactionStatus::Unreadable {
                    reason: format!("cannot read durable deployment status: {error}"),
                };
            }
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            return ReleaseTransactionStatus::Unreadable {
                reason: format!("durable deployment status is invalid: {error}"),
            }
        }
    };
    if value.get("source_kind").and_then(serde_json::Value::as_str) != Some("published_release") {
        return ReleaseTransactionStatus::None;
    }
    let string = |name: &str| {
        value
            .get(name)
            .and_then(|item| item.as_str())
            .map(str::to_string)
    };
    let Some(transaction_id) = string("transaction_id") else {
        return ReleaseTransactionStatus::Unreadable {
            reason: "durable deployment status has no transaction ID".to_string(),
        };
    };
    let Some(state) = string("state") else {
        return ReleaseTransactionStatus::Unreadable {
            reason: "durable deployment status has no state".to_string(),
        };
    };
    let updated_at = string("updated_at").or_else(|| {
        value
            .get("updated_at")
            .and_then(serde_json::Value::as_f64)
            .and_then(|seconds| seconds.trunc().to_string().parse::<i64>().ok())
            .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
            .map(|timestamp| timestamp.to_rfc3339())
    });
    let terminal = matches!(
        state.as_str(),
        "committed"
            | "precondition_failed"
            | "activation_failed_rolled_back"
            | "activation_failed_rollback_failed"
            | "rejected_concurrent"
    );
    let status_is_stale = !terminal
        && updated_at
            .as_deref()
            .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
            .is_some_and(|updated| {
                Utc::now()
                    .signed_duration_since(updated.with_timezone(&Utc))
                    .num_seconds()
                    > 120
            });
    ReleaseTransactionStatus::Present {
        transaction_id,
        state,
        source_commit: string("source_commit"),
        release_tag: string("release_tag"),
        expected_version: string("expected_version"),
        expected_git_sha: string("expected_git_sha"),
        created_at: string("created_at"),
        updated_at,
        failure: string("failure"),
        rollback_failure: string("rollback_failure"),
        stale: status_is_stale,
    }
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(name).is_file())
    })
}

fn authority(
    state: &AppState,
    local: bool,
    backend: ReleaseUpdateBackend,
    refresh_privilege: bool,
) -> ReleaseUpdateAuthority {
    if !local {
        return ReleaseUpdateAuthority::RemoteBrowser;
    }
    if !state.runtime_env.is_production() {
        return ReleaseUpdateAuthority::NotProduction;
    }
    if matches!(backend, ReleaseUpdateBackend::Unsupported) {
        return ReleaseUpdateAuthority::UnsupportedHost;
    }
    for command in ["uv", "gh"] {
        if !command_exists(command) {
            return ReleaseUpdateAuthority::MissingPrerequisite {
                reason: format!("{command} is required for published release updates"),
            };
        }
    }
    if matches!(backend, ReleaseUpdateBackend::Systemd) {
        let cache = SYSTEMD_AUTHORITY_CACHE.get_or_init(|| RwLock::new(None));
        let cached = if refresh_privilege {
            None
        } else {
            cache
                .read()
                .expect("systemd authority cache poisoned")
                .as_ref()
                .filter(|(sampled, _)| sampled.elapsed() < Duration::from_secs(60))
                .map(|(_, allowed)| *allowed)
        };
        let allowed = cached.unwrap_or_else(|| {
            let allowed = Command::new("sudo")
                .args(["-n", "true"])
                .status()
                .is_ok_and(|status| status.success());
            *cache.write().expect("systemd authority cache poisoned") =
                Some((Instant::now(), allowed));
            allowed
        });
        if !allowed {
            return ReleaseUpdateAuthority::MissingPrerequisite {
                reason: "systemd updates require non-interactive sudo authorization".to_string(),
            };
        }
    }
    ReleaseUpdateAuthority::Allowed
}

pub async fn snapshot(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<SnapshotQuery>,
) -> impl IntoResponse {
    let selected_backend = backend(&state);
    let local = client_is_local(peer.ip(), &headers);
    let preview = cached_preview(query.refresh)
        .await
        .unwrap_or_else(|reason| ReleasePreview::Unavailable { reason });
    Json(ReleaseUpdateSnapshot {
        backend: selected_backend,
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        current_git_sha: env!("PHOENIX_GIT_SHA").to_string(),
        authority: authority(&state, local, selected_backend, query.refresh),
        transaction: read_status(&state, selected_backend),
        preview,
        sampled_at: Utc::now(),
    })
}

fn valid_approval(request: &ApproveReleaseUpdateRequest, preview: &ReleasePreview) -> bool {
    matches!(
        preview,
        ReleasePreview::Available { tag, commit, asset_name, asset_sha256, .. }
            if tag == &request.tag
                && commit == &request.commit
                && asset_name == &request.asset_name
                && asset_sha256 == &request.asset_sha256
    )
}

fn updater_dir(state: &AppState) -> Result<PathBuf, String> {
    let root = state
        .runtime_env
        .home()
        .join(".phoenix-ide/deploy/controllers");
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    Ok(root)
}

async fn materialize_controller(commit: &str, destination: &Path) -> Result<(), String> {
    let _permit = GITHUB_REQUEST_GATE
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .acquire()
        .await
        .map_err(|_| "release controller request gate closed".to_string())?;
    let response = github_client()?
        .get(format!(
            "https://raw.githubusercontent.com/{REPOSITORY}/{commit}/dev.py"
        ))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let bytes = response
        .error_for_status()
        .map_err(|error| error.to_string())?
        .bytes()
        .await
        .map_err(|error| error.to_string())?;
    let temporary = destination.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, &bytes).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())?;
    fs::rename(&temporary, destination).map_err(|error| error.to_string())?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub async fn approve(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<ApproveReleaseUpdateRequest>,
) -> impl IntoResponse {
    let selected_backend = backend(&state);
    if !matches!(
        authority(
            &state,
            client_is_local(peer.ip(), &headers),
            selected_backend,
            true,
        ),
        ReleaseUpdateAuthority::Allowed
    ) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": "release update approval is not authorized on this request" }))).into_response();
    }
    let preview = match discover_release().await {
        Ok(preview) if valid_approval(&request, &preview) => preview,
        Ok(_) => return (
            StatusCode::CONFLICT,
            Json(
                serde_json::json!({ "error": "release preview changed; refresh before approving" }),
            ),
        )
            .into_response(),
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": error })),
            )
                .into_response()
        }
    };
    if !matches!(
        preview,
        ReleasePreview::Available {
            newer_than_current: true,
            ..
        }
    ) {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "the selected stable release is not newer than the running version"
            })),
        )
            .into_response();
    }
    let ReleasePreview::Available {
        tag,
        commit,
        asset_name,
        asset_sha256,
        ..
    } = preview
    else {
        unreachable!()
    };
    let transaction_id = Uuid::new_v4().simple().to_string();
    let root = match updater_dir(&state) {
        Ok(root) => root,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": error })),
            )
                .into_response()
        }
    };
    let controller = root.join(format!("dev-{commit}-{transaction_id}.py"));
    if let Err(error) = materialize_controller(&commit, &controller).await {
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response();
    }
    let log_path = root.join(format!("{transaction_id}.log"));
    let log = match fs::File::create(&log_path) {
        Ok(log) => log,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response()
        }
    };
    let stderr = match log.try_clone() {
        Ok(stderr) => stderr,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response()
        }
    };
    let mut controller_command = Command::new("uv");
    #[cfg(unix)]
    controller_command.process_group(0);
    let child = controller_command
        .arg("run")
        .arg(&controller)
        .args([
            "prod",
            "deploy",
            "--release",
            &tag,
            "--controller-mode",
            "--controller-release-tag",
            &tag,
            "--controller-expected-full-commit",
            &commit,
            "--controller-expected-asset-name",
            &asset_name,
            "--controller-expected-asset-sha256",
            &asset_sha256,
            "--transaction-id",
            &transaction_id,
        ])
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr))
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("could not launch release controller: {error}")
                })),
            )
                .into_response();
        }
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if let ReleaseTransactionStatus::Present {
            transaction_id: durable_id,
            state: durable_state,
            ..
        } = read_status(&state, selected_backend)
        {
            if durable_id == transaction_id && durable_state != "preparing" {
                tokio::task::spawn_blocking(move || {
                    if let Err(error) = child.wait() {
                        tracing::warn!(%error, "failed to reap release controller");
                    }
                });
                return (
                    StatusCode::ACCEPTED,
                    Json(ApproveReleaseUpdateResponse { transaction_id }),
                )
                    .into_response();
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": format!("release controller exited before durable handoff ({status})")
                    })),
                )
                    .into_response();
            }
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("could not observe release controller: {error}")
                    })),
                )
                    .into_response();
            }
            Ok(None) if tokio::time::Instant::now() >= deadline => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-child.id().cast_signed(), libc::SIGKILL);
                }
                #[cfg(not(unix))]
                let _ = child.kill();
                let _ = child.wait();
                return (
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(serde_json::json!({
                        "error": "release controller did not publish durable status within 60 seconds"
                    })),
                )
                    .into_response();
            }
            Ok(None) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_is_bound_to_both_tag_and_commit() {
        let preview = ReleasePreview::Available {
            tag: "v1.2.3".to_string(),
            version: "1.2.3".to_string(),
            commit: "a".repeat(40),
            asset_name: "asset".to_string(),
            asset_sha256: "b".repeat(64),
            release_url: "url".to_string(),
            notes: String::new(),
            newer_than_current: true,
        };
        assert!(valid_approval(
            &ApproveReleaseUpdateRequest {
                tag: "v1.2.3".to_string(),
                commit: "a".repeat(40),
                asset_name: "asset".to_string(),
                asset_sha256: "b".repeat(64),
            },
            &preview
        ));
        assert!(!valid_approval(
            &ApproveReleaseUpdateRequest {
                tag: "v1.2.4".to_string(),
                commit: "a".repeat(40),
                asset_name: "asset".to_string(),
                asset_sha256: "b".repeat(64),
            },
            &preview
        ));
        assert!(!valid_approval(
            &ApproveReleaseUpdateRequest {
                tag: "v1.2.3".to_string(),
                commit: "c".repeat(40),
                asset_name: "asset".to_string(),
                asset_sha256: "b".repeat(64),
            },
            &preview
        ));
    }

    #[test]
    fn semantic_version_ordering_does_not_treat_older_as_newer() {
        assert!(version_triplet("v2.0.0") > version_triplet("1.99.99"));
        assert!(version_triplet("1.9.0") < version_triplet("1.10.0"));
        assert_eq!(None, version_triplet("latest"));
    }

    #[test]
    fn asset_names_cover_release_platform_contract() {
        if matches!(std::env::consts::ARCH, "x86_64" | "aarch64")
            && matches!(std::env::consts::OS, "macos" | "linux")
        {
            assert!(asset_name().unwrap().starts_with("phoenix_ide-"));
        }
    }
}
