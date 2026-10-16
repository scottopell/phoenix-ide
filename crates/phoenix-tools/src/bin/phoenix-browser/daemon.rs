use super::cli::parse_idle;
use super::protocol::{self, ErrorCode, Outcome, Request, Response, SessionPaths};
use async_trait::async_trait;
use base64::Engine;
use fs2::FileExt;
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use phoenix_core::work_scope::ResourceScopeKey;
use phoenix_tools::browser::{
    BrowserAccess, BrowserOperation, BrowserSession, BrowserSessionManager,
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

struct Existing(Arc<RwLock<BrowserSession>>);
#[async_trait]
impl BrowserAccess for Existing {
    async fn browser(&self) -> Result<Arc<RwLock<BrowserSession>>, String> {
        Ok(self.0.clone())
    }
}
struct Unavailable;
#[async_trait]
impl BrowserAccess for Unavailable {
    async fn browser(&self) -> Result<Arc<RwLock<BrowserSession>>, String> {
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

async fn adapt(
    session: &str,
    lifetime: Option<&str>,
    output: &ToolOutput,
    image_directory: &Path,
) -> Response {
    let mut artifacts = Vec::new();
    let mut data = output.display_data().cloned();
    if let Some(data) = &mut data {
        take_artifacts(data, &mut artifacts);
    }
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
        let Some(browser) = self.manager.get_existing(&self.scope).await else {
            return Ok(None);
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
        let access = Existing(browser.clone());
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
        let snapshot = adapt(
            &self.session,
            Some(&self.lifetime),
            &BrowserOperation::Snapshot
                .execute(json!({"interactive":true}), access)
                .await,
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
        Response::success(
            &self.session,
            Some(&self.lifetime),
            json!({"url":url,"title":title,"observation_errors":observation_errors,"navigation":"succeeded","snapshot":snapshot.outcome,"console_errors":errors,"console_capture_error":console_capture_error}),
        ).with_browser_lifetime(Some(browser_lifetime.into()))
    }
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

async fn send_response(stream: &mut UnixStream, response: &Response) {
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        protocol::write_message(stream, response),
    )
    .await;
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
    let manager = BrowserSessionManager::with_lifecycle_sink_and_runtime_env(None, &runtime_env);
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
