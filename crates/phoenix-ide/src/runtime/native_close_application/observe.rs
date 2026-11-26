//! Dormant exact observation queue; the source actor owns application.

use super::{NativeCloseApplicationError, NativeCloseRunRef, SourceToolCall};

pub(crate) struct NativeCloseObserveRequest {
    pub source: SourceToolCall,
    pub referenced: NativeCloseRunRef,
    pub admitted: crate::runtime::AdmittedOperation,
    pub target_gate: tokio::sync::OwnedMutexGuard<()>,
    pub response: tokio::sync::oneshot::Sender<
        Result<phoenix_db::native_close::NativeCloseObserveResult, NativeCloseApplicationError>,
    >,
}

impl crate::runtime::RuntimeManager {
    /// Exact observation uses the same gate-before-enqueue and source-owned
    /// durable classification boundary as initial admission. No effects dispatch.
    #[allow(dead_code)] // Dormant until the complete unified adapter is qualified.
    pub(crate) async fn apply_native_close_observe(
        &self,
        source_transcript_id: &str,
        source: SourceToolCall,
        referenced: NativeCloseRunRef,
    ) -> Result<phoenix_db::native_close::NativeCloseObserveResult, NativeCloseApplicationError>
    {
        let gate = self
            .conversation_admission(referenced.target.as_str())
            .await;
        let target_gate = gate.lock_owned().await;
        let admitted = self
            .acquire_local_authority_pass()
            .map_err(|()| NativeCloseApplicationError::FatalAuthority)?;
        let sender = {
            let runtimes = self.runtimes.read().await;
            runtimes
                .get(source_transcript_id)
                .map(|handle| handle.acknowledged_event_tx.clone())
        }
        .ok_or(NativeCloseApplicationError::Unavailable)?;
        let (response, receiver) = tokio::sync::oneshot::channel();
        sender
            .send(
                crate::runtime::AcknowledgedRuntimeRequest::ExecuteNativeCloseObserve(Box::new(
                    NativeCloseObserveRequest {
                        source,
                        referenced,
                        admitted,
                        target_gate,
                        response,
                    },
                )),
            )
            .await
            .map_err(|_| NativeCloseApplicationError::Unavailable)?;
        receiver
            .await
            .map_err(|_| NativeCloseApplicationError::Unavailable)?
    }
}
