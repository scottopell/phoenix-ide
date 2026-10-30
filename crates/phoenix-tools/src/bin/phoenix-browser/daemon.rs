use super::cli::parse_idle;
use super::protocol::{self, ErrorCode, Outcome, Request, Response, SessionPaths};
use async_trait::async_trait;
use base64::Engine;
use fs2::FileExt;
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use phoenix_core::work_scope::ResourceScopeKey;
use phoenix_tools::browser::{
    BrowserAccess, BrowserEvalTool, BrowserEvaluation, BrowserEvaluationError, BrowserOperation,
    BrowserSession, BrowserSessionManager,
};
use phoenix_tools::{ToolImage, ToolOutput};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

/// Called before creating the daemon's Tokio runtime or starting any threads.
pub fn close_inherited_descriptors() -> Result<(), String> {
    let entries = std::fs::read_dir("/dev/fd")
        .or_else(|_| std::fs::read_dir("/proc/self/fd"))
        .map_err(|e| e.to_string())?;
    let descriptors: Vec<i32> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .filter(|fd| *fd > 2)
        .collect();
    for fd in descriptors {
        // SAFETY: daemon startup owns these inherited descriptors; there are no other threads.
        unsafe {
            libc::close(fd);
        }
    }
    Ok(())
}

pub fn private_directory(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("browser runtime directory must be absolute".into());
    }
    match std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
    {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("create {}: {error}", path.display())),
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "{} must be a private directory owned by this user (mode 0700)",
            path.display()
        ));
    }
    Ok(())
}

fn open_control(path: &Path, append: bool) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .append(append)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "{} must be a private regular file owned by this user",
            path.display()
        ));
    }
    Ok(file)
}

pub fn lock_file(path: &Path) -> Result<File, String> {
    open_control(path, false)
}
pub fn append_log(path: &Path) -> Result<File, String> {
    open_control(path, true)
}

struct Owner {
    _lock: File,
    socket: std::path::PathBuf,
}
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

struct Existing {
    session: Arc<RwLock<BrowserSession>>,
    manager: Arc<BrowserSessionManager>,
}
#[async_trait]
impl BrowserAccess for Existing {
    async fn browser(&self) -> Result<Arc<RwLock<BrowserSession>>, String> {
        Ok(self.session.clone())
    }

    async fn scenario_owner(
        &self,
        session: &Arc<RwLock<BrowserSession>>,
    ) -> Result<Arc<phoenix_browser::scenario::ScenarioOwner>, String> {
        self.manager.scenario_owner_for_session(session).await
    }
}
struct Unavailable;
#[async_trait]
impl BrowserAccess for Unavailable {
    async fn browser(&self) -> Result<Arc<RwLock<BrowserSession>>, String> {
        Err("browser state is gone; use open URL".into())
    }

    async fn scenario_owner(
        &self,
        _session: &Arc<RwLock<BrowserSession>>,
    ) -> Result<Arc<phoenix_browser::scenario::ScenarioOwner>, String> {
        Err("browser state is gone; use open URL".into())
    }
}

pub(super) fn operation(command: &str) -> Option<BrowserOperation> {
    Some(match command {
        "open" => BrowserOperation::Navigate,
        "snapshot" => BrowserOperation::Snapshot,
        "eval" => BrowserOperation::Eval,
        "screenshot" => BrowserOperation::TakeScreenshot,
        "console" => BrowserOperation::RecentConsoleLogs,
        "console-clear" => BrowserOperation::ClearConsoleLogs,
        "resize" => BrowserOperation::Resize,
        "wait" => BrowserOperation::WaitForSelector,
        "click" => BrowserOperation::Click,
        "type" => BrowserOperation::Type,
        "key" => BrowserOperation::KeyPress,
        "profile" => BrowserOperation::Profile,
        _ => return None,
    })
}

