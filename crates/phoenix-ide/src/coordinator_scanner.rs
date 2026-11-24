use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use futures::FutureExt;
use phoenix_db::{
    coordinator_scanner::{
        ReservedScanAttempt, ScanApplyOutcome, ScanConnection, ScanFailure, ScanMetrics,
        ScanPreparation, ScanProfile, ScanReservation, ScanResult, ScanUsage,
    },
    workflow::LocalAuthorityResult,
};
use phoenix_llm::{LlmResponse, LlmService};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::runtime::{AdmittedOperation, RuntimeManager};

mod model;
mod prompt;
#[cfg(test)]
mod quality;
#[cfg(test)]
mod tests;

const REVISIT_INTERVAL: Duration = Duration::from_secs(30);
const PASS_PACE: Duration = Duration::from_secs(1);
const KICK_COALESCE: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct ScannerWake {
    started: AtomicBool,
    notify: Arc<Notify>,
}

impl ScannerWake {
    pub(crate) fn begin_once(&self) -> Option<Arc<Notify>> {
        self.started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| self.notify.clone())
    }

    pub(crate) fn notifier(&self) -> Arc<Notify> {
        self.notify.clone()
    }
}

pub(crate) fn start(runtime: Arc<RuntimeManager>, wake: &ScannerWake) {
    let Some(notify) = wake.begin_once() else {
        return;
    };
    let supervisor = runtime.clone();
    tokio::spawn(async move {
        let runner = tokio::spawn(run(runtime, notify));
        if let Err(error) = runner.await {
            tracing::error!(%error, "Coordinator scanner task exited unexpectedly");
            supervisor.signal_fatal_local_authority("coordinator_scanner_supervisor");
        }
    });
}

async fn run(runtime: Arc<RuntimeManager>, notify: Arc<Notify>) {
    let mut closed = runtime.fatal_local_authority_receiver();
    loop {
        if runtime.local_authority_is_closed() {
            return;
        }
        let pass = run_pass(&runtime);
        let wait = tokio::select! {
            biased;
            _ = closed.changed() => return,
            wait = pass => wait,
        };
        tokio::select! {
            biased;
            _ = closed.changed() => return,
            () = tokio::time::sleep(wait) => {},
            () = notify.notified() => {
                tokio::select! {
                    biased;
                    _ = closed.changed() => return,
                    () = tokio::time::sleep(KICK_COALESCE) => {},
                }
            },
        }
        tokio::task::yield_now().await;
    }
}

struct WritePass {
    owner: Option<AdmittedOperation>,
    boundary: &'static str,
}

impl WritePass {
    fn begin(runtime: &RuntimeManager, boundary: &'static str) -> Option<Self> {
        runtime
            .acquire_local_authority_pass()
            .ok()
            .map(|owner| Self {
                owner: Some(owner),
                boundary,
            })
    }

    fn complete<T>(
        mut self,
        result: LocalAuthorityResult<phoenix_db::DbResult<T>>,
    ) -> Option<phoenix_db::DbResult<T>> {
        match result {
            LocalAuthorityResult::DurableFactEstablished(result) => {
                self.owner.take();
                if let Err(error) = &result {
                    tracing::warn!(%error, boundary=self.boundary, "Coordinator scanner operation remains pending");
                }
                Some(result)
            }
            LocalAuthorityResult::DurableFactUnclassified => {
                tracing::error!(
                    boundary = self.boundary,
                    "Coordinator scanner SQLite authority unclassified"
                );
                None
            }
        }
    }
}

impl Drop for WritePass {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            owner.close(self.boundary);
        }
    }
}

fn selected_profile(runtime: &RuntimeManager) -> Option<(ScanProfile, Arc<dyn LlmService>)> {
    let service = model::eligible_service(runtime.llm_registry())?;
    let connection = if service.uses_codex_bridge() {
        ScanConnection::Codex
    } else {
        ScanConnection::OpenAiResponses
    };
    Some((profile(service.as_ref(), connection), service))
}

