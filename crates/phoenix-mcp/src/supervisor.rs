use crate::{
    CallContext, McpRequestError, McpServer, McpServerConfig, McpToolDef, OAuthRecoveryKind,
};
use serde_json::Value;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

const MAILBOX_CAPACITY: usize = 64;

#[derive(Clone)]
pub(crate) enum SupervisorState {
    Ready(Arc<McpServer>),
    Connecting,
    Recovering,
    Failed,
    Removed,
}

#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub(crate) epoch: u64,
    pub(crate) state: SupervisorState,
    pub(crate) config: McpServerConfig,
    pub(crate) last_error: Option<String>,
    pub(crate) pending_oauth_url: Option<String>,
}

impl std::fmt::Debug for SupervisorState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready(_) => formatter.write_str("Ready(..)"),
            Self::Connecting => formatter.write_str("Connecting"),
            Self::Recovering => formatter.write_str("Recovering"),
            Self::Failed => formatter.write_str("Failed"),
            Self::Removed => formatter.write_str("Removed"),
        }
    }
}

impl Snapshot {
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self.state, SupervisorState::Ready(_))
    }
}

#[derive(Debug)]
pub(crate) enum CallRecovery {
    None,
    Transport,
    OAuth(OAuthRecoveryKind),
    CancelledTransport,
}

pub(crate) struct CallOutcome {
    pub(crate) epoch: u64,
    pub(crate) result: Result<String, McpRequestError>,
    pub(crate) recovery: CallRecovery,
}

pub(crate) struct DefinitionsOutcome {
    pub(crate) epoch: u64,
    pub(crate) result: Result<Vec<McpToolDef>, McpRequestError>,
    pub(crate) recoverable: bool,
}

#[derive(Clone)]
pub(crate) struct SupervisorHandle {
    mailbox: mpsc::Sender<Command>,
    snapshots: watch::Receiver<Snapshot>,
}

impl SupervisorHandle {
    pub(crate) fn same_actor(&self, other: &Self) -> bool {
        self.mailbox.same_channel(&other.mailbox)
    }

    pub(crate) fn connecting(config: McpServerConfig) -> Self {
        Self::spawn(config, SupervisorState::Connecting)
    }

    #[cfg(test)]
    pub(crate) fn connected(server: McpServer) -> Self {
        let config = server.config();
        Self::spawn(config, SupervisorState::Ready(Arc::new(server)))
    }

    fn spawn(config: McpServerConfig, state: SupervisorState) -> Self {
        let initial = Snapshot {
            epoch: 0,
            state: state.clone(),
            config,
            last_error: None,
            pending_oauth_url: None,
        };
        let (mailbox, commands) = mpsc::channel(MAILBOX_CAPACITY);
        let (snapshots, snapshot) = watch::channel(initial.clone());
        tokio::spawn(
            Actor {
                mailbox: mailbox.downgrade(),
                commands,
                snapshots,
                snapshot: initial,
                state,
                epoch: 0,
                recovery_from: None,
                next_call_id: 1,
                stdio_active: false,
                stdio_queue: VecDeque::new(),
                active_calls: HashMap::new(),
            }
            .run(),
        );
        Self {
            mailbox,
            snapshots: snapshot,
        }
    }

    /// Read the current snapshot without going through the mailbox.
    pub(crate) fn snapshot(&self) -> Snapshot {
        self.snapshots.borrow().clone()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.snapshots.clone()
    }

    pub(crate) async fn wait_for_settled(&self) {
        let mut snapshots = self.subscribe();
        loop {
            let snapshot = snapshots.borrow().clone();
            if snapshot.pending_oauth_url.is_some()
                || !matches!(
                    snapshot.state,
                    SupervisorState::Connecting | SupervisorState::Recovering
                )
            {
                return;
            }
            if snapshots.changed().await.is_err() {
                return;
            }
        }
    }

    pub(crate) async fn status(&self) -> Option<Snapshot> {
        let (reply, receive) = oneshot::channel();
        self.mailbox.send(Command::Status { reply }).await.ok()?;
        receive.await.ok()
    }

