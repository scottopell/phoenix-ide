use super::cli::parse_idle;
use super::protocol::{self, ErrorCode, Outcome, Request, Response, SessionPaths};
use async_trait::async_trait;
use fs2::FileExt;
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use phoenix_core::work_scope::ResourceScopeKey;
use phoenix_tools::browser::{
    BrowserAccess, BrowserOperation, BrowserSession, BrowserSessionManager,
};
use phoenix_tools::ToolOutput;
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;
use tokio::sync::{mpsc, watch};

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

fn take_artifacts(value: &mut Value, paths: &mut Vec<Value>) {
    match value {
        Value::Object(object) => {
            let keys: Vec<_> = object.keys().cloned().collect();
            for key in keys {
                if matches!(
                    key.as_str(),
                    "path" | "artifact_path" | "samples_path" | "post"
                ) && object[&key].is_string()
                {
                    paths.push(object.remove(&key).expect("artifact key exists"));
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

fn adapt(session: &str, lifetime: Option<&str>, output: &ToolOutput) -> Response {
    let mut artifacts = Vec::new();
    let mut data = output.display_data().cloned();
    if let Some(data) = &mut data {
        take_artifacts(data, &mut artifacts);
    }
    // The CLI is a file-artifact sink; images already have a saved screenshot path.
    if !output.images().is_empty() {
        tracing::debug!(
            images = output.images().len(),
            "CLI returns saved image artifacts rather than inline image blocks"
        );
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
    adapt(
        session,
        None,
        &BrowserOperation::Profile
            .execute(request.input.clone(), &Unavailable)
            .await,
    )
}

struct State {
    manager: Arc<BrowserSessionManager>,
    scope: ResourceScopeKey,
    session: String,
    lifetime: String,
    idle: Duration,
}

struct ReadyOperation {
    browser: Arc<RwLock<BrowserSession>>,
    operation: BrowserOperation,
    browser_lifetime: String,
}

enum Admission {
    Reply(Response),
    Ready(ReadyOperation),
}

impl State {
    async fn live_browser(&self) -> Result<Option<Arc<RwLock<BrowserSession>>>, String> {
        let Some(browser) = self.manager.get_existing(&self.scope).await else {
            return Ok(None);
        };
        let running = browser
            .write()
            .await
            .is_running()
            .map_err(|e| e.to_string())?;
        Ok(running.then_some(browser))
    }

    async fn admit(&self, request: &Request) -> Admission {
        let existing = match self.live_browser().await {
            Ok(browser) => browser,
            Err(error) => {
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
                    return Admission::Reply(Response::error(
                        &self.session,
                        Some(&self.lifetime),
                        ErrorCode::CleanupFailed,
                        error.to_string(),
                    ));
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
            return adapt(&self.session, Some(&self.lifetime), &output)
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
        );
        let (url, title) = {
            let guard = browser.read().await;
            let url = tokio::time::timeout(Duration::from_secs(5), guard.page.url())
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
            let title = tokio::time::timeout(Duration::from_secs(5), guard.page.get_title())
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
            (url, title)
        };
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
            json!({"url":url,"title":title,"navigation":"succeeded","snapshot":snapshot.outcome,"console_errors":errors,"console_capture_error":console_capture_error}),
        ).with_browser_lifetime(Some(browser_lifetime.into()))
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

async fn run_job(state: &State, job: &mut Job, shutdown: &mut watch::Receiver<bool>) -> Response {
    if *shutdown.borrow() {
        return cancelled(state, None);
    }
    // Startup retains ownership until the manager can authoritatively tear down its session.
    let admission = state.admit(&job.request).await;
    let ready = match admission {
        Admission::Reply(response) => return response,
        Admission::Ready(ready) => ready,
    };
    if *shutdown.borrow() {
        return cancelled(state, Some(ready.browser_lifetime));
    }
    tokio::select! {
        response = state.execute(&job.request, &ready) => response,
        _ = shutdown.changed() => cancelled(state, Some(ready.browser_lifetime.clone())),
    }
}

async fn worker(
    state: Arc<State>,
    mut jobs: mpsc::Receiver<Job>,
    mut shutdown: watch::Receiver<bool>,
    completed: mpsc::UnboundedSender<()>,
) {
    while let Some(mut job) = jobs.recv().await {
        let response = run_job(&state, &mut job, &mut shutdown).await;
        send_response(&mut job.stream, &response).await;
        let _ = completed.send(());
    }
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
    let mut close_stream = None;
    tokio::pin!(signal);
    let result = loop {
        tokio::select! {
            () = &mut signal => break Ok(()),
            () = tokio::time::sleep_until(deadline), if pending == 0 => break Ok(()),
            completion = completed_rx.recv(), if pending > 0 => {
                if completion.is_none() { break Err("browser worker stopped unexpectedly".into()); }
                pending -= 1;
                deadline = tokio::time::Instant::now() + state.idle;
            }
            accepted = listener.accept() => {
                let (stream, _) = match accepted { Ok(accepted) => accepted, Err(error) => break Err(format!("accept: {error}")) };
                let Some(mut job) = read_job(stream, &state).await else { continue; };
                if job.request.command == "close" { close_stream = Some(job.stream); break Ok(()); }
                match jobs_tx.try_send(job) {
                    Ok(()) => pending += 1,
                    Err(mpsc::error::TrySendError::Full(returned)) => {
                        job = returned;
                        send_response(&mut job.stream, &Response::error(&state.session, Some(&state.lifetime), ErrorCode::Busy, "Session request queue is full; this request was not executed.")).await;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break Err("browser worker stopped unexpectedly".into()),
                }
            }
        }
    };
    let _ = shutdown_tx.send(true);
    drop(jobs_tx);
    let worker_result = task
        .await
        .map_err(|e| format!("browser worker failed: {e}"));
    let cleanup = state
        .manager
        .shutdown_all()
        .await
        .map_err(|e| e.to_string());
    if cleanup.is_ok() {
        std::fs::remove_file(socket).map_err(|e| format!("retire endpoint: {e}"))?;
    }
    if let Some(mut stream) = close_stream {
        let response = match &cleanup {
            Ok(()) => Response::success(
                &state.session,
                Some(&state.lifetime),
                json!({"active":false,"closed":true}),
            ),
            Err(error) => Response::error(
                &state.session,
                Some(&state.lifetime),
                ErrorCode::CleanupFailed,
                error,
            ),
        };
        send_response(&mut stream, &response).await;
    }
    result.and(worker_result).and(cleanup)
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
    let manager = BrowserSessionManager::with_lifecycle_sink_and_runtime_env(
        None,
        &PhoenixRuntimeEnvironment::detect(),
    );
    manager.set_scope_liveness_hook(Arc::new(|_, _| Box::pin(async { true })));
    let lifetime = uuid::Uuid::new_v4().to_string();
    let state = Arc::new(State {
        manager: manager.clone(),
        scope: ResourceScopeKey::Unattached(format!("cli:{lifetime}")),
        session: args[1].clone(),
        lifetime,
        idle: parse_idle(&args[2])?,
    });
    serve(&listener, state, &socket, signal).await
}
