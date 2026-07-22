use phoenix_db::workflow::{CreateWorkflowWithExternalAcceptance, WorkflowRepository};
use phoenix_workflow::{
    AcceptanceProfile, CodecRef, CommitOutcome, ExternalAcceptanceEnabled,
    ExternalAcceptanceOutcome, NonEmptyExternalKey, ProfileRef, RuntimeAcceptanceDisabled, ScopeId,
    SupportedCodecRegistry, Timestamp, WorkflowId,
};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::api::{
    capture_pr_auto_fix_context_for_conversation, record_pr_auto_fix_context_baseline_for_artifact,
    AddressPrFeedbackResponse,
};
use crate::send_chat_service::{
    MessageExpansionPolicy, SendChatApplicationService, SendChatOutcome, SendChatRequest,
    SendChatServiceError,
};

const PROFILE_KIND: &str = "address-pr-feedback";
const PROFILE_VERSION: u32 = 1;
const SNAPSHOT_CODEC: CodecRef = CodecRef {
    family: "address-pr-feedback.snapshot.v1",
    version: 1,
};
#[derive(Debug, Clone)]
pub(crate) struct AddressFeedbackWorkflowRequest {
    pub conversation_id: String,
    pub message_id: String,
    pub guidance: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AddressFeedbackWorkflowError {
    #[error("durable workflow registry is unavailable: {0}")]
    Workflow(String),
    #[error("PR context capture failed: {0}")]
    Capture(String),
    #[error("Address feedback dispatch failed: {0}")]
    Dispatch(String),
    #[error("message_id was already used for a different target or payload")]
    IdempotencyConflict,
    #[error("Address feedback rejected: {message}")]
    Rejected { message: String, code: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AddressFeedbackSnapshot {
    conversation_id: String,
    message_id: String,
    guidance: Option<String>,
    status: AddressFeedbackStatus,
    target: Option<AddressFeedbackTarget>,
    head_oid: Option<String>,
    artifact_path: Option<String>,
    model_message: Option<String>,
    dispatch: Option<AddressFeedbackDispatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AddressFeedbackStatus {
    Accepted,
    Captured,
    HandedOff,
    DuplicateNoOp,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AddressFeedbackTarget {
    repo_owner: String,
    repo_name: String,
    pr_number: u64,
    head_oid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AddressFeedbackDispatch {
    queued: bool,
    steering: bool,
    already_persisted: bool,
}

#[derive(Clone)]
pub(crate) struct AddressFeedbackWorkflowService {
    state: crate::api::AppState,
}

impl AddressFeedbackWorkflowService {
    pub(crate) fn new(state: crate::api::AppState) -> Self {
        Self { state }
    }

    pub(crate) async fn recover_pending(&self) {
        let repo = WorkflowRepository::new(self.state.db.pool().clone());
        let rows = sqlx::query_as::<_, (i64, Vec<u8>, i64)>(
            "SELECT workflow_id, snapshot_payload, version FROM workflows WHERE profile_kind = ? AND profile_version = ? AND status = 'Active'",
        )
        .bind(PROFILE_KIND)
        .bind(i64::from(PROFILE_VERSION))
        .fetch_all(self.state.db.pool())
        .await;
        let Ok(rows) = rows else {
            tracing::warn!(error = ?rows.err(), "failed to load pending address feedback workflows");
            return;
        };
        for (workflow_id_raw, payload, version_raw) in rows {
            let Ok(snapshot) = serde_json::from_slice::<AddressFeedbackSnapshot>(&payload) else {
                tracing::warn!(
                    workflow_id = workflow_id_raw,
                    "skipping unreadable address feedback workflow snapshot"
                );
                continue;
            };
            if snapshot.dispatch.is_some()
                || snapshot.model_message.is_none()
                || snapshot.target.is_none()
            {
                continue;
            }
            let Ok(workflow_id) = u64::try_from(workflow_id_raw).map(WorkflowId) else {
                continue;
            };
            let req = AddressFeedbackWorkflowRequest {
                conversation_id: snapshot.conversation_id.clone(),
                message_id: snapshot.message_id.clone(),
                guidance: snapshot.guidance.clone(),
                user_agent: None,
            };
            if let Err(error) = self
                .dispatch_persisted_snapshot(
                    &repo,
                    workflow_id,
                    u64::try_from(version_raw).unwrap_or(0),
                    snapshot,
                    req,
                )
                .await
            {
                tracing::warn!(workflow_id = workflow_id_raw, error = ?error, "failed to recover address feedback workflow");
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn submit(
        &self,
        req: AddressFeedbackWorkflowRequest,
    ) -> Result<AddressPrFeedbackResponse, AddressFeedbackWorkflowError> {
        let workflow_id = workflow_id_for(&req.conversation_id, &req.message_id);
        let target_scope = ScopeId::new(format!("conversation:{}", req.conversation_id))
            .ok_or_else(|| {
                AddressFeedbackWorkflowError::Workflow("empty target scope".to_string())
            })?;
        let idempotency_key =
            NonEmptyExternalKey::new(req.message_id.clone()).ok_or_else(|| {
                AddressFeedbackWorkflowError::Workflow("empty idempotency key".to_string())
            })?;
        let head_oid = local_head_oid(&self.state.db, &req.conversation_id).await?;
        let initial = AddressFeedbackSnapshot {
            conversation_id: req.conversation_id.clone(),
            message_id: req.message_id.clone(),
            guidance: req.guidance.clone(),
            status: AddressFeedbackStatus::Accepted,
            target: None,
            head_oid: head_oid.clone(),
            artifact_path: None,
            model_message: None,
            dispatch: None,
        };
        let intent_fingerprint = snapshot_fingerprint(&initial)?;
        let repo = WorkflowRepository::new(self.state.db.pool().clone());
        let create = CreateWorkflowWithExternalAcceptance {
            workflow_id,
            profile: profile_ref(),
            acceptance: acceptance_profile().erase(),
            target_scope,
            idempotency_key,
            intent_fingerprint,
            snapshot_codec: SNAPSHOT_CODEC,
            snapshot_payload: encode_snapshot(&initial)?,
            receipt_handle: req.message_id.as_bytes().to_vec(),
            disposition_handle: req.conversation_id.as_bytes().to_vec(),
            now: now_ts(),
        };
        let accepted = repo
            .create_workflow_with_external_acceptance(&create)
            .await
            .map_err(|e| AddressFeedbackWorkflowError::Workflow(format!("{e:?}")))?;
        match accepted {
            ExternalAcceptanceOutcome::Conflict => {
                return Err(AddressFeedbackWorkflowError::IdempotencyConflict);
            }
            ExternalAcceptanceOutcome::Replayed(_) => {
                if let Some((snapshot, version)) =
                    load_snapshot(&self.state.db, workflow_id).await?
                {
                    if let Some(response) =
                        response_from_completed_snapshot(workflow_id, &req.message_id, &snapshot)
                    {
                        return Ok(response);
                    }
                    if snapshot.model_message.is_some() && snapshot.target.is_some() {
                        return self
                            .dispatch_persisted_snapshot(&repo, workflow_id, version, snapshot, req)
                            .await;
                    }
                }
            }
            ExternalAcceptanceOutcome::Created(_) => {}
        }

        let capture =
            capture_pr_auto_fix_context_for_conversation(&self.state, &req.conversation_id)
                .await
                .map_err(map_app_error_for_capture)?;
        let model_message =
            render_address_feedback_xml(&capture, head_oid.as_deref(), req.guidance.as_deref());

        let captured = AddressFeedbackSnapshot {
            status: AddressFeedbackStatus::Captured,
            target: Some(AddressFeedbackTarget {
                repo_owner: capture.repo_owner.clone(),
                repo_name: capture.repo_name.clone(),
                pr_number: capture.pr_number,
                head_oid: head_oid.clone(),
            }),
            head_oid,
            artifact_path: Some(capture.artifact_path.clone()),
            model_message: Some(model_message.clone()),
            ..initial.clone()
        };
        commit_snapshot(&repo, workflow_id, 0, 1, &captured).await?;

        let outcome =
            SendChatApplicationService::new(self.state.db.clone(), self.state.runtime.clone())
                .send(SendChatRequest {
                    conversation_id: req.conversation_id,
                    text: model_message,
                    message_id: req.message_id.clone(),
                    images: Vec::new(),
                    files: Vec::new(),
                    user_agent: req.user_agent,
                    expansion_policy: MessageExpansionPolicy::LiteralText,
                })
                .await
                .map_err(map_dispatch_error)?;
        let (queued, steering, no_op, already_persisted) = match outcome {
            SendChatOutcome::Delivered => (true, false, false, false),
            SendChatOutcome::QueuedAsSteering => (true, true, false, false),
            SendChatOutcome::AlreadyPersisted => (true, false, true, true),
            SendChatOutcome::Rejected { message, code } => {
                let failed = AddressFeedbackSnapshot {
                    status: AddressFeedbackStatus::Failed,
                    ..captured
                };
                let _ = commit_snapshot(&repo, workflow_id, 1, 2, &failed).await;
                return Err(AddressFeedbackWorkflowError::Rejected {
                    message,
                    code: code.to_string(),
                });
            }
        };
        record_pr_auto_fix_context_baseline_for_artifact(
            self.state.runtime.db(),
            &captured.conversation_id,
            captured.artifact_path.as_deref().unwrap_or_default(),
        )
        .await
        .map_err(|e| AddressFeedbackWorkflowError::Workflow(format!("{e:?}")))?;
        let handed_off = AddressFeedbackSnapshot {
            status: if no_op {
                AddressFeedbackStatus::DuplicateNoOp
            } else {
                AddressFeedbackStatus::HandedOff
            },
            dispatch: Some(AddressFeedbackDispatch {
                queued,
                steering,
                already_persisted,
            }),
            ..captured
        };
        let _ = commit_snapshot(&repo, workflow_id, 1, 2, &handed_off).await;

        Ok(AddressPrFeedbackResponse {
            workflow_id: workflow_id.0,
            message_id: req.message_id,
            queued,
            steering,
            no_op,
            artifact_path: Some(capture.artifact_path),
            pr_number: Some(capture.pr_number),
            repo_owner: Some(capture.repo_owner),
            repo_name: Some(capture.repo_name),
        })
    }

    async fn dispatch_persisted_snapshot(
        &self,
        repo: &WorkflowRepository,
        workflow_id: WorkflowId,
        version: u64,
        snapshot: AddressFeedbackSnapshot,
        req: AddressFeedbackWorkflowRequest,
    ) -> Result<AddressPrFeedbackResponse, AddressFeedbackWorkflowError> {
        let model_message = snapshot.model_message.clone().ok_or_else(|| {
            AddressFeedbackWorkflowError::Workflow(
                "workflow snapshot has no model message".to_string(),
            )
        })?;
        let outcome =
            SendChatApplicationService::new(self.state.db.clone(), self.state.runtime.clone())
                .send(SendChatRequest {
                    conversation_id: req.conversation_id,
                    text: model_message,
                    message_id: req.message_id.clone(),
                    images: Vec::new(),
                    files: Vec::new(),
                    user_agent: req.user_agent,
                    expansion_policy: MessageExpansionPolicy::LiteralText,
                })
                .await
                .map_err(map_dispatch_error)?;
        let (queued, steering, no_op, already_persisted) = match outcome {
            SendChatOutcome::Delivered => (true, false, false, false),
            SendChatOutcome::QueuedAsSteering => (true, true, false, false),
            SendChatOutcome::AlreadyPersisted => (true, false, true, true),
            SendChatOutcome::Rejected { message, code } => {
                return Err(AddressFeedbackWorkflowError::Rejected {
                    message,
                    code: code.to_string(),
                });
            }
        };
        if let Some(artifact_path) = snapshot.artifact_path.as_deref() {
            record_pr_auto_fix_context_baseline_for_artifact(
                self.state.runtime.db(),
                &snapshot.conversation_id,
                artifact_path,
            )
            .await
            .map_err(|e| AddressFeedbackWorkflowError::Workflow(format!("{e:?}")))?;
        }
        let completed = AddressFeedbackSnapshot {
            status: if no_op {
                AddressFeedbackStatus::DuplicateNoOp
            } else {
                AddressFeedbackStatus::HandedOff
            },
            dispatch: Some(AddressFeedbackDispatch {
                queued,
                steering,
                already_persisted,
            }),
            ..snapshot.clone()
        };
        let _ = commit_snapshot(repo, workflow_id, version, version + 1, &completed).await;
        Ok(response_from_snapshot(
            workflow_id,
            req.message_id,
            queued,
            steering,
            no_op,
            &completed,
        ))
    }
}

async fn load_snapshot(
    db: &crate::db::Database,
    workflow_id: WorkflowId,
) -> Result<Option<(AddressFeedbackSnapshot, u64)>, AddressFeedbackWorkflowError> {
    let Some((payload, version)) = sqlx::query_as::<_, (Vec<u8>, i64)>(
        "SELECT snapshot_payload, version FROM workflows WHERE workflow_id = ?",
    )
    .bind(i64::try_from(workflow_id.0).unwrap_or(i64::MAX))
    .fetch_optional(db.pool())
    .await
    .map_err(|e| AddressFeedbackWorkflowError::Workflow(e.to_string()))?
    else {
        return Ok(None);
    };
    let snapshot: AddressFeedbackSnapshot = serde_json::from_slice(&payload)
        .map_err(|e| AddressFeedbackWorkflowError::Workflow(e.to_string()))?;
    Ok(Some((snapshot, u64::try_from(version).unwrap_or(0))))
}

fn response_from_completed_snapshot(
    workflow_id: WorkflowId,
    message_id: &str,
    snapshot: &AddressFeedbackSnapshot,
) -> Option<AddressPrFeedbackResponse> {
    let dispatch = snapshot.dispatch.as_ref()?;
    Some(response_from_snapshot(
        workflow_id,
        message_id.to_string(),
        dispatch.queued,
        dispatch.steering,
        true,
        snapshot,
    ))
}

fn response_from_snapshot(
    workflow_id: WorkflowId,
    message_id: String,
    queued: bool,
    steering: bool,
    no_op: bool,
    snapshot: &AddressFeedbackSnapshot,
) -> AddressPrFeedbackResponse {
    AddressPrFeedbackResponse {
        workflow_id: workflow_id.0,
        message_id,
        queued,
        steering,
        no_op,
        artifact_path: snapshot.artifact_path.clone(),
        pr_number: snapshot.target.as_ref().map(|target| target.pr_number),
        repo_owner: snapshot
            .target
            .as_ref()
            .map(|target| target.repo_owner.clone()),
        repo_name: snapshot
            .target
            .as_ref()
            .map(|target| target.repo_name.clone()),
    }
}

async fn local_head_oid(
    db: &crate::db::Database,
    conversation_id: &str,
) -> Result<Option<String>, AddressFeedbackWorkflowError> {
    let conv = db
        .get_conversation(conversation_id)
        .await
        .map_err(|e| AddressFeedbackWorkflowError::Workflow(e.to_string()))?;
    let (phoenix_core::domain::db_schema::ConvMode::Work { worktree_path, .. }
    | phoenix_core::domain::db_schema::ConvMode::Branch { worktree_path, .. }) = conv.conv_mode
    else {
        return Ok(None);
    };
    let output = phoenix_core::git::command()
        .current_dir(worktree_path.as_str())
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| AddressFeedbackWorkflowError::Workflow(e.to_string()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!oid.is_empty()).then_some(oid))
}

async fn commit_snapshot(
    repo: &WorkflowRepository,
    workflow_id: WorkflowId,
    expected_version: u64,
    transition_id: u64,
    snapshot: &AddressFeedbackSnapshot,
) -> Result<(), AddressFeedbackWorkflowError> {
    let outcome = repo
        .commit_transition_head_cas(&phoenix_db::workflow::CommitTransitionHeadCas {
            workflow_id,
            expected_version: phoenix_workflow::Version(expected_version),
            transition_id: phoenix_workflow::TransitionId(transition_id),
            generation: phoenix_workflow::Generation(0),
            next_status: phoenix_workflow::WorkflowStatus::Active,
            event_codec: SNAPSHOT_CODEC,
            event_payload: encode_snapshot(snapshot)?,
            next_snapshot_codec: SNAPSHOT_CODEC,
            next_snapshot_payload: encode_snapshot(snapshot)?,
            committed_at: now_ts(),
        })
        .await
        .map_err(|e| AddressFeedbackWorkflowError::Workflow(format!("{e:?}")))?;
    match outcome {
        CommitOutcome::Committed | CommitOutcome::VersionConflict => Ok(()),
        CommitOutcome::InvalidPlan | CommitOutcome::UnsupportedCodec => {
            Err(AddressFeedbackWorkflowError::Workflow(format!(
                "invalid workflow transition: {outcome:?}"
            )))
        }
    }
}

fn profile_ref() -> ProfileRef {
    ProfileRef {
        profile_kind: PROFILE_KIND.to_string(),
        profile_version: PROFILE_VERSION,
    }
}

fn acceptance_profile() -> AcceptanceProfile<RuntimeAcceptanceDisabled, ExternalAcceptanceEnabled> {
    AcceptanceProfile::new(
        profile_ref(),
        SupportedCodecRegistry::new([SNAPSHOT_CODEC]).expect("snapshot codec is non-empty"),
    )
}

fn encode_snapshot(
    snapshot: &AddressFeedbackSnapshot,
) -> Result<Vec<u8>, AddressFeedbackWorkflowError> {
    serde_json::to_vec(snapshot)
        .map_err(|e| AddressFeedbackWorkflowError::Workflow(format!("{e:?}")))
}

fn snapshot_fingerprint(
    snapshot: &AddressFeedbackSnapshot,
) -> Result<String, AddressFeedbackWorkflowError> {
    let encoded = encode_snapshot(snapshot)?;
    Ok(hex_sha256(&encoded))
}

fn workflow_id_for(conversation_id: &str, message_id: &str) -> WorkflowId {
    let digest =
        sha2::Sha256::digest(format!("{PROFILE_KIND}\0{conversation_id}\0{message_id}").as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    let value = (u64::from_be_bytes(bytes) & i64::MAX as u64).max(1);
    WorkflowId(value)
}

fn hex_sha256(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(&mut out, "{byte:02x}");
            out
        })
}

fn now_ts() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Timestamp(u64::try_from(millis).unwrap_or(u64::MAX))
}

fn render_address_feedback_xml(
    capture: &crate::api::PrAutoFixContextResponse,
    head_oid: Option<&str>,
    guidance: Option<&str>,
) -> String {
    let guidance = guidance.unwrap_or("Address the captured PR feedback and failing checks. Use the context artifact as the source of truth for this request.");
    format!(
        "<address_pr_feedback>\n  <target repo_owner=\"{}\" repo_name=\"{}\" pr_number=\"{}\"{} />\n  <context artifact=\"{}\" />\n  <guidance>{}</guidance>\n  <instruction>{}</instruction>\n</address_pr_feedback>",
        escape_xml(&capture.repo_owner),
        escape_xml(&capture.repo_name),
        capture.pr_number,
        head_oid
            .map(|oid| format!(" head_oid=\"{}\"", escape_xml(oid)))
            .unwrap_or_default(),
        escape_xml(&capture.artifact_path),
        escape_xml(guidance),
        escape_xml(&capture.message),
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn map_app_error_for_capture(error: crate::api::AppError) -> AddressFeedbackWorkflowError {
    match error {
        crate::api::AppError::BadRequest(message) => AddressFeedbackWorkflowError::Rejected {
            message,
            code: "pr_context_unavailable".to_string(),
        },
        crate::api::AppError::NotFound(message) => AddressFeedbackWorkflowError::Rejected {
            message,
            code: "conversation_not_found".to_string(),
        },
        crate::api::AppError::Conflict(conflict) => AddressFeedbackWorkflowError::Rejected {
            message: conflict.error,
            code: conflict.error_type,
        },
        other @ (crate::api::AppError::TypedBadRequest { .. }
        | crate::api::AppError::Forbidden(_)
        | crate::api::AppError::Internal(_)
        | crate::api::AppError::TypedInternal { .. }
        | crate::api::AppError::UnprocessableEntity(_)) => {
            AddressFeedbackWorkflowError::Capture(format!("{other:?}"))
        }
    }
}

fn map_dispatch_error(error: SendChatServiceError) -> AddressFeedbackWorkflowError {
    match error {
        SendChatServiceError::IdempotencyConflict => {
            AddressFeedbackWorkflowError::IdempotencyConflict
        }
        other @ (SendChatServiceError::NotFound(_)
        | SendChatServiceError::AttachmentValidation(_)
        | SendChatServiceError::Expansion { .. }
        | SendChatServiceError::Internal(_)
        | SendChatServiceError::Dispatch(_)) => {
            AddressFeedbackWorkflowError::Dispatch(other.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_message_escapes_target_context_and_guidance() {
        let capture = crate::api::PrAutoFixContextResponse {
            artifact_path: ".phoenix/pr-context/a&b.json".to_string(),
            pr_number: 42,
            repo_owner: "o<wner".to_string(),
            repo_name: "r\"epo".to_string(),
            message: "legacy".to_string(),
        };
        let rendered =
            render_address_feedback_xml(&capture, Some("abc123"), Some("fix <all> & \"now\""));
        assert!(rendered.contains("repo_owner=\"o&lt;wner\""));
        assert!(rendered.contains("repo_name=\"r&quot;epo\""));
        assert!(rendered.contains("artifact=\".phoenix/pr-context/a&amp;b.json\""));
        assert!(rendered.contains("fix &lt;all&gt; &amp; &quot;now&quot;"));
        assert!(rendered.contains("<instruction>legacy</instruction>"));
    }

    #[test]
    fn workflow_id_is_stable_for_same_request() {
        assert_eq!(workflow_id_for("c", "m"), workflow_id_for("c", "m"));
        assert_ne!(workflow_id_for("c", "m"), workflow_id_for("c", "other"));
    }
}
