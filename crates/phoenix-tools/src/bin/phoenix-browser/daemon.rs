use super::cli::parse_idle;
use super::protocol::{self, Outcome, Request, Response, SessionPaths};
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
use tokio::net::UnixListener;
use tokio::sync::RwLock;

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
        Response::error(session, lifetime, "operation_failed", output.output())
    }
}

pub async fn offline(session: &str, request: &Request) -> Response {
    if request.command == "schema" {
        let selected = request.input["command"].as_str();
        let entries: Vec<_> = BrowserOperation::ALL.iter().copied().filter(|op| selected.is_none() || selected.and_then(operation) == Some(*op) || selected == Some(op.name())).map(|op| json!({"name":op.name(),"description":op.description(),"input_schema":op.input_schema()})).collect();
        if entries.is_empty() {
            return Response::error(session, None, "invalid_input", "unknown schema command");
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

impl State {
    async fn execute(&self, request: &Request) -> Response {
        if request.command == "status" {
            let browser = self.manager.get_existing(&self.scope).await;
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
            return Response::success(
                &self.session,
                Some(&self.lifetime),
                json!({"active":browser_lifetime.is_some(),"browser_lifetime":browser_lifetime,"idle_timeout_seconds":self.idle.as_secs(),"daemon_pid":std::process::id(),"chrome_pid":identity.as_ref().and_then(|id|id.chrome_pid),"profile_path":identity.map(|id|id.profile_path)}),
            );
        }
        let Some(operation) = operation(&request.command) else {
            return Response::error(
                &self.session,
                Some(&self.lifetime),
                "invalid_input",
                "unknown command",
            );
        };
        let browser = if request.command == "open" {
            match self.manager.get_session(&self.scope).await {
                Ok(browser) => browser,
                Err(error) => {
                    return Response::error(
                        &self.session,
                        Some(&self.lifetime),
                        "browser_start_failed",
                        error.to_string(),
                    )
                }
            }
        } else {
            match self.manager.get_existing(&self.scope).await {
                Some(browser) => browser,
                None => {
                    return Response::error(
                        &self.session,
                        Some(&self.lifetime),
                        "state_lost",
                        "browser state is gone; use open URL to start fresh",
                    )
                }
            }
        };
        let browser_lifetime = browser.read().await.launch_identity().launch_uuid.clone();
        let access = Existing(browser.clone());
        if request.command == "open" {
            browser
                .read()
                .await
                .console_logs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
        let output = operation.execute(request.input.clone(), &access).await;
        if request.command != "open" || !output.is_success() {
            let mut response = adapt(&self.session, Some(&self.lifetime), &output);
            if let Outcome::Success { result } = &mut response.outcome {
                result["browser_lifetime"] = json!(browser_lifetime);
            }
            return response;
        }
        self.open_result(&access, &browser, &browser_lifetime).await
    }

    async fn open_result(
        &self,
        access: &Existing,
        browser: &Arc<RwLock<BrowserSession>>,
        browser_lifetime: &str,
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
        let errors: Vec<_> = browser
            .read()
            .await
            .console_logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|entry| {
                matches!(
                    entry.level,
                    phoenix_browser::ConsoleLevel::Error | phoenix_browser::ConsoleLevel::Assert
                )
            })
            .map(|entry| json!({"level":entry.level,"text":entry.text}))
            .collect();
        Response::success(
            &self.session,
            Some(&self.lifetime),
            json!({"url":url,"title":title,"navigation":"succeeded","browser_lifetime":browser_lifetime,"snapshot":snapshot.outcome,"console_errors":errors}),
        )
    }
}

async fn wait_signal() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! { _ = terminate.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
}

async fn serve(listener: &UnixListener, state: &State, socket: &Path) -> Result<(), String> {
    loop {
        let accepted = tokio::time::timeout(state.idle, listener.accept()).await;
        let (mut stream, _) = match accepted {
            Err(_) => return Ok(()),
            Ok(Err(error)) => return Err(format!("accept: {error}")),
            Ok(Ok(stream)) => stream,
        };
        let request: Request = match tokio::time::timeout(
            Duration::from_secs(10),
            protocol::read_message(&mut stream),
        )
        .await
        {
            Ok(Ok(request)) => request,
            Ok(Err(error)) => {
                let _ = protocol::write_message(
                    &mut stream,
                    &Response::error(
                        &state.session,
                        Some(&state.lifetime),
                        "invalid_input",
                        error,
                    ),
                )
                .await;
                continue;
            }
            Err(_) => continue,
        };
        if request.command == "close" {
            let response = match state.manager.shutdown_all().await {
                Ok(()) => Response::success(
                    &state.session,
                    Some(&state.lifetime),
                    json!({"active":false,"closed":true}),
                ),
                Err(error) => Response::error(
                    &state.session,
                    Some(&state.lifetime),
                    "cleanup_failed",
                    error.to_string(),
                ),
            };
            if response.is_success() {
                std::fs::remove_file(socket).map_err(|e| format!("retire endpoint: {e}"))?;
            }
            let _ = protocol::write_message(&mut stream, &response).await;
            return if response.is_success() {
                Ok(())
            } else {
                Err("browser cleanup failed; inspect daemon log".into())
            };
        }
        let response = state.execute(&request).await;
        // A disconnected client does not cancel or replay the completed operation.
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            protocol::write_message(&mut stream, &response),
        )
        .await;
    }
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
    let state = State {
        manager: manager.clone(),
        scope: ResourceScopeKey::Unattached(format!("cli:{lifetime}")),
        session: args[1].clone(),
        lifetime,
        idle: parse_idle(&args[2])?,
    };
    let result = tokio::select! { result = serve(&listener,&state,&socket) => result, () = wait_signal() => Ok(()) };
    let cleanup = manager
        .shutdown_all()
        .await
        .map_err(|e| format!("browser cleanup failed: {e}"));
    result.and(cleanup)
}
