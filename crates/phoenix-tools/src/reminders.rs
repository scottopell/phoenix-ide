//! REQ-REM-001/002/003/006: host-bound, immediately acknowledged reminder operations.
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, LocalResult, NaiveDateTime, Offset, TimeZone, Utc};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::reminder::{
    ReminderId, ReminderRevision, ReminderText, ReminderTimestampUs, ReminderTimezone,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{Tool, ToolContext, ToolOutput};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReminderWhen {
    After {
        seconds: u32,
    },
    At {
        local_datetime: String,
        timezone: ReminderTimezone,
        utc_offset: Option<String>,
    },
}

impl ReminderWhen {
    /// # Errors
    /// Rejects invalid dates, DST gaps, unresolved folds and mismatching offsets.
    pub fn resolve(
        &self,
        now: DateTime<Utc>,
    ) -> Result<(ReminderTimestampUs, ReminderTimezone), String> {
        let (instant, zone) = match self {
            Self::After { seconds } => {
                if *seconds == 0 {
                    return Err("seconds must be positive".into());
                }
                (
                    now.checked_add_signed(chrono::Duration::seconds(i64::from(*seconds)))
                        .ok_or("duration exceeds supported dates")?,
                    ReminderTimezone::parse("UTC").map_err(|e| e.to_string())?,
                )
            }
            Self::At {
                local_datetime,
                timezone,
                utc_offset,
            } => {
                let local = NaiveDateTime::parse_from_str(local_datetime, "%Y-%m-%dT%H:%M:%S")
                    .map_err(|_| "local_datetime must be YYYY-MM-DDTHH:MM:SS with no offset")?;
                let zone: chrono_tz::Tz = timezone
                    .as_str()
                    .parse()
                    .map_err(|_| "invalid IANA timezone")?;
                let candidates = match zone.from_local_datetime(&local) {
                    LocalResult::None => return Err("This local time does not exist because the clock changes. Choose a valid time before or after the DST gap.".into()),
                    LocalResult::Single(value) => vec![value],
                    LocalResult::Ambiguous(a,b) => vec![a,b],
                };
                let offsets: Vec<String> = candidates
                    .iter()
                    .map(|c| c.offset().fix().to_string())
                    .collect();
                let selected = if let Some(offset) = utc_offset {
                    candidates.iter().find(|c| c.offset().fix().to_string() == *offset)
                        .ok_or_else(|| format!("utc_offset must match the timezone at this local time. Valid offsets: {}", offsets.join(", ")))?
                } else if candidates.len() == 1 {
                    &candidates[0]
                } else {
                    return Err(format!(
                        "This local time occurs twice. Supply utc_offset with one of: {}",
                        offsets.join(", ")
                    ));
                };
                (selected.with_timezone(&Utc), timezone.clone())
            }
        };
        Ok((
            ReminderTimestampUs::parse(instant.timestamp_micros()).map_err(|e| e.to_string())?,
            zone,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReminderOperation {
    Create {
        text: ReminderText,
        when: ReminderWhen,
    },
    List {},
    Reschedule {
        reminder_id: ReminderId,
        expected_revision: ReminderRevision,
        when: ReminderWhen,
    },
    Cancel {
        reminder_id: ReminderId,
        expected_revision: ReminderRevision,
    },
}

#[async_trait]
pub trait ReminderService: Send + Sync {
    /// Execute within the host-bound conversation using the trusted invocation identity.
    /// # Errors
    /// Returns actionable validation, ownership, conflict or persistence failures.
    async fn execute(
        &self,
        operation: ReminderOperation,
        source_call: &SourceToolCall,
    ) -> Result<Value, String>;
}

pub struct RemindersTool {
    service: Arc<dyn ReminderService>,
}
impl RemindersTool {
    #[must_use]
    pub fn new(service: Arc<dyn ReminderService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Tool for RemindersTool {
    fn name(&self) -> &'static str {
        "reminders"
    }
    fn description(&self) -> String {
        "Create, list, reschedule or cancel one-shot reminders for this conversation. Use list to obtain current UTC time before resolving calendar-relative requests. Specify an explicit local date/time and IANA timezone for calendar reminders; never assume the server timezone. Creation commits immediately and returns a receipt; finish your response normally. Claim scheduling only after success. When due, Phoenix shows the reminder and submits its text as ordinary conversation input, including normal busy queueing. Connected browser notifications follow user settings; closed-app phone alerts are unavailable. No arbitrary executable callbacks or cross-conversation targeting. Reschedule/cancel require the revision from a receipt/list. Due reminders cannot be retracted.".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","additionalProperties":false,"required":["op"],"properties":{
            "op":{"type":"string","enum":["create","list","reschedule","cancel"],"description":"create requires text and when; list accepts only op; reschedule requires reminder_id, expected_revision and when; cancel requires reminder_id and expected_revision."},
            "text":{"type":"string","minLength":1,"maxLength":4096},
            "reminder_id":{"type":"string","minLength":1},"expected_revision":{"type":"integer","minimum":1},
            "when":{"oneOf":[
                {"type":"object","additionalProperties":false,"required":["kind","seconds"],"properties":{"kind":{"const":"after"},"seconds":{"type":"integer","minimum":1,"maximum":u32::MAX}}},
                {"type":"object","additionalProperties":false,"required":["kind","local_datetime","timezone"],"properties":{"kind":{"const":"at"},"local_datetime":{"type":"string","description":"YYYY-MM-DDTHH:MM:SS, no offset"},"timezone":{"type":"string","description":"IANA timezone, e.g. America/New_York"},"utc_offset":{"type":"string","description":"Optional DST-fold choice, e.g. -04:00"}}}
            ]}
        }})
    }
    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let operation = match serde_json::from_value::<ReminderOperation>(input) {
            Ok(operation) => operation,
            Err(error) => return ToolOutput::error(format!("Invalid reminder operation: {error}. create requires text and when; list has no other fields; reschedule requires reminder_id, expected_revision and when; cancel requires reminder_id and expected_revision.")),
        };
        let Some(source_call) = ctx.source_tool_call() else {
            return ToolOutput::error(
                "Reminder operations require the trusted tool invocation identity.",
            );
        };
        match self.service.execute(operation, &source_call).await {
            Ok(receipt) => ToolOutput::success(receipt.to_string()),
            Err(error) => ToolOutput::error(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(time: &str, offset: Option<&str>) -> ReminderWhen {
        ReminderWhen::At {
            local_datetime: time.into(),
            timezone: ReminderTimezone::parse("America/New_York").unwrap(),
            utc_offset: offset.map(str::to_owned),
        }
    }
    #[test]
    fn resolves_dst_gap_fold_and_offset() {
        let now = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(at("2026-03-08T02:30:00", None)
            .resolve(now)
            .unwrap_err()
            .contains("does not exist"));
        let fold = at("2026-11-01T01:30:00", None).resolve(now).unwrap_err();
        assert!(fold.contains("-04:00") && fold.contains("-05:00"));
        let a = at("2026-11-01T01:30:00", Some("-04:00"))
            .resolve(now)
            .unwrap()
            .0;
        let b = at("2026-11-01T01:30:00", Some("-05:00"))
            .resolve(now)
            .unwrap()
            .0;
        assert_eq!(b.get() - a.get(), 3_600_000_000);
        assert!(at("2026-11-01T01:30:00", Some("-06:00"))
            .resolve(now)
            .is_err());
        assert!(at("2026-07-01T12:00:00", Some("-05:00"))
            .resolve(now)
            .is_err());
    }
    #[test]
    fn strict_operations_reject_targets_and_mixed_fields() {
        for value in [
            json!({"op":"list","conversation_id":"other"}),
            json!({"op":"create","text":"x","when":{"kind":"after","seconds":10},"expected_revision":1}),
            json!({"op":"cancel","reminder_id":"a"}),
        ] {
            assert!(serde_json::from_value::<ReminderOperation>(value).is_err());
        }
    }
}

#[cfg(test)]
mod registration_tests {
    use super::*;
    struct UnusedService;
    #[async_trait]
    impl ReminderService for UnusedService {
        async fn execute(&self, _: ReminderOperation, _: &SourceToolCall) -> Result<Value, String> {
            unreachable!()
        }
    }
    #[test]
    fn schema_has_provider_compatible_object_root_and_host_registration_excludes_subagents() {
        let tool: Arc<dyn Tool> = Arc::new(RemindersTool::new(Arc::new(UnusedService)));
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["op"]));
        assert!(schema.get("oneOf").is_none() && schema.get("anyOf").is_none());
        let policy = crate::ExploreToolPolicy::from_platform(
            &phoenix_core::platform::PlatformCapability::None {
                details: "test".into(),
            },
        );
        for registry in [
            crate::ToolRegistry::direct(Vec::new()),
            crate::ToolRegistry::explore("tasks", Vec::new(), policy),
        ] {
            assert!(registry
                .try_with_host_bound_tool(tool.clone())
                .unwrap()
                .definitions()
                .iter()
                .any(|d| d.name == "reminders"));
        }
        assert!(crate::ToolRegistry::coordinator(vec![tool], None)
            .definitions()
            .iter()
            .any(|d| d.name == "reminders"));
        assert!(!crate::ToolRegistry::for_subagent_explore(policy)
            .definitions()
            .iter()
            .any(|d| d.name == "reminders"));
        assert!(!crate::ToolRegistry::for_subagent_work()
            .definitions()
            .iter()
            .any(|d| d.name == "reminders"));
    }
}
