//! Application actions shared by HTTP ingress and the current Coordinator's tools.

use std::path::Path;

use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;

use crate::api::handlers::AppError;
use crate::api::product_conversations::db_to_app;
use crate::api::{ConflictErrorResponse, CreateProductConversationRequest};
use crate::db::{
    DbError, NormalizedConversationTitle, ProductCreationAcceptOutcome, ProductCreationImage,
    ProductCreationIntent, ProductCreationJobRecord, ProductCreationOrigin,
    ProductCreationResolution,
};
use crate::runtime::RuntimeManager;

pub(crate) fn normalize_product_conversation_title(title: &str) -> Result<String, AppError> {
    NormalizedConversationTitle::parse(title)
        .map(|title| title.as_str().to_owned())
        .map_err(db_to_app)
}

pub(crate) enum CreateSource {
    UserApi,
    CurrentCoordinator {
        product_conversation_id: ProductConversationId,
        transcript_id: String,
        source_call: SourceToolCall,
    },
}

impl CreateSource {
    fn origin(&self) -> ProductCreationOrigin {
        match self {
            Self::UserApi => ProductCreationOrigin::UserApi,
            Self::CurrentCoordinator {
                product_conversation_id,
                transcript_id,
                source_call,
            } => ProductCreationOrigin::InternalConversation {
                product_conversation_id: product_conversation_id.clone(),
                transcript_id: transcript_id.clone(),
                source_call: source_call.clone(),
            },
        }
    }

    fn coordinator_transcript_id(&self) -> Option<&str> {
        match self {
            Self::UserApi => None,
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
    resolution: ProductCreationResolution<'_>,
) -> Result<ProductCreationIntent, DbError> {
    let origin = source.origin();
    if let ProductCreationResolution::Existing(existing) = resolution {
        let cwd = if matches!(source, CreateSource::UserApi) && req.cwd != existing.intent.cwd {
            crate::conversation_cwd::normalize_product_creation_cwd_intent(
                &req.cwd,
                runtime.runtime_home(),
            )
            .map_err(|error| invalid(error.to_string()))?
        } else {
            req.cwd.clone()
        };
        return Ok(ProductCreationIntent {
            cwd,
            objective: req.objective.clone(),
            origin,
            model: req.model.clone().or_else(|| existing.intent.model.clone()),
            effort: req.effort.or(existing.intent.effort),
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
    let internal = matches!(source, CreateSource::CurrentCoordinator { .. });
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
    let cwd = if internal {
        crate::conversation_cwd::validate_conversation_cwd(&req.cwd)
            .map(crate::conversation_cwd::ValidConversationCwd::into_raw)
    } else {
        crate::conversation_cwd::normalize_product_creation_cwd_intent(
            &req.cwd,
            runtime.runtime_home(),
        )
    }
    .map_err(|error| invalid(error.to_string()))?;
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

pub(crate) async fn accept_create(
    runtime: &RuntimeManager,
    req: CreateProductConversationRequest,
    source: CreateSource,
) -> Result<CreateAccepted, AppError> {
    let _authority = runtime.acquire_local_authority_pass().map_err(|()| {
        AppError::Internal("runtime admission closed after fatal local authority loss".to_owned())
    })?;
    uuid::Uuid::parse_str(&req.request_id)
        .map_err(|_| AppError::BadRequest("request_id must be a UUID".to_owned()))?;
    let outcome = runtime
        .db()
        .accept_product_creation_resolved(
            &req.request_id,
            source.coordinator_transcript_id(),
            |resolution| resolve_intent(runtime, &req, &source, resolution),
        )
        .await
        .map_err(db_to_app)?;
    let job: ProductCreationJobRecord = match outcome {
        ProductCreationAcceptOutcome::Accepted(job)
        | ProductCreationAcceptOutcome::Replayed(job) => job,
        ProductCreationAcceptOutcome::Conflict(_) => {
            return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                "request_id conflicts with immutable creation intent",
                "product_creation_request_conflict",
            ))));
        }
    };
    runtime.kick_creation_worker();
    Ok(CreateAccepted {
        request_id: job.request_id,
        product_conversation_id: job.product_conversation_id,
    })
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

    #[tokio::test]
    async fn internal_new_intent_freezes_defaults_but_keeps_omitted_effort_native() {
        let dir = tempfile::tempdir().unwrap();
        let registry = ModelRegistry::new(&LlmConfig {
            openai_api_key: Some("test-never-dispatched".into()),
            ..LlmConfig::default()
        });
        let manager = runtime(Database::open_in_memory().await.unwrap(), registry);
        let mut req = request(dir.path().to_str().unwrap());
        req.model = Some("gpt-5.6-sol".into());
        // This fixture must falsify materializing the catalog native default into the intent.
        assert!(manager
            .model_registry()
            .effective_effort("gpt-5.6-sol", None)
            .level()
            .is_some());
        let intent = resolve_intent(
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
        assert!(resolve_intent(
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
        let dir = tempfile::tempdir_in(manager.runtime_home()).unwrap();
        let missing = dir.path().canonicalize().unwrap().join("not-created");
        let mut req = request(missing.to_str().unwrap());
        req.model = Some("gpt-5.6-sol".into());
        assert!(matches!(
            resolve_intent(
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
        let api_intent = resolve_intent(
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
    async fn replay_uses_frozen_defaults_and_preserves_first_source_attribution() {
        let db = Database::open_in_memory().await.unwrap();
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
            origin: source.origin(),
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
        let resolved = resolve_intent(
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
                |resolution| resolve_intent(&manager, &req, &retry, resolution),
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
        assert_eq!(replayed.intent.origin, source.origin());
        req.objective.push_str(" changed");
        assert!(matches!(
            db.accept_product_creation_resolved(
                &req.request_id,
                Some(&coordinator.id),
                |resolution| resolve_intent(&manager, &req, &retry, resolution)
            )
            .await
            .unwrap(),
            ProductCreationAcceptOutcome::Conflict(_)
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
                resolve_intent(&manager, &req, &CreateSource::UserApi, resolution)
            })
            .await
            .unwrap();
        let ProductCreationAcceptOutcome::Replayed(job) = outcome else {
            panic!("same HTTP cwd spelling must replay the normalized winner");
        };
        assert_eq!(job.intent, frozen);
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
            vec!["rename_conversation", "create_conversation"]
        );
        assert!(crate::conversation_action_tools::tools(manager, "missing")
            .await
            .unwrap()
            .is_empty());
    }
}
