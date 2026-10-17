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
                metadata.is_span()
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
                    )
            })),
    )
}

fn with_attribution_dispatch(app: axum::Router, dispatch: tracing::Dispatch) -> axum::Router {
    app.layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            next.run(request).with_subscriber(dispatch.clone())
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
