//! Application actions shared by HTTP ingress and the current Coordinator's tools.

use std::path::Path;

use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::instance_identity::InstanceId;
use phoenix_core::domain::product_conversation::ProductConversationId;

use crate::api::handlers::AppError;
use crate::api::product_conversations::db_to_app;
use crate::api::{ConflictErrorResponse, CreateProductConversationRequest};
use crate::db::{
    DbError, NormalizedConversationTitle, ProductConversationRenameCommand,
    ProductConversationRenameResult, ProductConversationRenameSource, ProductCreationAcceptOutcome,
    ProductCreationImage, ProductCreationIntent, ProductCreationJobRecord, ProductCreationOrigin,
    ProductCreationResolution,
};
use crate::runtime::{AdmittedOperation, RuntimeManager};

fn normalized_product_conversation_title(
    title: &str,
) -> Result<NormalizedConversationTitle, AppError> {
    NormalizedConversationTitle::parse(title).map_err(db_to_app)
}

pub(crate) fn normalize_product_conversation_title(title: &str) -> Result<String, AppError> {
    normalized_product_conversation_title(title).map(|title| title.as_str().to_owned())
}

pub(crate) struct LocalCoordinatorSource {
    pub product_conversation_id: ProductConversationId,
    pub transcript_id: String,
    pub source_call: SourceToolCall,
}

pub(crate) struct CoordinatorRenameIntent {
    pub operation_id: String,
    pub product_conversation_id: ProductConversationId,
    pub expected_root_transcript_id: String,
    pub expected_name_revision: i64,
    pub title: String,
}

pub(crate) async fn rename_from_user_api(
    runtime: &RuntimeManager,
    reference: &str,
    title: &str,
) -> Result<ProductConversationId, AppError> {
    let title = normalized_product_conversation_title(title)?;
    runtime
        .db()
        .set_ordinary_product_conversation_title(reference, title.as_str())
        .await
        .map_err(db_to_app)
}

pub(crate) async fn rename_from_current_coordinator(
    runtime: &RuntimeManager,
    source: LocalCoordinatorSource,
    intent: CoordinatorRenameIntent,
) -> Result<ProductConversationRenameResult, AppError> {
    let command = ProductConversationRenameCommand {
        operation_id: intent.operation_id,
        product_conversation_id: intent.product_conversation_id,
        expected_root_transcript_id: intent.expected_root_transcript_id,
        expected_name_revision: intent.expected_name_revision,
        title: normalized_product_conversation_title(&intent.title)?,
        source: ProductConversationRenameSource {
            source_instance_id: runtime.db().instance_id().await.map_err(db_to_app)?,
            product_conversation_id: source.product_conversation_id,
            transcript_id: source.transcript_id,
            message_id: source.source_call.message_id,
            tool_use_id: source.source_call.tool_use_id,
        },
    };
    let owner = runtime
        .acquire_local_authority_pass()
        .map_err(|()| AppError::Internal("local authority is closed".into()))?;
    let authority = LocalAuthorityPass::new(owner);
    let classified = runtime
        .db()
        .rename_product_conversation(&command.source.transcript_id, &command)
        .await;
    authority.complete(classified, db_to_app, local_authority_unclassified)
}

pub(crate) async fn rename_from_federation(
    runtime: &RuntimeManager,
    authority: &phoenix_db::FederationConversationAuthority,
    source: ProductConversationRenameSource,
    intent: CoordinatorRenameIntent,
) -> Result<ProductConversationRenameResult, AppError> {
    let command = ProductConversationRenameCommand {
        operation_id: intent.operation_id,
        product_conversation_id: intent.product_conversation_id,
        expected_root_transcript_id: intent.expected_root_transcript_id,
        expected_name_revision: intent.expected_name_revision,
        title: normalized_product_conversation_title(&intent.title)?,
        source,
    };
    let owner = runtime
        .acquire_local_authority_pass()
        .map_err(|()| AppError::Internal("local authority is closed".into()))?;
    LocalAuthorityPass::new(owner).complete(
        runtime
            .db()
            .rename_product_conversation_as_federation(authority, &command)
            .await,
        db_to_app,
        local_authority_unclassified,
    )
}

pub(crate) async fn accept_create_from_federation(
    runtime: &RuntimeManager,
    authority: phoenix_db::FederationConversationAuthority,
    req: CreateProductConversationRequest,
    product_conversation_id: ProductConversationId,
    transcript_id: String,
    source_call: SourceToolCall,
) -> Result<CreateAccepted, AppError> {
    accept_create_with_cwd_validator(
        runtime,
        req,
        CreateSource::FederatedConversation {
            authority,
            product_conversation_id,
            transcript_id,
            source_call,
        },
        validate_new_creation_cwd,
    )
    .await
}

pub(crate) async fn read_create_status_from_federation(
    runtime: &RuntimeManager,
    authority: &phoenix_db::FederationConversationAuthority,
    request_id: &str,
) -> Result<Option<phoenix_db::ProductCreationStatusRead>, AppError> {
    runtime
        .db()
        .read_product_creation_status_as_federation(authority, request_id)
        .await
        .map_err(product_creation_db_to_app)
}

fn product_creation_db_to_app(error: DbError) -> AppError {
    if let DbError::ProductCreationRequestConflict = error {
        AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "request_id conflicts with immutable creation intent",
            "product_creation_request_conflict",
        )))
    } else {
        db_to_app(error)
    }
}

fn local_authority_unclassified() -> AppError {
    AppError::Internal("local authority is unclassified; process admission closed".into())
}

