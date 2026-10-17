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
    let application = ObservationApplication::for_target(db.clone(), reference).await?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let address = listener.local_addr()?;
    let app = create_router(ApplicationState::Observation(application))
        .layer(CompressionLayer::new().gzip(true).br(true));
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
