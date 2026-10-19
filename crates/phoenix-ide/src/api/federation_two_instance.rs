use std::sync::Arc;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use phoenix_core::domain::instance_identity::{
    FederationCredentialVerifier, InstanceId, PeerBaseUrl, PeerBearerCredential,
    PeerCaCertificatePem, PeerTlsTrust,
};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

use super::handlers::{create_router, hard_delete_cascade_tests::make_test_state};

mod send;
mod smoke;

struct TestInstance {
    _tempdir: tempfile::TempDir,
    state: super::AppState,
    origin: PeerBaseUrl,
    trust: PeerTlsTrust,
    client: reqwest::Client,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl TestInstance {
    async fn start() -> Self {
        Self::start_with_search_ready(true).await
    }

    async fn start_with_search_ready(search_ready: bool) -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let mut tls = crate::tls::load_config(&crate::tls::ConfigSource::Auto {
            dir: tempdir.path().to_path_buf(),
            hosts: vec!["localhost".to_string()],
        })
        .unwrap();
        tls.server.alpn_protocols = vec![b"http/1.1".to_vec()];
        let certificate_pem = PeerCaCertificatePem::parse(
            std::fs::read_to_string(tls.ca_cert_path.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        let trust = PeerTlsTrust::PrivateCa { certificate_pem };
        let client = match &trust {
            PeerTlsTrust::PrivateCa { certificate_pem } => reqwest::Client::builder()
                .tls_certs_only([reqwest::Certificate::from_pem(
                    certificate_pem.expose().as_bytes(),
                )
                .unwrap()])
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            PeerTlsTrust::PlatformRoots => unreachable!("test fixture always uses its private CA"),
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin: PeerBaseUrl = format!("https://localhost:{}", address.port())
            .parse()
            .unwrap();
        let mut state = make_test_state().await;
        state.db = crate::db::Database::open(tempdir.path().join("instance.db").to_str().unwrap())
            .await
            .unwrap();
        crate::db::run_pending_migrations(state.db.pool())
            .await
            .unwrap();
        state.runtime = Arc::new(crate::runtime::RuntimeManager::new(
            state.db.clone(),
            state.llm_registry.clone(),
            state.platform.clone(),
            state.mcp_manager.clone(),
            None,
        ));
        state.terminals = state.runtime.terminals.clone();
        state.sessions = super::auth::SessionStore::new(state.db.clone(), String::new());
        state.message_retriever = Arc::new(state.db.fts_retriever());
        if search_ready {
            let retriever = state.db.fts_retriever();
            retriever.reconcile().await.unwrap();
            state.message_retriever = Arc::new(retriever);
        }
        state.chain_qa = crate::chain_qa::ChainQa::new(
            state.db.clone(),
            state.llm_registry.clone(),
            state.message_retriever.clone(),
        );
        state
            .db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let app = create_router(state.clone());
        let shutdown = CancellationToken::new();
        let server = spawn_https(listener, app, tls.server, shutdown.clone()).await;

        Self {
            _tempdir: tempdir,
            state,
            origin,
            trust,
            client,
            shutdown,
            server,
        }
    }

    async fn enroll(&self, caller_instance_id: InstanceId) -> PeerBearerCredential {
        let credential = PeerBearerCredential::parse(format!(
            "phx_peer_{}aaaaaaaaaaa",
            caller_instance_id.to_string().replace('-', "")
        ))
        .unwrap();
        let verifier = FederationCredentialVerifier::from_bearer(credential.expose().as_bytes());
        self.state
            .db
            .replace_federation_enrollment(caller_instance_id, "isolated test caller", &verifier)
            .await
            .unwrap();
        credential
    }

    async fn connect_to(&self, receiver: &Self) -> phoenix_db::FederationPeerConnection {
        let caller_id = self.state.db.instance_id().await.unwrap();
        let credential = receiver.enroll(caller_id).await;
        self.save_connection(receiver, &credential).await
    }

    async fn save_connection(
        &self,
        receiver: &Self,
        credential: &PeerBearerCredential,
    ) -> phoenix_db::FederationPeerConnection {
        let destination = receiver.state.db.instance_id().await.unwrap();
        self.state
            .db
            .save_federation_peer_connection(
                destination,
                "isolated receiver",
                &receiver.origin,
                credential,
                &receiver.trust,
            )
            .await
            .unwrap();
        self.state
            .db
            .federation_peer_connection(destination)
            .await
            .unwrap()
            .unwrap()
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.server.await.unwrap();
    }
}

async fn spawn_https(
    listener: TcpListener,
    app: Router,
    config: rustls::ServerConfig,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        ready_tx.send(()).unwrap();
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                accepted = listener.accept() => {
                    let (stream, peer) = accepted.unwrap();
                    let acceptor = acceptor.clone();
                    let app = app.clone();
                    connections.spawn(async move {
                        let stream = acceptor.accept(stream).await.unwrap();
                        let service = hyper_util::service::TowerToHyperService::new(app);
                        hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                            .serve_connection_with_upgrades(TokioIo::new(stream), service)
                            .await
                            .unwrap_or_else(|error| panic!("test HTTPS connection from {peer} failed: {error}"));
                    });
                }
            }
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    });
    ready_rx.await.unwrap();
    server
}
