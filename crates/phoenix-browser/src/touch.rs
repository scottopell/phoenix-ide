//! Single-contact CDP input state retained by the browser session.

use chromiumoxide::cdp::browser_protocol::input::{
    DispatchTouchEventParams, DispatchTouchEventType, TouchPoint,
};

#[derive(serde::Serialize)]
struct DispatchTouchEventCommand<'a> {
    #[serde(rename = "type")]
    r#type: &'a DispatchTouchEventType,
    #[serde(rename = "touchPoints")]
    touch_points: &'a [TouchPoint],
    #[serde(skip_serializing_if = "Option::is_none")]
    modifiers: Option<&'a i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<&'a chromiumoxide::cdp::browser_protocol::input::TimeSinceEpoch>,
}

impl<'a> DispatchTouchEventCommand<'a> {
    fn new(params: &'a DispatchTouchEventParams) -> Self {
        Self {
            r#type: &params.r#type,
            touch_points: &params.touch_points,
            modifiers: params.modifiers.as_ref(),
            timestamp: params.timestamp.as_ref(),
        }
    }
}

impl chromiumoxide::types::Method for DispatchTouchEventCommand<'_> {
    fn identifier(&self) -> chromiumoxide::types::MethodId {
        DispatchTouchEventParams::IDENTIFIER.into()
    }
}

impl chromiumoxide::types::Command for DispatchTouchEventCommand<'_> {
    type Response = chromiumoxide::cdp::browser_protocol::input::DispatchTouchEventReturns;
}

#[derive(Debug, Clone, Copy)]
pub enum TouchAction {
    Start { x: f64, y: f64 },
    Move { x: f64, y: f64 },
    End,
    Cancel,
}

