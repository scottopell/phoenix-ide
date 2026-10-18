use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cdp::Socket;
use async_tungstenite::tungstenite::Message;
use chromiumoxide::cdp::js_protocol::runtime::{
    ConsoleApiCalledType, EvaluateParams, EventConsoleApiCalled, EventExecutionContextCreated,
    EventExecutionContextDestroyed, ExecutionContextId,
};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Notify, OwnedMutexGuard};

use super::{
    extract_console_arg_text, BrowserError, BrowserSession, ConsoleEntry, MAX_CONSOLE_LOGS,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Generation(uuid::Uuid);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Document(uuid::Uuid);

impl Default for Document {
    fn default() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

enum Boundary {
    Begin(Generation),
    Drain,
}

struct Pending {
    context: ExecutionContextId,
    boundary: Boundary,
    acknowledged: oneshot::Sender<ContextBoundary>,
}

#[derive(Clone, Debug)]
struct ContextBoundary {
    document: Document,
}

struct Context {
    unique: String,
    document: Document,
}

struct ChildTarget {
    parent: Option<String>,
    frame: String,
    contexts: HashMap<ExecutionContextId, Context>,
}

#[derive(Default)]
struct State {
    children: HashMap<String, ChildTarget>,
    frames: HashMap<String, Document>,
    contexts: HashMap<ExecutionContextId, Context>,
    document: Document,
    generation: Option<Generation>,
    failure: Option<String>,
    pending: HashMap<String, Pending>,
}

impl State {
    fn detach_subtree(&mut self, session: &str) {
        let mut retired = HashSet::from([session.to_owned()]);
        loop {
            let before = retired.len();
            for (session, child) in &self.children {
                if child
                    .parent
                    .as_ref()
                    .is_some_and(|parent| retired.contains(parent))
                {
                    retired.insert(session.clone());
                }
            }
            if retired.len() == before {
                break;
            }
        }
        let frames: Vec<_> = self
            .children
            .iter()
            .filter(|(session, _)| retired.contains(*session))
            .map(|(_, target)| target.frame.clone())
            .collect();
        self.children
            .retain(|session, _| !retired.contains(session));
        for frame in frames {
            self.remove_frame(&frame);
        }
    }

    fn remove_frame(&mut self, frame: &str) {
        if !self.children.values().any(|target| target.frame == frame) {
            self.frames.remove(frame);
        }
    }
}

#[derive(Default)]
pub(super) struct ConsoleCapture {
    state: Mutex<State>,
    navigation: Arc<tokio::sync::Mutex<()>>,
    drains: Mutex<Option<mpsc::Sender<oneshot::Sender<()>>>>,
}

/// A navigation's source-event boundary, owned by one browser capture listener.
pub struct ConsoleNavigationCapture {
    owner: Arc<ConsoleCapture>,
    generation: Generation,
    boundary: ContextBoundary,
    _lease: OwnedMutexGuard<()>,
}

struct Registration {
    owner: Arc<ConsoleCapture>,
    marker: String,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.owner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .remove(&self.marker);
    }
}

impl ConsoleCapture {
    fn register(
        self: &Arc<Self>,
        context: ExecutionContextId,
        boundary: Boundary,
    ) -> (Registration, oneshot::Receiver<ContextBoundary>) {
        let marker = format!("__phoenix_console_fence_{}", uuid::Uuid::new_v4());
        let (acknowledged, receiver) = oneshot::channel();
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .insert(
                marker.clone(),
                Pending {
                    context,
                    boundary,
                    acknowledged,
                },
            );
        (
            Registration {
                owner: self.clone(),
                marker,
            },
            receiver,
        )
    }

    #[cfg(test)]
    fn accept(
        &self,
        event: &EventConsoleApiCalled,
        logs: &Mutex<VecDeque<ConsoleEntry>>,
        notification: &Notify,
    ) {
        self.accept_from(event, None, logs, notification);
    }

    fn accept_from(
        &self,
        event: &EventConsoleApiCalled,
        child_session: Option<&str>,
        logs: &Mutex<VecDeque<ConsoleEntry>>,
        notification: &Notify,
    ) {
        let generation = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let marker = (event.r#type == ConsoleApiCalledType::Debug && event.args.len() == 1)
                .then(|| {
                    event.args[0]
                        .value
                        .as_ref()
                        .and_then(serde_json::Value::as_str)
                })
                .flatten();
            if let Some(marker) = marker.filter(|marker| {
                child_session.is_none()
                    && state
                        .pending
                        .get(*marker)
                        .is_some_and(|pending| pending.context == event.execution_context_id)
            }) {
                let pending = state.pending.remove(marker).expect("registered marker");
                if state.failure.is_some() {
                    return;
                }
                if let Boundary::Begin(generation) = pending.boundary {
                    state.generation = Some(generation);
                }
                if state.contexts.contains_key(&event.execution_context_id) {
                    let _ = pending.acknowledged.send(ContextBoundary {
                        document: state.document,
                    });
                } else {
                    state.failure =
                        Some("source marker arrived from an unknown execution context".to_owned());
                }
                return;
            }
            let document = match child_session {
                None => state
                    .contexts
                    .get(&event.execution_context_id)
                    .map(|context| context.document),
                Some(session) => state.children.get(session).and_then(|target| {
                    target
                        .contexts
                        .get(&event.execution_context_id)
                        .map(|context| context.document)
                }),
            };
            if document.is_none() {
                state.failure = Some(format!(
                    "console event from unknown execution context {:?}",
                    event.execution_context_id
                ));
                tracing::warn!(failure = ?state.failure, "console capture lost context ownership");
            }
            (state.generation, document)
        };
        let level = event.r#type.clone().into();
        let text = event
            .args
            .iter()
            .map(extract_console_arg_text)
            .collect::<Vec<_>>()
            .join(" ");
        tracing::debug!(level = %level, text = %text, "Console event captured");
        let mut logs = logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if logs.len() >= MAX_CONSOLE_LOGS {
            logs.pop_front();
        }
        logs.push_back(ConsoleEntry {
            level,
            text,
            timestamp: Instant::now(),
            source_document: generation.1,
            capture_generation: generation.0,
        });
        notification.notify_one();
    }
}

fn capture_error(message: impl std::fmt::Display) -> BrowserError {
    BrowserError::OperationFailed(format!(
        "Could not fence navigation console capture: {message}"
    ))
}

impl ConsoleCapture {
    #[cfg(test)]
    fn observe(
        &self,
        message: &Value,
        logs: &Mutex<VecDeque<ConsoleEntry>>,
        notification: &Notify,
    ) -> Result<(), BrowserError> {
        self.observe_from(message, None, logs, notification)
    }

    fn observe_from(
        &self,
        message: &Value,
        child_session: Option<&str>,
        logs: &Mutex<VecDeque<ConsoleEntry>>,
        notification: &Notify,
    ) -> Result<(), BrowserError> {
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match message.get("method").and_then(Value::as_str) {
            Some("Page.frameAttached") => {
                let frame = params["frameId"]
                    .as_str()
                    .ok_or_else(|| capture_error("frame attachment has no frame identity"))?;
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let document = params["parentFrameId"]
                    .as_str()
                    .and_then(|parent| state.frames.get(parent))
                    .copied()
                    .or_else(|| {
                        child_session.and_then(|session| {
                            state
                                .children
                                .get(session)
                                .and_then(|target| state.frames.get(&target.frame).copied())
                        })
                    })
                    .unwrap_or(state.document);
                state.frames.insert(frame.to_owned(), document);
            }
            Some("Page.frameDetached") => {
                if params["reason"] != "swap" {
                    if let Some(frame) = params["frameId"].as_str() {
                        self.state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .remove_frame(frame);
                    }
                }
            }
            Some("Target.attachedToTarget") => {
                let session = params["sessionId"]
                    .as_str()
                    .ok_or_else(|| capture_error("child attachment has no session"))?;
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let document = match child_session {
                    None => state.document,
                    Some(parent) => {
                        let parent = state
                            .children
                            .get(parent)
                            .ok_or_else(|| capture_error("unknown parent target"))?;
                        *state
                            .frames
                            .get(&parent.frame)
                            .ok_or_else(|| capture_error("unknown parent frame ownership"))?
                    }
                };
                let frame = params["targetInfo"]["targetId"]
                    .as_str()
                    .ok_or_else(|| capture_error("child attachment has no target identity"))?
                    .to_owned();
                state.frames.entry(frame.clone()).or_insert(document);
                state.children.insert(
                    session.to_owned(),
                    ChildTarget {
                        parent: child_session.map(str::to_owned),
                        frame,
                        contexts: HashMap::new(),
                    },
                );
            }
            Some("Target.detachedFromTarget") => {
                if let Some(session) = params["sessionId"].as_str() {
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.detach_subtree(session);
                    notification.notify_waiters();
                }
            }
            Some(
                method @ ("Runtime.executionContextCreated"
                | "Runtime.executionContextDestroyed"
                | "Runtime.executionContextsCleared"),
            ) => self.observe_context(method, params, child_session)?,
            Some("Runtime.consoleAPICalled") => {
                let event: EventConsoleApiCalled =
                    serde_json::from_value(params).map_err(capture_error)?;
                self.accept_from(&event, child_session, logs, notification);
            }
            _ => {}
        }
        Ok(())
    }
    fn observe_context(
        &self,
        method: &str,
        params: Value,
        child_session: Option<&str>,
    ) -> Result<(), BrowserError> {
        match method {
            "Runtime.executionContextCreated" => {
                let event: EventExecutionContextCreated =
                    serde_json::from_value(params).map_err(capture_error)?;
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let document =
                    match child_session {
                        None => state.document,
                        Some(session) => {
                            let target = state.children.get(session).ok_or_else(|| {
                                capture_error("context from unknown child target")
                            })?;
                            *state.frames.get(&target.frame).ok_or_else(|| {
                                capture_error("context from unknown frame ownership")
                            })?
                        }
                    };
                let contexts = match child_session {
                    None => &mut state.contexts,
                    Some(session) => {
                        &mut state
                            .children
                            .get_mut(session)
                            .ok_or_else(|| capture_error("context from unknown child target"))?
                            .contexts
                    }
                };
                contexts.insert(
                    event.context.id,
                    Context {
                        unique: event.context.unique_id,
                        document,
                    },
                );
            }
            "Runtime.executionContextDestroyed" => {
                let event: EventExecutionContextDestroyed =
                    serde_json::from_value(params).map_err(capture_error)?;
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let contexts = match child_session {
                    None => &mut state.contexts,
                    Some(session) => {
                        let Some(target) = state.children.get_mut(session) else {
                            tracing::debug!(
                                session,
                                "context destruction after console target retirement"
                            );
                            return Ok(());
                        };
                        &mut target.contexts
                    }
                };
                contexts.retain(|_, context| context.unique != event.execution_context_unique_id);
            }
            "Runtime.executionContextsCleared" => {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match child_session {
                    None => {
                        state.contexts.clear();
                        state.document = Document::default();
                    }
                    Some(session) => {
                        if let Some(target) = state.children.get_mut(session) {
                            target.contexts.clear();
                        } else {
                            tracing::debug!(
                                session,
                                "context reset after console target retirement"
                            );
                        }
                    }
                }
            }
            _ => unreachable!("context event selected by observe_from"),
        }
        Ok(())
    }
}

struct Command {
    method: &'static str,
    session: String,
}

struct Drain {
    acknowledged: oneshot::Sender<()>,
    pending: HashSet<u64>,
}

fn retire_commands(
    state: &State,
    root_session: &str,
    pending: &mut HashMap<u64, Command>,
    mut drain: Option<&mut Drain>,
) {
    pending.retain(|id, command| {
        let live = command.session == root_session || state.children.contains_key(&command.session);
        if !live {
            if let Some(drain) = drain.as_mut() {
                drain.pending.remove(id);
            }
        }
        live
    });
}

struct Listener {
    socket: Socket,
    root_session: String,
    next_id: u64,
    pending: HashMap<u64, Command>,
    drain: Option<Drain>,
    capture: Arc<ConsoleCapture>,
    logs: Arc<Mutex<VecDeque<ConsoleEntry>>>,
    notification: Arc<Notify>,
}

impl Listener {
    async fn send(
        &mut self,
        session: &str,
        method: &'static str,
        params: Value,
    ) -> Result<u64, BrowserError> {
        self.next_id += 1;
        let id = self.next_id;
        self.socket
            .send(Message::Text(
                json!({"id":id,"sessionId":session,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(capture_error)?;
        self.pending.insert(
            id,
            Command {
                method,
                session: session.to_owned(),
            },
        );
        Ok(id)
    }

    async fn enable_target(&mut self, session: &str) -> Result<(), BrowserError> {
        self.send(session, "Target.setAutoAttach", json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true,"filter":[{"type":"iframe","exclude":false},{"exclude":true}]})).await?;
        self.send(session, "Page.enable", json!({})).await?;
        self.send(session, "Runtime.enable", json!({})).await?;
        self.send(session, "Runtime.runIfWaitingForDebugger", json!({}))
            .await?;
        if self.drain.is_some() {
            self.fence_child(session).await?;
        }
        Ok(())
    }

    async fn fence_child(&mut self, session: &str) -> Result<(), BrowserError> {
        let id = self
            .send(
                session,
                "Runtime.evaluate",
                json!({"expression":"0","returnByValue":true}),
            )
            .await?;
        self.drain
            .as_mut()
            .expect("drain registered")
            .pending
            .insert(id);
        Ok(())
    }

    fn acknowledge_drain(&mut self) {
        if self
            .drain
            .as_ref()
            .is_some_and(|drain| drain.pending.is_empty())
        {
            let _ = self
                .drain
                .take()
                .expect("registered drain")
                .acknowledged
                .send(());
        }
    }

    async fn begin_drain(&mut self, acknowledged: oneshot::Sender<()>) -> Result<(), BrowserError> {
        self.drain = Some(Drain {
            acknowledged,
            pending: HashSet::new(),
        });
        let children: Vec<_> = self
            .capture
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .children
            .keys()
            .cloned()
            .collect();
        for child in children {
            self.fence_child(&child).await?;
        }
        self.acknowledge_drain();
        Ok(())
    }

    async fn read(&mut self) -> Result<(), BrowserError> {
        let message = self
            .socket
            .next()
            .await
            .ok_or_else(|| capture_error("console connection ended"))?
            .map_err(capture_error)?;
        self.process(message).await
    }

    async fn process(&mut self, message: Message) -> Result<(), BrowserError> {
        let Message::Text(text) = message else {
            return if matches!(message, Message::Close(_)) {
                Err(capture_error("console connection ended"))
            } else {
                Ok(())
            };
        };
        let message: Value = serde_json::from_str(&text).map_err(capture_error)?;
        if let Some(id) = message["id"].as_u64() {
            if let Some(command) = self.pending.remove(&id) {
                if let Some(error) = message.get("error") {
                    let detached = command.session != self.root_session
                        && !self
                            .capture
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .children
                            .contains_key(&command.session);
                    if detached {
                        tracing::debug!(method = command.method, %error, "console target detached before command response");
                    } else {
                        return Err(capture_error(format!("{}: {error}", command.method)));
                    }
                }
                if let Some(drain) = self.drain.as_mut() {
                    drain.pending.remove(&id);
                }
            }
        }
        let session = message["sessionId"]
            .as_str()
            .filter(|session| *session != self.root_session);
        self.capture
            .observe_from(&message, session, &self.logs, &self.notification)?;
        if message["method"] == "Target.attachedToTarget" {
            let child = message["params"]["sessionId"]
                .as_str()
                .ok_or_else(|| capture_error("child attachment has no session"))?;
            self.enable_target(child).await?;
        }
        if message["method"] == "Target.detachedFromTarget" {
            let state = self
                .capture
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            retire_commands(
                &state,
                &self.root_session,
                &mut self.pending,
                self.drain.as_mut(),
            );
        }
        self.acknowledge_drain();
        Ok(())
    }
}

pub(super) async fn listen(
    session: &BrowserSession,
) -> Result<tokio::task::JoinHandle<()>, BrowserError> {
    let (drains, mut drain_requests) = mpsc::channel(1);
    let mut listener = tokio::time::timeout(Duration::from_secs(5), async {
        let (socket, root_session) = crate::cdp::connect_target(
            session.snapshot_websocket_address(),
            session.page.target_id(),
            8 * 1024 * 1024,
        )
        .await
        .map_err(capture_error)?;
        let mut listener = Listener {
            socket,
            root_session,
            next_id: 1,
            pending: HashMap::new(),
            drain: None,
            capture: session.console_capture.clone(),
            logs: session.console_logs.clone(),
            notification: session.console_event.clone(),
        };
        listener
            .enable_target(&listener.root_session.clone())
            .await?;
        while !listener.pending.is_empty() {
            listener.read().await?;
        }
        Ok::<_, BrowserError>(listener)
    })
    .await
    .map_err(|_| capture_error("console connection setup timed out"))??;
    *session
        .console_capture
        .drains
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(drains);
    Ok(tokio::spawn(async move {
        let failure = loop {
            let result = tokio::select! {
                message = listener.socket.next() => match message {
                    Some(Ok(message)) => listener.process(message).await,
                    Some(Err(error)) => Err(capture_error(error)),
                    None => Err(capture_error("console connection ended")),
                },
                Some(acknowledged) = drain_requests.recv(), if listener.drain.is_none() => listener.begin_drain(acknowledged).await,
            };
            if let Err(error) = result {
                break error.to_string();
            }
        };
        tracing::warn!(%failure, "console capture stopped");
        let mut state = listener
            .capture
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.failure = Some(failure);
        state.pending.clear();
    }))
}

impl BrowserSession {
    async fn console_boundary(&self, boundary: Boundary) -> Result<ContextBoundary, BrowserError> {
        if let Some(failure) = self
            .console_capture
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failure
            .clone()
        {
            return Err(capture_error(failure));
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            let context = self
                .page
                .secondary_execution_context()
                .await?
                .ok_or_else(|| capture_error("isolated execution context unavailable"))?;
            let (registration, acknowledged) = self.console_capture.register(context, boundary);
            let mut expression = EvaluateParams::new(format!(
                "console.debug({})",
                serde_json::to_string(&registration.marker).expect("marker serializes")
            ));
            expression.context_id = Some(context);
            self.page.evaluate_expression(expression).await?;
            acknowledged.await.map_err(|error| {
                let failure = self
                    .console_capture
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .failure
                    .clone();
                capture_error(failure.unwrap_or_else(|| error.to_string()))
            })
        })
        .await
        .map_err(|_| capture_error("source-event acknowledgement timed out"))?
    }

    async fn console_children_boundary(&self) -> Result<(), BrowserError> {
        let drains = self
            .console_capture
            .drains
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| capture_error("console listener unavailable"))?;
        tokio::time::timeout(Duration::from_secs(5), async {
            let (acknowledged, receiver) = oneshot::channel();
            drains.send(acknowledged).await.map_err(capture_error)?;
            receiver.await.map_err(capture_error)
        })
        .await
        .map_err(|_| capture_error("child target drain timed out"))??;
        if let Some(failure) = self
            .console_capture
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failure
            .clone()
        {
            return Err(capture_error(failure));
        }
        Ok(())
    }

    /// Drain earlier console events before starting a navigation's capture.
    ///
    /// # Errors
    /// Returns an error when the isolated source marker cannot be acknowledged.
    pub async fn begin_console_navigation_capture(
        &self,
    ) -> Result<ConsoleNavigationCapture, BrowserError> {
        let lease = self.console_capture.navigation.clone().lock_owned().await;
        self.console_children_boundary().await?;
        let generation = Generation(uuid::Uuid::new_v4());
        let boundary = self.console_boundary(Boundary::Begin(generation)).await?;
        Ok(ConsoleNavigationCapture {
            owner: self.console_capture.clone(),
            generation,
            boundary,
            _lease: lease,
        })
    }

    /// Drain emitted console events and return this navigation's document entries.
    ///
    /// # Errors
    /// Returns an error for a foreign capture or an unavailable source-event fence.
    pub async fn finish_console_navigation_capture(
        &self,
        capture: ConsoleNavigationCapture,
    ) -> Result<Vec<ConsoleEntry>, BrowserError> {
        if !Arc::ptr_eq(&self.console_capture, &capture.owner) {
            return Err(capture_error("capture belongs to another browser"));
        }
        let final_boundary = self.console_boundary(Boundary::Drain).await?;
        self.console_children_boundary().await?;
        let same_document = capture.boundary.document == final_boundary.document;
        let logs = self
            .console_logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(logs
            .iter()
            .filter(|entry| {
                entry.capture_generation == Some(capture.generation)
                    && entry.source_document.is_some_and(|document| {
                        same_document || capture.boundary.document != document
                    })
            })
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn context(capture: &ConsoleCapture, id: i64, unique: &str) {
        capture.observe(&json!({"method":"Runtime.executionContextCreated", "params":{"context":{"id":id,"uniqueId":unique,"origin":"","name":""}}}), &Mutex::new(VecDeque::new()), &Notify::new()).unwrap();
    }

    fn event(context: i64, text: &str, kind: &str) -> EventConsoleApiCalled {
        serde_json::from_value(json!({"type":kind,"args":[{"type":"string","value":text}],"executionContextId":context,"timestamp":1})).unwrap()
    }

    #[tokio::test]
    async fn source_markers_fence_a_backlogged_listener_and_exclude_prior_documents() {
        let capture = Arc::new(ConsoleCapture::default());
        let logs = Mutex::new(VecDeque::new());
        let notification = Notify::new();
        let generation = Generation(uuid::Uuid::new_v4());
        for id in [1, 9, 10] {
            context(&capture, id, &format!("initial-{id}"));
        }
        let (begin, mut begin_ack) =
            capture.register(ExecutionContextId::new(9), Boundary::Begin(generation));
        capture.accept(
            &event(1, "old error already queued before navigation", "error"),
            &logs,
            &notification,
        );
        assert!(matches!(
            begin_ack.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        capture.accept(&event(9, &begin.marker, "debug"), &logs, &notification);
        let initial = begin_ack.await.unwrap();
        assert_eq!(
            logs.lock().unwrap().len(),
            1,
            "navigation preserves session history"
        );
        capture.accept(
            &event(1, "late old-document error", "error"),
            &logs,
            &notification,
        );
        context(&capture, 4, "late-created-old-iframe");
        capture.accept(
            &event(4, "late-created old iframe error", "error"),
            &logs,
            &notification,
        );
        capture
            .observe(
                &json!({"method":"Runtime.executionContextsCleared","params":{}}),
                &logs,
                &notification,
            )
            .unwrap();
        context(&capture, 10, "new-marker");
        context(&capture, 2, "new-main");
        context(&capture, 3, "new-iframe");
        capture.accept(
            &event(2, "new navigation error", "error"),
            &logs,
            &notification,
        );
        capture.accept(&event(3, "new iframe error", "error"), &logs, &notification);
        let (end, end_ack) = capture.register(ExecutionContextId::new(10), Boundary::Drain);
        capture.accept(&event(10, &end.marker, "debug"), &logs, &notification);
        end_ack.await.unwrap();
        let logs = logs.lock().unwrap();
        let document = initial.document;
        let selected: Vec<_> = logs
            .iter()
            .filter(|entry| {
                entry.capture_generation == Some(generation)
                    && entry
                        .source_document
                        .is_some_and(|source| source != document)
            })
            .map(|entry| entry.text.as_str())
            .collect();
        assert_eq!(selected, ["new navigation error", "new iframe error"]);
        assert_eq!(logs.len(), 5, "internal markers are not console entries");
    }

    #[tokio::test]
    async fn source_order_keeps_detached_contexts_and_distinguishes_reused_numeric_ids() {
        let capture = Arc::new(ConsoleCapture::default());
        let logs = Mutex::new(VecDeque::new());
        let notification = Notify::new();
        context(&capture, 1, "old-main");
        context(&capture, 2, "old-iframe");
        context(&capture, 9, "old-marker-world");
        let generation = Generation(uuid::Uuid::new_v4());
        let (begin, ack) =
            capture.register(ExecutionContextId::new(9), Boundary::Begin(generation));
        capture.accept(&event(9, &begin.marker, "debug"), &logs, &notification);
        let initial = ack.await.unwrap();
        capture.accept(
            &event(2, "old iframe during navigation", "error"),
            &logs,
            &notification,
        );
        capture
            .observe(
                &json!({"method":"Runtime.executionContextsCleared","params":{}}),
                &logs,
                &notification,
            )
            .unwrap();
        context(&capture, 2, "new-iframe-reused-number");
        capture.observe(&json!({"method":"Runtime.executionContextDestroyed","params":{"executionContextUniqueId":"old-iframe"}}), &logs, &notification).unwrap();
        assert_eq!(
            capture
                .state
                .lock()
                .unwrap()
                .contexts
                .get(&ExecutionContextId::new(2))
                .map(|context| context.unique.as_str()),
            Some("new-iframe-reused-number")
        );
        capture.accept(
            &event(2, "removed new iframe error", "error"),
            &logs,
            &notification,
        );
        capture.observe(&json!({"method":"Runtime.executionContextDestroyed","params":{"executionContextUniqueId":"new-iframe-reused-number"}}), &logs, &notification).unwrap();
        let logs = logs.lock().unwrap();
        let selected: Vec<_> = logs
            .iter()
            .filter(|entry| {
                entry.capture_generation == Some(generation)
                    && entry
                        .source_document
                        .is_some_and(|source| source != initial.document)
            })
            .map(|entry| entry.text.as_str())
            .collect();
        assert_eq!(selected, ["removed new iframe error"]);
        assert_eq!(
            logs.len(),
            2,
            "both entries remain in ordinary session history"
        );
        assert!(!capture
            .state
            .lock()
            .unwrap()
            .contexts
            .contains_key(&ExecutionContextId::new(2)));
    }

    #[tokio::test]
    async fn reused_child_session_keeps_context_document_ownership() {
        let capture = Arc::new(ConsoleCapture::default());
        let logs = Mutex::new(VecDeque::new());
        let notification = Notify::new();
        context(&capture, 9, "main-marker");
        let generation = Generation(uuid::Uuid::new_v4());
        let (begin, acknowledged) =
            capture.register(ExecutionContextId::new(9), Boundary::Begin(generation));
        capture.accept(&event(9, &begin.marker, "debug"), &logs, &notification);
        let old = acknowledged.await.unwrap().document;
        let observe = |message: Value| {
            let session = message["sessionId"].as_str();
            capture
                .observe_from(&message, session, &logs, &notification)
                .unwrap();
        };
        observe(
            json!({"method":"Target.attachedToTarget","params":{"sessionId":"child","targetInfo":{"targetId":"frame"}}}),
        );
        observe(
            json!({"sessionId":"child","method":"Runtime.executionContextCreated","params":{"context":{"id":1,"uniqueId":"old-child-context","origin":"","name":""}}}),
        );
        observe(json!({"method":"Runtime.executionContextsCleared","params":{}}));
        observe(
            json!({"method":"Page.frameAttached","params":{"frameId":"frame","parentFrameId":"root"}}),
        );
        observe(
            json!({"sessionId":"child","method":"Runtime.executionContextCreated","params":{"context":{"id":2,"uniqueId":"new-child-context","origin":"","name":""}}}),
        );
        capture.accept_from(
            &event(1, "late-old-child", "error"),
            Some("child"),
            &logs,
            &notification,
        );
        capture.accept_from(
            &event(2, "new-child", "error"),
            Some("child"),
            &logs,
            &notification,
        );
        observe(json!({"method":"Target.detachedFromTarget","params":{"sessionId":"child"}}));
        let logs = logs.lock().unwrap();
        assert_eq!(logs[0].source_document, Some(old));
        assert_ne!(logs[1].source_document, Some(old));
        assert_eq!(logs[1].capture_generation, Some(generation));
        assert!(capture.state.lock().unwrap().children.is_empty());
    }

    #[tokio::test]
    async fn parent_detach_retires_descendants_and_their_pending_barriers() {
        let capture = Arc::new(ConsoleCapture::default());
        let logs = Mutex::new(VecDeque::new());
        let notification = Notify::new();
        let observe = |message: Value| {
            let session = message["sessionId"].as_str();
            capture
                .observe_from(&message, session, &logs, &notification)
                .unwrap();
        };
        observe(
            json!({"method":"Target.attachedToTarget","params":{"sessionId":"parent","targetInfo":{"targetId":"parent-frame"}}}),
        );
        observe(
            json!({"sessionId":"parent","method":"Target.attachedToTarget","params":{"sessionId":"nested","targetInfo":{"targetId":"nested-frame"}}}),
        );
        observe(
            json!({"method":"Target.attachedToTarget","params":{"sessionId":"sibling","targetInfo":{"targetId":"nested-frame"}}}),
        );
        observe(
            json!({"sessionId":"nested","method":"Runtime.executionContextCreated","params":{"context":{"id":1,"uniqueId":"nested-context","origin":"","name":""}}}),
        );
        capture.accept_from(
            &event(1, "nested error before detach", "error"),
            Some("nested"),
            &logs,
            &notification,
        );
        let mut pending = HashMap::from([
            (
                1,
                Command {
                    session: "parent".to_owned(),
                    method: "Runtime.enable",
                },
            ),
            (
                2,
                Command {
                    session: "nested".to_owned(),
                    method: "Runtime.evaluate",
                },
            ),
            (
                3,
                Command {
                    session: "sibling".to_owned(),
                    method: "Runtime.evaluate",
                },
            ),
            (
                4,
                Command {
                    session: "root".to_owned(),
                    method: "Runtime.enable",
                },
            ),
        ]);
        let (acknowledged, _receiver) = oneshot::channel();
        let mut drain = Drain {
            acknowledged,
            pending: HashSet::from([2, 3]),
        };
        observe(json!({"method":"Target.detachedFromTarget","params":{"sessionId":"parent"}}));
        {
            let state = capture.state.lock().unwrap();
            assert_eq!(state.children.len(), 1);
            assert!(state.children.contains_key("sibling"));
            assert!(
                state.frames.contains_key("nested-frame"),
                "a retained session still owns this frame"
            );
            assert!(!state.frames.contains_key("parent-frame"));
            retire_commands(&state, "root", &mut pending, Some(&mut drain));
        }
        assert_eq!(pending.len(), 2);
        assert!(pending.contains_key(&3) && pending.contains_key(&4));
        assert_eq!(drain.pending, HashSet::from([3]));
        observe(
            json!({"sessionId":"nested","method":"Runtime.executionContextDestroyed","params":{"executionContextUniqueId":"nested-context"}}),
        );
        observe(
            json!({"sessionId":"nested","method":"Runtime.executionContextsCleared","params":{}}),
        );
        assert!(capture.state.lock().unwrap().failure.is_none());
        capture.accept_from(
            &event(1, "uncertain late nested error", "error"),
            Some("nested"),
            &logs,
            &notification,
        );
        assert!(
            capture.state.lock().unwrap().failure.is_some(),
            "unknown late console ownership must be explicit"
        );
        let logs = logs.lock().unwrap();
        assert_eq!(logs[0].text, "nested error before detach");
        assert!(logs[0].source_document.is_some());
        assert_eq!(logs[1].text, "uncertain late nested error");
    }

    #[tokio::test]
    async fn unknown_context_marks_capture_failed_without_discarding_session_history() {
        let capture = Arc::new(ConsoleCapture::default());
        let logs = Mutex::new(VecDeque::new());
        let notification = Notify::new();
        capture.accept(&event(77, "unowned error", "error"), &logs, &notification);
        assert!(capture
            .state
            .lock()
            .unwrap()
            .failure
            .as_ref()
            .unwrap()
            .contains("unknown execution context"));
        assert_eq!(logs.lock().unwrap()[0].text, "unowned error");
        context(&capture, 9, "marker");
        let (registration, acknowledged) =
            capture.register(ExecutionContextId::new(9), Boundary::Drain);
        capture.accept(
            &event(9, &registration.marker, "debug"),
            &logs,
            &notification,
        );
        assert!(
            acknowledged.await.is_err(),
            "a failed capture cannot report a successful fence"
        );
    }

    #[tokio::test]
    async fn same_document_navigation_excludes_events_before_its_source_marker() {
        let capture = Arc::new(ConsoleCapture::default());
        let logs = Mutex::new(VecDeque::new());
        let notification = Notify::new();
        let generation = Generation(uuid::Uuid::new_v4());
        for id in [1, 9, 10] {
            context(&capture, id, &format!("initial-{id}"));
        }
        let (begin, acknowledged) =
            capture.register(ExecutionContextId::new(9), Boundary::Begin(generation));
        capture.accept(
            &event(1, "before fragment navigation", "error"),
            &logs,
            &notification,
        );
        capture.accept(&event(9, &begin.marker, "debug"), &logs, &notification);
        acknowledged.await.unwrap();
        capture.accept(
            &event(1, "after fragment navigation", "error"),
            &logs,
            &notification,
        );
        let logs = logs.lock().unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].text, "before fragment navigation");
        assert_eq!(logs[0].capture_generation, None);
        assert_eq!(logs[1].text, "after fragment navigation");
        assert_eq!(logs[1].capture_generation, Some(generation));
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "One browser lifetime verifies ordered document capture and history retention"
    )]
    async fn native_console_capture_preserves_new_document_and_iframe_errors() {
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};

        if matches!(
            std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
            Ok("1" | "true")
        ) {
            return;
        }
        let manager = crate::BrowserSessionManager::new();
        let scope = ResourceScopeKey::Work(
            WorkScopeId::parse(format!("console-fence-{}", uuid::Uuid::new_v4())).unwrap(),
        );
        let session = manager.get_session(&scope).await.expect("Chrome session");
        {
            let mut session = session.write().await;
            session.page.goto("data:text/html,<iframe srcdoc=\"<script>console.error('old-iframe-error')</script>\"></iframe>").await.unwrap();
            session.page.evaluate("console.error('old-page-error'); console.debug = () => { throw Error('page console override'); }; true").await.unwrap();
            session.console_task.take().unwrap().abort();
            session.console_capture = Arc::new(ConsoleCapture::default());
            session.console_logs.lock().unwrap().clear();
        }
        BrowserSession::setup_console_listener(session.clone())
            .await
            .unwrap();
        let (captured, fragment_captured, ordinary_logs) = {
            let session = session.read().await;
            let capture = session.begin_console_navigation_capture().await.unwrap();
            session
                .page
                .evaluate("frames[0].console.error('late-old-iframe-error'); true")
                .await
                .unwrap();
            let mut create_old_frame = EvaluateParams::new("new Promise(resolve => { const frame = document.createElement('iframe'); frame.srcdoc = `<script>console.error('late-created-old-iframe-error')</script>`; frame.onload = () => { frame.remove(); resolve(true); }; document.body.append(frame); })");
            create_old_frame.await_promise = Some(true);
            create_old_frame.return_by_value = Some(true);
            assert!(session
                .page
                .evaluate(create_old_frame)
                .await
                .unwrap()
                .into_value::<bool>()
                .unwrap());
            let url = "data:text/html,<script>console.error('new-page-error');console.debug=()=>{throw Error('new-page console override')};</script><iframe srcdoc=\"<script>console.error('new-iframe-error')</script>\"></iframe><iframe srcdoc=\"<script>console.error('removed-iframe-error');frameElement.remove()</script>\"></iframe>";
            session.page.goto(url).await.unwrap();
            let captured = session
                .finish_console_navigation_capture(capture)
                .await
                .unwrap();

            session
                .page
                .evaluate("console.error('before-fragment-error'); true")
                .await
                .unwrap();
            let fragment_capture = session.begin_console_navigation_capture().await.unwrap();
            session
                .page
                .evaluate("location.hash='fragment'; console.error('after-fragment-error'); true")
                .await
                .unwrap();
            let fragment_captured = session
                .finish_console_navigation_capture(fragment_capture)
                .await
                .unwrap();
            let ordinary_logs: Vec<_> = session
                .console_logs
                .lock()
                .unwrap()
                .iter()
                .map(|entry| entry.text.clone())
                .collect();
            (captured, fragment_captured, ordinary_logs)
        };
        manager.shutdown_all().await.expect("browser shutdown");
        let captured: Vec<_> = captured.iter().map(|entry| entry.text.as_str()).collect();
        assert!(captured.contains(&"new-page-error"), "{captured:?}");
        assert!(captured.contains(&"new-iframe-error"), "{captured:?}");
        assert!(captured.contains(&"removed-iframe-error"), "{captured:?}");
        assert!(!captured.contains(&"old-page-error"), "{captured:?}");
        assert!(!captured.contains(&"old-iframe-error"), "{captured:?}");
        assert!(!captured.contains(&"late-old-iframe-error"), "{captured:?}");
        assert!(
            !captured.contains(&"late-created-old-iframe-error"),
            "{captured:?}"
        );
        let fragment_captured: Vec<_> = fragment_captured
            .iter()
            .map(|entry| entry.text.as_str())
            .collect();
        assert_eq!(fragment_captured, ["after-fragment-error"]);
        assert_eq!(
            ordinary_logs.len(),
            9,
            "only application entries are retained"
        );
        for expected in [
            "old-page-error",
            "old-iframe-error",
            "late-old-iframe-error",
            "late-created-old-iframe-error",
            "new-page-error",
            "removed-iframe-error",
            "new-iframe-error",
            "before-fragment-error",
            "after-fragment-error",
        ] {
            assert!(
                ordinary_logs.iter().any(|entry| entry == expected),
                "{ordinary_logs:?}"
            );
        }
    }
    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "One browser and HTTP fixture establish real cross-process frame ownership"
    )]
    async fn native_console_capture_retains_detached_cross_origin_oopif_errors() {
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        if matches!(
            std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
            Ok("1" | "true")
        ) {
            return;
        }
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = server.local_addr().unwrap().port();
        let serving = tokio::spawn(async move {
            while let Ok((mut stream, _)) = server.accept().await {
                tokio::spawn(async move {
                    let mut request = vec![0; 8192];
                    let Ok(count) = stream.read(&mut request).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&request[..count]);
                    let path = request.split_whitespace().nth(1).unwrap_or("/");
                    let body = if path.starts_with("/child") {
                        let label = path.trim_start_matches("/child-");
                        format!("<script>console.error('{label}-oopif-error');parent.postMessage('{label}', '*')</script>")
                    } else {
                        let label = if path == "/new" { "new" } else { "old" };
                        format!("<script>window.seen=[];addEventListener('message',e=>seen.push(e.data))</script><iframe src='http://localhost:{port}/child-{label}'></iframe>")
                    };
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        let manager = crate::BrowserSessionManager::new();
        let scope = ResourceScopeKey::Work(
            WorkScopeId::parse(format!("console-oopif-{}", uuid::Uuid::new_v4())).unwrap(),
        );
        let session = manager.get_session(&scope).await.unwrap();
        let (captured, attached_capture) = {
            let session = session.read().await;
            session
                .page
                .goto(format!("http://127.0.0.1:{port}/old"))
                .await
                .unwrap();
            assert!(
                !session
                    .console_capture
                    .state
                    .lock()
                    .unwrap()
                    .children
                    .is_empty(),
                "cross-site iframe must be a separately attached CDP target"
            );
            let capture = session.begin_console_navigation_capture().await.unwrap();
            let mut late = EvaluateParams::new(format!("new Promise(resolve=>{{const f=document.createElement('iframe');f.src='http://localhost:{port}/child-late-old';f.onload=()=>resolve(true);document.body.append(f)}})"));
            late.await_promise = Some(true);
            session.page.evaluate(late).await.unwrap();
            session
                .page
                .goto(format!("http://127.0.0.1:{port}/new"))
                .await
                .unwrap();
            assert!(
                !session
                    .console_capture
                    .state
                    .lock()
                    .unwrap()
                    .children
                    .is_empty(),
                "new document must also have an OOPIF target"
            );
            session
                .page
                .evaluate("document.querySelector('iframe').remove();true")
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let changed = session.console_event.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
                    if session
                        .console_capture
                        .state
                        .lock()
                        .unwrap()
                        .children
                        .is_empty()
                    {
                        break;
                    }
                    changed.await;
                }
            })
            .await
            .expect("OOPIF detached from console connection");
            let detached_capture = session
                .finish_console_navigation_capture(capture)
                .await
                .unwrap();
            let capture = session.begin_console_navigation_capture().await.unwrap();
            session
                .page
                .goto(format!("http://127.0.0.1:{port}/new"))
                .await
                .unwrap();
            let attached_capture = session
                .finish_console_navigation_capture(capture)
                .await
                .unwrap();
            (detached_capture, attached_capture)
        };
        manager.shutdown_all().await.unwrap();
        serving.abort();
        let captured: Vec<_> = captured.iter().map(|entry| entry.text.as_str()).collect();
        assert!(captured.contains(&"new-oopif-error"), "{captured:?}");
        assert!(
            attached_capture
                .iter()
                .any(|entry| entry.text == "new-oopif-error"),
            "{attached_capture:?}"
        );
        assert!(!captured.contains(&"old-oopif-error"), "{captured:?}");
        assert!(!captured.contains(&"late-old-oopif-error"), "{captured:?}");
    }
    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "One browser lifetime verifies repeated nested-process navigation and retained history"
    )]
    async fn native_console_capture_survives_repeated_nested_oopif_navigation() {
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        if matches!(
            std::env::var("PHOENIX_SKIP_BROWSER_TESTS").as_deref(),
            Ok("1" | "true")
        ) {
            return;
        }
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = server.local_addr().unwrap().port();
        let serving = tokio::spawn(async move {
            while let Ok((mut stream, _)) = server.accept().await {
                tokio::spawn(async move {
                    let mut request = vec![0; 8192];
                    let Ok(count) = stream.read(&mut request).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&request[..count]);
                    let path = request.split_whitespace().nth(1).unwrap_or("/");
                    let script = format!("<script>console.error('{path}')</script>");
                    let body = if path.starts_with("/leaf") {
                        script
                    } else if path.starts_with("/child") {
                        format!(
                            "{script}<iframe src='http://127.0.0.1:{port}/leaf{path}'></iframe>"
                        )
                    } else {
                        format!(
                            "{script}<iframe src='http://localhost:{port}/child{path}'></iframe>"
                        )
                    };
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        let manager = crate::BrowserSessionManager::new();
        let scope = ResourceScopeKey::Work(
            WorkScopeId::parse(format!("nested-console-{}", uuid::Uuid::new_v4())).unwrap(),
        );
        let session = manager.get_session(&scope).await.unwrap();
        let mut captures = Vec::new();
        {
            let session = session.read().await;
            for path in ["old", "new", "third"] {
                let capture = session.begin_console_navigation_capture().await.unwrap();
                session
                    .page
                    .goto(format!("http://127.0.0.1:{port}/{path}"))
                    .await
                    .unwrap();
                assert!(
                    session
                        .console_capture
                        .state
                        .lock()
                        .unwrap()
                        .children
                        .values()
                        .any(|child| child.parent.is_some()),
                    "fixture must attach a nested OOPIF session"
                );
                captures.push((
                    path,
                    session
                        .finish_console_navigation_capture(capture)
                        .await
                        .unwrap(),
                ));
            }
            assert!(session
                .console_capture
                .state
                .lock()
                .unwrap()
                .failure
                .is_none());
            for path in ["/old", "/child/old", "/leaf/child/old"] {
                assert!(
                    session
                        .console_logs
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|entry| entry.text == path),
                    "prior document history must survive descendant retirement"
                );
            }
        }
        manager.shutdown_all().await.unwrap();
        serving.abort();
        for (path, captured) in captures {
            let texts: Vec<_> = captured.iter().map(|entry| entry.text.as_str()).collect();
            for expected in [
                format!("/{path}"),
                format!("/child/{path}"),
                format!("/leaf/child/{path}"),
            ] {
                assert!(
                    texts.contains(&expected.as_str()),
                    "expected {expected}, got {texts:?}"
                );
            }
            assert_eq!(texts.len(), 3, "exclude prior nested documents: {texts:?}");
        }
    }
}