fn take_artifacts(value: &mut Value, paths: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            let keys: Vec<_> = object.keys().cloned().collect();
            for key in keys {
                if matches!(
                    key.as_str(),
                    "path" | "artifact_path" | "samples_path" | "post"
                ) && object[&key].is_string()
                {
                    paths.push(
                        object
                            .remove(&key)
                            .expect("artifact key exists")
                            .as_str()
                            .expect("artifact path is a string")
                            .to_owned(),
                    );
                } else if let Some(child) = object.get_mut(&key) {
                    take_artifacts(child, paths);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                take_artifacts(value, paths);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

async fn externalize_saved_samples(data: &mut Value) -> Result<(), String> {
    let Some(object) = data.as_object_mut() else {
        return Ok(());
    };
    let (Some(path), Some(samples)) = (
        object.get("samples_path").and_then(Value::as_str),
        object.get("raw_samples"),
    ) else {
        return Ok(());
    };
    let saved = tokio::fs::read(path)
        .await
        .map_err(|error| format!("Read saved scenario samples {path}: {error}"))?;
    let saved: Value = serde_json::from_slice(&saved)
        .map_err(|error| format!("Decode saved scenario samples {path}: {error}"))?;
    if saved.get("raw_samples") != Some(samples) {
        return Err(format!(
            "Saved scenario samples {path} do not match the complete result"
        ));
    }
    object.remove("raw_samples");
    Ok(())
}

struct CliImageArtifact(std::path::PathBuf);

struct PendingImageArtifact(Option<std::path::PathBuf>);

impl Drop for PendingImageArtifact {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl CliImageArtifact {
    async fn from_image(
        image: &ToolImage,
        existing: &[String],
        directory: &Path,
    ) -> Result<Self, String> {
        let extension = match image.media_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            other => {
                return Err(format!(
                    "Unsupported CLI image artifact media type: {other}"
                ))
            }
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .map_err(|error| format!("Decode image artifact: {error}"))?;
        // SAFETY: geteuid has no preconditions.
        let owner = unsafe { libc::geteuid() };
        for path in existing {
            if let Ok(metadata) = tokio::fs::symlink_metadata(path).await {
                if metadata.is_file()
                    && metadata.uid() == owner
                    && metadata.len() == bytes.len() as u64
                    && tokio::fs::read(path)
                        .await
                        .is_ok_and(|saved| saved == bytes)
                {
                    return Ok(Self(std::path::PathBuf::from(path)));
                }
            }
        }
        private_directory(directory)?;
        let path = directory.join(format!("{}.{}", uuid::Uuid::new_v4(), extension));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("Create image artifact: {error}"))?;
        let mut pending = PendingImageArtifact(Some(path));
        let mut file = tokio::fs::File::from_std(file);
        file.write_all(&bytes)
            .await
            .map_err(|error| format!("Write image artifact: {error}"))?;
        file.flush()
            .await
            .map_err(|error| format!("Flush image artifact: {error}"))?;
        Ok(Self(pending.0.take().expect("pending image artifact")))
    }
}

fn adapt_evaluation_error(
    session: &str,
    lifetime: &str,
    error: &BrowserEvaluationError,
) -> Response {
    let mut response = Response::error(
        session,
        Some(lifetime),
        ErrorCode::OperationFailed,
        error.to_string(),
    );
    if let (Outcome::Error { error: failure }, Some(details)) =
        (&mut response.outcome, error.exception())
    {
        failure.details = Some(json!(details));
    }
    response
}

async fn adapt(
    session: &str,
    lifetime: Option<&str>,
    output: &ToolOutput,
    image_directory: &Path,
) -> Response {
    let mut artifacts = Vec::new();
    let mut data = output.display_data().cloned();
    let samples_error = if let Some(data) = &mut data {
        let error = externalize_saved_samples(data).await.err();
        take_artifacts(data, &mut artifacts);
        error
    } else {
        None
    };
    for image in output.images() {
        let artifact = match CliImageArtifact::from_image(image, &artifacts, image_directory).await
        {
            Ok(artifact) => artifact,
            Err(error) => {
                let mut response = Response::error(
                    session,
                    lifetime,
                    ErrorCode::OperationFailed,
                    format!(
                        "Browser operation returned an image that could not be preserved: {error}"
                    ),
                );
                if let Outcome::Error { error } = &mut response.outcome {
                    error.details = Some(json!({"tool_text":output.output(),"data":data}));
                    error.artifacts = artifacts;
                }
                return response;
            }
        };
        let path = artifact.0.to_string_lossy().into_owned();
        if !artifacts.contains(&path) {
            artifacts.push(path);
        }
    }
    if let Some(error) = samples_error {
        let mut response = Response::error(
            session,
            lifetime,
            ErrorCode::OperationFailed,
            format!("Browser operation completed but its saved samples could not be verified: {error}. Do not replay the operation automatically."),
        );
        if let Outcome::Error { error } = &mut response.outcome {
            error.details = Some(json!({"tool_text": output.output(), "data": data}));
            error.artifacts = artifacts;
        }
        return response;
    }
    if output.is_success() {
        Response::success(
            session,
            lifetime,
            json!({"text": output.output(), "data":data, "artifacts":artifacts}),
        )
    } else {
        let mut response = Response::error(
            session,
            lifetime,
            ErrorCode::OperationFailed,
            output.output(),
        );
        if let Outcome::Error { error } = &mut response.outcome {
            error.details = data;
            error.artifacts = artifacts;
        }
        response
    }
}

pub async fn offline(session: &str, request: &Request) -> Response {
    if request.command == "schema" {
        let selected = request.input["command"].as_str();
        let entries: Vec<_> = BrowserOperation::ALL.iter().copied().filter(|op| selected.is_none() || selected.and_then(operation) == Some(*op) || selected == Some(op.name())).map(|op| json!({"name":op.name(),"description":op.description(),"input_schema":op.input_schema()})).collect();
        if entries.is_empty() {
            return Response::error(
                session,
                None,
                ErrorCode::InvalidInput,
                "unknown schema command",
            );
        }
        return Response::success(session, None, json!({"operations":entries}));
    }
    let image_directory = PhoenixRuntimeEnvironment::detect()
        .tmp_root()
        .join("browser-cli-images");
    adapt(
        session,
        None,
        &BrowserOperation::Profile
            .execute(request.input.clone(), &Unavailable)
            .await,
        &image_directory,
    )
    .await
}

struct State {
    manager: Arc<BrowserSessionManager>,
    scope: ResourceScopeKey,
    session: String,
    lifetime: String,
    idle: Duration,
    image_directory: std::path::PathBuf,
}

struct ReadyOperation {
    browser: Arc<RwLock<BrowserSession>>,
    operation: BrowserOperation,
    browser_lifetime: String,
}

enum Admission {
    Reply(Response),
    Ready(ReadyOperation),
    CleanupFailed(RetainedCleanup),
}

struct RetainedCleanup {
    error: String,
    browser_lifetime: Option<String>,
}

enum BrowserLookupFailure {
    Unavailable(String),
    CleanupFailed(RetainedCleanup),
}

impl State {
    async fn live_browser(
        &self,
    ) -> Result<Option<Arc<RwLock<BrowserSession>>>, BrowserLookupFailure> {
        let browser = match self.manager.lookup_existing(&self.scope).await {
            phoenix_browser::session::BrowserSessionLookup::Absent => return Ok(None),
            phoenix_browser::session::BrowserSessionLookup::Ready(browser) => browser,
            phoenix_browser::session::BrowserSessionLookup::Retained(retained) => {
                return Err(BrowserLookupFailure::CleanupFailed(RetainedCleanup {
                    error: format!(
                        "retained browser initialization at {}: {}",
                        retained.profile_path.display(),
                        retained.reason
                    ),
                    browser_lifetime: Some(retained.browser_lifetime),
                }))
            }
        };
        let running = browser
            .write()
            .await
            .is_running()
            .map_err(|e| BrowserLookupFailure::Unavailable(e.to_string()))?;
        if running {
            Ok(Some(browser))
        } else {
            let browser_lifetime = browser.read().await.launch_identity().launch_uuid.clone();
            self.manager.kill_session(&self.scope).await.map_err(|e| {
                BrowserLookupFailure::CleanupFailed(RetainedCleanup {
                    error: e.to_string(),
                    browser_lifetime: Some(browser_lifetime),
                })
            })?;
            Ok(None)
        }
    }

    async fn admit(&self, request: &Request) -> Admission {
        let existing = match self.live_browser().await {
            Ok(browser) => browser,
            Err(BrowserLookupFailure::CleanupFailed(failure)) => {
                return Admission::CleanupFailed(failure);
            }
            Err(BrowserLookupFailure::Unavailable(error)) => {
                return Admission::Reply(Response::error(
                    &self.session,
                    Some(&self.lifetime),
                    ErrorCode::BrowserStateUnavailable,
                    error,
                ))
            }
        };
        if request.command == "status" {
            let browser = existing;
            let browser_lifetime = match browser {
                Some(browser) => Some(browser.read().await.launch_identity().launch_uuid.clone()),
                None => None,
            };
            let identity = self
                .manager
                .cleanup_identifiers(&self.scope)
                .await
                .into_iter()
                .next();
            return Admission::Reply(Response::success(
                &self.session,
                Some(&self.lifetime),
                json!({"active":browser_lifetime.is_some(),"idle_timeout_seconds":self.idle.as_secs(),"daemon_pid":std::process::id(),"chrome_pid":identity.as_ref().and_then(|id|id.chrome_pid),"profile_path":identity.map(|id|id.profile_path)}),
            ).with_browser_lifetime(browser_lifetime));
        }
        let Some(operation) = operation(&request.command) else {
            return Admission::Reply(Response::error(
                &self.session,
                Some(&self.lifetime),
                ErrorCode::InvalidInput,
                "unknown command",
            ));
        };
        let browser = match (request.command.as_str(), existing) {
            (_, Some(browser)) => browser,
            ("open", None) => {
                if let Err(error) = self.manager.kill_session(&self.scope).await {
                    return Admission::CleanupFailed(RetainedCleanup {
                        error: error.to_string(),
                        browser_lifetime: None,
                    });
                }
                match self.manager.get_session(&self.scope).await {
                    Ok(browser) => browser,
                    Err(error) => {
                        return Admission::Reply(Response::error(
                            &self.session,
                            Some(&self.lifetime),
                            ErrorCode::BrowserStartFailed,
                            error.to_string(),
                        ))
                    }
                }
            }
            (_, None) => {
                return Admission::Reply(Response::error(
                    &self.session,
                    Some(&self.lifetime),
                    ErrorCode::StateLost,
                    "browser state is gone; use open URL to start fresh",
                ))
            }
        };
        let browser_lifetime = browser.read().await.launch_identity().launch_uuid.clone();
        Admission::Ready(ReadyOperation {
            browser,
            operation,
            browser_lifetime,
        })
    }

    async fn execute(&self, request: &Request, ready: &ReadyOperation) -> Response {
        let ReadyOperation {
            browser,
            operation,
            browser_lifetime,
        } = ready;
        let access = Existing {
            session: browser.clone(),
            manager: self.manager.clone(),
        };
        if *operation == BrowserOperation::Eval {
            let response = match BrowserEvalTool::evaluate(request.input.clone(), &access).await {
                Ok(value) => {
                    let (text, data, artifacts) = match value {
                        BrowserEvaluation::Json(value) => (
                            "JavaScript evaluated",
                            json!({"kind":"json", "value":value}),
                            Vec::new(),
                        ),
                        BrowserEvaluation::Undefined => (
                            "JavaScript returned undefined",
                            json!({"kind":"undefined"}),
                            Vec::new(),
                        ),
                        BrowserEvaluation::Unserializable(value) => (
                            "JavaScript returned a non-JSON value",
                            json!({"kind":"unserializable", "value":value}),
                            Vec::new(),
                        ),
                        BrowserEvaluation::UnserializableArtifact(path) => (
                            "JavaScript non-JSON result saved to artifact",
                            json!({"kind":"unserializable_artifact"}),
                            vec![path.into_path()],
                        ),
                        BrowserEvaluation::JsonArtifact(path) => (
                            "JavaScript result saved to artifact",
                            json!({"kind":"json_artifact"}),
                            vec![path.into_path()],
                        ),
                    };
                    Response::success(
                        &self.session,
                        Some(&self.lifetime),
                        json!({"kind":"evaluation","text":text,"data":data,"artifacts":artifacts}),
                    )
                }
                Err(error) => adapt_evaluation_error(&self.session, &self.lifetime, &error),
            };
            return response.with_browser_lifetime(Some(browser_lifetime.clone()));
        }
        let console_capture = if request.command == "open" {
            Some(
                browser
                    .read()
                    .await
                    .begin_console_navigation_capture()
                    .await,
            )
        } else {
            None
        };
        let output = operation.execute(request.input.clone(), &access).await;
        if request.command != "open" || !output.is_success() {
            return adapt(
                &self.session,
                Some(&self.lifetime),
                &output,
                &self.image_directory,
            )
            .await
            .with_browser_lifetime(Some(browser_lifetime.clone()));
        }
        self.open_result(
            &access,
            browser,
            browser_lifetime,
            console_capture.expect("open capture attempted"),
        )
        .await
    }

    async fn open_result(
        &self,
        access: &Existing,
        browser: &Arc<RwLock<BrowserSession>>,
        browser_lifetime: &str,
        capture: Result<
            phoenix_browser::session::ConsoleNavigationCapture,
            phoenix_browser::BrowserError,
        >,
    ) -> Response {
        let output = BrowserOperation::Snapshot
            .execute(json!({"interactive":true}), access)
            .await;
        let checkpoint = browser.read().await.snapshot_checkpoint();
        let mut snapshot = adapt(
            &self.session,
            Some(&self.lifetime),
            &output,
            &self.image_directory,
        )
        .await;
        let (url, title) = {
            let guard = browser.read().await;
            let url = navigation_observation(guard.page.url()).await;
            let title = navigation_observation(guard.page.get_title()).await;
            (url, title)
        };
        let mut observation_errors = serde_json::Map::new();
        let url = observed_value("url", url, &mut observation_errors);
        let title = observed_value("title", title, &mut observation_errors);
        let captured = match capture {
            Ok(capture) => {
                browser
                    .read()
                    .await
                    .finish_console_navigation_capture(capture)
                    .await
            }
            Err(error) => Err(error),
        };
        let (errors, console_capture_error) = match captured {
            Ok(entries) => (
                Some(
                    entries
                        .into_iter()
                        .filter(|entry| {
                            matches!(
                                entry.level,
                                phoenix_browser::ConsoleLevel::Error
                                    | phoenix_browser::ConsoleLevel::Assert
                            )
                        })
                        .map(|entry| json!({"level":entry.level,"text":entry.text}))
                        .collect::<Vec<_>>(),
                ),
                None,
            ),
            Err(error) => (None, Some(error.to_string())),
        };
        revalidate_open_snapshot(browser, checkpoint.as_ref(), &mut snapshot).await;
        Response::success(
            &self.session,
            Some(&self.lifetime),
            json!({"url":url,"title":title,"observation_errors":observation_errors,"navigation":"succeeded","snapshot":snapshot.outcome,"console_errors":errors,"console_capture_error":console_capture_error}),
        ).with_browser_lifetime(Some(browser_lifetime.into()))
    }
}

async fn revalidate_open_snapshot(
    browser: &Arc<RwLock<BrowserSession>>,
    checkpoint: Option<&phoenix_browser::snapshot::SnapshotCheckpoint>,
    snapshot: &mut Response,
) {
    if !matches!(snapshot.outcome, Outcome::Success { .. }) {
        return;
    }
    let validation = async {
        let checkpoint = checkpoint.ok_or_else(|| "Snapshot references expired".to_string())?;
        browser
            .read()
            .await
            .validate_snapshot_checkpoint(checkpoint)
            .await
            .map_err(|error| error.to_string())
    };
    let error = match tokio::time::timeout(Duration::from_secs(5), validation).await {
        Ok(Ok(())) => return,
        Ok(Err(error)) => error,
        Err(_) => "Snapshot validation timed out after 5 seconds".into(),
    };
    snapshot.outcome = Response::error(
        &snapshot.session,
        snapshot.lifetime.as_deref(),
        ErrorCode::OperationFailed,
        format!("The open snapshot expired while observing the page: {error}. Call snapshot again for current references."),
    ).outcome;
}

async fn navigation_observation<E: std::fmt::Display>(
    observation: impl std::future::Future<Output = Result<Option<String>, E>>,
) -> Result<String, String> {
    tokio::time::timeout(Duration::from_secs(5), observation)
        .await
        .map_err(|_| "Navigation observation timed out after 5 seconds".to_string())?
        .map_err(|error| format!("Navigation observation failed: {error}"))?
        .ok_or_else(|| "Navigation observation returned no value".into())
}

fn observed_value(
    field: &str,
    observation: Result<String, String>,
    errors: &mut serde_json::Map<String, Value>,
) -> Option<String> {
    match observation {
        Ok(value) => Some(value),
        Err(error) => {
            errors.insert(field.into(), Value::String(error));
            None
        }
    }
}

fn shutdown_signal() -> Result<impl std::future::Future<Output = ()> + Send, String> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| e.to_string())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|e| e.to_string())?;
    Ok(async move {
        tokio::select! { _ = terminate.recv() => {}, _ = interrupt.recv() => {} }
    })
}