struct LocalAuthorityPass {
    owner: Option<AdmittedOperation>,
}

impl LocalAuthorityPass {
    fn new(owner: AdmittedOperation) -> Self {
        Self { owner: Some(owner) }
    }

    fn complete<T, E>(
        mut self,
        result: phoenix_db::workflow::LocalAuthorityResult<crate::db::DbResult<T>>,
        map_error: impl FnOnce(DbError) -> E,
        unclassified_error: impl FnOnce() -> E,
    ) -> Result<T, E> {
        match result {
            phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(result) => {
                self.owner.take();
                result.map_err(map_error)
            }
            phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified => {
                drop(map_error);
                Err(unclassified_error())
            }
        }
    }
}

impl Drop for LocalAuthorityPass {
    fn drop(&mut self) {
        if let Some(owner) = &self.owner {
            owner.close("local_authority");
        }
    }
}

pub(crate) enum CreateSource {
    UserApi,
    CurrentCoordinator {
        product_conversation_id: ProductConversationId,
        transcript_id: String,
        source_call: SourceToolCall,
    },
    FederatedConversation {
        authority: phoenix_db::FederationConversationAuthority,
        product_conversation_id: ProductConversationId,
        transcript_id: String,
        source_call: SourceToolCall,
    },
}

impl CreateSource {
    fn origin(&self, local_instance_id: InstanceId) -> ProductCreationOrigin {
        match self {
            Self::UserApi => ProductCreationOrigin::UserApi,
            Self::FederatedConversation {
                authority,
                product_conversation_id,
                transcript_id,
                source_call,
            } => ProductCreationOrigin::FederatedConversation {
                source_instance_id: authority.caller_instance_id(),
                product_conversation_id: product_conversation_id.clone(),
                transcript_id: transcript_id.clone(),
                source_call: source_call.clone(),
            },
            Self::CurrentCoordinator {
                product_conversation_id,
                transcript_id,
                source_call,
            } => ProductCreationOrigin::InternalConversation {
                source_instance_id: local_instance_id,
                product_conversation_id: product_conversation_id.clone(),
                transcript_id: transcript_id.clone(),
                source_call: source_call.clone(),
            },
        }
    }

    fn coordinator_transcript_id(&self) -> Option<&str> {
        match self {
            Self::UserApi | Self::FederatedConversation { .. } => None,
            Self::CurrentCoordinator { transcript_id, .. } => Some(transcript_id),
        }
    }
}

pub(crate) struct CreateAccepted {
    pub request_id: String,
    pub product_conversation_id: ProductConversationId,
}

fn invalid(message: impl Into<String>) -> DbError {
    DbError::ProductCreationIntentInvalid(message.into())
}

fn requested_images(req: &CreateProductConversationRequest) -> Vec<ProductCreationImage> {
    req.images
        .iter()
        .map(|image| ProductCreationImage {
            media_type: image.media_type.clone(),
            data: image.data.clone(),
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
fn resolve_intent(
    runtime: &RuntimeManager,
    req: &CreateProductConversationRequest,
    source: &CreateSource,
    local_instance_id: InstanceId,
    resolution: ProductCreationResolution<'_>,
    cwd_preparation: &CreationCwdPreparation,
) -> Result<ProductCreationIntent, DbError> {
    let origin = source.origin(local_instance_id);
    if let ProductCreationResolution::Existing(existing) = resolution {
        let cwd = if req.cwd == existing.intent.cwd {
            req.cwd.clone()
        } else if matches!(source, CreateSource::UserApi) {
            cwd_preparation
                .replay
                .as_ref()
                .map_err(|error| invalid(error.clone()))?
                .clone()
        } else {
            req.cwd.clone()
        };
        return Ok(ProductCreationIntent {
            cwd,
            objective: req.objective.clone(),
            origin,
            model: req.model.clone().or_else(|| existing.intent.model.clone()),
            effort: req.effort,
            llm_language: req.llm_language.unwrap_or(existing.intent.llm_language),
            images: requested_images(req),
        });
    }
    let ProductCreationResolution::New {
        default_llm_language,
    } = resolution
    else {
        unreachable!("existing creation returned above")
    };
    let internal = !matches!(source, CreateSource::UserApi);
    if req.objective.trim().is_empty() && (internal || req.images.is_empty()) {
        return Err(invalid(
            "Product creation requires nonblank objective text (or an image for user API input)",
        ));
    }
    if internal && !req.images.is_empty() {
        return Err(invalid("Coordinator creation accepts text only"));
    }
    if req
        .images
        .iter()
        .any(|image| image.media_type.trim().is_empty() || image.data.trim().is_empty())
    {
        return Err(invalid(
            "Product creation images require a media type and non-empty payload",
        ));
    }
    if req.images.len() > 20
        || req
            .images
            .iter()
            .map(|image| image.data.len())
            .sum::<usize>()
            > 35_000_000
    {
        return Err(invalid("Product creation images exceed the request limit"));
    }
    if internal && (!Path::new(&req.cwd).is_absolute() || req.cwd.trim() != req.cwd) {
        return Err(invalid(
            "cwd must be an exact normalized absolute path; use options first",
        ));
    }
    let cwd = cwd_preparation
        .new
        .as_ref()
        .map_err(|error| invalid(error.clone()))?
        .clone();
    if internal && cwd != req.cwd {
        return Err(invalid(format!(
            "cwd must be the exact normalized path {cwd:?}; use options first"
        )));
    }
    let registry = runtime.model_registry();
    let model = req
        .model
        .clone()
        .unwrap_or_else(|| registry.default_model_id());
    if registry.get(&model).is_none() {
        return Err(invalid(format!("Unknown or unavailable model '{model}'")));
    }
    if req
        .effort
        .is_some_and(|effort| !registry.supports_effort(&model, effort))
    {
        return Err(invalid(format!(
            "Model '{model}' does not support the requested effort"
        )));
    }
    let effort = req.effort;
    Ok(ProductCreationIntent {
        cwd,
        objective: req.objective.clone(),
        origin,
        model: Some(model),
        effort,
        llm_language: req.llm_language.unwrap_or(default_llm_language),
        images: requested_images(req),
    })
}

struct CreationCwdPreparation {
    new: Result<String, String>,
    replay: Result<String, String>,
}

fn validate_new_creation_cwd(cwd: &str, home: &Path, internal: bool) -> Result<String, String> {
    if internal {
        crate::conversation_cwd::validate_conversation_cwd(cwd)
            .map(crate::conversation_cwd::ValidConversationCwd::into_raw)
    } else {
        crate::conversation_cwd::normalize_product_creation_cwd_intent(cwd, home)
    }
    .map_err(|error| error.to_string())
}

fn finish_create_acceptance(
    runtime: &RuntimeManager,
    outcome: ProductCreationAcceptOutcome,
) -> Result<CreateAccepted, AppError> {
    let (job, newly_accepted): (ProductCreationJobRecord, bool) = match outcome {
        ProductCreationAcceptOutcome::Accepted(job) => (job, true),
        ProductCreationAcceptOutcome::Replayed(job) => (job, false),
        ProductCreationAcceptOutcome::Conflict => {
            return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                "request_id conflicts with immutable creation intent",
                "product_creation_request_conflict",
            ))));
        }
    };
    if newly_accepted {
        runtime.kick_creation_worker();
    }
    Ok(CreateAccepted {
        request_id: job.request_id,
        product_conversation_id: job.product_conversation_id,
    })
}

