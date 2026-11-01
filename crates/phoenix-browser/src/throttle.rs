//! Acknowledged CPU throttling retained by a managed browser session.

use chromiumoxide::cdp::browser_protocol::emulation::SetCpuThrottlingRateParams;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ThrottleState {
    Known(f64),
    Unknown,
}

impl Default for ThrottleState {
    fn default() -> Self {
        Self::Known(1.0)
    }
}

impl ThrottleState {
    pub(crate) fn known(&self) -> Result<f64, String> {
        match self {
            Self::Known(rate) => Ok(*rate),
            Self::Unknown => Err("CPU throttle is unknown after an incomplete CDP operation; set an explicit throttle rate before running a scenario".into()),
        }
    }

    pub(crate) async fn set(
        &mut self,
        page: &chromiumoxide::Page,
        rate: f64,
    ) -> Result<(), String> {
        self.apply(rate, async {
            page.execute(SetCpuThrottlingRateParams::new(rate))
                .await
                .map_err(|error| format!("Emulation.setCPUThrottlingRate failed: {error}"))?;
            Ok(())
        })
        .await
    }

    async fn apply(
        &mut self,
        rate: f64,
        operation: impl std::future::Future<Output = Result<(), String>>,
    ) -> Result<(), String> {
        if !rate.is_finite() || rate < 1.0 {
            return Err("CPU throttle rate must be finite and >= 1".into());
        }
        *self = Self::Unknown;
        operation.await?;
        *self = Self::Known(rate);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{pending, ready};

    #[test]
    fn fresh_browser_has_known_identity_rate() {
        assert_eq!(ThrottleState::default(), ThrottleState::Known(1.0));
    }

    #[tokio::test]
    async fn cancelled_pending_dispatch_loses_previous_acknowledgement() {
        let mut state = ThrottleState::Known(4.0);
        {
            let operation = state.apply(2.0, pending());
            futures::pin_mut!(operation);
            assert!(futures::poll!(operation).is_pending());
        }
        assert_eq!(state, ThrottleState::Unknown);
        assert!(state.known().is_err());
        state.apply(3.0, ready(Ok(()))).await.unwrap();
        assert_eq!(state, ThrottleState::Known(3.0));
    }

    #[tokio::test]
    async fn failed_dispatch_is_unknown_until_successful_acknowledgement() {
        let mut state = ThrottleState::default();
        let error = state
            .apply(4.0, ready(Err("injected CDP failure".into())))
            .await;
        assert_eq!(error.unwrap_err(), "injected CDP failure");
        assert_eq!(state, ThrottleState::Unknown);
        state.apply(1.0, ready(Ok(()))).await.unwrap();
        assert_eq!(state, ThrottleState::Known(1.0));
    }

    #[tokio::test]
    async fn pending_dispatch_becomes_known_only_after_acknowledgement() {
        let mut state = ThrottleState::default();
        let (ack, receiver) = tokio::sync::oneshot::channel();
        {
            let operation = state.apply(4.0, async {
                receiver.await.map_err(|error| error.to_string())
            });
            futures::pin_mut!(operation);
            assert!(futures::poll!(&mut operation).is_pending());
            ack.send(()).unwrap();
            operation.await.unwrap();
        }
        assert_eq!(state, ThrottleState::Known(4.0));
    }

    #[tokio::test]
    async fn independent_touch_failure_does_not_poison_throttle_acknowledgement() {
        let mut state = ThrottleState::Unknown;
        let (touch, throttle) = tokio::join!(
            ready(Err::<(), _>("injected touch failure")),
            state.apply(2.0, ready(Ok(()))),
        );
        assert!(touch.is_err());
        throttle.unwrap();
        assert_eq!(state, ThrottleState::Known(2.0));
    }

    #[tokio::test]
    async fn cancelled_touch_wait_does_not_erase_completed_throttle_acknowledgement() {
        let mut state = ThrottleState::Unknown;
        {
            let cleanup = async {
                tokio::join!(
                    pending::<Result<(), String>>(),
                    state.apply(1.0, ready(Ok(()))),
                )
            };
            futures::pin_mut!(cleanup);
            assert!(futures::poll!(cleanup).is_pending());
        }
        assert_eq!(state, ThrottleState::Known(1.0));
    }

    #[tokio::test]
    async fn invalid_rate_does_not_dispatch_or_destroy_known_state() {
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut state = ThrottleState::default();
            assert!(state
                .apply(rate, async { panic!("must not dispatch") })
                .await
                .is_err());
            assert_eq!(state, ThrottleState::Known(1.0));
        }
    }
}