struct Job {
    request: Request,
    stream: UnixStream,
}

fn collect_artifacts(value: &Value, artifacts: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == "artifacts" {
                    if let Some(paths) = value.as_array() {
                        for path in paths.iter().filter_map(Value::as_str) {
                            if !artifacts.iter().any(|existing| existing == path) {
                                artifacts.push(path.to_owned());
                            }
                        }
                    }
                } else {
                    collect_artifacts(value, artifacts);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_artifacts(value, artifacts);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

async fn send_response(stream: &mut UnixStream, response: &Response) {
    let encoded = match protocol::encode_message(response) {
        Ok(encoded) => encoded,
        Err(reason) => {
            eprintln!("browser CLI response could not be encoded: {reason}");
            let mut fallback = Response::error(
                &response.session,
                response.lifetime.as_deref(),
                ErrorCode::OperationFailed,
                format!("Browser operation produced a response that could not be delivered: {reason}. Browser-side effects may have completed. Do not replay the operation automatically; inspect any returned artifacts."),
            ).with_browser_lifetime(response.browser_lifetime.clone());
            if let Outcome::Error { error } = &mut fallback.outcome {
                match &response.outcome {
                    Outcome::Success { result } => collect_artifacts(result, &mut error.artifacts),
                    Outcome::Error { error: original } => {
                        error.artifacts.clone_from(&original.artifacts);
                    }
                }
            }
            match protocol::encode_message(&fallback) {
                Ok(encoded) => encoded,
                Err(reason) => {
                    eprintln!("browser CLI error response could not be encoded: {reason}");
                    return;
                }
            }
        }
    };
    match tokio::time::timeout(Duration::from_secs(10), encoded.write_to(stream)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => eprintln!("browser CLI response write failed: {error}"),
        Err(error) => eprintln!("browser CLI response write timed out: {error}"),
    }
}

fn cancelled(state: &State, browser_lifetime: Option<String>) -> Response {
    Response::error(&state.session, Some(&state.lifetime), ErrorCode::OperationCancelled, "Shutdown interrupted the operation; browser-side effects may have partially executed. The browser is being closed; do not replay the action automatically.").with_browser_lifetime(browser_lifetime)
}

enum JobResult {
    Reply(Response),
    CleanupFailed(RetainedCleanup),
}

enum WorkerEvent {
    Completed,
    CleanupFailed,
}

async fn run_job(state: &State, job: &mut Job, shutdown: &mut watch::Receiver<bool>) -> JobResult {
    if *shutdown.borrow() {
        return JobResult::Reply(cancelled(state, None));
    }
    // Startup retains ownership until the manager can authoritatively tear down its session.
    let admission = state.admit(&job.request).await;
    let ready = match admission {
        Admission::Reply(response) => return JobResult::Reply(response),
        Admission::CleanupFailed(failure) => return JobResult::CleanupFailed(failure),
        Admission::Ready(ready) => ready,
    };
    if *shutdown.borrow() {
        return JobResult::Reply(cancelled(state, Some(ready.browser_lifetime)));
    }
    JobResult::Reply(tokio::select! {
        response = state.execute(&job.request, &ready) => response,
        _ = shutdown.changed() => cancelled(state, Some(ready.browser_lifetime.clone())),
    })
}

async fn worker(
    state: Arc<State>,
    mut jobs: mpsc::Receiver<Job>,
    mut shutdown: watch::Receiver<bool>,
    completed: mpsc::UnboundedSender<WorkerEvent>,
) -> Option<RetainedCleanup> {
    let mut retained: Option<RetainedCleanup> = None;
    while let Some(mut job) = jobs.recv().await {
        let response = match &retained {
            Some(failure) => {
                cleanup_failure(&state, &failure.error, failure.browser_lifetime.clone())
            }
            None => match run_job(&state, &mut job, &mut shutdown).await {
                JobResult::Reply(response) => response,
                JobResult::CleanupFailed(failure) => {
                    let response =
                        cleanup_failure(&state, &failure.error, failure.browser_lifetime.clone());
                    retained = Some(failure);
                    let _ = completed.send(WorkerEvent::CleanupFailed);
                    response
                }
            },
        };
        send_response(&mut job.stream, &response).await;
        let _ = completed.send(WorkerEvent::Completed);
    }
    retained
}

async fn read_job(mut stream: UnixStream, state: &State) -> Option<Job> {
    match tokio::time::timeout(Duration::from_secs(10), protocol::read_message(&mut stream)).await {
        Ok(Ok(request)) => Some(Job { request, stream }),
        failure => {
            let message = match failure {
                Ok(Err(error)) => error,
                Err(_) => "request read timed out".into(),
                Ok(Ok(_)) => unreachable!(),
            };
            send_response(
                &mut stream,
                &Response::error(
                    &state.session,
                    Some(&state.lifetime),
                    ErrorCode::InvalidInput,
                    message,
                ),
            )
            .await;
            None
        }
    }
}

const MAX_PENDING_CLIENTS: usize = 32;

fn receive_client(clients: &mut JoinSet<Option<Job>>, stream: UnixStream, state: Arc<State>) {
    if clients.len() < MAX_PENDING_CLIENTS {
        clients.spawn(async move { read_job(stream, &state).await });
    }
}

fn reject_busy(clients: &mut JoinSet<Option<Job>>, mut job: Job, state: Arc<State>) {
    clients.spawn(async move {
        send_response(
            &mut job.stream,
            &Response::error(
                &state.session,
                Some(&state.lifetime),
                ErrorCode::Busy,
                "Session request queue is full; this request was not executed.",
            ),
        )
        .await;
        None
    });
}

enum StopReason {
    Close(UnixStream),
    Signal,
    Automatic,
}

async fn serve(
    listener: &UnixListener,
    state: Arc<State>,
    socket: &Path,
    signal: impl std::future::Future<Output = ()>,
) -> Result<(), String> {
    let (jobs_tx, jobs_rx) = mpsc::channel(16);
    let (completed_tx, mut completed_rx) = mpsc::unbounded_channel();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(worker(state.clone(), jobs_rx, shutdown_rx, completed_tx));
    let mut pending = 0_u32;
    let mut deadline = tokio::time::Instant::now() + state.idle;
    let mut clients = JoinSet::new();
    tokio::pin!(signal);
    let result = loop {
        tokio::select! {
            () = &mut signal => break Ok(StopReason::Signal),
            () = tokio::time::sleep_until(deadline), if pending == 0 => break Ok(StopReason::Automatic),
            completion = completed_rx.recv(), if pending > 0 => {
                match completion {
                    Some(WorkerEvent::Completed) => {},
                    Some(WorkerEvent::CleanupFailed) => break Ok(StopReason::Automatic),
                    None => break Err("browser worker stopped unexpectedly".into()),
                }
                pending -= 1;
                deadline = tokio::time::Instant::now() + state.idle;
            }
            accepted = listener.accept() => {
                let (stream, _) = match accepted { Ok(accepted) => accepted, Err(error) => break Err(format!("accept: {error}")) };
                receive_client(&mut clients, stream, state.clone());
            }
            received = clients.join_next(), if !clients.is_empty() => {
                let mut job = match received.expect("pending client") {
                    Ok(Some(job)) => job,
                    Ok(None) => continue,
                    Err(error) => break Err(format!("request reader failed: {error}")),
                };
                if job.request.command == "close" { break Ok(StopReason::Close(job.stream)); }
                match jobs_tx.try_send(job) {
                    Ok(()) => pending += 1,
                    Err(mpsc::error::TrySendError::Full(returned)) => {
                        job = returned;
                        reject_busy(&mut clients, job, state.clone());
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break Err("browser worker stopped unexpectedly".into()),
                }
            }
        }
    };
    let _ = shutdown_tx.send(true);
    clients.shutdown().await;
    drop(jobs_tx);
    let worker_result = task
        .await
        .map_err(|e| format!("browser worker failed: {e}"));
    let mut browser_lifetime = match state.manager.get_existing(&state.scope).await {
        Some(browser) => Some(browser.read().await.launch_identity().launch_uuid.clone()),
        None => None,
    };
    let (worker_result, mut retained) = match worker_result {
        Ok(retained) => (Ok(()), retained),
        Err(error) => (Err(error), None),
    };
    if matches!(&result, Ok(StopReason::Close(_) | StopReason::Signal)) {
        if let Some(failure) = retained.take() {
            browser_lifetime = failure.browser_lifetime;
        }
    }
    let (result, close_stream) = match result {
        Ok(StopReason::Close(stream)) => (Ok(()), Some(stream)),
        Ok(StopReason::Signal | StopReason::Automatic) => (Ok(()), None),
        Err(error) => (Err(error), None),
    };
    let cleanup = finish_shutdown(
        listener,
        state,
        socket,
        close_stream,
        browser_lifetime,
        retained,
    )
    .await;
    result.and(worker_result).and(cleanup)
}

async fn finish_shutdown(
    listener: &UnixListener,
    state: Arc<State>,
    socket: &Path,
    mut close_stream: Option<UnixStream>,
    browser_lifetime: Option<String>,
    initial_failure: Option<RetainedCleanup>,
) -> Result<(), String> {
    let (mut failure, browser_lifetime) = match initial_failure {
        Some(failure) => (Some(failure.error), failure.browser_lifetime),
        None => (None, browser_lifetime),
    };
    loop {
        let cleanup = match failure.take() {
            Some(error) => Err(error),
            None => state
                .manager
                .shutdown_all()
                .await
                .map_err(|error| error.to_string()),
        };
        if cleanup.is_ok() {
            std::fs::remove_file(socket).map_err(|e| format!("retire endpoint: {e}"))?;
            if let Some(mut stream) = close_stream {
                let response = Response::success(
                    &state.session,
                    Some(&state.lifetime),
                    json!({"active":false,"closed":true}),
                )
                .with_browser_lifetime(browser_lifetime);
                send_response(&mut stream, &response).await;
            }
            return Ok(());
        }
        let error = cleanup.expect_err("failed cleanup");
        tracing::error!(%error, "browser cleanup failed; retaining session ownership until explicit close retries cleanup");
        if let Some(mut stream) = close_stream.take() {
            send_response(
                &mut stream,
                &cleanup_failure(&state, &error, browser_lifetime.clone()),
            )
            .await;
        }
        let signal = match shutdown_signal() {
            Ok(signal) => signal,
            Err(failure) => retain_owner_forever(failure).await,
        };
        tokio::pin!(signal);
        let mut clients = JoinSet::new();
        loop {
            let received = tokio::select! {
                () = &mut signal => break,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _)) => receive_client(&mut clients, stream, state.clone()),
                        Err(failure) => retain_owner_forever(failure.to_string()).await,
                    }
                    continue;
                },
                received = clients.join_next(), if !clients.is_empty() => received,
            };
            let Some(Ok(Some(mut job))) = received else {
                continue;
            };
            if job.request.command == "close" {
                close_stream = Some(job.stream);
                break;
            }
            let response = if job.request.command == "status" {
                Response::success(
                    &state.session,
                    Some(&state.lifetime),
                    json!({
                        "phase":"stopping", "cleanup_error":error,
                    "resources": state.manager.cleanup_identifiers(&state.scope).await.into_iter().map(|resource| json!({"chrome_pid":resource.chrome_pid,"profile_path":resource.profile_path})).collect::<Vec<_>>(),
                    }),
                )
                .with_browser_lifetime(browser_lifetime.clone())
            } else {
                cleanup_failure(&state, &error, browser_lifetime.clone())
            };
            clients.spawn(async move {
                send_response(&mut job.stream, &response).await;
                None
            });
        }
        clients.shutdown().await;
    }
}

