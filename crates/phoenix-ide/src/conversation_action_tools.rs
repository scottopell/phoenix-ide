use std::sync::Arc;

use async_trait::async_trait;
use phoenix_core::domain::llm_types::ModelEffort;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::llm_language::LlmLanguage;
use phoenix_tools::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

use crate::api::CreateProductConversationRequest;
use crate::conversation_actions::{
    accept_create, normalize_product_conversation_title, CreateSource,
};
use crate::db::{
    NormalizedConversationTitle, ProductConversationRenameCommand, ProductConversationRenameResult,
    ProductConversationRenameSource,
};
use crate::runtime::RuntimeManager;

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
            Arc::new(RenameConversation(runtime.clone())),
            Arc::new(CreateConversation(runtime)),
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CreateStart {
    SubmitObjective,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum CreateRequest {
    Options {
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
        model: Option<String>,
        effort: Option<ModelEffort>,
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
) -> Result<ProductConversationRenameSource, String> {
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
    Ok(ProductConversationRenameSource {
        product_conversation_id: source.product_conversation_id,
        transcript_id: source.id,
        message_id: call.message_id,
        tool_use_id: call.tool_use_id,
    })
}

fn operation_schema() -> Value {
    json!({"type":"string", "format":"uuid", "pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"})
}

#[async_trait]
impl Tool for RenameConversation {
    fn name(&self) -> &'static str {
        "rename_conversation"
    }
    fn description(&self) -> String {
        "Current Global Coordinator only. Detail resolves a reference to immutable product/root identity and naming revision; use those exact values for rename. A rename updates the title (1-200 characters) and allocated slug, never the immutable product/work identity. Choose a new operation UUID for each intentional rename; read/retry the same operation UUID after an uncertain result. Durable replay reports the historical receipt, not current title/lifecycle.".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object", "additionalProperties":false, "required":["op"], "properties":{
            "op":{"type":"string","enum":["detail","read","rename"]},
            "reference":{"type":"string","description":"Required for detail; product id or slug.","minLength":1},
            "operation_id":operation_schema(),
            "product_conversation_id":{"type":"string","description":"Required for rename; immutable product id returned by detail, never a slug or transcript.","minLength":1},
            "expected_root_transcript_id":{"type":"string","description":"Required for rename; returned by detail.","minLength":1},
            "expected_name_revision":{"type":"integer","description":"Required for rename; returned by detail.","minimum":0},
            "title":{"type":"string","description":"Required for rename.","minLength":1,"maxLength":200}
        }})
    }
    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        output(self.run_inner(input, ctx).await)
    }
}

fn rename_detail_response(detail: &crate::db::ProductConversationRenameDetail) -> Value {
    let route = crate::api::product_conversations::product_route(
        detail.slug.as_deref(),
        detail.product_conversation_id.as_str(),
    );
    json!({"status":"detail", "canonical_route":route, "detail":detail,
        "allowed_actions": if detail.can_rename { vec!["detail", "read", "rename"] } else { vec!["detail", "read"] }})
}

impl RenameConversation {
    async fn run_inner(&self, input: Value, ctx: ToolContext) -> Result<Value, String> {
        self.0
            .db()
            .require_current_coordinator_transcript(&ctx.conversation_id)
            .await
            .map_err(|e| e.to_string())?;
        let request: RenameRequest = serde_json::from_value(input).map_err(|e| e.to_string())?;
        match request {
            RenameRequest::Detail { reference } => {
                let reference = reference.strip_prefix("@conv:").unwrap_or(&reference);
                let detail = self
                    .0
                    .db()
                    .read_product_conversation_rename_detail(&ctx.conversation_id, reference)
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
                let title =
                    normalize_product_conversation_title(&title).map_err(|e| e.to_string())?;
                let command = ProductConversationRenameCommand {
                    operation_id: operation_id.0,
                    product_conversation_id,
                    expected_root_transcript_id,
                    expected_name_revision,
                    title: NormalizedConversationTitle::parse(&title).map_err(|e| e.to_string())?,
                    source,
                };
                let _authority = self
                    .0
                    .acquire_local_authority_pass()
                    .map_err(|()| "local authority is closed")?;
                let result = self
                    .0
                    .db()
                    .rename_product_conversation(&ctx.conversation_id, &command)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(match result {
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
                })
            }
        }
    }
}

#[async_trait]
impl Tool for CreateConversation {
    fn name(&self) -> &'static str {
        "create_conversation"
    }
    fn description(&self) -> String {
        "Current Global Coordinator only. Explicitly create AND submit a nonblank text objective using start=submit_objective via the durable creation pipeline; dormant create is unsupported. Options discovers bounded project paths, normalized cwd, and available model/effort choices. Submit only an exact normalized absolute cwd. Objective text is literal agent content, never a user commission or slash command. Omit model/language to freeze current defaults on first acceptance; retry omission reuses those choices, while explicit different choices conflict. Omitted effort means native execution semantics, distinct from an explicit level. Choose a caller-owned request UUID before submission; read/retry that UUID after uncertainty (no duplicate or changed-intent retry). Accepted is not ready: read request_id for canonical pending/ready/failure status. No WorkScope attachment, artifact reference, cancellation, or retry-delivery operation is offered.".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object", "additionalProperties":false,"required":["op"],"properties":{
            "op":{"type":"string","enum":["options","read","create"]},"request_id":operation_schema(),
            "cwd":{"type":"string","description":"Optional path for options to normalize; required exact normalized absolute cwd for create.","minLength":1},
            "objective":{"type":"string","description":"Required nonblank literal objective for create.","minLength":1},
            "start":{"type":"string","enum":["submit_objective"],"description":"Required for create; explicitly submits the objective and may start execution."},
            "model":{"type":"string"},"effort":{"type":"string","enum":["none","minimal","low","medium","high","xhigh","max"]},
            "llm_language":{"type":"string","enum":["phoenix-native","caveman"]}
        }})
    }
    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        output(self.run_inner(input, ctx).await)
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
        execution_interpretation: "Publication means initial input accepted, not objective completed. Unavailable means no attributable outcome is retained; never infer success from later turns.",
        allowed_actions: ["read"],
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
    Ok(
        json!({"status":"accepted", "request_id":accepted.request_id,"product_conversation_id":accepted.product_conversation_id,"next":"read request_id; accepted does not imply ready"}),
    )
}

impl CreateConversation {
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
        let request: CreateRequest = serde_json::from_value(input).map_err(|e| e.to_string())?;
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
                let source_call = ctx
                    .source_tool_call()
                    .ok_or("Trusted source tool-call identity unavailable")?;
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
                        source_call,
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
mod tests;
