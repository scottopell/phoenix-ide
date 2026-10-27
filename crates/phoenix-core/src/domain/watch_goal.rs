use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum WatchGoalState {
    Underway,
    Blocked { reason: String },
    Complete { reason: String },
    Exhausted { reason: String },
    Suspended { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum WatchGoalUsage {
    UnavailableNotCaptured,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum WatchGoalCapture {
    LegacyNotCaptured,
    NotCapturedDisabled,
    CapturedNoGoal {
        owner_id: String,
        owner_version: i64,
        usage: WatchGoalUsage,
    },
    CapturedSavedGoal {
        owner_id: String,
        goal_id: String,
        objective: String,
        owner_version: i64,
        state: WatchGoalState,
        usage: WatchGoalUsage,
    },
}

#[cfg(test)]
mod tests {
    use super::{WatchGoalCapture, WatchGoalState, WatchGoalUsage};

    #[test]
    fn state_variants_keep_their_tagged_wire_shapes() {
        let cases = [
            (
                WatchGoalState::Underway,
                serde_json::json!({"kind":"underway"}),
            ),
            (
                WatchGoalState::Blocked {
                    reason: "waiting".into(),
                },
                serde_json::json!({"kind":"blocked","reason":"waiting"}),
            ),
            (
                WatchGoalState::Complete {
                    reason: "done".into(),
                },
                serde_json::json!({"kind":"complete","reason":"done"}),
            ),
            (
                WatchGoalState::Exhausted {
                    reason: "limit".into(),
                },
                serde_json::json!({"kind":"exhausted","reason":"limit"}),
            ),
            (
                WatchGoalState::Suspended {
                    reason: "paused".into(),
                },
                serde_json::json!({"kind":"suspended","reason":"paused"}),
            ),
        ];
        for (state, expected) in cases {
            assert_eq!(serde_json::to_value(&state).unwrap(), expected);
            assert_eq!(
                serde_json::from_value::<WatchGoalState>(expected).unwrap(),
                state
            );
        }
    }

    #[test]
    fn capture_variants_preserve_their_discriminants_and_fields() {
        let cases = [
            (
                WatchGoalCapture::LegacyNotCaptured,
                serde_json::json!({"kind":"legacy_not_captured"}),
            ),
            (
                WatchGoalCapture::NotCapturedDisabled,
                serde_json::json!({"kind":"not_captured_disabled"}),
            ),
            (
                WatchGoalCapture::CapturedNoGoal {
                    owner_id: "owner".into(),
                    owner_version: 3,
                    usage: WatchGoalUsage::UnavailableNotCaptured,
                },
                serde_json::json!({"kind":"captured_no_goal","owner_id":"owner","owner_version":3,"usage":"unavailable_not_captured"}),
            ),
            (
                WatchGoalCapture::CapturedSavedGoal {
                    owner_id: "owner".into(),
                    goal_id: "goal".into(),
                    objective: "ship it".into(),
                    owner_version: 4,
                    state: WatchGoalState::Blocked {
                        reason: "waiting".into(),
                    },
                    usage: WatchGoalUsage::UnavailableNotCaptured,
                },
                serde_json::json!({"kind":"captured_saved_goal","owner_id":"owner","goal_id":"goal","objective":"ship it","owner_version":4,"state":{"kind":"blocked","reason":"waiting"},"usage":"unavailable_not_captured"}),
            ),
        ];
        for (capture, expected) in cases {
            assert_eq!(serde_json::to_value(&capture).unwrap(), expected);
            assert_eq!(
                serde_json::from_value::<WatchGoalCapture>(expected).unwrap(),
                capture
            );
        }
    }
}
