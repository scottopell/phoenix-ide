use std::{collections::HashMap, future::Future, sync::Arc, task::Poll};
use tokio::sync::{watch, Mutex};

use super::ControlError;

#[derive(Debug)]
pub(crate) struct DispatchEpoch {
    cancelled: watch::Sender<bool>,
}

impl Default for DispatchEpoch {
    fn default() -> Self {
        Self {
            cancelled: watch::channel(false).0,
        }
    }
}

impl DispatchEpoch {
    pub(crate) fn commit_revocation(&mut self) -> ControlCommitRevocation<'_> {
        ControlCommitRevocation(self)
    }

    pub(crate) fn revoke(&mut self) {
        self.cancelled.send_replace(true);
        self.cancelled = watch::channel(false).0;
    }
}

pub(crate) struct ControlCommitRevocation<'a>(&'a mut DispatchEpoch);

impl Drop for ControlCommitRevocation<'_> {
    fn drop(&mut self) {
        self.0.revoke();
    }
}

#[derive(Debug, Default)]
pub(crate) struct GoalControlRuntime {
    owners: std::sync::Mutex<HashMap<String, Arc<Mutex<DispatchEpoch>>>>,
}

impl GoalControlRuntime {
    pub(crate) fn owner(&self, owner: &str) -> Result<Arc<Mutex<DispatchEpoch>>, ControlError> {
        let mut owners = self
            .owners
            .lock()
            .map_err(|_| ControlError::Integrity("dispatch owner registry poisoned"))?;
        Ok(owners.entry(owner.to_owned()).or_default().clone())
    }
}

#[derive(Debug)]
enum DispatchAuthority {
    Disabled,
    Installed {
        gate: Arc<Mutex<DispatchEpoch>>,
        cancelled: watch::Receiver<bool>,
    },
}

#[derive(Debug)]
#[must_use]
pub struct GoalDispatchPermit {
    authority: DispatchAuthority,
}

#[derive(Debug, thiserror::Error)]
#[error("goal control revoked this execution's dispatch authority")]
pub struct GoalDispatchRevoked;

#[derive(Debug, thiserror::Error)]
#[error("delegated runtime work does not carry installed goal control authority")]
pub struct GoalDispatchDelegationUnavailable;

impl GoalDispatchPermit {
    pub fn disabled() -> Self {
        Self {
            authority: DispatchAuthority::Disabled,
        }
    }

    pub(crate) fn admitted(gate: Arc<Mutex<DispatchEpoch>>, epoch: &DispatchEpoch) -> Self {
        Self {
            authority: DispatchAuthority::Installed {
                gate,
                cancelled: epoch.cancelled.subscribe(),
            },
        }
    }

    /// # Errors
    /// Installed control cannot yet be transferred into delegated runtime lifetimes.
    pub fn require_delegation_support(self) -> Result<(), GoalDispatchDelegationUnavailable> {
        match self.authority {
            DispatchAuthority::Disabled => Ok(()),
            DispatchAuthority::Installed { .. } => {
                tracing::debug!(
                    "refusing delegated runtime work without goal control transfer support"
                );
                Err(GoalDispatchDelegationUnavailable)
            }
        }
    }

    /// Poll an external operation only while this exact admitted execution remains authorized.
    ///
    /// # Errors
    /// Returns revocation without polling again after a committed Stop or release.
    pub async fn run<F: Future>(self, future: F) -> Result<F::Output, GoalDispatchRevoked> {
        let DispatchAuthority::Installed { gate, cancelled } = self.authority else {
            return Ok(future.await);
        };
        let mut changed = cancelled.clone();
        let mut future = Box::pin(future);
        let mut lock = Box::pin(gate.clone().lock_owned());
        let guarded = std::future::poll_fn(move |cx| {
            let epoch = match lock.as_mut().poll(cx) {
                Poll::Ready(epoch) => epoch,
                Poll::Pending => return Poll::Pending,
            };
            if *cancelled.borrow() {
                return Poll::Ready(Err(GoalDispatchRevoked));
            }
            let result = future.as_mut().poll(cx);
            drop(epoch);
            lock = Box::pin(gate.clone().lock_owned());
            result.map(Ok)
        });
        tokio::select! {
            biased;
            _ = changed.wait_for(|revoked| *revoked) => Err(GoalDispatchRevoked),
            result = guarded => result,
        }
    }
}
