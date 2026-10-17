//! Immutable, target-scoped observation startup for the normal Phoenix binary.

use crate::api::{
    application_state::{ApplicationState, ObservationApplication},
    create_router,
};
use crate::db::Database;
use crate::immutable_fixture::standalone_path;
use std::{net::SocketAddr, path::Path, time::Duration};
use tokio::net::TcpListener;
use tower_http::compression::CompressionLayer;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::{layer::SubscriberExt, Layer};

fn attribution_subscriber<W>(writer: W) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_ansi(false)
            .with_writer(writer)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                (metadata.is_event()
                    && metadata.target() == "phoenix::observation::completion"
                    && matches!(
                        metadata.name(),
                        "observation.stage_complete" | "observation.request_complete"
                    ))
                    || (metadata.is_span()
                        && matches!(
                            metadata.name(),
                            "observation.request"
                                | "observation.authorization"
                                | "replay.admission"
                                | "observation.resolve_ordinary"
                                | "observation.get_aggregate"
                                | "observation.require_member"
                                | "observation.member.resolve_ordinary"
                                | "observation.member.get_aggregate"
                                | "observation.db.product"
                                | "observation.db.topology"
                                | "observation.db.read_member"
                                | "observation.db.handoff"
                                | "observation.db.source"
                                | "product_conversation.detail_get"
                                | "product_conversation.post_snapshot_projection"
                                | "product_conversation.pool_wait"
                                | "product_conversation.begin_read"
                                | "product_conversation.resolve"
                                | "product_conversation.aggregate"
                                | "product_conversation.close"
                                | "product_conversation.message_page"
                                | "product_conversation.snapshot_read"
                                | "product_conversation.rollback_read"
                        ))
            })),
    )
}

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    Authorization,
    Admission,
    ResolveOrdinary,
    GetAggregate,
    RequireMember,
    MemberResolveOrdinary,
    MemberGetAggregate,
    Detail,
    Projection,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Authorization => "observation.authorization",
            Self::Admission => "replay.admission",
            Self::ResolveOrdinary => "observation.resolve_ordinary",
            Self::GetAggregate => "observation.get_aggregate",
            Self::RequireMember => "observation.require_member",
            Self::MemberResolveOrdinary => "observation.member.resolve_ordinary",
            Self::MemberGetAggregate => "observation.member.get_aggregate",
            Self::Detail => "product_conversation.detail_get",
            Self::Projection => "product_conversation.post_snapshot_projection",
        }
    }
}

pub(crate) struct StageCompletion {
    stage: Stage,
    started: std::time::Instant,
    dispatch: tracing::Dispatch,
    parent: tracing::Span,
    finished: bool,
}

impl StageCompletion {
    pub(crate) fn new(stage: Stage) -> Self {
        Self {
            stage,
            started: std::time::Instant::now(),
            dispatch: tracing::dispatcher::get_default(Clone::clone),
            parent: tracing::Span::current(),
            finished: false,
        }
    }

    pub(crate) fn complete<T, E>(mut self, result: &Result<T, E>) {
        self.emit(if result.is_ok() { "success" } else { "error" });
    }

    fn emit(&mut self, outcome: &'static str) {
        self.finished = true;
        let elapsed_us = u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX);
        tracing::dispatcher::with_default(&self.dispatch, || {
            self.parent.in_scope(|| {
                tracing::event!(name: "observation.stage_complete", target: "phoenix::observation::completion",
                    parent: &self.parent, tracing::Level::INFO,
                    stage = self.stage.name(), outcome, elapsed_us);
            });
        });
    }
}

impl Drop for StageCompletion {
    fn drop(&mut self) {
        if !self.finished {
            self.emit("cancel");
        }
    }
}

pub(crate) async fn complete_stage<T, E>(
    stage: Stage,
    future: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let completion = StageCompletion::new(stage);
    let result = future.await;
    completion.complete(&result);
    result
}

pub(crate) struct RequestCompletion {
    request_id: u64,
    started: std::time::Instant,
    dispatch: tracing::Dispatch,
    parent: tracing::Span,
    finished: bool,
}

impl RequestCompletion {
    pub(crate) fn new(request_id: u64) -> Self {
        Self {
            request_id,
            started: std::time::Instant::now(),
            dispatch: tracing::dispatcher::get_default(Clone::clone),
            parent: tracing::Span::current(),
            finished: false,
        }
    }

