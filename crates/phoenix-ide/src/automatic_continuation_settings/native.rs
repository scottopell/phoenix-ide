use std::sync::Arc;

use phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion;
use phoenix_db::workflow::LocalAuthorityResult;
use phoenix_db::{
    AutomaticContinuationSettings, AutomaticContinuationTarget,
    ResolvedAutomaticContinuationAggregate,
};
use serde::{Deserialize, Serialize};

use super::Target;
use crate::api::AutomaticContinuationAggregateView;
use crate::runtime::{AdmittedOperation, RuntimeManager};

pub(crate) enum Operation {
    Read,
    Set(AutoContinueOnContextExhaustion),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreferenceView {
    pub aggregate: AutomaticContinuationAggregateView,
    pub auto_continue_on_context_exhaustion: bool,
}

impl From<AutomaticContinuationSettings> for PreferenceView {
    fn from(settings: AutomaticContinuationSettings) -> Self {
        let AutomaticContinuationSettings {
            aggregate,
            preference,
        } = settings;
        let aggregate = match aggregate {
            ResolvedAutomaticContinuationAggregate::Ordinary(product_conversation_id) => {
                AutomaticContinuationAggregateView::Ordinary {
                    product_conversation_id: product_conversation_id.to_string(),
                }
            }
            ResolvedAutomaticContinuationAggregate::Coordinator(product_conversation_id) => {
                AutomaticContinuationAggregateView::Coordinator {
                    product_conversation_id: product_conversation_id.to_string(),
                }
            }
        };
        Self {
            aggregate,
            auto_continue_on_context_exhaustion: bool::from(preference),
        }
    }
}

pub(crate) enum Completion {
    Established(Result<PreferenceView, String>),
    LocalAuthorityClosed,
}

pub(crate) struct PreferenceWritePass {
    owner: Option<AdmittedOperation>,
}

impl PreferenceWritePass {
    pub(crate) fn new(owner: AdmittedOperation) -> Self {
        Self { owner: Some(owner) }
    }

    pub(crate) fn classify<T>(mut self, result: LocalAuthorityResult<T>) -> Result<T, String> {
        match result {
            LocalAuthorityResult::DurableFactEstablished(value) => {
                self.owner.take();
                Ok(value)
            }
            LocalAuthorityResult::DurableFactUnclassified => {
                Err("automatic-continuation preference durability is unclassified".into())
            }
        }
    }
}

impl Drop for PreferenceWritePass {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.as_ref() {
            owner.close("native_automatic_continuation_setting_unclassified");
        }
    }
}

pub(crate) async fn from_coordinator(
    runtime: Arc<RuntimeManager>,
    source_transcript_id: String,
    target: Target,
    operation: Operation,
) -> Completion {
    from_coordinator_inner(
        runtime,
        source_transcript_id,
        target,
        operation,
        #[cfg(test)]
        None,
    )
    .await
}

#[cfg(test)]
struct BoundaryObservation {
    source_gate_wait: tokio::sync::oneshot::Sender<()>,
    worker_completed: tokio::sync::oneshot::Sender<()>,
}

async fn from_coordinator_inner(
    runtime: Arc<RuntimeManager>,
    source_transcript_id: String,
    target: Target,
    operation: Operation,
    #[cfg(test)] observation: Option<BoundaryObservation>,
) -> Completion {
    let worker_runtime = runtime.clone();
    let result = tokio::spawn(async move {
        #[cfg(test)]
        let (source_gate_wait, worker_completed) = match observation {
            Some(observation) => (
                Some(observation.source_gate_wait),
                Some(observation.worker_completed),
            ),
            None => (None, None),
        };
        let result = apply_owned(
            worker_runtime,
            source_transcript_id,
            target,
            operation,
            #[cfg(test)]
            source_gate_wait,
        )
        .await;
        #[cfg(test)]
        if let Some(completed) = worker_completed {
            let _ = completed.send(());
        }
        result
    })
    .await
    .unwrap_or_else(|error| Err(error.to_string()));
    if runtime.local_authority_is_closed() {
        Completion::LocalAuthorityClosed
    } else {
        Completion::Established(result)
    }
}

async fn apply_owned(
    runtime: Arc<RuntimeManager>,
    source_transcript_id: String,
    target: Target,
    operation: Operation,
    #[cfg(test)] source_gate_wait: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<PreferenceView, String> {
    let _operation_owner = runtime
        .acquire_local_authority_pass()
        .map_err(|()| "runtime admission closed after fatal local authority loss".to_string())?;
    let db = runtime.db();
    db.require_current_coordinator_transcript(&source_transcript_id)
        .await
        .map_err(|error| error.to_string())?;
    let source = db
        .get_conversation(&source_transcript_id)
        .await
        .map_err(|error| error.to_string())?;
    #[cfg(test)]
    if let Some(waiting) = source_gate_wait {
        let _ = waiting.send(());
    }
    let _source_gate = runtime
        .lock_product_message_admission(source.product_conversation_id.as_str())
        .await;
    let target = match &target {
        Target::Ordinary(id) => AutomaticContinuationTarget::Ordinary(id),
        Target::Coordinator => AutomaticContinuationTarget::Coordinator,
    };
    let settings = match operation {
        Operation::Read => db
            .read_native_automatic_continuation_settings(&source_transcript_id, target)
            .await
            .map_err(|error| error.to_string())?,
        Operation::Set(preference) => {
            let owner = runtime.acquire_local_authority_pass().map_err(|()| {
                "runtime admission closed after fatal local authority loss".to_string()
            })?;
            let pass = PreferenceWritePass::new(owner);
            pass.classify(
                db.set_native_automatic_continuation_settings(
                    &source_transcript_id,
                    target,
                    preference,
                )
                .await,
            )?
            .map_err(|error| error.to_string())?
        }
    };
    Ok(settings.into())
}

#[cfg(test)]
mod tests;
