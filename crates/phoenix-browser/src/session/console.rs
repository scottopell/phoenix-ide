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
use tokio::sync::{oneshot, Notify, OwnedMutexGuard};

use super::{
    extract_console_arg_text, BrowserError, BrowserSession, ConsoleEntry, MAX_CONSOLE_LOGS,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Generation(uuid::Uuid);

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
    marker_context: String,
    contexts: HashSet<String>,
}

#[derive(Default)]
struct State {
    contexts: HashMap<ExecutionContextId, String>,
    generation: Option<Generation>,
    failure: Option<String>,
    pending: HashMap<String, Pending>,
}

#[derive(Default)]
pub(super) struct ConsoleCapture {
    state: Mutex<State>,
    navigation: Arc<tokio::sync::Mutex<()>>,
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

    pub(super) fn accept(
        &self,
        event: &EventConsoleApiCalled,
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
                state
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
                if let Some(marker_context) = state.contexts.get(&event.execution_context_id) {
                    let _ = pending.acknowledged.send(ContextBoundary {
                        marker_context: marker_context.clone(),
                        contexts: state.contexts.values().cloned().collect(),
                    });
                } else {
                    state.failure =
                        Some("source marker arrived from an unknown execution context".to_owned());
                }
                return;
            }
            if !state.contexts.contains_key(&event.execution_context_id) {
                state.failure = Some(format!(
                    "console event from unknown execution context {:?}",
                    event.execution_context_id
                ));
                tracing::warn!(failure = ?state.failure, "console capture lost context ownership");
            }
            (
                state.generation,
                state.contexts.get(&event.execution_context_id).cloned(),
            )
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
            source_context: generation.1,
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
    fn observe(
        &self,
        message: &Value,
        logs: &Mutex<VecDeque<ConsoleEntry>>,
        notification: &Notify,
    ) -> Result<(), BrowserError> {
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match message.get("method").and_then(Value::as_str) {
            Some("Runtime.executionContextCreated") => {
                let event: EventExecutionContextCreated =
                    serde_json::from_value(params).map_err(capture_error)?;
                self.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contexts
                    .insert(event.context.id, event.context.unique_id);
            }
            Some("Runtime.executionContextDestroyed") => {
                let event: EventExecutionContextDestroyed =
                    serde_json::from_value(params).map_err(capture_error)?;
                self.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contexts
                    .retain(|_, unique| *unique != event.execution_context_unique_id);
            }
            Some("Runtime.executionContextsCleared") => {
                self.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contexts
                    .clear();
            }
            Some("Runtime.consoleAPICalled") => {
                let event: EventConsoleApiCalled =
                    serde_json::from_value(params).map_err(capture_error)?;
                self.accept(&event, logs, notification);
            }
            _ => {}
        }
        Ok(())
    }

    async fn command(
        &self,
        socket: &mut Socket,
        request: Value,
        logs: &Mutex<VecDeque<ConsoleEntry>>,
        notification: &Notify,
    ) -> Result<Value, BrowserError> {
        socket
            .send(Message::Text(request.to_string().into()))
            .await
            .map_err(capture_error)?;
        while let Some(message) = socket.next().await {
            let message = message.map_err(capture_error)?;
            if let Message::Text(text) = message {
                let message: Value = serde_json::from_str(&text).map_err(capture_error)?;
                self.observe(&message, logs, notification)?;
                if message.get("id") == request.get("id") {
                    if let Some(error) = message.get("error") {
                        return Err(capture_error(error));
                    }
                    return Ok(message["result"].clone());
                }
            } else if matches!(message, Message::Close(_)) {
                break;
            }
        }
        Err(capture_error("console connection ended"))
    }
}

pub(super) async fn listen(
    session: &BrowserSession,
) -> Result<tokio::task::JoinHandle<()>, BrowserError> {
    let capture = session.console_capture.clone();
    let logs = session.console_logs.clone();
    let notification = session.console_event.clone();
    let mut socket = tokio::time::timeout(Duration::from_secs(5), async {
        let (mut socket, session_id) = crate::cdp::connect_target(
            session.snapshot_websocket_address(),
            session.page.target_id(),
            8 * 1024 * 1024,
        )
        .await
        .map_err(capture_error)?;
        capture
            .command(
                &mut socket,
                json!({"id":2,"sessionId":session_id,"method":"Runtime.enable"}),
                &logs,
                &notification,
            )
            .await?;
        Ok::<_, BrowserError>(socket)
    })
    .await
    .map_err(|_| capture_error("console connection setup timed out"))??;
    Ok(tokio::spawn(async move {
        let result = async {
            while let Some(message) = socket.next().await {
                let message = message.map_err(capture_error)?;
                if let Message::Text(text) = message {
                    let message: Value = serde_json::from_str(&text).map_err(capture_error)?;
                    capture.observe(&message, &logs, &notification)?;
                } else if matches!(message, Message::Close(_)) {
                    break;
                }
            }
            Ok::<_, BrowserError>(())
        }
        .await;
        let failure = result.err().map_or_else(
            || "console connection ended".to_owned(),
            |error| error.to_string(),
        );
        tracing::warn!(%failure, "console capture stopped");
        let mut state = capture
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

    /// Drain earlier console events before starting a navigation's capture.
    ///
    /// # Errors
    /// Returns an error when the isolated source marker cannot be acknowledged.
    pub async fn begin_console_navigation_capture(
        &self,
    ) -> Result<ConsoleNavigationCapture, BrowserError> {
        let lease = self.console_capture.navigation.clone().lock_owned().await;
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
        let same_document = capture.boundary.marker_context == final_boundary.marker_context;
        let logs = self
            .console_logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(logs
            .iter()
            .filter(|entry| {
                entry.capture_generation == Some(capture.generation)
                    && entry.source_context.as_ref().is_some_and(|context| {
                        same_document || !capture.boundary.contexts.contains(context)
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
        let contexts = &initial.contexts;
        let selected: Vec<_> = logs
            .iter()
            .filter(|entry| {
                entry.capture_generation == Some(generation)
                    && entry
                        .source_context
                        .as_ref()
                        .is_some_and(|context| !contexts.contains(context))
            })
            .map(|entry| entry.text.as_str())
            .collect();
        assert_eq!(selected, ["new navigation error", "new iframe error"]);
        assert_eq!(logs.len(), 4, "internal markers are not console entries");
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
                        .source_context
                        .as_ref()
                        .is_some_and(|context| !initial.contexts.contains(context))
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
        let fragment_captured: Vec<_> = fragment_captured
            .iter()
            .map(|entry| entry.text.as_str())
            .collect();
        assert_eq!(fragment_captured, ["after-fragment-error"]);
        assert_eq!(
            ordinary_logs.len(),
            8,
            "only application entries are retained"
        );
        for expected in [
            "old-page-error",
            "old-iframe-error",
            "late-old-iframe-error",
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
}
