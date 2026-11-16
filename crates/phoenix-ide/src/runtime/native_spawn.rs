use super::{AdmittedOperation, RuntimeManager, SubAgentAdmissionResponse};
use chrono::Utc;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
enum NativeSpawnAdmissionGate {
    Unissued(oneshot::Sender<NativeSpawnClosureReceipt>),
    Issued(oneshot::Sender<NativeSpawnClosureReceipt>),
    Closed,
}

#[derive(Debug)]
pub struct NativeSpawnControl {
    invocation: phoenix_db::SubAgentSpawnInvocation,
    cancellation: CancellationToken,
    gate: Mutex<NativeSpawnAdmissionGate>,
    #[cfg(test)]
    issue_attempt: tokio::sync::watch::Sender<Option<bool>>,
    #[cfg(test)]
    publications: std::sync::atomic::AtomicUsize,
}

impl NativeSpawnControl {
    pub(super) fn new(
        invocation: phoenix_db::SubAgentSpawnInvocation,
        reply: oneshot::Sender<NativeSpawnClosureReceipt>,
    ) -> Self {
        Self {
            invocation,
            cancellation: CancellationToken::new(),
            gate: Mutex::new(NativeSpawnAdmissionGate::Unissued(reply)),
            #[cfg(test)]
            issue_attempt: tokio::sync::watch::channel(None).0,
            #[cfg(test)]
            publications: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn take_reply(
        gate: &mut NativeSpawnAdmissionGate,
    ) -> Option<oneshot::Sender<NativeSpawnClosureReceipt>> {
        match std::mem::replace(gate, NativeSpawnAdmissionGate::Closed) {
            NativeSpawnAdmissionGate::Unissued(reply) | NativeSpawnAdmissionGate::Issued(reply) => {
                Some(reply)
            }
            NativeSpawnAdmissionGate::Closed => None,
        }
    }

    fn send(
        &self,
        reply: oneshot::Sender<NativeSpawnClosureReceipt>,
        evidence: NativeSpawnTerminalEvidence,
    ) -> bool {
        #[cfg(test)]
        self.publications
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        reply
            .send(NativeSpawnClosureReceipt {
                invocation: self.invocation.clone(),
                evidence,
            })
            .is_ok()
    }

    pub(super) fn request_cancel(&self) {
        let mut gate = self.gate.lock().expect("native spawn gate poisoned");
        self.cancellation.cancel();
        if matches!(*gate, NativeSpawnAdmissionGate::Unissued(_)) {
            let reply = Self::take_reply(&mut gate).expect("unissued gate owns reply");
            self.send(reply, NativeSpawnTerminalEvidence::NotAdmitted);
        }
    }

    pub(super) fn try_issue(&self) -> bool {
        let issued = {
            let mut gate = self.gate.lock().expect("native spawn gate poisoned");
            match std::mem::replace(&mut *gate, NativeSpawnAdmissionGate::Closed) {
                NativeSpawnAdmissionGate::Unissued(reply) => {
                    *gate = NativeSpawnAdmissionGate::Issued(reply);
                    true
                }
                NativeSpawnAdmissionGate::Issued(reply) => {
                    *gate = NativeSpawnAdmissionGate::Issued(reply);
                    false
                }
                NativeSpawnAdmissionGate::Closed => false,
            }
        };
        #[cfg(test)]
        let _ = self.issue_attempt.send_replace(Some(issued));
        issued
    }

    pub(super) fn publish(&self, evidence: NativeSpawnTerminalEvidence) -> bool {
        let mut gate = self.gate.lock().expect("native spawn gate poisoned");
        Self::take_reply(&mut gate).is_some_and(|reply| self.send(reply, evidence))
    }

    pub(super) fn activated(&self) {
        let mut gate = self.gate.lock().expect("native spawn gate poisoned");
        drop(Self::take_reply(&mut gate));
    }

    fn owner_disappeared(&self) {
        let mut gate = self.gate.lock().expect("native spawn gate poisoned");
        let evidence = match &*gate {
            NativeSpawnAdmissionGate::Unissued(_) => NativeSpawnTerminalEvidence::NotAdmitted,
            NativeSpawnAdmissionGate::Issued(_) => NativeSpawnTerminalEvidence::Failed(
                "native spawn manager disappeared without classified closure".into(),
            ),
            NativeSpawnAdmissionGate::Closed => return,
        };
        let reply = Self::take_reply(&mut gate).expect("open gate owns reply");
        self.send(reply, evidence);
    }

    pub(super) async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }
    pub(super) fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    #[cfg(test)]
    pub(super) fn test_for_batch(parent: &str, batch: &str) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::new(
            phoenix_db::SubAgentSpawnInvocation {
                parent_conversation_id: parent.into(),
                assistant_message_id: "assistant".into(),
                tool_use_id: "spawn".into(),
                batch_id: batch.into(),
            },
            oneshot::channel().0,
        ))
    }

    #[cfg(test)]
    pub(super) fn subscribe_issue_attempt(&self) -> tokio::sync::watch::Receiver<Option<bool>> {
        self.issue_attempt.subscribe()
    }
    #[cfg(test)]
    pub(super) fn is_closed(&self) -> bool {
        matches!(
            *self.gate.lock().expect("native spawn gate poisoned"),
            NativeSpawnAdmissionGate::Closed
        )
    }
    #[cfg(test)]
    pub(super) fn publication_count(&self) -> usize {
        self.publications.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub struct SubAgentAdmissionReceipt {
    pub invocation: phoenix_db::SubAgentSpawnInvocation,
    pub outcome: SubAgentAdmissionResponse,
}

#[derive(Debug)]
pub enum NativeSpawnTerminalEvidence {
    NotAdmitted,
    Terminal(Vec<phoenix_db::UnactivatedSubAgentTerminal>),
    Failed(String),
    Unclassified,
}

#[derive(Debug)]
pub struct NativeSpawnClosureReceipt {
    pub invocation: phoenix_db::SubAgentSpawnInvocation,
    pub evidence: NativeSpawnTerminalEvidence,
}

pub(super) struct NativeSpawnWriteAuthority {
    pub(super) owner: Option<AdmittedOperation>,
}

pub(super) struct NativeSpawnLifecycleOwner {
    pub(super) control: Arc<NativeSpawnControl>,
}

impl NativeSpawnLifecycleOwner {
    pub(super) fn new(control: Arc<NativeSpawnControl>) -> Self {
        Self { control }
    }

    pub(super) fn arm_write(
        &mut self,
        owner: AdmittedOperation,
    ) -> NativeSpawnManagerWriteAuthority<'_> {
        NativeSpawnManagerWriteAuthority {
            authority: NativeSpawnWriteAuthority { owner: Some(owner) },
            _lifecycle: self,
        }
    }
}

