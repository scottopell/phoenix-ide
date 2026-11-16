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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_write_owner_abort_or_panic_closes_shared_authority() {
        for panic in [false, true] {
            let manager = Arc::new(super::super::scope_liveness_tests::test_manager().await);
            let owner = manager.acquire_local_authority_pass().unwrap();
            let (started, observed) = oneshot::channel();
            let task = tokio::spawn(async move {
                let _authority = NativeSpawnWriteAuthority { owner: Some(owner) };
                started.send(()).unwrap();
                if panic {
                    panic!("injected native write owner panic");
                }
                std::future::pending::<()>().await;
            });
            observed.await.unwrap();
            if !panic {
                task.abort();
            }
            assert!(task.await.is_err());
            assert!(manager.fatal_local_authority_fence.is_closed());
            assert_eq!(manager.fatal_local_authority_fence.owner_count(), 0);
        }
    }
}