async fn retain_owner_forever(error: String) -> ! {
    tracing::error!(%error, "cleanup control unavailable; retaining ownership requires manual process termination");
    match std::future::pending::<std::convert::Infallible>().await {}
}

fn cleanup_failure(state: &State, error: &str, browser_lifetime: Option<String>) -> Response {
    Response::error(
        &state.session,
        Some(&state.lifetime),
        ErrorCode::CleanupFailed,
        format!("Session cleanup is incomplete and ownership is retained: {error}. Inspect status, repair the reported resource, then retry close."),
    ).with_browser_lifetime(browser_lifetime)
}

pub async fn run_from_args(args: &[String]) -> Result<(), String> {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .try_init();
    if args.len() != 4 {
        return Err("invalid internal daemon arguments".into());
    }
    let key = &args[0];
    if key.len() != 24 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid session key".into());
    }
    let paths = SessionPaths {
        directory: args[3].clone().into(),
        key: key.clone(),
    };
    private_directory(&paths.directory)?;
    let lock = lock_file(&paths.owner_lock())?;
    FileExt::try_lock_exclusive(&lock).map_err(|e| format!("session already owned: {e}"))?;
    let socket = paths.socket();
    if socket.as_os_str().as_encoded_bytes().len() > 100 {
        return Err("runtime socket path too long; set PHOENIX_BROWSER_RUNTIME_DIR to a shorter absolute private directory".into());
    }
    if let Ok(metadata) = std::fs::symlink_metadata(&socket) {
        if !metadata.file_type().is_socket() {
            return Err("refusing to replace non-socket session endpoint".into());
        }
        std::fs::remove_file(&socket).map_err(|e| e.to_string())?;
    }
    let signal = shutdown_signal()?;
    let listener =
        UnixListener::bind(&socket).map_err(|e| format!("bind {}: {e}", socket.display()))?;
    let _owner = Owner {
        _lock: lock,
        socket: socket.clone(),
    };
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    let runtime_env = PhoenixRuntimeEnvironment::detect();
    let image_directory = runtime_env.tmp_root().join("browser-cli-images");
    let manager = BrowserSessionManager::for_cli(&runtime_env);
    manager.set_scope_liveness_hook(Arc::new(|_, _| Box::pin(async { true })));
    let lifetime = uuid::Uuid::new_v4().to_string();
    let state = Arc::new(State {
        manager: manager.clone(),
        scope: ResourceScopeKey::Unattached(format!("cli:{lifetime}")),
        session: args[1].clone(),
        lifetime,
        idle: parse_idle(&args[2])?,
        image_directory,
    });
    serve(&listener, state, &socket, signal).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn existing_access_rejects_competing_scenario_while_worker_holds_session_lock() {
        if matches!(
            std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
            Ok("1" | "true")
        ) {
            return;
        }
        let manager = BrowserSessionManager::new();
        let scope =
            ResourceScopeKey::Unattached(format!("existing-owner-{}", uuid::Uuid::new_v4()));
        let session = manager.get_session(&scope).await.unwrap();
        let owner = manager.scenario_owner_for_session(&session).await.unwrap();
        let (entered, worker_entered) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let worker_session = session.clone();
        owner
            .start(move |_| async move {
                let _guard = worker_session.write().await;
                entered.send(()).unwrap();
                released.await.unwrap();
            })
            .unwrap();
        worker_entered.await.unwrap();
        let access = Existing {
            session,
            manager: manager.clone(),
        };
        let mut admission = Box::pin(BrowserOperation::Profile.execute(
            json!({
                "action":"run_scenario", "runs":1, "warmup":0,
                "steps":[{"kind":"eval", "expression":"true"}]
            }),
            &access,
        ));
        let std::task::Poll::Ready(output) = futures::poll!(&mut admission) else {
            panic!("existing-session scenario admission waited for the worker's lock");
        };
        assert!(!output.is_success());
        assert!(
            output.output().contains("Another browser scenario"),
            "{}",
            output.output()
        );
        drop(admission);
        release.send(()).unwrap();
        manager.shutdown_all().await.expect("browser shutdown");
    }

    #[test]
    fn evaluation_error_values_remain_data_instead_of_artifact_handles() {
        let details = serde_json::from_value(json!({
            "exceptionId":1,"text":"Uncaught","lineNumber":0,"columnNumber":0,
            "exception":{"type":"object","value":{"path":"user/path","post":"user/post","artifact_path":"user/artifact","artifacts":["user/data"],"raw_samples":[{"path":"user"}]}}
        })).unwrap();
        let error = BrowserEvaluationError::Javascript(Box::new(details));
        let expected = json!(error.exception().unwrap());
        let response = adapt_evaluation_error("named:test", "lifetime", &error);
        let Outcome::Error { error } = response.outcome else {
            panic!("expected error")
        };
        assert_eq!(error.details, Some(expected));
        assert!(error.artifacts.is_empty());
    }

    fn reader_state() -> Arc<State> {
        Arc::new(State {
            manager: BrowserSessionManager::new(),
            scope: ResourceScopeKey::Unattached("reader-test".into()),
            session: "named:reader-test".into(),
            lifetime: uuid::Uuid::new_v4().to_string(),
            idle: Duration::from_secs(300),
            image_directory: PhoenixRuntimeEnvironment::detect()
                .tmp_root()
                .join("unused-reader-test-images"),
        })
    }

    #[tokio::test]
    async fn open_snapshot_expires_during_delayed_page_observation() {
        if matches!(
            std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
            Ok("1" | "true")
        ) {
            return;
        }
        let state = reader_state();
        let browser = state.manager.get_session(&state.scope).await.unwrap();
        browser.read().await.page.goto("about:blank").await.unwrap();
        browser.read().await.page.evaluate("document.body.innerHTML = Array.from({length:100}, (_,i) => `<button>${i} ${'accessible action '.repeat(5)}</button>`).join('')").await.unwrap();
        let output = BrowserOperation::Snapshot
            .execute(
                json!({"interactive":true}),
                &Existing {
                    session: browser.clone(),
                    manager: state.manager.clone(),
                },
            )
            .await;
        assert!(output.is_success(), "{}", output.output());
        let checkpoint = browser.read().await.snapshot_checkpoint().unwrap();
        let mut snapshot = adapt(
            &state.session,
            Some(&state.lifetime),
            &output,
            &state.image_directory,
        )
        .await;
        let Outcome::Success { result } = &snapshot.outcome else {
            panic!("snapshot failed")
        };
        let artifact = result["artifacts"][0].as_str().unwrap().to_owned();
        assert!(Path::new(&artifact).exists());
        revalidate_open_snapshot(&browser, Some(&checkpoint), &mut snapshot).await;
        assert!(matches!(snapshot.outcome, Outcome::Success { .. }));

        let (release_observation, delayed_observation) = tokio::sync::oneshot::channel();
        let navigation = tokio::spawn({
            let browser = browser.clone();
            async move {
                let guard = browser.read().await;
                guard
                    .page
                    .goto("data:text/html,<title>Replacement</title><button>New action</button>")
                    .await
                    .unwrap();
                release_observation.send(()).unwrap();
            }
        });
        delayed_observation.await.unwrap();
        let title = browser
            .read()
            .await
            .page
            .get_title()
            .await
            .unwrap()
            .unwrap();
        navigation.await.unwrap();
        revalidate_open_snapshot(&browser, Some(&checkpoint), &mut snapshot).await;
        assert_eq!(title, "Replacement", "later page observation is retained");
        let Outcome::Error { error } = snapshot.outcome else {
            panic!("expired snapshot reported success")
        };
        assert!(error.message.contains("snapshot expired"));
        assert!(error.artifacts.is_empty());
        assert!(!Path::new(&artifact).exists());
        assert!(browser.read().await.snapshot_checkpoint().is_none());
        state.manager.shutdown_all().await.unwrap();
    }

    #[tokio::test]
    async fn partial_clients_do_not_block_complete_requests() {
        use tokio::io::AsyncReadExt;

        let state = reader_state();
        let mut clients = JoinSet::new();
        let (mut partial, server) = UnixStream::pair().unwrap();
        partial.write_all(b"{\"command\":").await.unwrap();
        receive_client(&mut clients, server, state.clone());
        let (mut complete, server) = UnixStream::pair().unwrap();
        receive_client(&mut clients, server, state);
        protocol::write_message(
            &mut complete,
            &Request {
                command: "close".into(),
                input: json!({}),
            },
        )
        .await
        .unwrap();
        let job = tokio::time::timeout(Duration::from_secs(30), clients.join_next())
            .await
            .expect("complete client must be independent of partial client")
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(job.request.command, "close");
        assert_eq!(clients.len(), 1);
        clients.shutdown().await;
        assert_eq!(partial.read(&mut [0_u8; 1]).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn pending_clients_are_bounded_and_cancelled_at_shutdown() {
        use tokio::io::AsyncReadExt;

        let state = reader_state();
        let mut clients = JoinSet::new();
        let mut peers = Vec::new();
        for _ in 0..MAX_PENDING_CLIENTS {
            let (peer, server) = UnixStream::pair().unwrap();
            peers.push(peer);
            receive_client(&mut clients, server, state.clone());
        }
        let (mut excess, server) = UnixStream::pair().unwrap();
        receive_client(&mut clients, server, state);
        assert_eq!(clients.len(), MAX_PENDING_CLIENTS);
        assert_eq!(excess.read(&mut [0_u8; 1]).await.unwrap(), 0);
        clients.shutdown().await;
        assert!(clients.is_empty());
        for mut peer in peers {
            assert_eq!(peer.read(&mut [0_u8; 1]).await.unwrap(), 0);
        }
    }

    #[tokio::test]
    async fn oversized_samples_use_complete_saved_artifact_and_retain_methodology() {
        let directory = tempfile::tempdir().unwrap();
        let saved_path = directory.path().join("samples.json");
        let raw = json!([{"run_index": 0, "trace": "x".repeat(usize::try_from(protocol::MAX_MESSAGE).unwrap())}]);
        let mut payload = json!({
            "outcome": "completed", "requested_runs": 1, "warmup": 2,
            "raw_samples": raw,
            "methodology_warnings": ["no throttle"],
        });
        std::fs::write(&saved_path, serde_json::to_vec(&payload).unwrap()).unwrap();
        payload["samples_path"] = json!(saved_path);
        let output =
            ToolOutput::success("run_scenario completed; full samples saved").with_display(payload);
        let response = adapt("named:test", Some("daemon"), &output, directory.path())
            .await
            .with_browser_lifetime(Some("browser".into()));
        protocol::encode_message(&response).expect("response fits the wire budget");
        let Outcome::Success { result } = response.outcome else {
            panic!("saved raw samples should be delivered successfully");
        };
        assert!(result["data"].get("raw_samples").is_none());
        assert_eq!(result["data"]["requested_runs"], 1);
        assert_eq!(result["data"]["warmup"], 2);
        assert_eq!(
            result["data"]["methodology_warnings"],
            json!(["no throttle"])
        );
        assert_eq!(result["artifacts"], json!([saved_path]));
        let saved: Value = serde_json::from_slice(&std::fs::read(saved_path).unwrap()).unwrap();
        assert_eq!(saved["raw_samples"], raw);
    }

    #[tokio::test]
    async fn saved_sample_mismatch_is_explicit_and_does_not_discard_samples() {
        let directory = tempfile::tempdir().unwrap();
        let saved_path = directory.path().join("samples.json");
        std::fs::write(&saved_path, br#"{"raw_samples":[]}"#).unwrap();
        let raw = json!([{"run_index": 0, "script_ms": 1}]);
        let output = ToolOutput::success("experiment completed").with_display(json!({
            "samples_path": saved_path, "raw_samples": raw,
            "methodology_warnings": ["no throttle"],
        }));
        let response = adapt("named:test", None, &output, directory.path()).await;
        let Outcome::Error { error } = response.outcome else {
            panic!("mismatched samples reported successful");
        };
        assert!(error.message.contains("do not match"));
        assert_eq!(
            error.artifacts,
            vec![saved_path.to_string_lossy().into_owned()]
        );
        assert_eq!(error.details.unwrap()["data"]["raw_samples"], raw);
    }

    #[tokio::test]
    async fn oversized_response_returns_explicit_failure_with_identity_and_artifacts() {
        for successful in [true, false] {
            let huge = "x".repeat(usize::try_from(protocol::MAX_MESSAGE).unwrap());
            let mut response = if successful {
                Response::success(
                    "named:test",
                    Some("daemon"),
                    json!({
                        "text": huge, "artifacts": ["/saved-result"],
                    }),
                )
            } else {
                let mut response = Response::error(
                    "named:test",
                    Some("daemon"),
                    ErrorCode::OperationFailed,
                    huge,
                );
                if let Outcome::Error { error } = &mut response.outcome {
                    error.artifacts.push("/saved-result".into());
                    error.details =
                        Some(json!({"exception":{"value":{"artifacts":["/user-data-only"]}}}));
                }
                response
            };
            response.browser_lifetime = Some("browser".into());
            let (mut client, mut server) = UnixStream::pair().unwrap();
            let ((), delivered) = tokio::join!(
                send_response(&mut server, &response),
                protocol::read_message::<Response>(&mut client)
            );
            let delivered = delivered.unwrap();
            assert_eq!(delivered.session, "named:test");
            assert_eq!(delivered.lifetime.as_deref(), Some("daemon"));
            assert_eq!(delivered.browser_lifetime.as_deref(), Some("browser"));
            let Outcome::Error { error } = delivered.outcome else {
                panic!("oversized result unexpectedly succeeded");
            };
            assert_eq!(error.code, ErrorCode::OperationFailed);
            assert!(error.message.contains("Do not replay"));
            assert_eq!(error.artifacts, ["/saved-result"]);
        }
    }

    #[tokio::test]
    async fn oversized_open_response_retains_nested_snapshot_artifact() {
        let response = Response::success(
            "named:test",
            Some("daemon"),
            json!({
                "url": "http://localhost/", "title": "Page",
                "console_errors": ["x".repeat(usize::try_from(protocol::MAX_MESSAGE).unwrap())],
                "snapshot": {
                    "status": "success",
                    "result": {"text": "snapshot", "artifacts": ["/saved-snapshot"]},
                },
            }),
        )
        .with_browser_lifetime(Some("browser".into()));
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let ((), delivered) = tokio::join!(
            send_response(&mut server, &response),
            protocol::read_message::<Response>(&mut client)
        );
        let delivered = delivered.unwrap();
        let Outcome::Error { error } = delivered.outcome else {
            panic!("oversized open response unexpectedly succeeded");
        };
        assert_eq!(error.code, ErrorCode::OperationFailed);
        assert_eq!(error.artifacts, ["/saved-snapshot"]);
        assert!(error.message.contains("Do not replay"));
    }

    fn image(bytes: &[u8]) -> ToolImage {
        ToolImage {
            media_type: "image/png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    #[tokio::test]
    async fn inline_images_become_private_artifacts_on_success_and_error() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = b"\x89PNG\r\n\x1a\nimage-payload";
        for output in [
            ToolOutput::success("image result"),
            ToolOutput::error("operation failed after capture"),
        ] {
            let successful = output.is_success();
            let output = output.with_images(vec![image(bytes)]);
            let response = adapt(
                "named:test",
                Some("daemon"),
                &output,
                &directory.path().join("images"),
            )
            .await;
            let paths = match response.outcome {
                Outcome::Success { result } => {
                    assert!(successful);
                    assert!(result.get("images").is_none());
                    serde_json::from_value::<Vec<String>>(result["artifacts"].clone()).unwrap()
                }
                Outcome::Error { error } => {
                    assert!(!successful);
                    assert_eq!(error.message, output.output());
                    error.artifacts
                }
            };
            assert_eq!(paths.len(), 1);
            assert_eq!(std::fs::read(&paths[0]).unwrap(), bytes);
            assert_eq!(std::fs::metadata(&paths[0]).unwrap().mode() & 0o777, 0o600);
        }
    }

    #[tokio::test]
    async fn image_metadata_is_reused_only_after_verifying_its_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let existing = directory.path().join("existing.png");
        let image_directory = directory.path().join("images");
        let bytes = b"image-bytes";
        for matches in [true, false] {
            std::fs::write(
                &existing,
                if matches {
                    bytes.as_slice()
                } else {
                    b"other-bytes"
                },
            )
            .unwrap();
            let output = ToolOutput::success("image")
                .with_display(json!({"artifact_path":existing}))
                .with_images(vec![image(bytes)]);
            let response = adapt("named:test", None, &output, &image_directory).await;
            let Outcome::Success { result } = response.outcome else {
                panic!("image conversion failed");
            };
            let paths: Vec<String> = serde_json::from_value(result["artifacts"].clone()).unwrap();
            assert_eq!(paths.len(), if matches { 1 } else { 2 });
            assert!(paths
                .iter()
                .any(|path| std::fs::read(path).unwrap() == bytes));
        }
    }

    #[tokio::test]
    async fn unpreservable_image_reports_failure_and_retains_other_tool_data() {
        let directory = tempfile::tempdir().unwrap();
        let output = ToolOutput::success("operation already executed")
            .with_display(json!({"artifact_path":"/saved-before-image","measurement":42}))
            .with_images(vec![ToolImage {
                media_type: "image/png".into(),
                data: "invalid base64!".into(),
            }]);
        let response = adapt("named:test", None, &output, directory.path()).await;
        let Outcome::Error { error } = response.outcome else {
            panic!("lost image was reported successful");
        };
        assert!(error.message.contains("could not be preserved"));
        assert_eq!(error.details.unwrap()["data"]["measurement"], 42);
        assert_eq!(error.artifacts, ["/saved-before-image"]);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    async fn request(socket: &Path, command: &str) -> Response {
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut stream = UnixStream::connect(socket).await.unwrap();
            protocol::write_message(
                &mut stream,
                &Request {
                    command: command.into(),
                    input: json!({}),
                },
            )
            .await
            .unwrap();
            protocol::read_message(&mut stream).await.unwrap()
        })
        .await
        .expect("daemon response")
    }

    #[tokio::test]
    async fn retained_retirement_failure_requires_explicit_cleanup_retry() {
        let directory = tempfile::Builder::new()
            .prefix("pb-retained-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = directory.path().join("session.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let state = reader_state();
        let task_socket = socket.clone();
        let task = tokio::spawn(async move {
            finish_shutdown(
                &listener,
                state,
                &task_socket,
                None,
                None,
                Some(RetainedCleanup {
                    error: "original retirement failure".into(),
                    browser_lifetime: Some("failed-browser".into()),
                }),
            )
            .await
        });
        for command in ["status", "open", "status"] {
            let response = request(&socket, command).await;
            assert_eq!(response.browser_lifetime.as_deref(), Some("failed-browser"));
            match response.outcome {
                Outcome::Success { result } => {
                    assert_eq!(command, "status");
                    assert_eq!(result["phase"], "stopping");
                    assert_eq!(result["cleanup_error"], "original retirement failure");
                }
                Outcome::Error { error } => {
                    assert_eq!(command, "open");
                    assert_eq!(error.code, ErrorCode::CleanupFailed);
                    assert!(error.message.contains("original retirement failure"));
                }
            }
            assert!(socket.exists());
            assert!(!task.is_finished());
        }
        let closed = request(&socket, "close").await;
        assert_eq!(closed.browser_lifetime.as_deref(), Some("failed-browser"));
        assert!(matches!(closed.outcome, Outcome::Success { result } if result["closed"] == true));
        task.await.unwrap().unwrap();
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn cleanup_failure_retains_owner_until_explicit_repair_and_close() {
        if matches!(
            std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
            Ok("1" | "true")
        ) {
            return;
        }
        let directory = tempfile::Builder::new()
            .prefix("pb-close-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = directory.path().join("session.sock");
        let lock_path = directory.path().join("owner");
        let lock = lock_file(&lock_path).unwrap();
        FileExt::try_lock_exclusive(&lock).unwrap();
        let probe = lock_file(&lock_path).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let manager = BrowserSessionManager::new();
        manager.set_scope_liveness_hook(Arc::new(|_, _| Box::pin(async { true })));
        let state = Arc::new(State {
            manager: manager.clone(),
            scope: ResourceScopeKey::Unattached(format!("cleanup-test:{}", uuid::Uuid::new_v4())),
            session: "named:cleanup-test".into(),
            lifetime: uuid::Uuid::new_v4().to_string(),
            idle: Duration::from_secs(300),
            image_directory: directory.path().join("images"),
        });
        let browser = manager.get_session(&state.scope).await.unwrap();
        let browser_lifetime = browser.read().await.launch_identity().launch_uuid.clone();
        let profile = std::path::PathBuf::from(
            &manager.cleanup_identifiers(&state.scope).await[0].profile_path,
        );
        let marker = profile.join("phoenix-profile-owner.json");
        let original = std::fs::read(&marker).unwrap();
        std::fs::write(&marker, b"broken-owner-marker").unwrap();
        let task_socket = socket.clone();
        let task = tokio::spawn(async move {
            let _owner = Owner {
                _lock: lock,
                socket: task_socket.clone(),
            };
            serve(&listener, state, &task_socket, std::future::pending()).await
        });
        let failed = request(&socket, "close").await;
        assert_eq!(
            failed.browser_lifetime.as_deref(),
            Some(browser_lifetime.as_str())
        );
        assert!(
            matches!(failed.outcome, Outcome::Error { error } if error.code == ErrorCode::CleanupFailed)
        );
        assert!(profile.exists());
        assert!(!task.is_finished());
        assert_eq!(
            FileExt::try_lock_exclusive(&probe).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let status = request(&socket, "status").await;
        assert!(
            matches!(status.outcome, Outcome::Success { result } if result["phase"] == "stopping")
        );
        assert!(
            matches!(request(&socket, "open").await.outcome, Outcome::Error { error } if error.code == ErrorCode::CleanupFailed)
        );
        std::fs::write(&marker, original).unwrap();
        let closed = request(&socket, "close").await;
        assert_eq!(
            closed.browser_lifetime.as_deref(),
            Some(browser_lifetime.as_str())
        );
        assert!(matches!(closed.outcome, Outcome::Success { result } if result["closed"] == true));
        task.await.unwrap().unwrap();
        assert!(!profile.exists());
        assert!(!socket.exists());
        FileExt::try_lock_exclusive(&probe).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn navigation_observation_preserves_error_absence_and_timeout() {
        assert_eq!(
            navigation_observation(async { Ok::<_, String>(Some("https://example.test/".into())) })
                .await
                .unwrap(),
            "https://example.test/"
        );
        assert!(
            navigation_observation(async { Err::<Option<String>, _>("CDP disconnected") })
                .await
                .unwrap_err()
                .contains("CDP disconnected")
        );
        assert!(navigation_observation(async { Ok::<_, String>(None) })
            .await
            .unwrap_err()
            .contains("no value"));
        assert!(
            navigation_observation(std::future::pending::<Result<Option<String>, String>>())
                .await
                .unwrap_err()
                .contains("timed out")
        );
        let mut errors = serde_json::Map::new();
        assert_eq!(
            observed_value("url", Err("disconnected".into()), &mut errors),
            None
        );
        assert_eq!(errors["url"], "disconnected");
    }
}
