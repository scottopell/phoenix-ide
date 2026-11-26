//! Thin native ingress. Receipts acknowledge admission, not completed retirement.

use std::sync::Arc;

use async_trait::async_trait;
use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal};
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::native_close::{
    NativeCloseInitialResult, NativeCloseIntent, NativeCloseObservation, NativeCloseObserveResult,
    NativeCloseReceipt, NativeCloseRetryResult, NativeCloseRunRef,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::runtime::{NativeCloseApplicationError, NativeCloseApplicationResult};
use crate::send_chat_service::SendChatApplicationService;
use crate::tools::{Tool, ToolContext, ToolOutput};

pub(super) struct CloseConversation(pub Arc<SendChatApplicationService>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputEnvelope {
    request: Input,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    Initial {
        product_conversation_id: String,
    },
    Observe {
        product_conversation_id: String,
        attempt_id: String,
        run_ordinal: i64,
    },
    ExplicitRetry {
        product_conversation_id: String,
        attempt_id: String,
        failed_run_ordinal: i64,
    },
}

fn parse(input: &Value) -> Result<NativeCloseIntent, String> {
    // Publish only the envelope, but retain exact legacy occurrences without
    // normalizing their wire representation or source identity.
    let raw = match serde_json::from_value::<InputEnvelope>(input.clone()) {
        Ok(envelope) => envelope.request,
        Err(_) => {
            serde_json::from_value::<Input>(input.clone()).map_err(|error| error.to_string())?
        }
    };
    let intent = match raw {
        Input::Initial {
            product_conversation_id,
        } => NativeCloseIntent::Initial {
            target: ProductConversationId::parse(product_conversation_id)
                .map_err(|e| e.to_string())?,
        },
        Input::Observe {
            product_conversation_id,
            attempt_id,
            run_ordinal,
        } => NativeCloseIntent::Observe {
            target: ProductConversationId::parse(product_conversation_id)
                .map_err(|e| e.to_string())?,
            attempt_id: CloseAttemptId::parse(attempt_id).map_err(|e| e.to_string())?,
            run_ordinal: CloseRunOrdinal::parse(run_ordinal).map_err(|e| e.to_string())?,
        },
        Input::ExplicitRetry {
            product_conversation_id,
            attempt_id,
            failed_run_ordinal,
        } => NativeCloseIntent::ExplicitRetry {
            target: ProductConversationId::parse(product_conversation_id)
                .map_err(|e| e.to_string())?,
            attempt_id: CloseAttemptId::parse(attempt_id).map_err(|e| e.to_string())?,
            failed_run_ordinal: CloseRunOrdinal::parse(failed_run_ordinal)
                .map_err(|e| e.to_string())?,
        },
    };
    if !intent.matches_source_input(input) {
        return Err("Close input must exactly match its canonical action coordinates".into());
    }
    Ok(intent)
}

// Provider schemas support nested anyOf, not root unions or oneOf. The action
// enums keep these closed variants disjoint, with no nullable coordinates.
fn schema() -> Value {
    let target = json!({"type":"string", "minLength":1});
    let attempt = json!({"type":"string", "minLength":1});
    let ordinal = json!({"type":"integer", "minimum":1, "maximum":i64::MAX});
    json!({
        "type":"object",
        "properties": {
            "request": {"anyOf": [
                {
                    "type":"object",
                    "properties": {
                        "action":{"type":"string", "enum":["initial"]},
                        "product_conversation_id":target
                    },
                    "required":["action", "product_conversation_id"],
                    "additionalProperties":false
                },
                {
                    "type":"object",
                    "properties": {
                        "action":{"type":"string", "enum":["observe"]},
                        "product_conversation_id":target,
                        "attempt_id":attempt,
                        "run_ordinal":ordinal
                    },
                    "required":["action", "product_conversation_id", "attempt_id", "run_ordinal"],
                    "additionalProperties":false
                },
                {
                    "type":"object",
                    "properties": {
                        "action":{"type":"string", "enum":["explicit_retry"]},
                        "product_conversation_id":target,
                        "attempt_id":attempt,
                        "failed_run_ordinal":ordinal
                    },
                    "required":["action", "product_conversation_id", "attempt_id", "failed_run_ordinal"],
                    "additionalProperties":false
                }
            ]}
        },
        "required":["request"],
        "additionalProperties":false
    })
}

fn reference(value: &NativeCloseRunRef) -> Value {
    json!({"product_conversation_id":value.target.as_str(),
        "attempt_id":value.attempt_id.as_str(), "run_ordinal":value.run_ordinal.get()})
}

fn receipt(action: &str, status: &str, value: &NativeCloseReceipt) -> Value {
    json!({"action":action, "status":status, "request_id":value.request_id,
        "admitted":reference(&value.admitted)})
}

fn observation(status: &str, value: &NativeCloseObservation) -> Value {
    json!({"action":"observe", "status":status, "request_id":value.receipt.request_id,
        "referenced":reference(&value.receipt.referenced),
        "current_referenced_run_status":value.current_referenced_run_status,
        "current_operation":value.current_operation})
}

fn output(result: NativeCloseApplicationResult) -> Value {
    match result {
        NativeCloseApplicationResult::Initial(result) => match result {
            NativeCloseInitialResult::NewlyAdmitted(value) => {
                receipt("initial", "newly_admitted", &value)
            }
            NativeCloseInitialResult::Replay(value) => receipt("initial", "replay", &value),
            NativeCloseInitialResult::ExistingOperationNeedsExactReference(value) => json!({
                "action":"initial", "status":"needs_exact_reference", "existing":reference(&value)
            }),
        },
        NativeCloseApplicationResult::Observe(result) => match result {
            NativeCloseObserveResult::Observed(value) => observation("observed", &value),
            NativeCloseObserveResult::Replay(value) => observation("replay", &value),
        },
        NativeCloseApplicationResult::ExplicitRetry(result) => match result {
            NativeCloseRetryResult::NewlyAdmitted(value) => {
                receipt("explicit_retry", "newly_admitted", &value)
            }
            NativeCloseRetryResult::Replay(value) => receipt("explicit_retry", "replay", &value),
        },
    }
}

fn application_error(error: &NativeCloseApplicationError) -> ToolOutput {
    let code = match error {
        NativeCloseApplicationError::RetryInspectionBlocked(gap) => {
            return ToolOutput::error(
                json!({
                    "code":"retry_inspection_blocked",
                    "message":error.to_string(),
                    "facts": {
                        "path":gap.path.display().to_string(),
                        "detector":gap.detector.as_str(),
                        "reason":gap.reason.as_str(),
                        "pid":gap.pid,
                        "new_run_admitted":false,
                        "disk_contents_retained":true,
                        "requires_explicit_fresh_retry":true
                    }
                })
                .to_string(),
            );
        }
        NativeCloseApplicationError::SourceRefused => "source_refused",
        NativeCloseApplicationError::Unavailable => "unavailable",
        NativeCloseApplicationError::Storage => "storage_rejected",
        NativeCloseApplicationError::FatalAuthority => "fatal_authority",
    };
    ToolOutput::error(json!({"code":code, "message":error.to_string()}).to_string())
}

#[async_trait]
impl Tool for CloseConversation {
    fn foreground_execution(&self) -> phoenix_tools::ForegroundExecution {
        phoenix_tools::ForegroundExecution::Serial
    }

    fn name(&self) -> &'static str {
        "close_conversation"
    }

    fn description(&self) -> String {
        "Native Coordinator Close: initial admits a new exact operation; observe reads an exact retained run; explicit_retry freshly validates and retries only the named failed run's retained resources. Use ProductConversation IDs, not transcript aliases. Receipts identify admission, not completed shutdown or cleanup. The server derives request identity from the live tool occurrence; replay never redispatches. No caller safety flags, automatic retry loop, or implicit latest-run selection.".into()
    }

    fn input_schema(&self) -> Value {
        schema()
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let Some(source) = ctx.source_tool_call() else {
            return application_error(&NativeCloseApplicationError::SourceRefused);
        };
        let intent = match parse(&input) {
            Ok(intent) => intent,
            Err(error) => {
                return ToolOutput::error(
                    json!({"code":"invalid_input", "message":error}).to_string(),
                )
            }
        };
        match self
            .0
            .close_conversation(&ctx.conversation_id, source, intent)
            .await
        {
            Ok(result) => ToolOutput::success(output(result).to_string()),
            Err(error) => application_error(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_inspection_error_exposes_only_typed_gap_facts() {
        let error = NativeCloseApplicationError::RetryInspectionBlocked(
            crate::runtime::close_retirement::WriterInspectionGap {
                path: std::path::PathBuf::from("/retained/worktree"),
                detector: crate::db::CloseWriterDetector::LinuxDescriptor,
                reason: crate::db::CloseWriterGap::Unavailable,
                pid: None,
            },
        );
        let rendered = application_error(&error);
        let value: Value = serde_json::from_str(rendered.output()).unwrap();
        assert_eq!(value["code"], "retry_inspection_blocked");
        assert_eq!(value["facts"]["path"], "/retained/worktree");
        assert_eq!(value["facts"]["detector"], "linux_descriptor");
        assert_eq!(value["facts"]["reason"], "unavailable");
        assert!(value["facts"]["pid"].is_null());
        assert_eq!(value["facts"]["new_run_admitted"], false);
        assert_eq!(value["facts"]["disk_contents_retained"], true);
        assert_eq!(value["facts"]["requires_explicit_fresh_retry"], true);
        assert!(value["facts"].get("permission").is_none());
        assert!(value["facts"].get("syscall_stage").is_none());
    }

    #[test]
    fn storage_and_fatal_errors_keep_existing_codes() {
        for (error, code) in [
            (NativeCloseApplicationError::Storage, "storage_rejected"),
            (
                NativeCloseApplicationError::FatalAuthority,
                "fatal_authority",
            ),
        ] {
            let rendered = application_error(&error);
            let value: Value = serde_json::from_str(rendered.output()).unwrap();
            assert_eq!(value["code"], code);
            assert!(value.get("facts").is_none());
        }
    }

    #[test]
    fn native_close_parser_accepts_only_exact_action_coordinates() {
        for input in [
            json!({"action":"initial","product_conversation_id":"target"}),
            json!({"action":"observe","product_conversation_id":"target","attempt_id":"attempt","run_ordinal":1}),
            json!({"action":"explicit_retry","product_conversation_id":"target","attempt_id":"attempt","failed_run_ordinal":2}),
        ] {
            let wrapped = json!({"request":input});
            assert_eq!(parse(&wrapped).unwrap().source_input(), wrapped);
            let legacy = parse(&input).unwrap();
            assert!(legacy.matches_source_input(&input));
            assert_eq!(legacy.source_input(), wrapped);
            for field in input.as_object().unwrap().keys() {
                let mut missing = input.clone();
                missing.as_object_mut().unwrap().remove(field);
                let mut null = input.clone();
                null[field] = Value::Null;
                for invalid in [missing, null] {
                    assert!(parse(&invalid).is_err(), "accepted {invalid}");
                    assert!(parse(&json!({"request":invalid})).is_err());
                }
            }
            for key in [
                "source",
                "request_id",
                "authorizer",
                "target_conversation_id",
                "safety_evidence",
            ] {
                let mut extra = input.clone();
                extra[key] = json!("untrusted");
                assert!(parse(&extra).is_err(), "accepted {key}");
                assert!(parse(&json!({"request":extra})).is_err());
                let mut extra_root = wrapped.clone();
                extra_root[key] = json!("untrusted");
                assert!(parse(&extra_root).is_err(), "accepted root {key}");
            }
        }
        for input in [
            json!({"product_conversation_id":"target"}),
            json!({"action":"unknown","product_conversation_id":"target"}),
            json!({"action":"initial","product_conversation_id":""}),
            json!({"action":"initial","product_conversation_id":null}),
            json!({"action":"initial","product_conversation_id":"target","attempt_id":null}),
            json!({"action":"initial","product_conversation_id":"target","run_ordinal":1}),
            json!({"action":"observe","product_conversation_id":"target","attempt_id":"attempt"}),
            json!({"action":"explicit_retry","product_conversation_id":"target","attempt_id":"attempt","failed_run_ordinal":0}),
            json!({"action":"explicit_retry","product_conversation_id":"target","attempt_id":"attempt","failed_run_ordinal":-1}),
            json!({"action":"explicit_retry","product_conversation_id":"target","attempt_id":"attempt","failed_run_ordinal":2.0}),
            json!({"action":"observe","product_conversation_id":"target","attempt_id":"attempt","run_ordinal":u64::MAX}),
            json!({"action":"observe","product_conversation_id":"target","attempt_id":"attempt","run_ordinal":1,"failed_run_ordinal":1}),
        ] {
            assert!(parse(&input).is_err(), "accepted {input}");
            assert!(parse(&json!({"request":input})).is_err());
        }
        for input in [
            Value::Null,
            json!({}),
            json!({"request":null}),
            json!({"request":[]}),
            json!({"request":{"request":{"action":"initial","product_conversation_id":"target"}}}),
            json!({"request":{"action":"initial","product_conversation_id":"target"},"action":"initial"}),
        ] {
            assert!(parse(&input).is_err(), "accepted {input}");
        }
    }

    #[test]
    fn native_close_parser_preserves_whitespace_attempt_ids_in_both_wire_shapes() {
        for (action, ordinal) in [
            ("observe", "run_ordinal"),
            ("explicit_retry", "failed_run_ordinal"),
        ] {
            // Persisted attempt IDs are exact nonempty strings, not trimmed identifiers.
            for attempt_id in ["   ", " \tattempt\n "] {
                let input = json!({
                    "action": action,
                    "product_conversation_id": "target",
                    "attempt_id": attempt_id,
                    ordinal: 1
                });
                let wrapped = json!({"request": input});
                for source in [&input, &wrapped] {
                    let intent = parse(source).unwrap();
                    assert!(intent.matches_source_input(source));
                    assert_eq!(intent.source_input(), wrapped);
                    assert_eq!(intent.source_input()["request"]["attempt_id"], attempt_id);
                }
            }
        }
    }

    #[test]
    fn native_close_parser_rejects_invalid_coordinates_in_both_wire_shapes() {
        for (action, ordinal) in [
            ("observe", "run_ordinal"),
            ("explicit_retry", "failed_run_ordinal"),
        ] {
            let valid = json!({
                "action": action,
                "product_conversation_id": "target",
                "attempt_id": "attempt",
                ordinal: 1
            });
            for value in [json!(1), json!(i64::MAX)] {
                let mut input = valid.clone();
                input[ordinal] = value;
                assert!(parse(&input).is_ok());
                assert!(parse(&json!({"request":input})).is_ok());
            }
            for value in [
                Value::Null,
                json!(0),
                json!(-1),
                json!(2.0),
                json!(u64::MAX),
                json!("1"),
                json!(true),
            ] {
                let mut input = valid.clone();
                input[ordinal] = value;
                assert!(parse(&input).is_err(), "accepted {input}");
                assert!(parse(&json!({"request":input})).is_err());
            }
            for field in ["product_conversation_id", "attempt_id"] {
                for value in [json!(""), json!(1)] {
                    let mut input = valid.clone();
                    input[field] = value;
                    assert!(parse(&input).is_err(), "accepted {input}");
                    assert!(parse(&json!({"request":input})).is_err());
                }
            }
            let mut blank_target = valid.clone();
            blank_target["product_conversation_id"] = json!("   ");
            assert!(parse(&blank_target).is_err());
            assert!(parse(&json!({"request":blank_target})).is_err());
            let mut wrong = valid.clone();
            wrong.as_object_mut().unwrap().remove(ordinal);
            wrong[if action == "observe" {
                "failed_run_ordinal"
            } else {
                "run_ordinal"
            }] = json!(1);
            assert!(parse(&wrong).is_err());
            assert!(parse(&json!({"request":wrong})).is_err());
        }
    }

    #[test]
    fn native_close_schema_exposes_only_closed_required_request_variants() {
        let schema = schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["request"]));
        assert_eq!(schema["properties"].as_object().unwrap().len(), 1);
        for combinator in ["oneOf", "anyOf", "allOf"] {
            assert!(schema.get(combinator).is_none());
        }
        let request = &schema["properties"]["request"];
        assert_eq!(request.as_object().unwrap().len(), 1);
        let variants = request["anyOf"].as_array().unwrap();
        assert_eq!(variants.len(), 3);
        for (variant, action, fields) in [
            (
                &variants[0],
                "initial",
                vec!["action", "product_conversation_id"],
            ),
            (
                &variants[1],
                "observe",
                vec![
                    "action",
                    "product_conversation_id",
                    "attempt_id",
                    "run_ordinal",
                ],
            ),
            (
                &variants[2],
                "explicit_retry",
                vec![
                    "action",
                    "product_conversation_id",
                    "attempt_id",
                    "failed_run_ordinal",
                ],
            ),
        ] {
            assert_eq!(variant["type"], "object");
            assert_eq!(variant["additionalProperties"], false);
            assert_eq!(variant["required"], json!(fields));
            let properties = variant["properties"].as_object().unwrap();
            assert_eq!(properties.len(), fields.len());
            assert_eq!(
                properties["action"],
                json!({"type":"string","enum":[action]})
            );
            for field in fields.into_iter().filter(|field| *field != "action") {
                let expected = if field.ends_with("ordinal") {
                    json!({"type":"integer","minimum":1,"maximum":i64::MAX})
                } else {
                    json!({"type":"string","minLength":1})
                };
                assert_eq!(properties[field], expected);
            }
        }
    }

    #[test]
    fn native_close_receipt_output_does_not_claim_completion() {
        let value = NativeCloseReceipt {
            request_id: uuid::Uuid::nil(),
            admitted: NativeCloseRunRef {
                target: ProductConversationId::parse("target").unwrap(),
                attempt_id: CloseAttemptId::parse("attempt").unwrap(),
                run_ordinal: CloseRunOrdinal::parse(2).unwrap(),
            },
        };
        let result = output(NativeCloseApplicationResult::ExplicitRetry(
            NativeCloseRetryResult::Replay(value),
        ));
        assert_eq!(result["status"], "replay");
        assert_eq!(result["admitted"]["run_ordinal"], 2);
        assert!(result.get("closed").is_none());
        assert!(result.get("current_operation").is_none());
    }
}