fn profile(service: &dyn LlmService, connection: ScanConnection) -> ScanProfile {
    ScanProfile {
        connection,
        endpoint_hash: model::endpoint_hash(service),
        prompt_hash: Sha256::digest(prompt::INSTRUCTIONS.as_bytes()).into(),
        instruction_bytes: prompt::INSTRUCTIONS.len(),
    }
}

fn next_wait(now_us: i64, next_at_us: Option<i64>) -> Duration {
    next_at_us.map_or(REVISIT_INTERVAL, |next| {
        Duration::from_micros(u64::try_from(next.saturating_sub(now_us)).unwrap_or_default())
            .min(REVISIT_INTERVAL)
            .max(PASS_PACE)
    })
}

async fn run_pass(runtime: &Arc<RuntimeManager>) -> Duration {
    run_pass_with_selected(runtime, selected_profile(runtime)).await
}

async fn run_pass_with_selected(
    runtime: &Arc<RuntimeManager>,
    selected: Option<(ScanProfile, Arc<dyn LlmService>)>,
) -> Duration {
    let now_us = chrono::Utc::now().timestamp_micros();
    let Some(write) = WritePass::begin(runtime, "coordinator_scanner_prepare") else {
        return REVISIT_INTERVAL;
    };
    let Some(Ok(prepared)) = write.complete(
        runtime
            .db()
            .prepare_coordinator_scan(selected.as_ref().map(|(profile, _)| profile), now_us)
            .await,
    ) else {
        return REVISIT_INTERVAL;
    };
    let batch = match prepared {
        ScanPreparation::Ready(batch) => batch,
        ScanPreparation::Progress => return PASS_PACE,
        ScanPreparation::Wait { reason, next_at_us } => {
            tracing::debug!(?reason, ?next_at_us, "Coordinator scanner waiting");
            return next_wait(now_us, next_at_us);
        }
        ScanPreparation::Idle => return REVISIT_INTERVAL,
    };
    let Some((_, service)) = selected else {
        tracing::error!("Coordinator scanner prepared work without an eligible route");
        return REVISIT_INTERVAL;
    };
    let request = match model::BoundedRequest::new(prompt::INSTRUCTIONS, batch.input()) {
        Ok(request) => request,
        Err(error) => {
            tracing::error!(%error, "Coordinator scanner refused oversized or unencodable input");
            return REVISIT_INTERVAL;
        }
    };
    let Some(write) = WritePass::begin(runtime, "coordinator_scanner_reserve") else {
        return REVISIT_INTERVAL;
    };
    let Some(Ok(reservation)) = write.complete(
        runtime
            .db()
            .reserve_coordinator_scan(
                batch,
                request.content_bytes(),
                chrono::Utc::now().timestamp_micros(),
            )
            .await,
    ) else {
        return REVISIT_INTERVAL;
    };
    let attempt = match reservation {
        ScanReservation::Reserved(attempt) => attempt,
        ScanReservation::Paused { reason, next_at_us } => {
            tracing::debug!(
                ?reason,
                ?next_at_us,
                "Coordinator scanner reservation paused"
            );
            return next_wait(chrono::Utc::now().timestamp_micros(), next_at_us);
        }
        ScanReservation::Conflict => return PASS_PACE,
    };
    let (result, metrics) = infer(service.as_ref(), &request).await;
    finish_attempt(runtime, &attempt, result, metrics).await;
    PASS_PACE
}