impl TouchAction {
    fn label(self) -> &'static str {
        match self {
            Self::Start { .. } => "touch start",
            Self::Move { .. } => "touch move",
            Self::End => "touch end",
            Self::Cancel => "touch cancel",
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub(crate) enum TouchState {
    #[default]
    Idle,
    Active,
    Unknown,
}

impl TouchState {
    pub(crate) fn idle(&self) -> Result<(), String> {
        if *self == Self::Idle {
            Ok(())
        } else {
            Err("Touch contact is active or unacknowledged; cancellation must succeed first".into())
        }
    }

    fn prepare(&self, action: TouchAction) -> Result<(DispatchTouchEventParams, Self), String> {
        let (kind, point, next) = match action {
            TouchAction::Start { x, y } if *self == Self::Idle => (
                DispatchTouchEventType::TouchStart,
                Some((x, y)),
                Self::Active,
            ),
            TouchAction::Move { x, y } if *self == Self::Active => (
                DispatchTouchEventType::TouchMove,
                Some((x, y)),
                Self::Active,
            ),
            TouchAction::End if *self == Self::Active => {
                (DispatchTouchEventType::TouchEnd, None, Self::Idle)
            }
            TouchAction::Cancel => (DispatchTouchEventType::TouchCancel, None, Self::Idle),
            TouchAction::Start { .. } | TouchAction::Move { .. } | TouchAction::End => {
                return Err(
                    "Invalid touch transition; start once, move while active, then end".into(),
                );
            }
        };
        let points = match point {
            Some((x, y)) => {
                if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
                    return Err("Touch coordinates must be finite nonnegative CSS pixels".into());
                }
                let mut point = TouchPoint::new(x, y);
                point.id = Some(0.0);
                vec![point]
            }
            None => vec![],
        };
        Ok((DispatchTouchEventParams::new(kind, points), next))
    }

    async fn apply<F, Fut>(&mut self, action: TouchAction, dispatch: F) -> Result<(), String>
    where
        F: FnOnce(DispatchTouchEventParams) -> Fut + Send,
        Fut: std::future::Future<Output = Result<(), String>> + Send,
    {
        let (params, next) = self.prepare(action)?;
        *self = Self::Unknown;
        dispatch(params).await?;
        *self = next;
        Ok(())
    }

    async fn cancel<F, Fut>(&mut self, dispatch: F) -> Result<(), String>
    where
        F: FnOnce(DispatchTouchEventParams) -> Fut + Send,
        Fut: std::future::Future<Output = Result<(), String>> + Send,
    {
        if *self == Self::Idle {
            return Ok(());
        }
        self.apply(TouchAction::Cancel, dispatch).await
    }

    pub(crate) async fn cancel_on(&mut self, page: &chromiumoxide::Page) -> Result<(), String> {
        self.cancel(async |params| {
            page.execute(DispatchTouchEventCommand::new(&params))
                .await
                .map(|_| ())
                .map_err(|e| format!("touch cancel failed: {e}"))
        })
        .await
    }

    pub(crate) async fn dispatch(
        &mut self,
        page: &chromiumoxide::Page,
        action: TouchAction,
    ) -> Result<(), String> {
        let label = action.label();
        self.apply(action, async |params| {
            page.execute(DispatchTouchEventCommand::new(&params))
                .await
                .map(|_| ())
                .map_err(|e| format!("{label} failed: {e}"))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_wire_shape_keeps_required_touch_points_for_every_action() {
        for (action, expected_type) in [
            (TouchAction::End, "touchEnd"),
            (TouchAction::Cancel, "touchCancel"),
        ] {
            let (params, _) = TouchState::Active.prepare(action).unwrap();
            let wire = serde_json::to_value(DispatchTouchEventCommand::new(&params)).unwrap();
            assert_eq!(wire["touchPoints"], serde_json::json!([]));
            assert_eq!(wire["type"], expected_type);
        }

        for (action, expected_type) in [
            (TouchAction::Start { x: 20.0, y: 100.0 }, "touchStart"),
            (TouchAction::Move { x: 20.0, y: 90.0 }, "touchMove"),
        ] {
            let state = if matches!(action, TouchAction::Start { .. }) {
                TouchState::Idle
            } else {
                TouchState::Active
            };
            let (params, _) = state.prepare(action).unwrap();
            let wire = serde_json::to_value(DispatchTouchEventCommand::new(&params)).unwrap();
            assert_eq!(wire["type"], expected_type);
            assert_eq!(wire["touchPoints"][0]["id"], 0.0);
        }
    }

    #[tokio::test]
    async fn contact_identity_persists_until_empty_end() {
        let mut state = TouchState::default();
        for action in [
            TouchAction::Start { x: 20.0, y: 100.0 },
            TouchAction::Move { x: 20.0, y: 90.0 },
        ] {
            state
                .apply(action, async |params| {
                    assert_eq!(params.touch_points.len(), 1);
                    assert_eq!(params.touch_points[0].id, Some(0.0));
                    Ok(())
                })
                .await
                .unwrap();
            assert_eq!(state, TouchState::Active);
        }
        state
            .apply(TouchAction::End, async |params| {
                assert!(params.touch_points.is_empty());
                assert_eq!(params.r#type, DispatchTouchEventType::TouchEnd);
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(state, TouchState::Idle);
    }

    #[tokio::test]
    async fn cancelled_dispatch_requires_acknowledged_cancel() {
        let mut state = TouchState::default();
        {
            let operation = state.apply(TouchAction::Start { x: 1.0, y: 2.0 }, async |_| {
                std::future::pending::<Result<(), String>>().await
            });
            futures::pin_mut!(operation);
            assert!(futures::poll!(operation).is_pending());
        }
        assert_eq!(state, TouchState::Unknown);
        assert!(state
            .apply(TouchAction::Cancel, async |_| Err("lost ACK".into()))
            .await
            .is_err());
        assert_eq!(state, TouchState::Unknown);
        state
            .apply(TouchAction::Cancel, async |params| {
                assert!(params.touch_points.is_empty());
                assert_eq!(params.r#type, DispatchTouchEventType::TouchCancel);
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(state, TouchState::Idle);
    }

    #[tokio::test]
    async fn idle_cleanup_does_not_dispatch() {
        TouchState::Idle
            .cancel(async |_| panic!("idle contact must not receive cancellation"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn interrupted_move_end_and_cancel_remain_unknown_until_cancel_ack() {
        for action in [
            TouchAction::Move { x: 10.0, y: 20.0 },
            TouchAction::End,
            TouchAction::Cancel,
        ] {
            let mut state = TouchState::Active;
            {
                let operation = state.apply(action, async |_| {
                    std::future::pending::<Result<(), String>>().await
                });
                futures::pin_mut!(operation);
                assert!(futures::poll!(operation).is_pending());
            }
            assert_eq!(state, TouchState::Unknown);
            assert!(state.idle().is_err());
            for forbidden in [
                TouchAction::Start { x: 1.0, y: 2.0 },
                TouchAction::Move { x: 1.0, y: 2.0 },
                TouchAction::End,
            ] {
                assert!(state
                    .apply(forbidden, async |_| panic!(
                        "unknown contact dispatched input"
                    ))
                    .await
                    .is_err());
            }
            state
                .cancel(async |params| {
                    assert_eq!(params.r#type, DispatchTouchEventType::TouchCancel);
                    assert!(params.touch_points.is_empty());
                    Ok(())
                })
                .await
                .unwrap();
            assert_eq!(state, TouchState::Idle);
        }
    }

    #[tokio::test]
    async fn failed_release_and_failed_cleanup_never_report_idle() {
        let mut state = TouchState::Active;
        assert!(state
            .apply(TouchAction::End, async |_| Err("end ACK lost".into()))
            .await
            .is_err());
        assert_eq!(state, TouchState::Unknown);
        assert!(state
            .cancel(async |_| Err("cancel ACK lost".into()))
            .await
            .is_err());
        assert_eq!(state, TouchState::Unknown);
    }

    #[test]
    fn invalid_sequence_or_coordinate_never_dispatches() {
        let idle = TouchState::Idle;
        assert!(idle.prepare(TouchAction::Move { x: 1.0, y: 1.0 }).is_err());
        assert!(idle.prepare(TouchAction::End).is_err());
        assert!(idle
            .prepare(TouchAction::Start {
                x: f64::NAN,
                y: 1.0
            })
            .is_err());
        assert!(TouchState::Active
            .prepare(TouchAction::Start { x: 1.0, y: 1.0 })
            .is_err());
    }
}
