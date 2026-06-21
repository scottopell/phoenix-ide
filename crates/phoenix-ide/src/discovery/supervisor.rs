use super::probe::{client, probe, ProbeTarget};
use super::{DiscoveryConfig, DiscoveryRegistry};
use futures::{stream, StreamExt};
use rand::RngExt as _;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

pub fn start(config: DiscoveryConfig, registry: Arc<DiscoveryRegistry>) {
    tokio::spawn(async move {
        let client = match client(&config) {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(%error, "local service discovery disabled; failed to build HTTP client");
                return;
            }
        };

        tracing::debug!(
            ports = config.ports.len(),
            "local service discovery started"
        );
        loop {
            run_sweep(&client, &config, &registry, SweepMode::All).await;
            let mut slept = Duration::ZERO;
            while slept < config.sweep_interval {
                sleep_with_jitter(config.recent_interval).await;
                slept += config.recent_interval;
                run_sweep(&client, &config, &registry, SweepMode::RecentOnly).await;
            }
        }
    });
}

#[derive(Debug, Clone, Copy)]
enum SweepMode {
    All,
    RecentOnly,
}

async fn run_sweep(
    client: &reqwest::Client,
    config: &DiscoveryConfig,
    registry: &Arc<DiscoveryRegistry>,
    mode: SweepMode,
) {
    let mut targets = BTreeSet::new();
    for host in &config.hosts {
        if matches!(mode, SweepMode::All) {
            for port in &config.ports {
                targets.insert(ProbeTarget {
                    host: *host,
                    port: *port,
                });
            }
        }
        for port in registry.recent_ports() {
            targets.insert(ProbeTarget { host: *host, port });
        }
    }

    stream::iter(targets)
        .for_each_concurrent(config.max_concurrent_probes, |target| async move {
            match probe(client, config, target).await {
                Ok(service) => registry.observe(service),
                Err(error) => tracing::debug!(%error, host = %target.host, port = target.port, "local service discovery probe failed"),
            }
        })
        .await;

    registry.refresh_lifecycle();
}

async fn sleep_with_jitter(base: Duration) {
    let millis = base.as_millis();
    let jitter_max = u64::try_from((millis / 5).max(1)).unwrap_or(u64::MAX);
    let jitter = rand::rng().random_range(0..=jitter_max);
    tokio::time::sleep(base + Duration::from_millis(jitter)).await;
}