async fn infer(
    service: &dyn LlmService,
    request: &model::BoundedRequest,
) -> (Result<ScanResult, ScanFailure>, ScanMetrics) {
    let began = Instant::now();
    let completion =
        std::panic::AssertUnwindSafe(service.complete(request.request())).catch_unwind();
    let result = tokio::time::timeout(model::TIMEOUT, completion).await;
    let elapsed_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    match result {
        Ok(Ok(Ok(response))) => {
            let metrics = metrics(&response, elapsed_ms);
            let result = model::complete_text(&response)
                .map_err(str::to_owned)
                .and_then(|text| {
                    serde_json::from_str::<ScanResult>(&text).map_err(|error| {
                        format!(
                            "closed result schema mismatch at {}:{}",
                            error.line(),
                            error.column()
                        )
                    })
                })
                .map_err(ScanFailure::InvalidOutput);
            (result, metrics)
        }
        Ok(Ok(Err(error))) => (
            Err(ScanFailure::Provider(format!("{:?}", error.kind))),
            ScanMetrics {
                elapsed_ms,
                usage: None,
            },
        ),
        Ok(Err(_)) => (
            Err(ScanFailure::Provider("provider task panicked".into())),
            ScanMetrics {
                elapsed_ms,
                usage: None,
            },
        ),
        Err(_) => (
            Err(ScanFailure::Timeout),
            ScanMetrics {
                elapsed_ms,
                usage: None,
            },
        ),
    }
}

async fn finish_attempt(
    runtime: &RuntimeManager,
    attempt: &ReservedScanAttempt,
    result: Result<ScanResult, ScanFailure>,
    metrics: ScanMetrics,
) {
    let now_us = chrono::Utc::now().timestamp_micros();
    match result {
        Ok(result) => {
            let Some(write) = WritePass::begin(runtime, "coordinator_scanner_apply") else {
                return;
            };
            match write.complete(
                runtime
                    .db()
                    .apply_coordinator_scan(attempt, &result, metrics.clone(), now_us)
                    .await,
            ) {
                Some(Ok(ScanApplyOutcome::Applied {
                    changed_item_count, ..
                })) => {
                    tracing::debug!(changed_item_count, "Coordinator scanner batch committed");
                }
                Some(Ok(ScanApplyOutcome::Rejected(reason))) => {
                    tracing::debug!(%reason, "Coordinator scanner rejected source evidence");
                }
                Some(Ok(ScanApplyOutcome::Replayed | ScanApplyOutcome::Conflict)) | None => {}
                Some(Err(error)) => {
                    fail_attempt(
                        runtime,
                        attempt,
                        ScanFailure::Storage(error.to_string()),
                        metrics,
                        now_us,
                    )
                    .await;
                }
            }
        }
        Err(failure) => fail_attempt(runtime, attempt, failure, metrics, now_us).await,
    }
}

async fn fail_attempt(
    runtime: &RuntimeManager,
    attempt: &ReservedScanAttempt,
    failure: ScanFailure,
    metrics: ScanMetrics,
    now_us: i64,
) {
    let Some(write) = WritePass::begin(runtime, "coordinator_scanner_fail") else {
        return;
    };
    let _ = write.complete(
        runtime
            .db()
            .fail_coordinator_scan(attempt, failure, metrics, now_us)
            .await,
    );
}

fn metrics(response: &LlmResponse, elapsed_ms: u64) -> ScanMetrics {
    let observation = response.usage_observation;
    let any_observed = observation.input_total
        || observation.output
        || observation.cache_read
        || observation.cache_write;
    let usage = any_observed.then(|| ScanUsage {
        input_tokens: observation
            .input_total
            .then_some(response.usage.input_tokens),
        cache_read_tokens: observation
            .cache_read
            .then_some(response.usage.cache_read_tokens),
        cache_creation_tokens: observation
            .cache_write
            .then_some(response.usage.cache_creation_tokens),
        output_tokens: observation.output.then_some(response.usage.output_tokens),
    });
    if !(observation.input_total
        && observation.output
        && observation.cache_read
        && observation.cache_write)
    {
        tracing::debug!(
            ?observation,
            "Coordinator scanner usage partial or unavailable; unknown fields remain absent"
        );
    }
    ScanMetrics { elapsed_ms, usage }
}