    /// Advance the epoch and set the actor to Connecting, terminating
    /// any existing server.  Returns the new epoch for the caller to
    /// use when publishing or failing the connect attempt.
    pub(crate) async fn reconfigure(&self, config: McpServerConfig) -> Result<u64, String> {
        let (reply, receive) = oneshot::channel();
        self.mailbox
            .send(Command::Reconfigure { config, reply })
            .await
            .map_err(|_| stopped())?;
        receive.await.map_err(|_| stopped())
    }

    pub(crate) async fn call(
        &self,
        tool: String,
        arguments: Value,
        cancel: CancellationToken,
    ) -> Result<CallOutcome, String> {
        let (reply, receive) = oneshot::channel();
        let deadline = tokio::time::Instant::now()
            .checked_add(self.snapshot().config.tool_call_timeout())
            .ok_or_else(|| "MCP tool call timeout exceeds platform deadline range".to_string())?;
        let call_cancel = cancel.child_token();
        let command = Command::Call {
            tool,
            arguments,
            cancel: call_cancel.clone(),
            reply,
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Err("MCP tool call cancelled".to_string()),
            () = tokio::time::sleep_until(deadline) => {
                return Err("MCP tool call timed out while enqueueing".to_string());
            }
            result = self.mailbox.send(command) => result.map_err(|_| stopped())?,
        }
        tokio::select! {
            biased;
            result = receive => result.unwrap_or_else(|_| Err(stopped())),
            () = tokio::time::sleep_until(deadline) => {
                call_cancel.cancel();
                Err("MCP tool call timed out".to_string())
            }
        }
    }

    pub(crate) async fn inspect(&self) -> Result<DefinitionsOutcome, String> {
        let (reply, receive) = oneshot::channel();
        self.mailbox
            .send(Command::Inspect { reply })
            .await
            .map_err(|_| stopped())?;
        receive.await.unwrap_or_else(|_| Err(stopped()))
    }

    /// Attempt to claim recovery leadership for `observed_epoch`.
    pub(crate) async fn claim_recovery(&self, observed_epoch: u64) -> RecoveryClaim {
        let (reply, receive) = oneshot::channel();
        if self
            .mailbox
            .send(Command::Claim {
                observed_epoch,
                reply,
            })
            .await
            .is_err()
        {
            return RecoveryClaim::Unavailable(stopped());
        }
        receive
            .await
            .unwrap_or_else(|_| RecoveryClaim::Unavailable(stopped()))
    }

    /// Publish a freshly connected server for `epoch`.  Returns false
    /// (and terminates the server) if the epoch is stale.
    pub(crate) async fn publish(&self, epoch: u64, server: McpServer) -> bool {
        let (reply, receive) = oneshot::channel();
        if let Err(error) = self
            .mailbox
            .send(Command::Publish {
                epoch,
                server,
                reply,
            })
            .await
        {
            if let Command::Publish { server, .. } = error.0 {
                server.terminate().await;
            }
            return false;
        }
        receive.await.unwrap_or(false)
    }

    pub(crate) async fn fail(&self, epoch: u64, error: String) -> bool {
        let (reply, receive) = oneshot::channel();
        if self
            .mailbox
            .send(Command::Fail {
                epoch,
                error,
                reply,
            })
            .await
            .is_err()
        {
            return false;
        }
        receive.await.unwrap_or(false)
    }

    pub(crate) async fn unauthorized(&self, epoch: u64, url: String, error: String) -> bool {
        let (reply, receive) = oneshot::channel();
        if self
            .mailbox
            .send(Command::Unauthorized {
                epoch,
                url,
                error,
                reply,
            })
            .await
            .is_err()
        {
            return false;
        }
        receive.await.unwrap_or(false)
    }

    pub(crate) async fn remove(&self) {
        let (reply, receive) = oneshot::channel();
        if self.mailbox.send(Command::Remove { reply }).await.is_ok() {
            let _ = receive.await;
        }
    }

    pub(crate) async fn shutdown(&self) {
        let (reply, receive) = oneshot::channel();
        if self.mailbox.send(Command::Shutdown { reply }).await.is_ok() {
            let _ = receive.await;
        }
    }
}

fn stopped() -> String {
    "MCP supervisor stopped".to_string()
}

#[derive(Debug, Clone)]
pub(crate) struct RecoveryPermit {
    pub(crate) epoch: u64,
    pub(crate) config: McpServerConfig,
}