pub(crate) async fn accept_create(
    runtime: &RuntimeManager,
    req: CreateProductConversationRequest,
    source: CreateSource,
) -> Result<CreateAccepted, AppError> {
    accept_create_with_cwd_validator(runtime, req, source, validate_new_creation_cwd).await
}

async fn accept_create_with_cwd_validator<F>(
    runtime: &RuntimeManager,
    req: CreateProductConversationRequest,
    source: CreateSource,
    validator: F,
) -> Result<CreateAccepted, AppError>
where
    F: FnOnce(&str, &Path, bool) -> Result<String, String> + Send + 'static,
{
    let owner = runtime.acquire_local_authority_pass().map_err(|()| {
        AppError::Internal("runtime admission closed after fatal local authority loss".to_owned())
    })?;
    uuid::Uuid::parse_str(&req.request_id)
        .map_err(|_| AppError::BadRequest("request_id must be a UUID".to_owned()))?;
    let existing = runtime
        .db()
        .get_product_creation_job(&req.request_id)
        .await
        .map_err(product_creation_db_to_app)?;
    let internal = !matches!(&source, CreateSource::UserApi);
    let cwd_preparation = if existing
        .as_ref()
        .is_some_and(|job| internal || job.intent.cwd == req.cwd)
    {
        CreationCwdPreparation {
            new: Err("creation receipt disappeared during acceptance".into()),
            replay: Ok(req.cwd.clone()),
        }
    } else {
        let cwd = req.cwd.clone();
        let home = runtime.runtime_home().to_path_buf();
        let prepared = tokio::task::spawn_blocking(move || validator(&cwd, &home, internal))
            .await
            .map_err(|error| {
                AppError::Internal(format!(
                    "creation directory validation task failed: {error}"
                ))
            })?;
        CreationCwdPreparation {
            new: prepared.clone(),
            replay: if internal {
                Ok(req.cwd.clone())
            } else {
                prepared
            },
        }
    };
    let local_instance_id = runtime.db().instance_id().await.map_err(db_to_app)?;
    let outcome = match &source {
        CreateSource::FederatedConversation { authority, .. } => {
            let authority_pass = LocalAuthorityPass::new(owner);
            let result = runtime
                .db()
                .accept_product_creation_as_federation(authority, &req.request_id, |resolution| {
                    resolve_intent(
                        runtime,
                        &req,
                        &source,
                        local_instance_id,
                        resolution,
                        &cwd_preparation,
                    )
                })
                .await;
            authority_pass.complete(
                result,
                product_creation_db_to_app,
                local_authority_unclassified,
            )?
        }
        CreateSource::UserApi | CreateSource::CurrentCoordinator { .. } => {
            let authority_pass = LocalAuthorityPass::new(owner);
            let result = runtime
                .db()
                .accept_product_creation_resolved_classified(
                    &req.request_id,
                    source.coordinator_transcript_id(),
                    |resolution| {
                        resolve_intent(
                            runtime,
                            &req,
                            &source,
                            local_instance_id,
                            resolution,
                            &cwd_preparation,
                        )
                    },
                )
                .await;
            authority_pass.complete(
                result,
                product_creation_db_to_app,
                local_authority_unclassified,
            )?
        }
    };
    finish_create_acceptance(runtime, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::platform::PlatformCapability;
    use crate::tools::mcp::McpClientManager;
    use phoenix_core::llm_language::LlmLanguage;
    use phoenix_llm::{LlmConfig, ModelRegistry};
    use std::sync::Arc;

    #[tokio::test]
    async fn local_authority_unclassified_result_closes_fence_before_releasing_owner() {
        let runtime = runtime(
            Database::open_in_memory().await.unwrap(),
            ModelRegistry::new_empty(),
        );
        let authority = LocalAuthorityPass::new(runtime.acquire_local_authority_pass().unwrap());
        let result: Result<ProductConversationRenameResult, AppError> = authority.complete(
            phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified,
            db_to_app,
            local_authority_unclassified,
        );
        assert!(result.is_err());
        assert!(runtime.local_authority_is_closed());
        assert_eq!(runtime.fatal_authority_owners_at_first_close(), Some(1));
        assert!(runtime.acquire_local_authority_pass().is_err());
    }

    #[tokio::test]
    async fn local_authority_established_results_keep_admission_open() {
        let runtime = runtime(
            Database::open_in_memory().await.unwrap(),
            ModelRegistry::new_empty(),
        );
        let authority = LocalAuthorityPass::new(runtime.acquire_local_authority_pass().unwrap());
        let result: Result<ProductConversationRenameResult, AppError> = authority.complete(
            phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Err(
                crate::db::DbError::CurrentCoordinatorRequired,
            )),
            db_to_app,
            local_authority_unclassified,
        );
        assert!(result.is_err());
        assert!(!runtime.local_authority_is_closed());
        let authority = LocalAuthorityPass::new(runtime.acquire_local_authority_pass().unwrap());
        assert_eq!(
            authority
                .complete(
                    phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Ok(
                        ProductConversationRenameResult::Stale
                    ),),
                    db_to_app,
                    local_authority_unclassified,
                )
                .unwrap(),
            ProductConversationRenameResult::Stale
        );
        assert!(!runtime.local_authority_is_closed());
        assert_eq!(runtime.fatal_authority_owners_at_first_close(), None);
    }

    #[tokio::test]
    async fn cancelled_local_authority_closes_fence_before_releasing_owner() {
        let runtime = runtime(
            Database::open_in_memory().await.unwrap(),
            ModelRegistry::new_empty(),
        );
        let owner = runtime.acquire_local_authority_pass().unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let authority = LocalAuthorityPass::new(owner);
            entered_tx.send(()).unwrap();
            std::future::pending::<()>().await;
            let result: Result<ProductConversationRenameResult, AppError> = authority.complete(
                phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified,
                db_to_app,
                local_authority_unclassified,
            );
            result
        });
        entered_rx.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(runtime.local_authority_is_closed());
        assert_eq!(runtime.fatal_authority_owners_at_first_close(), Some(1));
    }

    #[tokio::test]
    async fn panicked_local_authority_closes_fence_before_releasing_owner() {
        let runtime = runtime(
            Database::open_in_memory().await.unwrap(),
            ModelRegistry::new_empty(),
        );
        let owner = runtime.acquire_local_authority_pass().unwrap();
        let task = tokio::spawn(async move {
            let _authority = LocalAuthorityPass::new(owner);
            panic!("injected local authority boundary panic");
        });
        assert!(task.await.unwrap_err().is_panic());
        assert!(runtime.local_authority_is_closed());
        assert_eq!(runtime.fatal_authority_owners_at_first_close(), Some(1));
    }

    fn runtime(db: Database, registry: ModelRegistry) -> RuntimeManager {
        RuntimeManager::new(
            db,
            Arc::new(registry),
            PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(McpClientManager::new()),
            None,
        )
    }

    fn coordinator_source(call: &str) -> CreateSource {
        CreateSource::CurrentCoordinator {
            product_conversation_id: "source-product".parse().unwrap(),
            transcript_id: "source-coordinator".into(),
            source_call: SourceToolCall {
                message_id: "source-assistant".into(),
                tool_use_id: call.into(),
            },
        }
    }

    fn request(cwd: &str) -> CreateProductConversationRequest {
        CreateProductConversationRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            cwd: cwd.into(),
            objective: "/not-a-user-skill @not-a-user-file".into(),
            model: None,
            effort: None,
            llm_language: None,
            images: Vec::new(),
        }
    }

    fn resolve_test_intent(
        local_instance_id: InstanceId,
        runtime: &RuntimeManager,
        req: &CreateProductConversationRequest,
        source: &CreateSource,
        resolution: ProductCreationResolution<'_>,
    ) -> Result<ProductCreationIntent, DbError> {
        let internal = !matches!(source, CreateSource::UserApi);
        let prepared = validate_new_creation_cwd(&req.cwd, runtime.runtime_home(), internal);
        let preparation = CreationCwdPreparation {
            new: prepared.clone(),
            replay: if internal {
                Ok(req.cwd.clone())
            } else {
                prepared
            },
        };
        resolve_intent(
            runtime,
            req,
            source,
            local_instance_id,
            resolution,
            &preparation,
        )
    }

    #[tokio::test]
    async fn internal_new_intent_freezes_defaults_but_keeps_omitted_effort_native() {
        let dir = tempfile::tempdir().unwrap();
        let registry = ModelRegistry::new(&LlmConfig {
            openai_api_key: Some("test-never-dispatched".into()),
            ..LlmConfig::default()
        });
        let manager = runtime(Database::open_in_memory().await.unwrap(), registry);
        let local_instance_id = manager.db().instance_id().await.unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let mut req = request(cwd.to_str().unwrap());
        req.model = Some("gpt-5.6-sol".into());
        // This fixture must falsify materializing the catalog native default into the intent.
        assert!(manager
            .model_registry()
            .effective_effort("gpt-5.6-sol", None)
            .level()
            .is_some());
        let intent = resolve_test_intent(
            local_instance_id,
            &manager,
            &req,
            &coordinator_source("first"),
            ProductCreationResolution::New {
                default_llm_language: phoenix_core::llm_language::LlmLanguage::Caveman,
            },
        )
        .unwrap();
        assert_eq!(intent.model, req.model);
        assert!(intent.effort.is_none());
        assert_eq!(
            intent.llm_language,
            phoenix_core::llm_language::LlmLanguage::Caveman
        );
        assert_eq!(intent.objective, req.objective);
        req.images = serde_json::from_value(serde_json::json!([
            {"media_type": "image/png", "data": "bytes"}
        ]))
        .unwrap();
        assert!(resolve_test_intent(
            local_instance_id,
            &manager,
            &req,
            &coordinator_source("first"),
            ProductCreationResolution::New {
                default_llm_language: LlmLanguage::default()
            }
        )
        .is_err());
    }

    #[tokio::test]
    async fn native_new_intent_requires_existing_directory_without_changing_user_api() {
        let manager = runtime(
            Database::open_in_memory().await.unwrap(),
            ModelRegistry::new(&LlmConfig {
                openai_api_key: Some("test-never-dispatched".into()),
                ..LlmConfig::default()
            }),
        );
        let local_instance_id = manager.db().instance_id().await.unwrap();
        let dir = tempfile::tempdir_in(manager.runtime_home()).unwrap();
        let missing = dir.path().canonicalize().unwrap().join("not-created");
        let mut req = request(missing.to_str().unwrap());
        req.model = Some("gpt-5.6-sol".into());
        assert!(matches!(
            resolve_test_intent(
                local_instance_id,
                &manager,
                &req,
                &coordinator_source("first"),
                ProductCreationResolution::New {
                    default_llm_language: LlmLanguage::default(),
                },
            ),
            Err(DbError::ProductCreationIntentInvalid(_))
        ));
        assert!(!missing.exists());
        let api_intent = resolve_test_intent(
            local_instance_id,
            &manager,
            &req,
            &CreateSource::UserApi,
            ProductCreationResolution::New {
                default_llm_language: LlmLanguage::default(),
            },
        )
        .unwrap();
        assert_eq!(api_intent.cwd, req.cwd);
        assert!(!missing.exists());
    }

    #[tokio::test]
    async fn federated_native_creation_rejects_invalid_cwd_without_durable_acceptance() {
        use phoenix_core::domain::instance_identity::FederationCredentialVerifier;

        let db = Database::open_in_memory().await.unwrap();
        let caller = InstanceId::new();
        let verifier = FederationCredentialVerifier::from_bearer(b"native-create-peer");
        db.replace_federation_enrollment(caller, &"native peer".parse().unwrap(), &verifier)
            .await
            .unwrap();
        let authority: phoenix_db::FederationConversationAuthority =
            phoenix_db::FederationSendAuthority::new(caller, verifier).into();
        let manager = runtime(
            db.clone(),
            ModelRegistry::new(&LlmConfig {
                openai_api_key: Some("test-never-dispatched".into()),
                ..LlmConfig::default()
            }),
        );
        let dir = tempfile::tempdir_in(manager.runtime_home()).unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let cwd = cwd.to_str().unwrap();
        let missing = format!("{cwd}/not-created");
        let product = ProductConversationId::new();
        for invalid_cwd in [
            ".".to_owned(),
            "~".to_owned(),
            "~/project".to_owned(),
            format!(" {cwd}"),
            format!("{cwd} "),
            format!("{cwd}\n"),
            format!("{cwd}/."),
            missing.clone(),
        ] {
            let req = request(&invalid_cwd);
            let request_id = req.request_id.clone();
            let result = accept_create_from_federation(
                &manager,
                authority.clone(),
                req,
                product.clone(),
                "foreign-transcript".into(),
                SourceToolCall {
                    message_id: "foreign-message".into(),
                    tool_use_id: "foreign-call".into(),
                },
            )
            .await;
            assert!(
                matches!(result, Err(AppError::BadRequest(_))),
                "{invalid_cwd:?}"
            );
            assert!(db
                .get_product_creation_job(&request_id)
                .await
                .unwrap()
                .is_none());
            assert!(db
                .read_product_creation_status_as_federation(&authority, &request_id)
                .await
                .unwrap()
                .is_none());
        }
        assert!(!Path::new(&missing).exists());
        let job_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM product_creation_jobs")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(job_count, 0);
    }

    #[tokio::test]
    async fn federated_creation_request_uuid_is_private_to_its_accepting_peer() {
        use phoenix_core::domain::instance_identity::FederationCredentialVerifier;

        let db = Database::open_in_memory().await.unwrap();
        let owner_id = InstanceId::new();
        let other_id = InstanceId::new();
        let owner_verifier = FederationCredentialVerifier::from_bearer(b"owner-create-peer");
        let other_verifier = FederationCredentialVerifier::from_bearer(b"other-create-peer");
        db.replace_federation_enrollment(owner_id, &"owner peer".parse().unwrap(), &owner_verifier)
            .await
            .unwrap();
        db.replace_federation_enrollment(other_id, &"other peer".parse().unwrap(), &other_verifier)
            .await
            .unwrap();
        let owner: phoenix_db::FederationConversationAuthority =
            phoenix_db::FederationSendAuthority::new(owner_id, owner_verifier).into();
        let other: phoenix_db::FederationConversationAuthority =
            phoenix_db::FederationSendAuthority::new(other_id, other_verifier).into();
        let manager = runtime(
            db.clone(),
            ModelRegistry::new(&LlmConfig {
                openai_api_key: Some("test-never-dispatched".into()),
                ..LlmConfig::default()
            }),
        );
        let dir = tempfile::tempdir_in(manager.runtime_home()).unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let original = request(cwd.to_str().unwrap());
        let request_id = original.request_id.clone();
        let product = ProductConversationId::new();
        let source_call = SourceToolCall {
            message_id: "owner-message".into(),
            tool_use_id: "owner-tool".into(),
        };
        let validation_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&validation_count);
        let accepted = accept_create_with_cwd_validator(
            &manager,
            original,
            CreateSource::FederatedConversation {
                authority: owner.clone(),
                product_conversation_id: product.clone(),
                transcript_id: "owner-transcript".into(),
                source_call: source_call.clone(),
            },
            move |cwd, home, internal| {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                validate_new_creation_cwd(cwd, home, internal)
            },
        )
        .await
        .unwrap();
        drop(dir);
        assert!(!cwd.exists());

        for different_intent in [false, true] {
            let mut other_request = request(cwd.to_str().unwrap());
            other_request.request_id = request_id.clone();
            if different_intent {
                other_request.objective = "different objective".into();
            }
            let count = Arc::clone(&validation_count);
            let result = accept_create_with_cwd_validator(
                &manager,
                other_request,
                CreateSource::FederatedConversation {
                    authority: other.clone(),
                    product_conversation_id: ProductConversationId::new(),
                    transcript_id: "other-transcript".into(),
                    source_call: SourceToolCall {
                        message_id: "other-message".into(),
                        tool_use_id: "other-tool".into(),
                    },
                },
                move |cwd, home, internal| {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    validate_new_creation_cwd(cwd, home, internal)
                },
            )
            .await;
            assert!(matches!(result, Err(AppError::Conflict(_))));
            assert!(
                read_create_status_from_federation(&manager, &other, &request_id)
                    .await
                    .is_err()
            );
        }

        let mut changed_same_owner = request(cwd.to_str().unwrap());
        changed_same_owner.request_id = request_id.clone();
        changed_same_owner.objective = "different objective".into();
        let count = Arc::clone(&validation_count);
        let conflict = accept_create_with_cwd_validator(
            &manager,
            changed_same_owner,
            CreateSource::FederatedConversation {
                authority: owner.clone(),
                product_conversation_id: ProductConversationId::new(),
                transcript_id: "owner-transcript".into(),
                source_call: SourceToolCall {
                    message_id: "owner-message".into(),
                    tool_use_id: "owner-tool".into(),
                },
            },
            move |cwd, home, internal| {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                validate_new_creation_cwd(cwd, home, internal)
            },
        )
        .await;
        assert!(matches!(conflict, Err(AppError::Conflict(_))));

        let mut replay_request = request(cwd.to_str().unwrap());
        replay_request.request_id = request_id.clone();
        let count = Arc::clone(&validation_count);
        let replay = accept_create_with_cwd_validator(
            &manager,
            replay_request,
            CreateSource::FederatedConversation {
                authority: owner.clone(),
                product_conversation_id: product,
                transcript_id: "owner-transcript".into(),
                source_call,
            },
            move |cwd, home, internal| {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                validate_new_creation_cwd(cwd, home, internal)
            },
        )
        .await
        .unwrap();
        assert_eq!(
            replay.product_conversation_id,
            accepted.product_conversation_id
        );
        assert_eq!(
            validation_count.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            read_create_status_from_federation(&manager, &owner, &request_id)
                .await
                .unwrap()
                .unwrap()
                .request_id,
            request_id
        );
    }

    #[tokio::test]
    async fn retry_preserves_omitted_effort_but_conflicts_when_explicit_effort_is_omitted() {
        use phoenix_core::domain::llm_types::ModelEffort;

        let db = Database::open_in_memory().await.unwrap();
        let local_instance_id = db.instance_id().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::PhoenixNative)
            .await
            .unwrap();
        let source = CreateSource::CurrentCoordinator {
            product_conversation_id: coordinator.product_conversation_id.clone(),
            transcript_id: coordinator.id.clone(),
            source_call: SourceToolCall {
                message_id: "effort-message".into(),
                tool_use_id: "effort-call".into(),
            },
        };
        let initial = runtime(
            db.clone(),
            ModelRegistry::new(&LlmConfig {
                openai_api_key: Some("test-never-dispatched".into()),
                default_model: Some("gpt-5.6-sol".into()),
                ..LlmConfig::default()
            }),
        );
        let changed_defaults = runtime(
            db.clone(),
            ModelRegistry::new(&LlmConfig {
                openai_api_key: Some("test-never-dispatched".into()),
                default_model: Some("gpt-6.1-sol".into()),
                ..LlmConfig::default()
            }),
        );
        assert_ne!(
            initial.model_registry().default_model_id(),
            changed_defaults.model_registry().default_model_id()
        );
        assert!(initial
            .model_registry()
            .effective_effort("gpt-5.6-sol", None)
            .level()
            .is_some());
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        for effort in [None, Some(ModelEffort::Medium)] {
            db.set_default_llm_language(LlmLanguage::PhoenixNative)
                .await
                .unwrap();
            let mut req = request(cwd.to_str().unwrap());
            req.effort = effort;
            let ProductCreationAcceptOutcome::Accepted(accepted) = db
                .accept_product_creation_resolved(
                    &req.request_id,
                    Some(&coordinator.id),
                    |resolution| {
                        resolve_test_intent(local_instance_id, &initial, &req, &source, resolution)
                    },
                )
                .await
                .unwrap()
            else {
                panic!("first request must be accepted");
            };
            assert_eq!(accepted.intent.effort, effort);
            assert_eq!(accepted.intent.model.as_deref(), Some("gpt-5.6-sol"));
            assert_eq!(accepted.intent.llm_language, LlmLanguage::PhoenixNative);
            db.set_default_llm_language(LlmLanguage::Caveman)
                .await
                .unwrap();
            req.effort = None;
            let outcome = db
                .accept_product_creation_resolved(
                    &req.request_id,
                    Some(&coordinator.id),
                    |resolution| {
                        resolve_test_intent(
                            local_instance_id,
                            &changed_defaults,
                            &req,
                            &source,
                            resolution,
                        )
                    },
                )
                .await
                .unwrap();
            if effort.is_some() {
                assert!(matches!(outcome, ProductCreationAcceptOutcome::Conflict));
            } else {
                let ProductCreationAcceptOutcome::Replayed(replayed) = outcome else {
                    panic!("originally omitted effort must replay after defaults change");
                };
                assert_eq!(replayed.intent, accepted.intent);
                assert!(replayed.intent.effort.is_none());
                assert_eq!(
                    replayed.product_conversation_id,
                    accepted.product_conversation_id
                );
            }
        }
    }

    #[tokio::test]
    async fn retry_reconciles_frozen_model_and_language_but_conflicts_changed_choices() {
        let db = Database::open_in_memory().await.unwrap();
        let local_instance_id = db.instance_id().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::PhoenixNative)
            .await
            .unwrap();
        let source = CreateSource::CurrentCoordinator {
            product_conversation_id: coordinator.product_conversation_id.clone(),
            transcript_id: coordinator.id.clone(),
            source_call: SourceToolCall {
                message_id: "frozen-choice-message".into(),
                tool_use_id: "frozen-choice-call".into(),
            },
        };
        let [initial, changed_defaults] = ["gpt-5.6-sol", "gpt-6.1-sol"].map(|model| {
            runtime(
                db.clone(),
                ModelRegistry::new(&LlmConfig {
                    openai_api_key: Some("test-never-dispatched".into()),
                    default_model: Some(model.into()),
                    ..LlmConfig::default()
                }),
            )
        });
        assert_eq!(
            changed_defaults.model_registry().default_model_id(),
            "gpt-6.1-sol"
        );

        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let mut req = request(cwd.to_str().unwrap());
        req.model = Some("gpt-5.6-sol".into());
        req.llm_language = Some(LlmLanguage::PhoenixNative);
        let ProductCreationAcceptOutcome::Accepted(accepted) = db
            .accept_product_creation_resolved(&req.request_id, Some(&coordinator.id), |r| {
                resolve_test_intent(local_instance_id, &initial, &req, &source, r)
            })
            .await
            .unwrap()
        else {
            panic!("first request must be accepted");
        };
        db.set_default_llm_language(LlmLanguage::Caveman)
            .await
            .unwrap();

        let cases = [
            ("omitted", None, None, true),
            (
                "explicit_same",
                Some("gpt-5.6-sol"),
                Some(LlmLanguage::PhoenixNative),
                true,
            ),
            ("changed_model", Some("gpt-6.1-sol"), None, false),
            ("changed_language", None, Some(LlmLanguage::Caveman), false),
        ];
        for (label, model, language, replay) in cases {
            let expected_model = model.unwrap_or("gpt-5.6-sol");
            let expected_language = language.unwrap_or(LlmLanguage::PhoenixNative);
            req.model = model.map(str::to_owned);
            req.llm_language = language;
            let candidate = resolve_test_intent(
                local_instance_id,
                &changed_defaults,
                &req,
                &source,
                ProductCreationResolution::Existing(&accepted),
            )
            .unwrap();
            assert_eq!(candidate.model.as_deref(), Some(expected_model), "{label}");
            assert_eq!(candidate.llm_language, expected_language, "{label}");
            assert!(candidate.effort.is_none(), "{label}");
            match db
                .accept_product_creation_resolved(&req.request_id, Some(&coordinator.id), |r| {
                    resolve_test_intent(local_instance_id, &changed_defaults, &req, &source, r)
                })
                .await
                .unwrap()
            {
                ProductCreationAcceptOutcome::Accepted(_) => panic!("{label}: retry accepted"),
                ProductCreationAcceptOutcome::Replayed(result) => {
                    assert!(replay, "{label}: changed choice replayed");
                    assert_eq!(result.intent, accepted.intent, "{label}");
                    assert_eq!(
                        result.product_conversation_id, accepted.product_conversation_id,
                        "{label}"
                    );
                }
                ProductCreationAcceptOutcome::Conflict => {
                    assert!(!replay, "{label}: expected replay");
                }
            }
        }
        let stored = db
            .get_product_creation_job(&req.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.intent, accepted.intent);
    }

    #[tokio::test]
    async fn replay_uses_frozen_defaults_and_preserves_first_source_attribution() {
        let db = Database::open_in_memory().await.unwrap();
        let local_instance_id = db.instance_id().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let source = CreateSource::CurrentCoordinator {
            product_conversation_id: coordinator.product_conversation_id.clone(),
            transcript_id: coordinator.id.clone(),
            source_call: SourceToolCall {
                message_id: "first-message".into(),
                tool_use_id: "first-call".into(),
            },
        };
        let mut req = request("/no-longer-present-project");
        let frozen = ProductCreationIntent {
            cwd: req.cwd.clone(),
            objective: req.objective.clone(),
            model: Some("removed-model".into()),
            effort: None,
            llm_language: phoenix_core::llm_language::LlmLanguage::Caveman,
            images: Vec::new(),
            origin: source.origin(local_instance_id),
        };
        let ProductCreationAcceptOutcome::Accepted(existing) = db
            .accept_product_creation_resolved(&req.request_id, Some(&coordinator.id), |_| {
                Ok(frozen.clone())
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        let manager = runtime(db.clone(), ModelRegistry::new_empty());
        let retry = CreateSource::CurrentCoordinator {
            product_conversation_id: coordinator.product_conversation_id.clone(),
            transcript_id: coordinator.id.clone(),
            source_call: SourceToolCall {
                message_id: "retry-message".into(),
                tool_use_id: "retry-call".into(),
            },
        };
        let resolved = resolve_test_intent(
            local_instance_id,
            &manager,
            &req,
            &retry,
            ProductCreationResolution::Existing(&existing),
        )
        .unwrap();
        assert_eq!(resolved.model, frozen.model);
        assert_eq!(resolved.llm_language, frozen.llm_language);
        assert!(resolved.effort.is_none());
        let outcome = db
            .accept_product_creation_resolved(
                &req.request_id,
                Some(&coordinator.id),
                |resolution| {
                    resolve_test_intent(local_instance_id, &manager, &req, &retry, resolution)
                },
            )
            .await
            .unwrap();
        let ProductCreationAcceptOutcome::Replayed(replayed) = outcome else {
            panic!()
        };
        assert_eq!(
            replayed.product_conversation_id,
            existing.product_conversation_id
        );
        assert_eq!(replayed.intent.origin, source.origin(local_instance_id));
        req.objective.push_str(" changed");
        assert!(matches!(
            db.accept_product_creation_resolved(
                &req.request_id,
                Some(&coordinator.id),
                |resolution| resolve_test_intent(
                    local_instance_id,
                    &manager,
                    &req,
                    &retry,
                    resolution
                )
            )
            .await
            .unwrap(),
            ProductCreationAcceptOutcome::Conflict
        ));
        assert!(matches!(
            db.accept_product_creation_resolved(&req.request_id, None, |_| Ok(frozen.clone()))
                .await,
            Err(DbError::CurrentCoordinatorRequired)
        ));
    }

    #[tokio::test]
    async fn http_retry_normalizes_original_cwd_without_revalidating_frozen_model() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().await.unwrap();
        let local_instance_id = db.instance_id().await.unwrap();
        let manager = runtime(db.clone(), ModelRegistry::new_empty());
        let raw_cwd = format!("  {}/./  ", dir.path().display());
        let req = request(&raw_cwd);
        let cwd = crate::conversation_cwd::normalize_product_creation_cwd_intent(
            &raw_cwd,
            manager.runtime_home(),
        )
        .unwrap();
        let frozen = ProductCreationIntent {
            cwd,
            objective: req.objective.clone(),
            origin: ProductCreationOrigin::UserApi,
            model: Some("no-longer-available-model".into()),
            effort: None,
            llm_language: LlmLanguage::default(),
            images: Vec::new(),
        };
        db.accept_product_creation(&req.request_id, &frozen)
            .await
            .unwrap();
        let outcome = db
            .accept_product_creation_resolved(&req.request_id, None, |resolution| {
                resolve_test_intent(
                    local_instance_id,
                    &manager,
                    &req,
                    &CreateSource::UserApi,
                    resolution,
                )
            })
            .await
            .unwrap();
        let ProductCreationAcceptOutcome::Replayed(job) = outcome else {
            panic!("same HTTP cwd spelling must replay the normalized winner");
        };
        assert_eq!(job.intent, frozen);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocked_creation_cwd_validation_does_not_hold_sqlite_writer() {
        let db = Database::open_in_memory().await.unwrap();
        let manager = Arc::new(runtime(db.clone(), ModelRegistry::new_empty()));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let acceptance = tokio::spawn(async move {
            accept_create_with_cwd_validator(
                &manager,
                request("/blocked-filesystem"),
                CreateSource::UserApi,
                move |_, _, _| {
                    entered_tx.send(()).unwrap();
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(30))
                        .expect("test must release blocked filesystem validator");
                    Err("deliberately invalid test directory".into())
                },
            )
            .await
        });
        entered_rx.await.unwrap();
        let write = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            db.set_default_llm_language(LlmLanguage::Caveman),
        )
        .await;
        release_tx.send(()).unwrap();
        write
            .expect("independent settings write must finish before filesystem validation resumes")
            .unwrap();
        assert!(matches!(
            acceptance.await.unwrap(),
            Err(AppError::BadRequest(_))
        ));
        assert_eq!(
            db.get_default_llm_language().await.unwrap(),
            LlmLanguage::Caveman
        );
    }

    #[tokio::test]
    async fn ordinary_runtime_does_not_get_gc_action_tools() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation(
            "ordinary-tool-test",
            "ordinary-tool-test",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        let manager = Arc::new(runtime(db.clone(), ModelRegistry::new_empty()));
        assert!(
            crate::conversation_action_tools::tools(manager.clone(), "ordinary-tool-test")
                .await
                .unwrap()
                .is_empty()
        );
        let current = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let tools = crate::conversation_action_tools::tools(manager.clone(), &current.id)
            .await
            .unwrap();
        assert_eq!(
            tools.iter().map(|tool| tool.name()).collect::<Vec<_>>(),
            vec![
                "rename_conversation",
                "create_conversation",
                "continue_conversation",
                "read_automatic_continuation",
                "set_automatic_continuation"
            ]
        );
        assert!(crate::conversation_action_tools::tools(manager, "missing")
            .await
            .unwrap()
            .is_empty());
    }
}
