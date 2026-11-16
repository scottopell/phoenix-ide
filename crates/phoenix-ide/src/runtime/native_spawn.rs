use super::*;

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
    ) -> NativeSpawnTerminalEvidence {
        let Ok(owner) = self.acquire_local_authority_pass() else {
            return NativeSpawnTerminalEvidence::Unclassified;
        };
        let mut authority = NativeSpawnWriteAuthority { owner: Some(owner) };
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
        authority.owner = None;
        evidence
    }
}
