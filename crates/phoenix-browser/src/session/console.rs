use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chromiumoxide::cdp::js_protocol::runtime::{
    ConsoleApiCalledType, EvaluateParams, EventConsoleApiCalled, ExecutionContextId,
};
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
    acknowledged: oneshot::Sender<()>,
}

#[derive(Default)]
struct State {
    generation: Option<Generation>,
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
    ) -> (Registration, oneshot::Receiver<()>) {
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
                if let Boundary::Begin(generation) = pending.boundary {
                    state.generation = Some(generation);
                }
                let _ = pending.acknowledged.send(());
                return;
            }
            state.generation
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
            source_context: event.execution_context_id,
            capture_generation: generation,
        });
        notification.notify_one();
    }
}

fn capture_error(message: impl std::fmt::Display) -> BrowserError {
    BrowserError::OperationFailed(format!(
        "Could not fence navigation console capture: {message}"
    ))
}

impl BrowserSession {
    async fn console_boundary(&self, boundary: Boundary) -> Result<(), BrowserError> {
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
            acknowledged.await.map_err(capture_error)?;
            Ok(())
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
        self.console_boundary(Boundary::Begin(generation)).await?;
        Ok(ConsoleNavigationCapture {
            owner: self.console_capture.clone(),
            generation,
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
        self.console_boundary(Boundary::Drain).await?;
        let context = self
            .page
            .execution_context()
            .await?
            .ok_or_else(|| capture_error("main execution context unavailable"))?;
        let mut contexts = HashSet::from([context]);
        for frame in self.page.frames().await? {
            if let Some(context) = self.page.frame_execution_context(frame).await? {
                contexts.insert(context);
            }
        }
        let logs = self
            .console_logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(logs
            .iter()
            .filter(|entry| {
                entry.capture_generation == Some(capture.generation)
                    && contexts.contains(&entry.source_context)
            })
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(context: i64, text: &str, kind: &str) -> EventConsoleApiCalled {
        serde_json::from_value(json!({"type":kind,"args":[{"type":"string","value":text}],"executionContextId":context,"timestamp":1})).unwrap()
    }

    #[tokio::test]
    async fn source_markers_fence_a_backlogged_listener_and_exclude_prior_documents() {
        let capture = Arc::new(ConsoleCapture::default());
        let logs = Mutex::new(VecDeque::new());
        let notification = Notify::new();
        let generation = Generation(uuid::Uuid::new_v4());
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
        begin_ack.await.unwrap();
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
        let contexts = HashSet::from([ExecutionContextId::new(2), ExecutionContextId::new(3)]);
        let selected: Vec<_> = logs
            .iter()
            .filter(|entry| {
                entry.capture_generation == Some(generation)
                    && contexts.contains(&entry.source_context)
            })
            .map(|entry| entry.text.as_str())
            .collect();
        assert_eq!(selected, ["new navigation error", "new iframe error"]);
        assert_eq!(logs.len(), 4, "internal markers are not console entries");
    }

    #[tokio::test]
    async fn same_document_navigation_excludes_events_before_its_source_marker() {
        let capture = Arc::new(ConsoleCapture::default());
        let logs = Mutex::new(VecDeque::new());
        let notification = Notify::new();
        let generation = Generation(uuid::Uuid::new_v4());
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
        let (captured, fragment_captured, ordinary_logs) = {
            let session = session.read().await;
            session.page.goto("about:blank").await.unwrap();
            session.page.evaluate("console.error('old-page-error'); console.debug = () => { throw Error('page console override'); }; true").await.unwrap();
            let capture = session.begin_console_navigation_capture().await.unwrap();
            let url = "data:text/html,<script>console.error('new-page-error');console.debug=()=>{throw Error('new-page console override')};</script><iframe srcdoc=\"<script>console.error('new-iframe-error')</script>\"></iframe>";
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
        assert!(!captured.contains(&"old-page-error"), "{captured:?}");
        let fragment_captured: Vec<_> = fragment_captured
            .iter()
            .map(|entry| entry.text.as_str())
            .collect();
        assert_eq!(fragment_captured, ["after-fragment-error"]);
        assert_eq!(
            ordinary_logs.len(),
            5,
            "only application entries are retained"
        );
        for expected in [
            "old-page-error",
            "new-page-error",
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