pub(crate) enum RecoveryClaim {
    Leader(RecoveryPermit),
    Follow(watch::Receiver<Snapshot>),
    Stale,
    Unavailable(String),
}

struct QueuedCall {
    epoch: u64,
    context: CallContext,
    call_id: u64,
    tool: String,
    arguments: Value,
    cancel: CancellationToken,
    reply: oneshot::Sender<Result<CallOutcome, String>>,
    cancellation_watch: tokio::task::JoinHandle<()>,
}

enum Command {
    Status {
        reply: oneshot::Sender<Snapshot>,
    },
    Reconfigure {
        config: McpServerConfig,
        reply: oneshot::Sender<u64>,
    },
    Call {
        tool: String,
        arguments: Value,
        cancel: CancellationToken,
        reply: oneshot::Sender<Result<CallOutcome, String>>,
    },
    CallCompleted {
        call_id: u64,
        epoch: u64,
        context: CallContext,
        result: Result<String, McpRequestError>,
        reply: oneshot::Sender<Result<CallOutcome, String>>,
    },
    CancelQueued {
        call_id: u64,
    },
    Inspect {
        reply: oneshot::Sender<Result<DefinitionsOutcome, String>>,
    },
    InspectCompleted {
        epoch: u64,
        context: CallContext,
        result: Result<Vec<McpToolDef>, McpRequestError>,
        reply: oneshot::Sender<Result<DefinitionsOutcome, String>>,
    },
    Claim {
        observed_epoch: u64,
        reply: oneshot::Sender<RecoveryClaim>,
    },
    Publish {
        epoch: u64,
        server: McpServer,
        reply: oneshot::Sender<bool>,
    },
    Fail {
        epoch: u64,
        error: String,
        reply: oneshot::Sender<bool>,
    },
    Unauthorized {
        epoch: u64,
        url: String,
        error: String,
        reply: oneshot::Sender<bool>,
    },
    Remove {
        reply: oneshot::Sender<()>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

struct Actor {
    mailbox: mpsc::WeakSender<Command>,
    commands: mpsc::Receiver<Command>,
    snapshots: watch::Sender<Snapshot>,
    snapshot: Snapshot,
    state: SupervisorState,
    epoch: u64,
    recovery_from: Option<u64>,
    next_call_id: u64,
    stdio_active: bool,
    stdio_queue: VecDeque<QueuedCall>,
    active_calls: HashMap<u64, CancellationToken>,
}

impl Actor {
    async fn run(mut self) {
        while let Some(command) = self.commands.recv().await {
            let stop = matches!(command, Command::Remove { .. } | Command::Shutdown { .. });
            self.handle(command).await;
            if stop {
                return;
            }
        }
        self.stop_server().await;
    }

    #[allow(clippy::too_many_lines)]
    async fn handle(&mut self, command: Command) {
        match command {
            Command::Status { reply } => {
                let _ = reply.send(self.snapshot.clone());
            }
            Command::Reconfigure { config, reply } => {
                for cancellation in self.active_calls.values() {
                    cancellation.cancel();
                }
                self.active_calls.clear();
                self.epoch = self.epoch.wrapping_add(1);
                self.stop_server().await;
                self.recovery_from = None;
                self.snapshot.config = config;
                self.state = SupervisorState::Connecting;
                self.publish_snapshot(None, None);
                let _ = reply.send(self.epoch);
            }
            Command::Call {
                tool,
                arguments,
                cancel,
                reply,
            } => {
                let SupervisorState::Ready(server) = &self.state else {
                    let _ = reply.send(Err(self.not_ready_message()));
                    return;
                };
                let context = server.call_context();
                let epoch = self.epoch;
                let call_id = self.next_call_id;
                self.next_call_id = self.next_call_id.wrapping_add(1);
                self.active_calls.insert(call_id, cancel.clone());
                if context.is_http() {
                    let Some(mailbox) = self.mailbox.upgrade() else {
                        let _ = reply.send(Err(stopped()));
                        return;
                    };
                    tokio::spawn(async move {
                        let result = context.call_tool(&tool, arguments, &cancel).await;
                        if let Err(error) = mailbox
                            .send(Command::CallCompleted {
                                call_id,
                                epoch,
                                context,
                                result,
                                reply,
                            })
                            .await
                        {
                            if let Command::CallCompleted { reply, .. } = error.0 {
                                let _ = reply.send(Err(stopped()));
                            }
                        }
                    });
                } else {
                    let cancellation = cancel.clone();
                    let Some(mailbox) = self.mailbox.upgrade() else {
                        let _ = reply.send(Err(stopped()));
                        return;
                    };
                    let cancellation_watch = tokio::spawn(async move {
                        cancellation.cancelled().await;
                        let _ = mailbox.send(Command::CancelQueued { call_id }).await;
                    });
                    self.stdio_queue.push_back(QueuedCall {
                        epoch,
                        context,
                        call_id,
                        tool,
                        arguments,
                        cancel,
                        reply,
                        cancellation_watch,
                    });
                    self.start_next_stdio_call();
                }
            }
            Command::CallCompleted {
                call_id,
                epoch,
                context,
                result,
                reply,
            } => {
                self.active_calls.remove(&call_id);
                let is_stdio = !context.is_http();
                let _ = reply.send(Ok(context.outcome(epoch, result)));
                if is_stdio {
                    self.stdio_active = false;
                    self.start_next_stdio_call();
                }
            }
            Command::CancelQueued { call_id } => {
                if let Some(position) = self
                    .stdio_queue
                    .iter()
                    .position(|queued| queued.call_id == call_id)
                {
                    let queued = self
                        .stdio_queue
                        .remove(position)
                        .expect("queued call exists");
                    queued.cancellation_watch.abort();
                    self.active_calls.remove(&call_id);
                    let _ = queued.reply.send(Ok(CallOutcome {
                        result: Err(McpRequestError::Cancelled),
                        epoch: queued.epoch,
                        recovery: CallRecovery::None,
                    }));
                }
            }
            Command::Inspect { reply } => {
                let SupervisorState::Ready(server) = &self.state else {
                    let _ = reply.send(Err(self.not_ready_message()));
                    return;
                };
                let epoch = self.epoch;
                let stale = server
                    .tools_changed
                    .swap(false, std::sync::atomic::Ordering::AcqRel);
                if !stale {
                    let _ = reply.send(Ok(DefinitionsOutcome {
                        epoch,
                        result: Ok(server.tools()),
                        recoverable: false,
                    }));
                    return;
                }
                let context = server.call_context();
                let server = Arc::clone(server);
                let Some(mailbox) = self.mailbox.upgrade() else {
                    let _ = reply.send(Err(stopped()));
                    return;
                };
                tokio::spawn(async move {
                    let result = server.list_tools().await;
                    if result.is_err() {
                        server
                            .tools_changed
                            .store(true, std::sync::atomic::Ordering::Release);
                    }
                    let _ = mailbox
                        .send(Command::InspectCompleted {
                            epoch,
                            context,
                            result,
                            reply,
                        })
                        .await;
                });
            }
            Command::InspectCompleted {
                epoch,
                context,
                result,
                reply,
            } => {
                let recoverable = result
                    .as_ref()
                    .err()
                    .is_some_and(|error| context.should_reestablish(error));
                let _ = reply.send(Ok(DefinitionsOutcome {
                    epoch,
                    result,
                    recoverable,
                }));
            }
            Command::Claim {
                observed_epoch,
                reply,
            } => {
                if matches!(self.state, SupervisorState::Recovering)
                    && self.recovery_from == Some(observed_epoch)
                {
                    let _ = reply.send(RecoveryClaim::Follow(self.snapshots.subscribe()));
                } else if observed_epoch != self.epoch {
                    let _ = reply.send(RecoveryClaim::Stale);
                } else if matches!(self.state, SupervisorState::Ready(_)) {
                    self.recovery_from = Some(observed_epoch);
                    self.epoch = self.epoch.wrapping_add(1);
                    self.stop_server().await;
                    self.state = SupervisorState::Recovering;
                    self.publish_snapshot(None, None);
                    let _ = reply.send(RecoveryClaim::Leader(RecoveryPermit {
                        epoch: self.epoch,
                        config: self.snapshot.config.clone(),
                    }));
                } else {
                    let _ = reply.send(RecoveryClaim::Unavailable(self.not_ready_message()));
                }
            }
            Command::Publish {
                epoch,
                server,
                reply,
            } => {
                if epoch == self.epoch
                    && matches!(
                        self.state,
                        SupervisorState::Connecting
                            | SupervisorState::Recovering
                            | SupervisorState::Failed
                    )
                {
                    self.recovery_from = None;
                    self.state = SupervisorState::Ready(Arc::new(server));
                    self.publish_snapshot(None, None);
                    let _ = reply.send(true);
                } else {
                    server.terminate().await;
                    let _ = reply.send(false);
                }
            }
            Command::Fail {
                epoch,
                error,
                reply,
            } => {
                let current = epoch == self.epoch;
                if current {
                    self.recovery_from = None;
                    self.stop_server().await;
                    self.state = SupervisorState::Failed;
                    self.publish_snapshot(Some(error), None);
                }
                let _ = reply.send(current);
            }
            Command::Unauthorized {
                epoch,
                url,
                error,
                reply,
            } => {
                let current = epoch == self.epoch;
                if current {
                    self.recovery_from = None;
                    self.stop_server().await;
                    self.state = SupervisorState::Recovering;
                    self.publish_snapshot(Some(error), Some(url));
                }
                let _ = reply.send(current);
            }
            Command::Remove { reply } | Command::Shutdown { reply } => {
                for cancellation in self.active_calls.values() {
                    cancellation.cancel();
                }
                self.active_calls.clear();
                self.epoch = self.epoch.wrapping_add(1);
                self.stop_server().await;
                self.recovery_from = None;
                self.state = SupervisorState::Removed;
                self.publish_snapshot(None, None);
                let _ = reply.send(());
            }
        }
    }

    fn start_next_stdio_call(&mut self) {
        if self.stdio_active {
            return;
        }
        while let Some(call) = self.stdio_queue.pop_front() {
            if call.cancel.is_cancelled() {
                self.active_calls.remove(&call.call_id);
                let _ = call.reply.send(Ok(CallOutcome {
                    result: Err(McpRequestError::Cancelled),
                    epoch: call.epoch,
                    recovery: CallRecovery::None,
                }));
                continue;
            }
            call.cancellation_watch.abort();
            self.stdio_active = true;
            let Some(mailbox) = self.mailbox.upgrade() else {
                let _ = call.reply.send(Err(stopped()));
                self.stdio_active = false;
                continue;
            };
            tokio::spawn(async move {
                let result = call
                    .context
                    .call_tool(&call.tool, call.arguments, &call.cancel)
                    .await;
                if let Err(error) = mailbox
                    .send(Command::CallCompleted {
                        epoch: call.epoch,
                        call_id: call.call_id,
                        context: call.context,
                        result,
                        reply: call.reply,
                    })
                    .await
                {
                    if let Command::CallCompleted { reply, .. } = error.0 {
                        let _ = reply.send(Err(stopped()));
                    }
                }
            });
            break;
        }
    }

    fn not_ready_message(&self) -> String {
        self.snapshot.last_error.clone().unwrap_or_else(|| {
            format!(
                "MCP server is {}",
                match self.state {
                    SupervisorState::Connecting => "connecting",
                    SupervisorState::Ready(_) => "ready",
                    SupervisorState::Recovering if self.snapshot.pending_oauth_url.is_some() => {
                        "awaiting authorization"
                    }
                    SupervisorState::Recovering => "recovering",
                    SupervisorState::Failed => "failed",
                    SupervisorState::Removed => "removed",
                }
            )
        })
    }

    async fn stop_server(&mut self) {
        if let SupervisorState::Ready(server) =
            std::mem::replace(&mut self.state, SupervisorState::Removed)
        {
            server.terminate().await;
        }
    }

    fn publish_snapshot(&mut self, error: Option<String>, auth_url: Option<String>) {
        self.snapshot.epoch = self.epoch;
        self.snapshot.state = self.state.clone();
        self.snapshot.last_error = error;
        self.snapshot.pending_oauth_url = auth_url;
        self.snapshots.send_replace(self.snapshot.clone());
    }
}

#[cfg(test)]
mod epoch_tests {
    use super::*;
    use crate::{HttpAuth, McpTransport, SharedBearer, TransportError, DEFAULT_TOOL_CALL_TIMEOUT};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{mpsc, RwLock, Semaphore};

    struct GatedTransport {
        started: mpsc::UnboundedSender<()>,
        releases: Arc<Semaphore>,
    }

    #[async_trait]
    impl McpTransport for GatedTransport {
        async fn request(
            &self,
            _method: &str,
            _params: Value,
            _timeout: Duration,
            _sink: &dyn crate::ServerMessageSink,
        ) -> Result<Value, TransportError> {
            let _ = self.started.send(());
            self.releases
                .acquire()
                .await
                .expect("test semaphore open")
                .forget();
            Ok(serde_json::json!({
                "content": [{"type": "text", "text": "ok"}]
            }))
        }

        async fn notify(&self, _notification: &Value) -> Result<(), TransportError> {
            Ok(())
        }

        fn requested_protocol_version(&self) -> &'static str {
            "2025-03-26"
        }

        fn is_alive(&self) -> bool {
            true
        }

        async fn shutdown(&self) {}
    }

    fn server(config: McpServerConfig) -> (McpServer, mpsc::UnboundedReceiver<()>, Arc<Semaphore>) {
        let (started, receiver) = mpsc::unbounded_channel();
        let releases = Arc::new(Semaphore::new(0));
        (
            McpServer {
                name: "test".to_string(),
                transport: Arc::new(GatedTransport {
                    started,
                    releases: Arc::clone(&releases),
                }),
                tools: std::sync::RwLock::new(Vec::new()),
                config,
                tools_changed: Arc::new(AtomicBool::new(false)),
                pending_oauth_urls: Arc::new(RwLock::new(HashMap::new())),
                oauth_bearer: SharedBearer::default(),
            },
            receiver,
            releases,
        )
    }

    fn http_config() -> McpServerConfig {
        McpServerConfig::Http {
            url: "https://example.test/mcp".to_string(),
            headers: HashMap::new(),
            auth: HttpAuth::None,
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
        }
    }

    fn stdio_config() -> McpServerConfig {
        McpServerConfig::Stdio {
            command: "test".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            tool_call_timeout: DEFAULT_TOOL_CALL_TIMEOUT,
        }
    }

    #[tokio::test]
    async fn http_calls_run_concurrently_but_stdio_calls_queue() {
        let (http, mut http_started, http_releases) = server(http_config());
        let http = SupervisorHandle::connected(http);
        let first = tokio::spawn({
            let http = http.clone();
            async move {
                http.call(
                    "one".to_string(),
                    serde_json::json!({}),
                    CancellationToken::new(),
                )
                .await
            }
        });
        let second = tokio::spawn({
            let http = http.clone();
            async move {
                http.call(
                    "two".to_string(),
                    serde_json::json!({}),
                    CancellationToken::new(),
                )
                .await
            }
        });
        () = tokio::time::timeout(Duration::from_secs(5), http_started.recv())
            .await
            .expect("first HTTP call starts in time")
            .expect("first HTTP call started");
        () = tokio::time::timeout(Duration::from_secs(5), http_started.recv())
            .await
            .expect("second HTTP call starts in time")
            .expect("second HTTP call starts concurrently");
        http_releases.add_permits(2);
        first.await.expect("first task").expect("first call");
        second.await.expect("second task").expect("second call");

        let (stdio, mut stdio_started, stdio_releases) = server(stdio_config());
        let stdio = SupervisorHandle::connected(stdio);
        let first = tokio::spawn({
            let stdio = stdio.clone();
            async move {
                stdio
                    .call(
                        "one".to_string(),
                        serde_json::json!({}),
                        CancellationToken::new(),
                    )
                    .await
            }
        });
        let second = tokio::spawn({
            let stdio = stdio.clone();
            async move {
                stdio
                    .call(
                        "two".to_string(),
                        serde_json::json!({}),
                        CancellationToken::new(),
                    )
                    .await
            }
        });
        () = tokio::time::timeout(Duration::from_secs(5), stdio_started.recv())
            .await
            .expect("first stdio call starts in time")
            .expect("first stdio call started");
        assert!(
            stdio_started.try_recv().is_err(),
            "the actor queues stdio calls sequentially"
        );
        stdio_releases.add_permits(1);
        () = tokio::time::timeout(Duration::from_secs(5), stdio_started.recv())
            .await
            .expect("second stdio call starts in time")
            .expect("second stdio call started");
        stdio_releases.add_permits(1);
        first.await.expect("first task").expect("first call");
        second.await.expect("second task").expect("second call");
    }

    #[tokio::test]
    async fn same_config_reconfigure_supersedes_late_connect() {
        let handle = SupervisorHandle::connecting(stdio_config());
        let first_epoch = handle
            .reconfigure(stdio_config())
            .await
            .expect("first connect");
        let second_epoch = handle
            .reconfigure(stdio_config())
            .await
            .expect("second connect");
        let (late, _, _) = server(stdio_config());
        assert!(!handle.publish(first_epoch, late).await);
        let (current, _, _) = server(stdio_config());
        assert!(handle.publish(second_epoch, current).await);
        assert!(handle.snapshot().is_ready());
    }

    #[tokio::test]
    async fn remove_supersedes_late_connect() {
        let handle = SupervisorHandle::connecting(stdio_config());
        let epoch = handle
            .reconfigure(stdio_config())
            .await
            .expect("connect epoch");
        handle.remove().await;
        let (late, _, _) = server(stdio_config());
        assert!(!handle.publish(epoch, late).await);
        assert!(matches!(handle.snapshot().state, SupervisorState::Removed));
    }

    #[tokio::test]
    async fn changed_config_supersedes_recovery_epoch() {
        let (serving, _, _) = server(stdio_config());
        let handle = SupervisorHandle::connected(serving);
        let RecoveryClaim::Leader(recovery) = handle.claim_recovery(0).await else {
            panic!("recovery leader");
        };
        let changed_epoch = handle
            .reconfigure(http_config())
            .await
            .expect("changed config epoch");
        let (stale, _, _) = server(stdio_config());
        assert!(!handle.publish(recovery.epoch, stale).await);
        let (replacement, _, _) = server(http_config());
        assert!(handle.publish(changed_epoch, replacement).await);
        assert_eq!(handle.snapshot().config, http_config());
    }

    #[tokio::test]
    async fn recovery_has_one_epoch_fenced_leader_and_followers() {
        let (serving, _, _) = server(stdio_config());
        let handle = SupervisorHandle::connected(serving);

        let RecoveryClaim::Leader(permit) = handle.claim_recovery(0).await else {
            panic!("first claimant leads recovery");
        };
        assert_eq!(permit.epoch, 1);
        assert!(matches!(
            handle.claim_recovery(0).await,
            RecoveryClaim::Follow(_)
        ));

        let new_epoch = handle
            .reconfigure(http_config())
            .await
            .expect("reconfigure");
        assert_eq!(new_epoch, 2);
        let (late, _, _) = server(stdio_config());
        assert!(!handle.publish(permit.epoch, late).await);
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.epoch, new_epoch);
        assert!(matches!(snapshot.state, SupervisorState::Connecting));
        assert_eq!(snapshot.config, http_config());
    }

    #[tokio::test]
    async fn http_completion_is_fenced_by_the_serving_epoch() {
        let (serving, mut started, releases) = server(http_config());
        let handle = SupervisorHandle::connected(serving);
        let call = tokio::spawn({
            let handle = handle.clone();
            async move {
                handle
                    .call(
                        "report".to_string(),
                        serde_json::json!({}),
                        CancellationToken::new(),
                    )
                    .await
            }
        });
        () = tokio::time::timeout(Duration::from_secs(5), started.recv())
            .await
            .expect("HTTP call starts in time")
            .expect("HTTP call started");

        let RecoveryClaim::Leader(permit) = handle.claim_recovery(0).await else {
            panic!("recovery advances the actor epoch");
        };
        releases.add_permits(1);
        let outcome = call
            .await
            .expect("call task")
            .expect("correlated success reaches its waiter");
        assert_eq!(outcome.result.expect("successful call"), "ok");
        assert_eq!(
            handle.snapshot().epoch,
            permit.epoch,
            "stale completion does not mutate lifecycle epoch"
        );

        let (replacement, _, _) = server(http_config());
        assert!(handle.publish(permit.epoch, replacement).await);
        assert!(handle.snapshot().is_ready());
    }
}
