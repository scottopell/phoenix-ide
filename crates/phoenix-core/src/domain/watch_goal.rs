use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::conversation_goal::{GoalError, GoalText, GoalVersion};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct WatchSavedGoalVersion(GoalVersion);

impl WatchSavedGoalVersion {
    /// # Errors
    /// A captured saved goal must have a committed positive owner version.
    pub fn parse(value: i64) -> Result<Self, GoalError> {
        let version = GoalVersion::parse(value)?;
        if version == GoalVersion::INITIAL {
            return Err(GoalError::Range);
        }
        Ok(Self(version))
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0.get()
    }
}

impl<'de> Deserialize<'de> for WatchSavedGoalVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(i64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum WatchGoalState {
    Underway,
    Blocked {
        #[ts(type = "string")]
        reason: GoalText,
    },
    Complete {
        #[ts(type = "string")]
        reason: GoalText,
    },
    Exhausted {
        #[ts(type = "string")]
        reason: GoalText,
    },
    Suspended {
        #[ts(type = "string")]
        reason: GoalText,
    },
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
        #[ts(type = "string")]
        owner_id: GoalText,
        #[ts(type = "number")]
        owner_version: GoalVersion,
        usage: WatchGoalUsage,
    },
    CapturedSavedGoal {
        #[ts(type = "string")]
        owner_id: GoalText,
        #[ts(type = "string")]
        goal_id: GoalText,
        #[ts(type = "string")]
        objective: GoalText,
        #[ts(type = "number")]
        owner_version: WatchSavedGoalVersion,
        state: WatchGoalState,
        usage: WatchGoalUsage,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        GoalText, GoalVersion, WatchGoalCapture, WatchGoalState, WatchGoalUsage,
        WatchSavedGoalVersion,
    };

    #[test]
    fn reconstructed_captures_reject_invalid_raw_facts() {
        assert!(WatchSavedGoalVersion::parse(0).is_err());
        assert!(WatchSavedGoalVersion::parse(-1).is_err());
        assert_eq!(WatchSavedGoalVersion::parse(2).unwrap().get(), 2);
        let saved = serde_json::json!({"kind":"captured_saved_goal","owner_id":"owner","goal_id":"goal","objective":"ship it","owner_version":4,"state":{"kind":"blocked","reason":"waiting"},"usage":"unavailable_not_captured"});
        for (field, invalid) in [
            ("owner_id", serde_json::json!(" \t")),
            ("goal_id", serde_json::json!("")),
            ("objective", serde_json::json!("\n")),
            ("owner_version", serde_json::json!(0)),
            ("owner_version", serde_json::json!(-1)),
        ] {
            let mut bad = saved.clone();
            bad[field] = invalid;
            assert!(
                serde_json::from_value::<WatchGoalCapture>(bad).is_err(),
                "{field}"
            );
        }
        let mut bad = saved.clone();
        bad["state"]["reason"] = serde_json::json!("  ");
        assert!(serde_json::from_value::<WatchGoalCapture>(bad).is_err());
        assert!(serde_json::from_value::<WatchGoalState>(
            serde_json::json!({"kind":"suspended","reason":""})
        )
        .is_err());
        let valid: WatchGoalCapture = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(serde_json::to_value(&valid).unwrap(), saved);
        let no_goal = serde_json::json!({"kind":"captured_no_goal","owner_id":"owner","owner_version":0,"usage":"unavailable_not_captured"});
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<WatchGoalCapture>(no_goal.clone()).unwrap()
            )
            .unwrap(),
            no_goal
        );
        for (field, invalid) in [
            ("owner_id", serde_json::json!("")),
            ("owner_version", serde_json::json!(-1)),
        ] {
            let mut bad = no_goal.clone();
            bad[field] = invalid;
            assert!(
                serde_json::from_value::<WatchGoalCapture>(bad).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn state_variants_keep_their_tagged_wire_shapes() {
        let cases = [
            (
                WatchGoalState::Underway,
                serde_json::json!({"kind":"underway"}),
            ),
            (
                WatchGoalState::Blocked {
                    reason: GoalText::parse("waiting").unwrap(),
                },
                serde_json::json!({"kind":"blocked","reason":"waiting"}),
            ),
            (
                WatchGoalState::Complete {
                    reason: GoalText::parse("done").unwrap(),
                },
                serde_json::json!({"kind":"complete","reason":"done"}),
            ),
            (
                WatchGoalState::Exhausted {
                    reason: GoalText::parse("limit").unwrap(),
                },
                serde_json::json!({"kind":"exhausted","reason":"limit"}),
            ),
            (
                WatchGoalState::Suspended {
                    reason: GoalText::parse("paused").unwrap(),
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
                    owner_id: GoalText::parse("owner").unwrap(),
                    owner_version: GoalVersion::parse(3).unwrap(),
                    usage: WatchGoalUsage::UnavailableNotCaptured,
                },
                serde_json::json!({"kind":"captured_no_goal","owner_id":"owner","owner_version":3,"usage":"unavailable_not_captured"}),
            ),
            (
                WatchGoalCapture::CapturedSavedGoal {
                    owner_id: GoalText::parse("owner").unwrap(),
                    goal_id: GoalText::parse("goal").unwrap(),
                    objective: GoalText::parse("ship it").unwrap(),
                    owner_version: WatchSavedGoalVersion::parse(4).unwrap(),
                    state: WatchGoalState::Blocked {
                        reason: GoalText::parse("waiting").unwrap(),
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