impl Drop for NativeSpawnLifecycleOwner {
    fn drop(&mut self) {
        self.control.owner_disappeared();
    }
}

pub(super) struct NativeSpawnManagerWriteAuthority<'a> {
    authority: NativeSpawnWriteAuthority,
    _lifecycle: &'a mut NativeSpawnLifecycleOwner,
}

impl NativeSpawnManagerWriteAuthority<'_> {
    pub(super) fn classified(mut self) {
        self.authority.owner = None;
    }
}

impl Drop for NativeSpawnWriteAuthority {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            owner.close("native_spawn_write_owner_disappeared");
        }
    }
}

impl RuntimeManager {
    pub(super) async fn close_native_spawn_batch(
        &self,
        batch: &phoenix_db::SubAgentBatchAdmission,
        lifecycle: &mut NativeSpawnLifecycleOwner,
    ) -> NativeSpawnTerminalEvidence {
        let Ok(owner) = self.acquire_local_authority_pass() else {
            return NativeSpawnTerminalEvidence::Failed(
                "native spawn closure was not issued because local write admission is closed"
                    .into(),
            );
        };
        let authority = lifecycle.arm_write(owner);
        let result = self
            .db
            .close_live_unactivated_sub_agent_batch(batch, Utc::now())
            .await;
        let evidence = match result {
            phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Ok(terminals)) => {
                NativeSpawnTerminalEvidence::Terminal(terminals)
            }
            phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                NativeSpawnTerminalEvidence::Failed(error.to_string())
            }
            phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified => {
                self.signal_fatal_local_authority("native_spawn_closure");
                NativeSpawnTerminalEvidence::Unclassified
            }
        };
        authority.classified();
        evidence
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn native_write_owner_abort_or_panic_closes_shared_authority() {
        for panic in [false, true] {
            let manager = Arc::new(super::super::scope_liveness_tests::test_manager().await);
            let owner = manager.acquire_local_authority_pass().unwrap();
            let (reply, mut closure) = oneshot::channel();
            let control = Arc::new(NativeSpawnControl::new(
                phoenix_db::SubAgentSpawnInvocation {
                    parent_conversation_id: "parent".into(),
                    assistant_message_id: "assistant".into(),
                    tool_use_id: "spawn".into(),
                    batch_id: "batch".into(),
                },
                reply,
            ));
            assert!(control.try_issue());
            let (started, observed) = oneshot::channel();
            let task = tokio::spawn(async move {
                let mut lifecycle = NativeSpawnLifecycleOwner::new(control);
                let _authority = lifecycle.arm_write(owner);
                started.send(()).unwrap();
                assert!(!panic, "injected native write owner panic");
                std::future::pending::<()>().await;
            });
            observed.await.unwrap();
            if !panic {
                task.abort();
            }
            let receipt = (&mut closure).await.unwrap();
            assert!(manager.fatal_local_authority_fence.is_closed());
            assert!(matches!(
                receipt.evidence,
                NativeSpawnTerminalEvidence::Failed(_)
            ));
            assert!(task.await.is_err());
            assert_eq!(manager.fatal_local_authority_fence.owner_count(), 0);
        }
    }
}