    pub(crate) fn response_ready(mut self, status: axum::http::StatusCode) {
        let outcome = if status.is_client_error() || status.is_server_error() {
            "error"
        } else {
            "success"
        };
        self.emit(outcome, Some(status.as_u16()));
    }

    fn emit(&mut self, outcome: &'static str, status: Option<u16>) {
        self.finished = true;
        let elapsed_us = u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX);
        tracing::dispatcher::with_default(&self.dispatch, || {
            self.parent.in_scope(|| {
                tracing::event!(name: "observation.request_complete", target: "phoenix::observation::completion",
                    parent: &self.parent, tracing::Level::INFO,
                    stage = "observation.request", request_id = self.request_id,
                    outcome, elapsed_us, status, response_ready = status.is_some());
            });
        });
    }
}

impl Drop for RequestCompletion {
    fn drop(&mut self) {
        if !self.finished {
            self.emit("cancel", None);
        }
    }
}

struct AttributionRequest {
    future: Option<
        std::pin::Pin<Box<dyn std::future::Future<Output = axum::response::Response> + Send>>,
    >,
    dispatch: tracing::Dispatch,
}

impl std::future::Future for AttributionRequest {
    type Output = axum::response::Response;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.get_mut();
        tracing::dispatcher::with_default(&this.dispatch, || {
            this.future.as_mut().unwrap().as_mut().poll(cx)
        })
    }
}

impl Drop for AttributionRequest {
    fn drop(&mut self) {
        tracing::dispatcher::with_default(&self.dispatch, || drop(self.future.take()));
    }
}

fn with_attribution_dispatch(app: axum::Router, dispatch: tracing::Dispatch) -> axum::Router {
    app.layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| AttributionRequest {
            future: Some(Box::pin(next.run(request))),
            dispatch: dispatch.clone(),
        },
    ))
}

type Error = Box<dyn std::error::Error + Send + Sync>;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Serve supported observations without loading live configuration or services.
///
/// # Errors
/// Returns an error for an inadmissible fixture, unsupported reads, target,
/// listener failure, or shutdown that exceeds the bounded drain.
pub async fn run(fixture: &Path, reference: &str, port: u16) -> Result<(), Error> {
    let dispatch = tracing::Dispatch::new(attribution_subscriber(std::io::stderr));
    run_with_attribution(fixture, reference, port, dispatch.clone())
        .with_subscriber(dispatch)
        .await
}

async fn run_with_attribution(
    fixture: &Path,
    reference: &str,
    port: u16,
    dispatch: tracing::Dispatch,
) -> Result<(), Error> {
    use tokio::signal::unix::{signal, SignalKind};
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let shutdown = async move {
        tokio::select! {
            _ = terminate.recv() => {},
            _ = interrupt.recv() => {},
        }
    };
    let canonical = standalone_path(fixture)?;
    let db =
        Database::open_read_only(canonical.to_str().ok_or("fixture path must be UTF-8")?).await?;
    let result = serve(&db, reference, port, shutdown, dispatch).await;
    let closed = tokio::time::timeout(SHUTDOWN_GRACE, db.pool().close()).await;
    result?;
    closed.map_err(|_| "observation database close exceeded shutdown deadline")?;
    Ok(())
}

async fn serve(
    db: &Database,
    reference: &str,
    port: u16,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    dispatch: tracing::Dispatch,
) -> Result<(), Error> {
    let application = ObservationApplication::for_target(db.clone(), reference).await?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let address = listener.local_addr()?;
    let app = with_attribution_dispatch(
        create_router(ApplicationState::Observation(application))
            .layer(CompressionLayer::new().gzip(true).br(true)),
        dispatch,
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = shutdown_rx.await;
    });
    let server = std::future::IntoFuture::into_future(server);
    tokio::pin!(server);
    println!("Observation listening on http://{address}");
    tokio::select! {
        result = &mut server => result?,
        () = shutdown => {
            let _ = shutdown_tx.send(());
            tokio::time::timeout(SHUTDOWN_GRACE, &mut server).await
                .map_err(|_| "observation HTTP drain exceeded shutdown deadline")??;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "observation_startup/attribution_tests.rs"]
mod attribution_tests;
