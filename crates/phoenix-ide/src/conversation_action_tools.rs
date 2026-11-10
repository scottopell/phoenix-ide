use std::sync::Arc;

use async_trait::async_trait;
use phoenix_core::domain::instance_identity::InstanceId;
use phoenix_core::domain::llm_types::ModelEffort;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::llm_language::LlmLanguage;
use phoenix_tools::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

use crate::api::global_read::deserialize_present_non_null;
use crate::api::{federation, CreateProductConversationRequest};
use crate::conversation_actions::{
    self, accept_create, CoordinatorRenameIntent, CreateSource, LocalCoordinatorSource,
};
use crate::coordinator_tools::instance::{Route, RouteTool, RoutedTool, SelfPeerPolicy};
use crate::db::ProductConversationRenameResult;
use crate::runtime::RuntimeManager;

mod automatic_continuation;
mod continuation;

pub(crate) async fn tools(
    runtime: Arc<RuntimeManager>,
    transcript_id: &str,
) -> Result<Vec<Arc<dyn Tool>>, crate::db::DbError> {
    match runtime
        .db()
        .require_current_coordinator_transcript(transcript_id)
        .await
    {
        Ok(()) => Ok(vec![
            RouteTool::boxed(RenameConversation(runtime.clone())),
            RouteTool::boxed(CreateConversation(runtime.clone())),
            RouteTool::boxed(continuation::ContinueConversation(runtime.clone())),
            Arc::new(automatic_continuation::AutomaticContinuationTool::read(
                runtime.clone(),
            )),
            Arc::new(automatic_continuation::AutomaticContinuationTool::set(
                runtime,
            )),
        ]),
        Err(crate::db::DbError::CurrentCoordinatorRequired) => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
struct OperationId(String);

impl<'de> Deserialize<'de> for OperationId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let parsed = uuid::Uuid::parse_str(&value).map_err(serde::de::Error::custom)?;
        if parsed.to_string() != value {
            return Err(serde::de::Error::custom(
                "use a canonical lowercase hyphenated UUID",
            ));
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum RenameRequest {
    Detail {
        reference: String,
    },
    Read {
        operation_id: OperationId,
    },
    Rename {
        operation_id: OperationId,
        product_conversation_id: ProductConversationId,
        expected_root_transcript_id: String,
        expected_name_revision: i64,
        title: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameEnvelope {
    request: RenameRequest,
}

fn parse_rename_request(input: Value) -> Result<RenameRequest, String> {
    serde_json::from_value::<RenameEnvelope>(input)
        .map(|envelope| envelope.request)
        .map_err(|error| error.to_string())
}

fn rename_request_schema() -> Value {
    json!({"type":"object", "additionalProperties":false, "required":["request"], "properties":{
        "request":{"oneOf":[
            {"type":"object","additionalProperties":false,"required":["op","reference"],"properties":{
                "op":{"const":"detail"},
                "reference":{"type":"string","description":"Product id or slug.","minLength":1}
            }},
            {"type":"object","additionalProperties":false,"required":["op","operation_id"],"properties":{
                "op":{"const":"read"},"operation_id":operation_schema()
            }},
            {"type":"object","additionalProperties":false,"required":["op","operation_id","product_conversation_id","expected_root_transcript_id","expected_name_revision","title"],"properties":{
                "op":{"const":"rename"},"operation_id":operation_schema(),
                "product_conversation_id":{"type":"string","description":"Immutable product id returned by detail, never a slug or transcript.","minLength":1},
                "expected_root_transcript_id":{"type":"string","description":"Returned by detail.","minLength":1},
                "expected_name_revision":{"type":"integer","description":"Returned by detail.","minimum":0},
                "title":{"type":"string","minLength":1,"maxLength":200}
            }}
        ]}
    }})
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CreateStart {
    SubmitObjective,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum CreateRequest {
    Options {
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        cwd: Option<String>,
    },
    Read {
        request_id: OperationId,
    },
    Create {
        request_id: OperationId,
        cwd: String,
        objective: String,
        start: CreateStart,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        model: Option<String>,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        effort: Option<ModelEffort>,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        llm_language: Option<LlmLanguage>,
    },
}

struct RenameConversation(Arc<RuntimeManager>);
struct CreateConversation(Arc<RuntimeManager>);

fn output(result: Result<Value, String>) -> ToolOutput {
    match result {
        Ok(value) => ToolOutput::success(value.to_string()),
        Err(error) => ToolOutput::error(json!({"status": "error", "error": error}).to_string()),
    }
}

async fn mutation_source(
    runtime: &RuntimeManager,
    ctx: &ToolContext,
) -> Result<LocalCoordinatorSource, String> {
    runtime
        .db()
        .require_current_coordinator_transcript(&ctx.conversation_id)
        .await
        .map_err(|e| e.to_string())?;
    let call = ctx
        .source_tool_call()
        .ok_or("Trusted source tool-call identity unavailable")?;
    let source = runtime
        .db()
        .get_conversation(&ctx.conversation_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(LocalCoordinatorSource {
        product_conversation_id: source.product_conversation_id,
        transcript_id: source.id,
        source_call: call,
    })
}

async fn remote_dispatch_authority(
    runtime: Arc<RuntimeManager>,
    ctx: ToolContext,
) -> Result<tokio::sync::OwnedMutexGuard<()>, String> {
    let source = runtime
        .db()
        .get_conversation(&ctx.conversation_id)
        .await
        .map_err(|error| error.to_string())?;
    let authority = runtime
        .lock_product_message_admission(source.product_conversation_id.as_str())
        .await;
    runtime
        .db()
        .require_current_coordinator_transcript(&ctx.conversation_id)
        .await
        .map_err(|error| error.to_string())?;
    Ok(authority)
}

fn operation_schema() -> Value {
    json!({"type":"string", "format":"uuid", "pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"})
}

#[async_trait]
impl RoutedTool for RenameConversation {
    fn validate_before_peer_lookup(&self, input: &Value) -> Result<(), String> {
        let request = parse_rename_request(input.clone())?;
        if matches!(request, RenameRequest::Read { .. }) {
            return Err("Remote rename receipt read by operation_id alone is unsupported. If the outcome is uncertain, retry op=rename with the same operation_id, destination instance, product_conversation_id, expected_root_transcript_id, expected_name_revision, title, and source ProductConversation/transcript. A later assistant-message/tool-use locator is allowed for that same source owner; the original receipt remains authoritative. Never mint a replacement operation.".into());
        }
        Ok(())
    }
    fn name(&self) -> &'static str {
        "rename_conversation"
    }
    fn description(&self) -> String {
        "Current Global Coordinator only. Supply exactly one operation in request. Detail resolves a reference to immutable product/root identity and naming revision; use those exact values for rename. A rename updates the title (1-200 characters) and allocated slug, never the immutable product/work identity. Choose a new operation UUID for each intentional rename. Remote read by operation_id alone is unsupported; if rename outcome is uncertain, retry op=rename with the same operation UUID, destination instance, product/root/revision/title, and source ProductConversation/transcript. A later assistant-message/tool-use locator is allowed for that same source owner, and the original receipt remains authoritative. Never mint a replacement operation to recover uncertainty. Durable replay reports the historical receipt, not current title/lifecycle.".into()
    }
    fn schema_without_instance(&self) -> Value {
        rename_request_schema()
    }
    fn clearable(&self) -> bool {
        false
    }
    fn resolution_db(&self) -> &crate::db::Database {
        self.0.db()
    }
    fn self_peer_policy(&self) -> SelfPeerPolicy {
        SelfPeerPolicy::Reject
    }
    async fn authorize(&self, ctx: &ToolContext) -> Result<(), String> {
        self.0
            .db()
            .require_current_coordinator_transcript(&ctx.conversation_id)
            .await
            .map_err(|error| error.to_string())
    }
    async fn run_routed(
        &self,
        input: Value,
        route: Route<federation::ResolvedPeer>,
        ctx: ToolContext,
    ) -> ToolOutput {
        output(match route {
            Route::Local => self.run_inner(input, ctx).await,
            Route::Remote(peer) => self.run_remote(input, &peer, ctx).await,
        })
    }
}

fn rename_detail_reference(reference: &str) -> Result<String, String> {
    federation::conversation_lookup_identity(reference).map_err(|error| error.to_string())
}

fn rename_detail_response(detail: &crate::db::ProductConversationRenameDetail) -> Value {
    let route = crate::api::product_conversations::product_route(
        detail.slug.as_deref(),
        detail.product_conversation_id.as_str(),
    );
    json!({"status":"detail", "canonical_route":route, "detail":detail,
        "allowed_actions": if detail.can_rename { vec!["detail", "read", "rename"] } else { vec!["detail", "read"] }})
}

fn remote_rename_detail_response(response: federation::NativeRenameOptionsResponse) -> Value {
    let federation::NativeRenameOptionsResponse {
        caller_instance_id: _,
        destination_instance_id: _,
        correlation_id: _,
        canonical_route,
        detail,
        mut allowed_actions,
    } = response;
    allowed_actions.retain(|action| {
        if matches!(action, federation::NativeRenameDetailAction::Read) {
            tracing::debug!("omitting remote rename read action: operation_id alone cannot prove immutable intent");
            false
        } else {
            true
        }
    });
    json!({
        "status": "detail",
        "canonical_route": canonical_route,
        "detail": detail,
        "allowed_actions": allowed_actions,
        "next": "Use these exact product/root/revision values and this same instance for rename. Remote read by operation_id alone is unsupported. If outcome is uncertain, retry op=rename with this same operation_id, destination, product/root/revision/title, and source ProductConversation/transcript. A later assistant-message/tool-use locator is allowed for that same source owner; the original receipt remains authoritative. Never mint a replacement operation."
    })
}

fn remote_create_options_response(response: federation::NativeCreateOptionsResponse) -> Value {
    let federation::NativeCreateOptionsResponse {
        caller_instance_id: _,
        destination_instance_id: _,
        correlation_id: _,
        cwd,
        projects,
        projects_truncated,
        models,
        default_model,
        default_llm_language,
    } = response;
    json!({
        "status": "options",
        "cwd": cwd,
        "projects": projects,
        "projects_truncated": projects_truncated,
        "models": models,
        "default_model": default_model,
        "default_llm_language": default_llm_language,
    })
}

fn rename_response(result: ProductConversationRenameResult) -> Value {
    match result {
        ProductConversationRenameResult::Applied(receipt) => {
            json!({"status":"applied", "receipt":receipt})
        }
        ProductConversationRenameResult::Replayed(receipt) => {
            json!({"status":"replayed", "receipt":receipt, "historical":true})
        }
        ProductConversationRenameResult::Stale => {
            json!({"status":"stale", "next":"read detail; use a new operation UUID with a fresh root/revision"})
        }
        ProductConversationRenameResult::Conflict => json!({"status":"conflict"}),
        ProductConversationRenameResult::NotFound => json!({"status":"not_found"}),
        ProductConversationRenameResult::Unavailable => json!({"status":"unavailable"}),
    }
}

fn remote_rename_receipt_source(
    receipt_source_instance_id: InstanceId,
    receipt_source: federation::NativeActionSourceCall,
) -> phoenix_db::ProductConversationRenameReceiptSource {
    phoenix_db::ProductConversationRenameReceiptSource {
        source_instance_id: receipt_source_instance_id,
        product_conversation_id: receipt_source.product_conversation.as_str().to_owned(),
        transcript_id: receipt_source.source_transcript_id,
        message_id: receipt_source.source_assistant_message_id,
        tool_use_id: receipt_source.source_tool_use_id,
    }
}

fn remote_rename_receipt_result(
    receipt: phoenix_db::ProductConversationRenameReceipt,
    replayed: bool,
) -> ProductConversationRenameResult {
    if replayed {
        ProductConversationRenameResult::Replayed(receipt)
    } else {
        ProductConversationRenameResult::Applied(receipt)
    }
}

fn remote_rename_response(response: federation::NativeRenameResponse) -> Value {
    use federation::NativeRenameResponse as Response;
    use phoenix_db::ProductConversationRenameResult as RenameResult;

    let replayed = matches!(&response, Response::Replayed { .. });
    let result = match response {
        Response::Applied {
            caller_instance_id: _,
            destination_instance_id: _,
            operation_id,
            product_conversation_id,
            root_transcript_id,
            receipt_title,
            receipt_slug_at_commit,
            receipt_name_revision_before,
            receipt_committed_at_unix_micros,
            receipt_source_instance_id,
            receipt_source,
        }
        | Response::Replayed {
            caller_instance_id: _,
            destination_instance_id: _,
            operation_id,
            product_conversation_id,
            root_transcript_id,
            receipt_title,
            receipt_slug_at_commit,
            receipt_name_revision_before,
            receipt_committed_at_unix_micros,
            receipt_source_instance_id,
            receipt_source,
        } => {
            let receipt = phoenix_db::ProductConversationRenameReceipt {
                operation_id,
                product_conversation_id,
                title: receipt_title,
                slug_at_commit: receipt_slug_at_commit,
                naming_root_transcript_id: root_transcript_id,
                name_revision_before: receipt_name_revision_before,
                committed_at_unix_micros: receipt_committed_at_unix_micros,
                source: remote_rename_receipt_source(receipt_source_instance_id, receipt_source),
            };
            remote_rename_receipt_result(receipt, replayed)
        }
        Response::Stale {
            caller_instance_id: _,
            destination_instance_id: _,
            operation_id: _,
            product_conversation_id: _,
            root_transcript_id: _,
        } => RenameResult::Stale,
        Response::Conflict {
            caller_instance_id: _,
            destination_instance_id: _,
            operation_id: _,
            product_conversation_id: _,
            root_transcript_id: _,
        } => RenameResult::Conflict,
        Response::NotFound {
            caller_instance_id: _,
            destination_instance_id: _,
            operation_id: _,
            product_conversation_id: _,
            root_transcript_id: _,
        } => RenameResult::NotFound,
        Response::Unavailable {
            caller_instance_id: _,
            destination_instance_id: _,
            operation_id: _,
            product_conversation_id: _,
            root_transcript_id: _,
        } => RenameResult::Unavailable,
    };
    rename_response(result)
}

async fn remote_source(
    runtime: &RuntimeManager,
    ctx: &ToolContext,
) -> Result<federation::NativeActionSourceCall, String> {
    let source = mutation_source(runtime, ctx).await?;
    Ok(federation::NativeActionSourceCall {
        product_conversation: source.product_conversation_id,
        source_transcript_id: source.transcript_id,
        source_assistant_message_id: source.source_call.message_id,
        source_tool_use_id: source.source_call.tool_use_id,
    })
}

impl RenameConversation {
    async fn run_remote(
        &self,
        input: Value,
        peer: &federation::ResolvedPeer,
        ctx: ToolContext,
    ) -> Result<Value, String> {
        let request = parse_rename_request(input)?;
        self.authorize(&ctx).await?;
        let _dispatch_authority = remote_dispatch_authority(self.0.clone(), ctx.clone()).await?;
        let caller_instance_id = self
            .0
            .db()
            .instance_id()
            .await
            .map_err(|error| error.to_string())?;
        let destination_instance_id = peer.instance_id();
        match request {
            RenameRequest::Detail { reference } => {
                let request = federation::NativeRenameOptionsRequest {
                    caller_instance_id,
                    destination_instance_id,
                    correlation_id: uuid::Uuid::new_v4().to_string(),
                    reference,
                };
                let response = federation::remote_native_rename_options(self.0.db(), peer, &request)
                    .await.map_err(|error| error.to_string())?;
                Ok(remote_rename_detail_response(response))
            }
            RenameRequest::Read { .. } => Err("Remote rename receipt read by operation_id alone is unsupported. If the outcome is uncertain, retry op=rename with the same operation_id, destination instance, product_conversation_id, expected_root_transcript_id, expected_name_revision, title, and source ProductConversation/transcript. A later assistant-message/tool-use locator is allowed for that same source owner; the original receipt remains authoritative. Never mint a replacement operation.".into()),
            RenameRequest::Rename {
                operation_id,
                product_conversation_id,
                expected_root_transcript_id,
                expected_name_revision,
                title,
            } => {
                let request = federation::NativeRenameRequest {
                    caller_instance_id,
                    destination_instance_id,
                    operation_id: operation_id.0,
                    product_conversation_id,
                    expected_root_transcript_id,
                    expected_name_revision,
                    title,
                    source: remote_source(&self.0, &ctx).await?,
                };
                let response = federation::remote_native_rename(self.0.db(), peer, &request)
                    .await.map_err(|error| error.to_string())?;
                Ok(remote_rename_response(response))
            }
        }
    }

    async fn run_inner(&self, input: Value, ctx: ToolContext) -> Result<Value, String> {
        self.0
            .db()
            .require_current_coordinator_transcript(&ctx.conversation_id)
            .await
            .map_err(|e| e.to_string())?;
        let request = parse_rename_request(input)?;
        match request {
            RenameRequest::Detail { reference } => {
                let reference = rename_detail_reference(&reference)?;
                let detail = self
                    .0
                    .db()
                    .read_product_conversation_rename_detail(&ctx.conversation_id, &reference)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(rename_detail_response(&detail))
            }
            RenameRequest::Read { operation_id } => {
                let receipt = self
                    .0
                    .db()
                    .read_product_conversation_rename_receipt(&ctx.conversation_id, &operation_id.0)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(
                    json!({"status":if receipt.is_some() {"applied"} else {"unknown"}, "receipt":receipt, "historical":true}),
                )
            }
            RenameRequest::Rename {
                operation_id,
                product_conversation_id,
                expected_root_transcript_id,
                expected_name_revision,
                title,
            } => {
                let source = mutation_source(&self.0, &ctx).await?;
                let result = conversation_actions::rename_from_current_coordinator(
                    &self.0,
                    source,
                    CoordinatorRenameIntent {
                        operation_id: operation_id.0,
                        product_conversation_id,
                        expected_root_transcript_id,
                        expected_name_revision,
                        title,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
                Ok(rename_response(result))
            }
        }
    }
}

#[async_trait]
impl RoutedTool for CreateConversation {
    fn validate_before_peer_lookup(&self, input: &Value) -> Result<(), String> {
        parse_create_request(input).map(|_| ())
    }
    fn name(&self) -> &'static str {
        "create_conversation"
    }
    fn description(&self) -> String {
        "Current Global Coordinator only. Supply a single request object with op=options, read, or create and only that operation's fields. Explicitly create AND submit a nonblank text objective using start=submit_objective via the durable creation pipeline; dormant create is unsupported. Options discovers bounded project paths, normalized cwd, and available model/effort choices. Submit only an exact normalized absolute cwd. Objective text is literal agent content, never a user commission or slash command. Omit model/language to freeze current defaults on first acceptance; retry omission reuses those choices, while explicit different choices conflict. Omitted effort means native execution semantics, distinct from an explicit level. Choose a caller-owned request UUID before submission; read/retry that UUID after uncertainty (no duplicate or changed-intent retry). Accepted is not ready: read request_id on the same instance for canonical pending/ready/failure status. Remote options and cwd are destination-scoped; never substitute source-local paths or defaults. No WorkScope attachment, artifact reference, cancellation, or retry-delivery operation is offered.".into()
    }
    fn schema_without_instance(&self) -> Value {
        let cwd = json!({"type":"string","minLength":1});
        json!({"type":"object","additionalProperties":false,"required":["request"],"properties":{
            "request":{"oneOf":[
                {"type":"object","additionalProperties":false,"required":["op"],"properties":{
                    "op":{"const":"options"},"cwd":cwd}},
                {"type":"object","additionalProperties":false,"required":["op","request_id"],"properties":{
                    "op":{"const":"read"},"request_id":operation_schema()}},
                {"type":"object","additionalProperties":false,"required":["op","request_id","cwd","objective","start"],"properties":{
                    "op":{"const":"create"},"request_id":operation_schema(),"cwd":cwd,
                    "objective":{"type":"string","minLength":1,"description":"Nonblank literal objective; never a user commission or slash command."},
                    "start":{"const":"submit_objective"},"model":{"type":"string"},
                    "effort":{"type":"string","enum":["none","minimal","low","medium","high","xhigh","max"]},
                    "llm_language":{"type":"string","enum":["phoenix-native","caveman"]}
                }}
            ]}

        }})
    }
    fn clearable(&self) -> bool {
        false
    }
    fn resolution_db(&self) -> &crate::db::Database {
        self.0.db()
    }
    fn self_peer_policy(&self) -> SelfPeerPolicy {
        SelfPeerPolicy::Reject
    }
    async fn authorize(&self, ctx: &ToolContext) -> Result<(), String> {
        self.0
            .db()
            .require_current_coordinator_transcript(&ctx.conversation_id)
            .await
            .map_err(|error| error.to_string())
    }
    async fn run_routed(
        &self,
        input: Value,
        route: Route<federation::ResolvedPeer>,
        ctx: ToolContext,
    ) -> ToolOutput {
        output(match route {
            Route::Local => self.run_inner(input, ctx).await,
            Route::Remote(peer) => self.run_remote(input, &peer, ctx).await,
        })
    }
}

fn validated_creation_directory(cwd: &str, home: &std::path::Path) -> Result<String, String> {
    let cwd = cwd.trim();
    let path = if cwd == "~" {
        home.to_path_buf()
    } else if let Some(relative) = cwd.strip_prefix("~/") {
        home.join(relative)
    } else {
        std::path::PathBuf::from(cwd)
    };
    if cwd.is_empty() || !path.is_dir() {
        return Err("cwd must name an existing directory, not a file".into());
    }
    let canonical = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    crate::conversation_cwd::validate_conversation_cwd(canonical.to_string_lossy())
        .map(crate::conversation_cwd::ValidConversationCwd::into_raw)
        .map_err(|error| error.to_string())
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum NativeCreationPublication {
    Available {
        conversation_id: String,
        canonical_route: String,
    },
    Historical {
        conversation_id: String,
    },
}

const CREATION_EXECUTION_INTERPRETATION: &str = "Publication means initial input accepted, not objective completed. Unavailable means no attributable outcome is retained; never infer success from later turns.";

#[derive(Serialize)]
struct NativeCreationStatus {
    request_id: String,
    product_conversation_id: ProductConversationId,
    status: String,
    provisioning_stage: phoenix_db::ProductCreationProvisioningStage,
    provisioning_stage_updated_at: chrono::DateTime<chrono::Utc>,
    retry_at: Option<chrono::DateTime<chrono::Utc>>,
    error: Option<String>,
    publication: Option<NativeCreationPublication>,
    initial_objective_execution: phoenix_db::ProductCreationInitialExecution,
    execution_interpretation: &'static str,
    allowed_actions: [&'static str; 1],
}

fn native_creation_status(status: phoenix_db::ProductCreationStatusRead) -> NativeCreationStatus {
    NativeCreationStatus {
        request_id: status.request_id,
        product_conversation_id: status.product_conversation_id,
        status: status.status,
        provisioning_stage: status.provisioning_stage,
        provisioning_stage_updated_at: status.stage_updated_at,
        retry_at: status.retry_at,
        error: status.error,
        publication: status.publication.map(|publication| match publication {
            phoenix_db::ProductCreationPublicationRead::Available {
                conversation_id,
                aggregate,
            } => NativeCreationPublication::Available {
                conversation_id,
                canonical_route: crate::api::product_conversations::canonical_route(&aggregate),
            },
            phoenix_db::ProductCreationPublicationRead::Historical { conversation_id } => {
                NativeCreationPublication::Historical { conversation_id }
            }
        }),
        initial_objective_execution: status.initial_objective_execution,
        execution_interpretation: CREATION_EXECUTION_INTERPRETATION,
        allowed_actions: ["read"],
    }
}

fn native_creation_acceptance(
    request_id: &str,
    product_conversation_id: &ProductConversationId,
) -> Value {
    json!({
        "status": "accepted",
        "request_id": request_id,
        "product_conversation_id": product_conversation_id,
        "next": "read request_id; accepted does not imply ready",
    })
}

fn remote_native_creation_acceptance(acceptance: federation::NativeCreateResponse) -> Value {
    let federation::NativeCreateResponse {
        status: _,
        caller_instance_id: _,
        destination_instance_id: _,
        request_id,
        product_conversation_id,
    } = acceptance;
    native_creation_acceptance(&request_id, &product_conversation_id)
}

fn remote_native_creation_status(
    response: federation::NativeCreateStatusResponse,
) -> Result<Value, String> {
    use federation::{
        NativeCreateInitialExecution as Execution, NativeCreatePublication as Publication,
    };

    match response {
        federation::NativeCreateStatusResponse::Unknown {
            caller_instance_id: _,
            destination_instance_id: _,
            request_id,
        } => Ok(json!({"status": "unknown", "request_id": request_id})),
        federation::NativeCreateStatusResponse::Known {
            caller_instance_id: _,
            destination_instance_id: _,
            request_id,
            product_conversation_id,
            creation_status,
            provisioning_stage,
            provisioning_stage_updated_at_unix_micros,
            retry_at_unix_micros,
            error,
            publication,
            initial_execution,
        } => {
            let stage_updated_at =
                chrono::DateTime::from_timestamp_micros(provisioning_stage_updated_at_unix_micros)
                    .ok_or("Invalid remote creation stage timestamp")?;
            let retry_at = retry_at_unix_micros
                .map(|micros| {
                    chrono::DateTime::from_timestamp_micros(micros)
                        .ok_or("Invalid remote creation retry timestamp")
                })
                .transpose()?;
            let publication = match publication {
                Publication::None {} => None,
                Publication::Available {
                    conversation_id,
                    canonical_route,
                } => Some(NativeCreationPublication::Available {
                    conversation_id,
                    canonical_route,
                }),
                Publication::Historical { conversation_id } => {
                    Some(NativeCreationPublication::Historical { conversation_id })
                }
            };
            let initial_objective_execution = match initial_execution {
                Execution::Queued {} => phoenix_db::ProductCreationInitialExecution::Queued,
                Execution::Running {} => phoenix_db::ProductCreationInitialExecution::Running,
                Execution::Completed {} => phoenix_db::ProductCreationInitialExecution::Completed,
                Execution::Failed { reason } => {
                    phoenix_db::ProductCreationInitialExecution::Failed { reason }
                }
                Execution::Cancelled {} => phoenix_db::ProductCreationInitialExecution::Cancelled,
                Execution::AwaitingUserResponse {} => {
                    phoenix_db::ProductCreationInitialExecution::AwaitingUserResponse
                }
                Execution::AwaitingTaskApproval {} => {
                    phoenix_db::ProductCreationInitialExecution::AwaitingTaskApproval
                }
                Execution::Unavailable {} => {
                    phoenix_db::ProductCreationInitialExecution::Unavailable
                }
            };
            let status = match creation_status {
                federation::NativeCreationState::Accepted => "accepted",
                federation::NativeCreationState::Claimed => "claimed",
                federation::NativeCreationState::RetryScheduled => "retry_scheduled",
                federation::NativeCreationState::Cancelling => "cancelling",
                federation::NativeCreationState::Cancelled => "cancelled",
                federation::NativeCreationState::DeletionPending => "deletion_pending",
                federation::NativeCreationState::DeliveryPending => "delivery_pending",
                federation::NativeCreationState::DeliveryFailed => "delivery_failed",
                federation::NativeCreationState::Published => "published",
                federation::NativeCreationState::Failed => "failed",
                federation::NativeCreationState::CleanupAmbiguous => "cleanup_ambiguous",
            }
            .to_owned();
            serde_json::to_value(NativeCreationStatus {
                request_id,
                product_conversation_id,
                status,
                provisioning_stage,
                provisioning_stage_updated_at: stage_updated_at,
                retry_at,
                error,
                publication,
                initial_objective_execution,
                execution_interpretation: CREATION_EXECUTION_INTERPRETATION,
                allowed_actions: ["read"],
            })
            .map_err(|error| error.to_string())
        }
    }
}

async fn read_native_creation_status(
    runtime: &RuntimeManager,
    transcript_id: &str,
    request_id: &str,
) -> Result<Option<NativeCreationStatus>, String> {
    runtime
        .db()
        .read_product_creation_status_as_coordinator(transcript_id, request_id)
        .await
        .map(|status| status.map(native_creation_status))
        .map_err(|error| error.to_string())
}

async fn native_creation_acceptance_receipt(
    runtime: &RuntimeManager,
    transcript_id: &str,
    request_id: &str,
) -> Result<Value, String> {
    let accepted = read_native_creation_status(runtime, transcript_id, request_id)
        .await?
        .ok_or("Accepted creation request is no longer available; reconcile request_id")?;
    Ok(native_creation_acceptance(
        &accepted.request_id,
        &accepted.product_conversation_id,
    ))
}

fn create_request_guidance(request: &Value) -> &'static str {
    match request.get("op").and_then(Value::as_str) {
        Some("options") => "options accepts only op and optional cwd",
        Some("read") => "read requires op and request_id",
        Some("create") => "create requires op, request_id, cwd, objective, and start=submit_objective; optional model, effort, llm_language",
        _ => "op must be options, read, or create",
    }
}

fn parse_create_request(input: &Value) -> Result<CreateRequest, String> {
    let Some(root) = input.as_object() else {
        return Err("Invalid create_conversation call: supply only request with options {op, cwd?}, read {op, request_id}, or create {op, request_id, cwd, objective, start, model?, effort?, llm_language?}.".into());
    };
    let Some(request_input) = root.get("request") else {
        return Err("Invalid create_conversation call: supply only request with options {op, cwd?}, read {op, request_id}, or create {op, request_id, cwd, objective, start, model?, effort?, llm_language?}.".into());
    };
    if root.len() != 1 {
        return Err(format!(
            "Invalid create_conversation call: supply only request; {}.",
            create_request_guidance(request_input)
        ));
    }
    serde_json::from_value(request_input.clone()).map_err(|_| {
        format!(
            "Invalid create_conversation request: {}.",
            create_request_guidance(request_input)
        )
    })
}

impl CreateConversation {
    async fn run_remote(
        &self,
        input: Value,
        peer: &federation::ResolvedPeer,
        ctx: ToolContext,
    ) -> Result<Value, String> {
        let request = parse_create_request(&input)?;
        self.authorize(&ctx).await?;
        let _dispatch_authority = remote_dispatch_authority(self.0.clone(), ctx.clone()).await?;
        let caller_instance_id = self
            .0
            .db()
            .instance_id()
            .await
            .map_err(|error| error.to_string())?;
        let destination_instance_id = peer.instance_id();
        match request {
            CreateRequest::Options { cwd } => {
                let correlation_id = uuid::Uuid::new_v4().to_string();
                let request = match cwd {
                    None => federation::NativeCreateOptionsRequest::WithoutCwd {
                        caller_instance_id,
                        destination_instance_id,
                        correlation_id,
                    },
                    Some(cwd) => federation::NativeCreateOptionsRequest::WithCwd {
                        caller_instance_id,
                        destination_instance_id,
                        correlation_id,
                        cwd,
                    },
                };
                let response =
                    federation::remote_native_create_options(self.0.db(), peer, &request)
                        .await
                        .map_err(|error| error.to_string())?;
                Ok(remote_create_options_response(response))
            }
            CreateRequest::Read { request_id } => {
                let request = federation::NativeCreateStatusRequest {
                    caller_instance_id,
                    destination_instance_id,
                    request: request_id.0,
                };
                let response = federation::remote_native_create_status(self.0.db(), peer, &request)
                    .await
                    .map_err(|error| error.to_string())?;
                remote_native_creation_status(response)
            }
            CreateRequest::Create {
                request_id,
                cwd,
                objective,
                start: CreateStart::SubmitObjective,
                model,
                effort,
                llm_language,
            } => {
                let request = federation::NativeCreateRequest {
                    caller_instance_id,
                    destination_instance_id,
                    request_id: request_id.0,
                    cwd,
                    objective,
                    model,
                    effort: effort.map(|effort| effort.to_string()),
                    language: llm_language.map(|language| language.as_str().to_string()),
                    start: federation::NativeCreateStart::SubmitObjective,
                    source: remote_source(&self.0, &ctx).await?,
                };
                match federation::remote_native_create(self.0.db(), peer, &request)
                    .await
                    .map_err(|error| error.to_string())?
                {
                    federation::NativeCreateOutcome::Accepted { acceptance } => {
                        Ok(remote_native_creation_acceptance(acceptance))
                    }
                    federation::NativeCreateOutcome::Reconciled { status: response } => {
                        remote_native_creation_status(response)
                    }
                }
            }
        }
    }

    async fn project_options(&self, transcript_id: &str) -> Result<(Vec<Value>, bool), String> {
        let mut projects = Vec::new();
        let mut after_id = None::<String>;
        while projects.len() < 51 {
            let candidates = self
                .0
                .db()
                .product_creation_project_options_as_coordinator(transcript_id, after_id.as_deref())
                .await
                .map_err(|error| error.to_string())?;
            let Some((last_id, _)) = candidates.last() else {
                break;
            };
            after_id = Some(last_id.clone());
            let home = self.0.runtime_home().to_path_buf();
            let usable = tokio::task::spawn_blocking(move || {
                candidates.into_iter().filter_map(|(id, cwd)| {
                    match validated_creation_directory(&cwd, &home) {
                        Ok(cwd) => Some(json!({"project_id":id,"cwd":cwd})),
                        Err(error) => {
                            tracing::debug!(project_id = %id, cwd = %cwd, %error, "omitting unusable creation project option");
                            None
                        }
                    }
                }).collect::<Vec<_>>()
            }).await.map_err(|error| error.to_string())?;
            projects.extend(usable.into_iter().take(51 - projects.len()));
        }
        let truncated = projects.len() > 50;
        projects.truncate(50);
        Ok((projects, truncated))
    }

    #[allow(clippy::too_many_lines)]
    async fn run_inner(&self, input: Value, ctx: ToolContext) -> Result<Value, String> {
        self.0
            .db()
            .require_current_coordinator_transcript(&ctx.conversation_id)
            .await
            .map_err(|e| e.to_string())?;

        let request = parse_create_request(&input)?;
        match request {
            CreateRequest::Options { cwd } => {
                let cwd = match cwd {
                    Some(cwd) => Some({
                        let home = self.0.runtime_home().to_path_buf();
                        tokio::task::spawn_blocking(move || {
                            validated_creation_directory(&cwd, &home)
                        })
                        .await
                        .map_err(|e| e.to_string())??
                    }),
                    None => None,
                };
                let (projects, truncated) = self.project_options(&ctx.conversation_id).await?;
                let registry = self.0.model_registry();
                let routes = registry.available_execution_routes();
                let models: Vec<_> = routes.into_iter().map(|route| json!({"model":route.model,"supported_efforts":route.supported_efforts})).collect();
                Ok(
                    json!({"status":"options","cwd":cwd,"projects":projects,"projects_truncated":truncated,"models":models,"default_model":registry.default_model_id(),"default_llm_language":self.0.db().get_default_llm_language().await.map_err(|e| e.to_string())?}),
                )
            }
            CreateRequest::Read { request_id } => {
                let status =
                    read_native_creation_status(&self.0, &ctx.conversation_id, &request_id.0)
                        .await?;
                let Some(status) = status else {
                    return Ok(json!({"status":"unknown", "request_id":request_id.0}));
                };
                serde_json::to_value(status).map_err(|error| error.to_string())
            }
            CreateRequest::Create {
                request_id,
                cwd,
                objective,
                start: CreateStart::SubmitObjective,
                model,
                effort,
                llm_language,
            } => {
                let source = mutation_source(&self.0, &ctx).await?;
                let accepted = accept_create(
                    &self.0,
                    CreateProductConversationRequest {
                        request_id: request_id.0,
                        cwd,
                        objective,
                        model,
                        effort,
                        llm_language,
                        images: Vec::new(),
                    },
                    CreateSource::CurrentCoordinator {
                        product_conversation_id: source.product_conversation_id,
                        transcript_id: source.transcript_id,
                        source_call: source.source_call,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
                native_creation_acceptance_receipt(
                    &self.0,
                    &ctx.conversation_id,
                    &accepted.request_id,
                )
                .await
            }
        }
    }
}

#[cfg(test)]
#[path = "conversation_action_tools/tests.rs"]
pub(crate) mod tests;
