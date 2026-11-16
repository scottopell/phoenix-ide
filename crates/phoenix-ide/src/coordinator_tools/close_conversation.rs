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
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    Initial { product_conversation_id: String },
    Observe { product_conversation_id: String, attempt_id: String, run_ordinal: i64 },
    ExplicitRetry { product_conversation_id: String, attempt_id: String, failed_run_ordinal: i64 },
}

fn parse(input: &Value) -> Result<NativeCloseIntent, String> {
    let raw: Input = serde_json::from_value(input.clone()).map_err(|error| error.to_string())?;
    let intent = match raw {
        Input::Initial { product_conversation_id } => NativeCloseIntent::Initial {
            target: ProductConversationId::parse(product_conversation_id).map_err(|e| e.to_string())?,
        },
        Input::Observe { product_conversation_id, attempt_id, run_ordinal } => NativeCloseIntent::Observe {
            target: ProductConversationId::parse(product_conversation_id).map_err(|e| e.to_string())?,
            attempt_id: CloseAttemptId::parse(attempt_id).map_err(|e| e.to_string())?,
            run_ordinal: CloseRunOrdinal::parse(run_ordinal).map_err(|e| e.to_string())?,
        },
        Input::ExplicitRetry { product_conversation_id, attempt_id, failed_run_ordinal } => NativeCloseIntent::ExplicitRetry {
            target: ProductConversationId::parse(product_conversation_id).map_err(|e| e.to_string())?,
            attempt_id: CloseAttemptId::parse(attempt_id).map_err(|e| e.to_string())?,
            failed_run_ordinal: CloseRunOrdinal::parse(failed_run_ordinal).map_err(|e| e.to_string())?,
        },
    };
    if &intent.source_input() != input {
        return Err("Close input must exactly match its canonical action coordinates".into());
    }
    Ok(intent)
}

fn schema() -> Value {
    let target = json!({"type":"string", "minLength":1});
    let ordinal = json!({"type":"integer", "minimum":1, "maximum":i64::MAX});
    json!({
        "type":"object",
        "properties": {
            "action":{"type":"string", "enum":["initial", "observe", "explicit_retry"]},
            "product_conversation_id":target,
            "attempt_id":{"type":"string", "minLength":1},
            "run_ordinal":ordinal,
            "failed_run_ordinal":ordinal
        },
        "required":["action", "product_conversation_id"],
        "additionalProperties":false,
        "oneOf":[
            {"properties":{"action":{"const":"initial"}},
             "not":{"anyOf":[{"required":["attempt_id"]},{"required":["run_ordinal"]},{"required":["failed_run_ordinal"]}]}},
            {"properties":{"action":{"const":"observe"}},
             "required":["attempt_id","run_ordinal"], "not":{"required":["failed_run_ordinal"]}},
            {"properties":{"action":{"const":"explicit_retry"}},
             "required":["attempt_id","failed_run_ordinal"], "not":{"required":["run_ordinal"]}}
        ]
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
            NativeCloseInitialResult::NewlyAdmitted(value) => receipt("initial", "newly_admitted", &value),
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
            NativeCloseRetryResult::NewlyAdmitted(value) => receipt("explicit_retry", "newly_admitted", &value),
            NativeCloseRetryResult::Replay(value) => receipt("explicit_retry", "replay", &value),
        },
    }
}

fn application_error(error: NativeCloseApplicationError) -> ToolOutput {
    let code = match error {
        NativeCloseApplicationError::SourceRefused => "source_refused",
        NativeCloseApplicationError::Unavailable => "unavailable",
        NativeCloseApplicationError::Storage => "storage_rejected",
        NativeCloseApplicationError::FatalAuthority => "fatal_authority",
    };
    ToolOutput::error(json!({"code":code, "message":error.to_string()}).to_string())
}

#[async_trait]
impl Tool for CloseConversation {
    fn name(&self) -> &'static str { "close_conversation" }

    fn description(&self) -> String {
        "Native Coordinator Close: initial admits a new exact operation; observe reads an exact retained run; explicit_retry freshly validates and retries only the named failed run's retained resources. Use ProductConversation IDs, not transcript aliases. Receipts identify admission, not completed shutdown or cleanup. The server derives request identity from the live tool occurrence; replay never redispatches. No caller safety flags, automatic retry loop, or implicit latest-run selection.".into()
    }

    fn input_schema(&self) -> Value { schema() }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let Some(source) = ctx.source_tool_call() else {
            return application_error(NativeCloseApplicationError::SourceRefused);
        };
        let intent = match parse(&input) {
            Ok(intent) => intent,
            Err(error) => return ToolOutput::error(json!({"code":"invalid_input", "message":error}).to_string()),
        };
        match self.0.close_conversation(&ctx.conversation_id, source, intent).await {
            Ok(result) => ToolOutput::success(output(result).to_string()),
            Err(error) => application_error(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_close_parser_accepts_only_exact_action_coordinates() {
        for input in [
            json!({"action":"initial","product_conversation_id":"target"}),
            json!({"action":"observe","product_conversation_id":"target","attempt_id":"attempt","run_ordinal":1}),
            json!({"action":"explicit_retry","product_conversation_id":"target","attempt_id":"attempt","failed_run_ordinal":2}),
        ] {
            assert_eq!(parse(&input).unwrap().source_input(), input);
            for key in ["source", "request_id", "authorizer", "target_conversation_id", "safety_evidence"] {
                let mut extra = input.clone();
                extra[key] = json!("untrusted");
                assert!(parse(&extra).is_err(), "accepted {key}");
            }
        }
        for input in [
            json!({"product_conversation_id":"target"}),
            json!({"action":"initial","product_conversation_id":"target","attempt_id":null}),
            json!({"action":"initial","product_conversation_id":"target","run_ordinal":1}),
            json!({"action":"observe","product_conversation_id":"target","attempt_id":"attempt"}),
            json!({"action":"explicit_retry","product_conversation_id":"target","attempt_id":"attempt","failed_run_ordinal":0}),
            json!({"action":"explicit_retry","product_conversation_id":"target","attempt_id":"attempt","failed_run_ordinal":-1}),
            json!({"action":"explicit_retry","product_conversation_id":"target","attempt_id":"attempt","failed_run_ordinal":2.0}),
            json!({"action":"observe","product_conversation_id":"target","attempt_id":"attempt","run_ordinal":18446744073709551615_u64}),
            json!({"action":"observe","product_conversation_id":"target","attempt_id":"attempt","run_ordinal":1,"failed_run_ordinal":1}),
        ] { assert!(parse(&input).is_err(), "accepted {input}"); }
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
        let result = output(NativeCloseApplicationResult::ExplicitRetry(NativeCloseRetryResult::Replay(value)));
        assert_eq!(result["status"], "replay");
        assert_eq!(result["admitted"]["run_ordinal"], 2);
        assert!(result.get("closed").is_none());
        assert!(result.get("current_operation").is_none());
    }
}
