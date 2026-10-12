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
use tracing::Instrument;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

type Error = Box<dyn std::error::Error + Send + Sync>;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Serve supported observations without loading live configuration or services.
///
/// # Errors
/// Returns an error for an inadmissible fixture, unsupported reads, target,
/// listener failure, or shutdown that exceeds the bounded drain.
pub async fn run(fixture: &Path, reference: &str, port: u16) -> Result<(), Error> {
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
    tracing_subscriber::registry()
        .with(ElapsedLayer)
        .try_init()?;
    let db =
        Database::open_read_only(canonical.to_str().ok_or("fixture path must be UTF-8")?).await?;
    let result = serve(&db, reference, port, shutdown).await;
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
) -> Result<(), Error> {
    // This performs the exact resolver, aggregate and row decodes required by the routes.
    let application = ObservationApplication::for_target(db.clone(), reference).await?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let address = listener.local_addr()?;
    let app = create_router(ApplicationState::Observation(application))
        .layer(CompressionLayer::new().gzip(true).br(true))
        .layer(axum::middleware::from_fn(timed_request));
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

#[derive(Default)]
struct SafeFields(std::collections::BTreeMap<String, serde_json::Value>);
impl tracing::field::Visit for SafeFields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if matches!(
            field.name(),
            "request_id" | "kind" | "limit" | "has_before" | "has_open"
        ) {
            self.0.insert(
                field.name().to_owned(),
                serde_json::Value::String(format!("{value:?}")),
            );
        }
    }
}
struct ElapsedSpan {
    start: std::time::Instant,
    fields: SafeFields,
    request_id: Option<String>,
}
struct ElapsedLayer;
impl<S> tracing_subscriber::Layer<S> for ElapsedLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let span = ctx.span(id).expect("registered span");
        let mut fields = SafeFields::default();
        attrs.record(&mut fields);
        let request_id = fields
            .0
            .get("request_id")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .or_else(|| {
                span.parent().and_then(|p| {
                    p.extensions()
                        .get::<ElapsedSpan>()
                        .and_then(|s| s.request_id.clone())
                })
            });
        span.extensions_mut().insert(ElapsedSpan {
            start: std::time::Instant::now(),
            fields,
            request_id,
        });
    }
    fn on_close(&self, id: tracing::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let span = ctx.span(&id).expect("closing span");
        let name = span.name();
        if !(name.starts_with("replay.") || name.starts_with("product_conversation.")) {
            return;
        }
        let ext = span.extensions();
        let timing = ext.get::<ElapsedSpan>().expect("elapsed span");
        if timing.request_id.is_none() {
            return;
        }
        eprintln!(
            "REPLAY_TIMING {}",
            serde_json::json!({"request_id": timing.request_id, "stage":name,"elapsed_ms":timing.start.elapsed().as_secs_f64()*1000.0,"fields":timing.fields.0})
        );
    }
}
async fn timed_request(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let kind = if request
        .uri()
        .path()
        .starts_with("/api/product-conversations/")
    {
        "detail"
    } else if request.uri().path().ends_with("/observation") {
        "member"
    } else {
        "other"
    };
    let query = request.uri().query().unwrap_or_default();
    let parts = query
        .split('&')
        .filter_map(|p| p.split_once('='))
        .collect::<std::collections::BTreeMap<_, _>>();
    let limit = parts
        .get("message_limit")
        .or_else(|| parts.get("limit"))
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(if kind == "member" { 50 } else { 100 });
    let span = tracing::info_span!("replay.request", request_id = %uuid::Uuid::new_v4(), kind, limit, has_before=parts.contains_key("before"),has_open=parts.contains_key("open_id"));
    next.run(request).instrument(span).await
}

#[cfg(test)]
mod timing_tests {
    use super::*;
    use tracing_subscriber::registry::LookupSpan;
    #[test]
    fn timing_fields_drop_payloads_and_inherit_safe_request_identity() {
        let subscriber = tracing_subscriber::registry().with(ElapsedLayer);
        tracing::subscriber::with_default(subscriber, || {
            let request = tracing::info_span!(
                "replay.request",
                request_id = "safe-id",
                limit = 1,
                payload = "secret",
                headers = "secret"
            );
            let _guard = request.enter();
            let child = tracing::info_span!("replay.page.clone", limit = 1, content = "secret");
            tracing::dispatcher::get_default(|dispatch| {
                let registry =
                    dispatch
                        .downcast_ref::<tracing_subscriber::layer::Layered<
                            ElapsedLayer,
                            tracing_subscriber::Registry,
                        >>()
                        .unwrap();
                let span = registry.span(&child.id().unwrap()).unwrap();
                let ext = span.extensions();
                let state = ext.get::<ElapsedSpan>().unwrap();
                assert_eq!(state.request_id.as_deref(), Some("\"safe-id\""));
                assert_eq!(state.fields.0.len(), 1);
                assert!(state.fields.0.contains_key("limit"));
            });
        });
    }
}
