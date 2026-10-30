#![allow(clippy::missing_errors_doc)]

use super::*;
use phoenix_core::domain::product_conversation::ProductConversationId;
pub use phoenix_core::domain::product_creation::ProductCreationProvisioningStage;
use phoenix_core::llm_language::LlmLanguage;
use sqlx::SqliteConnection;

mod initial_execution;
pub(crate) use initial_execution::record_initial_execution_receipt_tx;
pub use initial_execution::ProductCreationInitialExecution;

const PRODUCT_CREATION_MAX_ATTEMPTS: i64 = 4;
const PRODUCT_CREATION_RETRY_DELAYS_SECONDS: [i64; 3] = [2, 10, 30];
const PRODUCT_CREATION_MAX_DELIVERY_ATTEMPTS: i64 = 4;

fn unix_micros_now() -> i64 {
    Utc::now().timestamp_micros()
}

fn datetime_to_unix_micros(value: DateTime<Utc>) -> i64 {
    value.timestamp_micros()
}

fn unix_micros_to_datetime(value: i64) -> Result<DateTime<Utc>, sqlx::Error> {
    DateTime::<Utc>::from_timestamp_micros(value).ok_or_else(|| {
        sqlx::Error::Decode(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid unix micros timestamp: {value}"),
        )))
    })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProductCreationImage {
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductCreationOrigin {
    UnknownHistorical,
    UserApi,
    SlackGateway {
        request_id: String,
    },
    InternalConversation {
        product_conversation_id: ProductConversationId,
        transcript_id: String,
        source_call: phoenix_core::domain::db_schema::SourceToolCall,
    },
}
#[derive(Clone, Copy)]
pub enum ProductCreationResolution<'a> {
    Existing(&'a ProductCreationJobRecord),
    New { default_llm_language: LlmLanguage },
}

impl ProductCreationOrigin {
    fn as_str(&self) -> &'static str {
        match self {
            Self::UnknownHistorical => "unknown_historical",
            Self::UserApi => "user_api",
            Self::SlackGateway { .. } => "slack_gateway",
            Self::InternalConversation { .. } => "internal_conversation",
        }
    }
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        use phoenix_core::domain::db_schema::InputOrigin;
        let kind: String = row.try_get("objective_origin")?;
        let origin = match kind.as_str() {
            "unknown_historical" => InputOrigin::UnknownHistorical,
            "user_api" => InputOrigin::UserApi,
            "slack_gateway" => InputOrigin::SlackGateway {
                request_id: row.try_get("objective_slack_request_id")?,
            },
            "internal_conversation" => {
                let product_id: Option<String> =
                    row.try_get("objective_source_product_conversation_id")?;
                let transcript_id: Option<String> =
                    row.try_get("objective_source_transcript_id")?;
                let product_id = product_id.ok_or_else(|| {
                    sqlx::Error::Decode("missing internal creation product source".into())
                })?;
                let transcript_id = transcript_id.ok_or_else(|| {
                    sqlx::Error::Decode("missing internal creation transcript source".into())
                })?;
                InputOrigin::InternalConversation {
                    product_conversation_id: ProductConversationId::parse(&product_id)
                        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                    transcript_id,
                    source_call: None,
                }
            }
            value => {
                return Err(sqlx::Error::Decode(
                    format!("invalid creation origin: {value}").into(),
                ));
            }
        };
        let source_product_id: Option<String> =
            row.try_get("objective_source_product_conversation_id")?;
        let source_transcript_id: Option<String> = row.try_get("objective_source_transcript_id")?;
        let source_message_id: Option<String> = row.try_get("objective_source_message_id")?;
        let source_tool_use_id: Option<String> = row.try_get("objective_source_tool_use_id")?;
        let origin = origin
            .with_source_call_columns(source_message_id, source_tool_use_id)
            .map_err(|error| sqlx::Error::Decode(error.into()))?;
        match origin {
            InputOrigin::UnknownHistorical
                if source_product_id.is_none() && source_transcript_id.is_none() =>
            {
                Ok(Self::UnknownHistorical)
            }
            InputOrigin::UserApi
                if source_product_id.is_none() && source_transcript_id.is_none() =>
            {
                Ok(Self::UserApi)
            }
            InputOrigin::SlackGateway { request_id }
                if source_product_id.is_none() && source_transcript_id.is_none() =>
            {
                Ok(Self::SlackGateway { request_id })
            }
            InputOrigin::InternalConversation {
                product_conversation_id,
                transcript_id,
                source_call: Some(source_call),
            } => {
                if source_product_id.as_deref() != Some(product_conversation_id.as_str())
                    || source_transcript_id.as_deref() != Some(transcript_id.as_str())
                {
                    return Err(sqlx::Error::Decode(
                        "invalid internal creation source columns".into(),
                    ));
                }
                Ok(Self::InternalConversation {
                    product_conversation_id,
                    transcript_id,
                    source_call: *source_call,
                })
            }
            InputOrigin::UnknownHistorical
            | InputOrigin::UserApi
            | InputOrigin::InternalConversation { .. }
            | InputOrigin::SystemGenerated
            | InputOrigin::SlackGateway { .. }
            | InputOrigin::SubscriptionEvent { .. } => Err(sqlx::Error::Decode(
                "internal creation origin requires a source-call locator".into(),
            )),
        }
    }
    #[must_use]
    pub fn input_origin(&self) -> phoenix_core::domain::db_schema::InputOrigin {
        use phoenix_core::domain::db_schema::InputOrigin;
        match self {
            Self::UnknownHistorical => InputOrigin::UnknownHistorical,
            Self::UserApi => InputOrigin::UserApi,
            Self::SlackGateway { request_id } => InputOrigin::SlackGateway {
                request_id: request_id.clone(),
            },
            Self::InternalConversation {
                product_conversation_id,
                transcript_id,
                source_call,
            } => InputOrigin::InternalConversation {
                product_conversation_id: product_conversation_id.clone(),
                transcript_id: transcript_id.clone(),
                source_call: Some(Box::new(source_call.clone())),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationIntent<I = Vec<ProductCreationImage>> {
    pub cwd: String,
    pub objective: String,
    pub origin: ProductCreationOrigin,
    pub model: Option<String>,
    pub effort: Option<ModelEffort>,
    pub llm_language: LlmLanguage,
    pub images: I,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ImageReplayIdentity {
    ordinal: i64,
    media_type: String,
    data_sha256: String,
}

pub(crate) fn creation_image_data_sha256(data: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(data.as_bytes()) {
        write!(&mut encoded, "{byte:02X}").expect("writing to String cannot fail");
    }
    encoded
}

impl ImageReplayIdentity {
    fn of(ordinal: usize, image: &ProductCreationImage) -> Self {
        Self {
            ordinal: i64::try_from(ordinal).expect("bounded creation image count"),
            media_type: image.media_type.clone(),
            data_sha256: creation_image_data_sha256(&image.data),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredProductCreationImages {
    Live(Vec<ProductCreationImage>),
    Retired(Vec<ImageReplayIdentity>),
}

impl ProductCreationIntent<StoredProductCreationImages> {
    fn matches_retry(&self, retry: &ProductCreationIntent) -> bool {
        self.origin
            .input_origin()
            .accepts_retry_origin(&retry.origin.input_origin())
            && self.cwd == retry.cwd
            && self.objective == retry.objective
            && self.model == retry.model
            && self.effort == retry.effort
            && self.llm_language == retry.llm_language
            && match &self.images {
                StoredProductCreationImages::Live(images) => images == &retry.images,
                StoredProductCreationImages::Retired(identity) => {
                    identity
                        == &retry
                            .images
                            .iter()
                            .enumerate()
                            .map(|(ordinal, image)| ImageReplayIdentity::of(ordinal, image))
                            .collect::<Vec<_>>()
                }
            }
    }

    pub fn live_intent(&self) -> Result<ProductCreationIntent, &'static str> {
        let StoredProductCreationImages::Live(images) = &self.images else {
            return Err("creation image payload has been retired");
        };
        Ok(ProductCreationIntent {
            cwd: self.cwd.clone(),
            objective: self.objective.clone(),
            origin: self.origin.clone(),
            model: self.model.clone(),
            effort: self.effort,
            llm_language: self.llm_language,
            images: images.clone(),
        })
    }
}

impl PartialEq<ProductCreationIntent> for ProductCreationIntent<StoredProductCreationImages> {
    fn eq(&self, other: &ProductCreationIntent) -> bool {
        self.origin == other.origin && self.matches_retry(other)
    }
}

fn provisioning_stage_from_row(
    row: &SqliteRow,
) -> Result<ProductCreationProvisioningStage, sqlx::Error> {
    use ProductCreationProvisioningStage as Stage;

    match row.try_get::<String, _>("provisioning_stage")?.as_str() {
        "validate_intent" => Ok(Stage::ValidateIntent),
        "resolve_repository" => Ok(Stage::ResolveRepository),
        "resolve_starting_pin" => Ok(Stage::ResolveStartingPin),
        "materialize_worktree" => Ok(Stage::MaterializeWorktree),
        "finalize_attachments" => Ok(Stage::FinalizeAttachments),
        "commit_metadata" => Ok(Stage::CommitMetadata),
        "bootstrap_initial_turn" => Ok(Stage::BootstrapInitialTurn),
        "finalize_publication" => Ok(Stage::FinalizePublication),
        value => Err(sqlx::Error::Decode(
            format!("invalid product creation stage: {value}").into(),
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationJobRecord {
    pub request_id: String,
    pub product_conversation_id: ProductConversationId,
    pub intent: ProductCreationIntent<StoredProductCreationImages>,
    pub status: String,
    pub provisioning_stage: ProductCreationProvisioningStage,
    pub stage_updated_at: DateTime<Utc>,
    pub accepted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub attempt_count: i64,
    pub claim_generation: i64,
    pub claim_worker_id: Option<String>,
    pub claim_token: Option<String>,
    pub claim_lease_until: Option<DateTime<Utc>>,
    pub retry_at: Option<DateTime<Utc>>,
    pub delivery_attempt_count: i64,
    pub delivery_retry_at: Option<DateTime<Utc>>,
    pub pin_exact_checkout_oid: Option<String>,
    pub pin_logical_base: Option<String>,
    pub pin_freshness: Option<String>,
    pub staging_path: Option<String>,
    pub staging_repo_root: Option<String>,
    pub staging_exact_oid: Option<String>,
    pub published_product_id: Option<ProductConversationId>,
    pub published_conversation_id: Option<String>,
    pub last_error: Option<String>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub deletion_requested_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
pub enum ProductCreationPublicationRead {
    Available {
        conversation_id: String,
        aggregate: Box<ProductConversationAggregate>,
    },
    Historical {
        conversation_id: String,
    },
}

#[derive(Debug)]
pub struct PublicProductCreationStatusRead {
    pub job: ProductCreationJobRecord,
    pub publication: Option<ProductCreationPublicationRead>,
}

#[derive(Debug)]
pub struct ProductCreationStatusRead {
    pub request_id: String,
    pub product_conversation_id: ProductConversationId,
    pub status: String,
    pub provisioning_stage: ProductCreationProvisioningStage,
    pub stage_updated_at: DateTime<Utc>,
    pub retry_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub publication: Option<ProductCreationPublicationRead>,
    pub initial_objective_execution: ProductCreationInitialExecution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationRecoveryJobRecord {
    pub request_id: String,
    pub status: String,
    pub delivery_retryable: bool,
    pub intent: ProductCreationIntent,
    pub published_product_conversation_id: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductCreationAcceptOutcome {
    Accepted(ProductCreationJobRecord),
    Replayed(ProductCreationJobRecord),
    Conflict(ProductCreationJobRecord),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductCreationPinOutcome {
    Pinned(ProductCreationJobRecord),
    Same(ProductCreationJobRecord),
    Conflict(ProductCreationJobRecord),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationClaim {
    pub worker_id: String,
    pub token: String,
    pub generation: i64,
    pub lease_until: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedProductCreationJob {
    pub job: ProductCreationJobRecord,
    pub claim: ProductCreationClaim,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationCleanupClaim {
    pub worker_id: String,
    pub token: String,
    pub generation: i64,
    pub lease_until: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationResourceReservation {
    pub id: String,
    pub request_id: String,
    pub generation: i64,
    pub repository_identity: String,
    pub resource_identity: String,
    pub status: String,
    pub ownership_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductCreationResourceCleanupDisposition {
    Released,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationCleanupJob {
    pub job: ProductCreationJobRecord,
    pub claim: ProductCreationCleanupClaim,
    pub reservations: Vec<ProductCreationResourceReservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationGitPublicationFacts {
    pub exact_checkout_oid: String,
    pub repository_root: String,
}

#[derive(Debug, Clone)]
pub struct ProductCreationPublishInput {
    pub request_id: String,
    pub claim: ProductCreationClaim,
    pub conversation: Conversation,
    pub authority_kind: AuthorityKind,
    pub environment: EnvironmentContext,
    pub git_publication: Option<ProductCreationGitPublicationFacts>,
}

async fn require_creation_origin_tx(
    conn: &mut SqliteConnection,
    current_gc_transcript_id: Option<&str>,
    origin: &ProductCreationOrigin,
) -> DbResult<()> {
    match (current_gc_transcript_id, origin) {
        (
            Some(caller),
            ProductCreationOrigin::InternalConversation {
                product_conversation_id,
                transcript_id,
                source_call,
            },
        ) if caller == transcript_id
            && !source_call.message_id.is_empty()
            && !source_call.tool_use_id.is_empty() =>
        {
            let owner: Option<String> = sqlx::query_scalar(
                "SELECT product_conversation_id FROM conversations WHERE id = ?1",
            )
            .bind(caller)
            .fetch_optional(conn)
            .await?;
            if owner.as_deref() == Some(product_conversation_id.as_str()) {
                Ok(())
            } else {
                Err(DbError::CurrentCoordinatorRequired)
            }
        }
        (
            None,
            ProductCreationOrigin::UserApi
            | ProductCreationOrigin::UnknownHistorical
            | ProductCreationOrigin::SlackGateway { .. },
        ) => Ok(()),
        _ => Err(DbError::CurrentCoordinatorRequired),
    }
}

fn normalize_product_creation_intent(intent: &ProductCreationIntent) -> DbResult<()> {
    if intent.cwd.trim().is_empty() {
        return Err(DbError::Serialization(
            "product creation cwd must not be empty".to_string(),
        ));
    }
    if intent.objective.trim().is_empty() && intent.images.is_empty() {
        return Err(DbError::Serialization(
            "product creation objective must not be empty unless images are provided".to_string(),
        ));
    }
    if intent
        .images
        .iter()
        .any(|image| image.media_type.trim().is_empty() || image.data.trim().is_empty())
    {
        return Err(DbError::Serialization(
            "product creation images require a media type and non-empty payload".to_string(),
        ));
    }
    Ok(())
}

fn next_product_creation_retry_at(attempt_count: i64, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    PRODUCT_CREATION_RETRY_DELAYS_SECONDS
        .get(usize::try_from(attempt_count.saturating_sub(1)).ok()?)
        .map(|seconds| now + chrono::Duration::seconds(*seconds))
}

fn parse_product_creation_resource_reservation_row(
    row: &SqliteRow,
) -> Result<ProductCreationResourceReservation, sqlx::Error> {
    Ok(ProductCreationResourceReservation {
        id: row.try_get("id")?,
        request_id: row.try_get("request_id")?,
        generation: row.try_get("generation")?,
        repository_identity: row.try_get("repository_identity")?,
        resource_identity: row.try_get("resource_identity")?,
        status: row.try_get("status")?,
        ownership_token: row.try_get("ownership_token")?,
    })
}

fn decode_stored_creation_images(
    row: &SqliteRow,
    images_json: &str,
) -> Result<StoredProductCreationImages, sqlx::Error> {
    #[derive(serde::Deserialize)]
    struct PayloadRow {
        ordinal: i64,
        media_type: String,
        data: String,
    }
    let payload: Vec<PayloadRow> =
        serde_json::from_str(images_json).map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    let identity_json: String = row.try_get("image_identity_json")?;
    let identity: Vec<ImageReplayIdentity> = serde_json::from_str(&identity_json)
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    let retired: bool = row.try_get("images_retired")?;
    if identity.len() > 20
        || identity.iter().enumerate().any(|(n, image)| {
            i64::try_from(n) != Ok(image.ordinal) || image.data_sha256.len() != 64
        })
    {
        return Err(sqlx::Error::Decode(
            "creation image identity has missing ordinal".into(),
        ));
    }
    if retired {
        if !payload.is_empty() {
            return Err(sqlx::Error::Decode(
                "retired creation retains image payload".into(),
            ));
        }
        return Ok(StoredProductCreationImages::Retired(identity));
    }
    if payload.len() != identity.len() {
        return Err(sqlx::Error::Decode(
            "live creation image payload incomplete".into(),
        ));
    }
    let mut images = Vec::with_capacity(payload.len());
    for (n, (image, fingerprint)) in payload.into_iter().zip(&identity).enumerate() {
        let ordinal = i64::try_from(n).map_err(|_| {
            sqlx::Error::Decode("creation image ordinal exceeds SQLite integer range".into())
        })?;
        if image.ordinal != ordinal || image.media_type != fingerprint.media_type {
            return Err(sqlx::Error::Decode(
                "creation image order or media type mismatch".into(),
            ));
        }
        let live = ProductCreationImage {
            media_type: image.media_type,
            data: image.data,
        };
        if ImageReplayIdentity::of(n, &live).data_sha256 != fingerprint.data_sha256 {
            return Err(sqlx::Error::Decode(
                "creation image payload digest mismatch".into(),
            ));
        }
        images.push(live);
    }
    Ok(StoredProductCreationImages::Live(images))
}

#[allow(clippy::needless_pass_by_value)]
fn parse_product_creation_job_row(row: SqliteRow) -> Result<ProductCreationJobRecord, sqlx::Error> {
    let request_id: String = row.try_get("request_id")?;
    let images_json: String = row.try_get("images_json")?;
    let images = decode_stored_creation_images(&row, &images_json)?;
    Ok(ProductCreationJobRecord {
        request_id,
        product_conversation_id: ProductConversationId::parse(
            &row.try_get::<String, _>("product_conversation_id")?,
        )
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
        intent: ProductCreationIntent {
            cwd: row.try_get("cwd")?,
            objective: row.try_get("objective")?,
            origin: ProductCreationOrigin::from_row(&row)?,
            model: row.try_get("model")?,
            effort: row
                .try_get::<Option<String>, _>("effort")?
                .map(|value| {
                    ModelEffort::from_str(&value).map_err(|error| sqlx::Error::Decode(error.into()))
                })
                .transpose()?,
            llm_language: LlmLanguage::parse_or_default(&row.try_get::<String, _>("llm_language")?),
            images,
        },
        status: row.try_get("status")?,
        provisioning_stage: provisioning_stage_from_row(&row)?,
        stage_updated_at: unix_micros_to_datetime(row.try_get("stage_updated_at_unix_micros")?)?,
        accepted_at: unix_micros_to_datetime(row.try_get("accepted_at_unix_micros")?)?,
        updated_at: unix_micros_to_datetime(row.try_get("updated_at_unix_micros")?)?,
        attempt_count: row.try_get("attempt_count")?,
        claim_generation: row.try_get("claim_generation")?,
        claim_worker_id: row.try_get("claim_worker_id")?,
        claim_token: row.try_get("claim_token")?,
        claim_lease_until: row
            .try_get::<Option<i64>, _>("claim_lease_until_unix_micros")?
            .map(unix_micros_to_datetime)
            .transpose()?,
        retry_at: row
            .try_get::<Option<i64>, _>("retry_at_unix_micros")?
            .map(unix_micros_to_datetime)
            .transpose()?,
        delivery_attempt_count: row.try_get("delivery_attempt_count")?,
        delivery_retry_at: row
            .try_get::<Option<i64>, _>("delivery_retry_at_unix_micros")?
            .map(unix_micros_to_datetime)
            .transpose()?,
        pin_exact_checkout_oid: row.try_get("pin_exact_checkout_oid")?,
        pin_logical_base: row.try_get("pin_logical_base")?,
        pin_freshness: row.try_get("pin_freshness")?,
        staging_path: row.try_get("staging_path")?,
        staging_repo_root: row.try_get("staging_repo_root")?,
        staging_exact_oid: row.try_get("staging_exact_oid")?,
        published_product_id: row
            .try_get::<Option<String>, _>("published_product_id")?
            .map(ProductConversationId::parse)
            .transpose()
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
        published_conversation_id: row.try_get("published_conversation_id")?,
        last_error: row.try_get("last_error")?,
        cancelled_at: row
            .try_get::<Option<i64>, _>("cancelled_at_unix_micros")?
            .map(unix_micros_to_datetime)
            .transpose()?,
        deletion_requested_at: row
            .try_get::<Option<i64>, _>("deletion_requested_at_unix_micros")?
            .map(unix_micros_to_datetime)
            .transpose()?,
    })
}

impl Database {
    pub async fn product_creation_objective_already_durably_accepted(
        &self,
        request_id: &str,
        steering_fingerprint: Option<&SteeringAcceptanceFingerprint>,
        intent: &ProductCreationIntent,
    ) -> DbResult<bool> {
        normalize_product_creation_intent(intent)?;
        let Some(record) = self.get_product_creation_job(request_id).await? else {
            return Ok(false);
        };
        let expected_fingerprint = format!("product-create:{request_id}");
        Ok(match steering_fingerprint {
            Some(SteeringAcceptanceFingerprint::LegacyUnknown) => false,
            Some(SteeringAcceptanceFingerprint::Exact(exact)) => {
                exact == &expected_fingerprint && record.intent == *intent
            }
            None => record.intent == *intent,
        })
    }

    pub async fn get_product_creation_job(
        &self,
        request_id: &str,
    ) -> DbResult<Option<ProductCreationJobRecord>> {
        let mut connection = self.pool.acquire().await?;
        Self::get_product_creation_job_on(&mut connection, request_id).await
    }

    async fn get_product_creation_job_on(
        connection: &mut sqlx::SqliteConnection,
        request_id: &str,
    ) -> DbResult<Option<ProductCreationJobRecord>> {
        sqlx::query(
            "SELECT j.request_id, j.product_conversation_id, j.cwd, j.objective, j.objective_origin, j.objective_slack_request_id, j.objective_source_product_conversation_id, j.objective_source_transcript_id, j.objective_source_message_id, j.objective_source_tool_use_id, j.model, j.effort, j.llm_language, j.status,
                    j.provisioning_stage, j.stage_updated_at_unix_micros,
                    j.accepted_at_unix_micros, j.updated_at_unix_micros, j.attempt_count,
                    j.claim_generation, j.claim_worker_id, j.claim_token,
                    j.claim_lease_until_unix_micros, j.retry_at_unix_micros,
                    j.delivery_attempt_count, j.delivery_retry_at_unix_micros,
                    j.pin_exact_checkout_oid, j.pin_logical_base, j.pin_freshness,
                    j.staging_path, j.staging_repo_root, j.staging_exact_oid,
                    j.published_product_id, j.published_conversation_id, j.last_error,
                    j.cancelled_at_unix_micros, j.deletion_requested_at_unix_micros,
                    COALESCE((
                        SELECT json_group_array(json_object('ordinal', i.ordinal, 'media_type', i.media_type, 'data', i.data))
                        FROM (
                            SELECT ordinal, media_type, data
                            FROM product_creation_job_images
                            WHERE request_id = j.request_id
                            ORDER BY ordinal
                        ) i
                    ), '[]') AS images_json,
                    (SELECT COALESCE(json_group_array(json_object('ordinal', f.ordinal, 'media_type', f.media_type, 'data_sha256', f.data_sha256)), '[]') FROM
                        (SELECT ordinal, media_type, data_sha256 FROM product_creation_image_replay_identity WHERE request_id = j.request_id ORDER BY ordinal) f) AS image_identity_json,
                    EXISTS(SELECT 1 FROM product_creation_retired_images WHERE request_id = j.request_id) AS images_retired
             FROM product_creation_jobs j
             WHERE j.request_id = ?1",
        )
        .bind(request_id)
        .try_map(parse_product_creation_job_row)
        .fetch_optional(connection)
        .await
        .map_err(DbError::Sqlx)
    }

    pub async fn immutable_starting_oid_for_conversation(
        &self,
        conversation_id: &str,
    ) -> DbResult<Option<String>> {
        sqlx::query_scalar(
            "SELECT COALESCE(provisioning.exact_checkout_oid, direct.staging_exact_oid)
             FROM conversations conversation
             LEFT JOIN conversation_creation_jobs provisioning
               ON provisioning.conversation_id = conversation.id
             LEFT JOIN product_creation_jobs direct
               ON direct.published_product_id = conversation.product_conversation_id
             WHERE conversation.id = ?1
             LIMIT 1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::Sqlx)
    }

    pub async fn product_creation_project_options_as_coordinator(
        &self,
        transcript_id: &str,
        after_id: Option<&str>,
    ) -> DbResult<Vec<(String, String)>> {
        let mut tx = self.pool.begin().await?;
        crate::product_conversation_rename::require_current_gc_tx(&mut tx, transcript_id).await?;
        let projects = sqlx::query_as::<_, (String, String)>(
            "SELECT id, canonical_path FROM projects WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT 64",
        )
        .bind(after_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(projects)
    }

    pub async fn get_product_creation_job_as_coordinator(
        &self,
        transcript_id: &str,
        request_id: &str,
    ) -> DbResult<Option<ProductCreationJobRecord>> {
        let mut tx = self.pool.begin().await?;
        crate::product_conversation_rename::require_current_gc_tx(&mut tx, transcript_id).await?;
        let job = Self::get_product_creation_job_on(&mut tx, request_id).await?;
        tx.commit().await?;
        Ok(job)
    }

    /// Read native creation reconciliation evidence and current Coordinator authority together.
    /// # Errors
    /// Returns an error if persisted Coordinator authority or snapshot decoding fails.
    pub async fn read_product_creation_status_as_coordinator(
        &self,
        transcript_id: &str,
        request_id: &str,
    ) -> DbResult<Option<ProductCreationStatusRead>> {
        self.read_product_creation_status_as_coordinator_inner(
            transcript_id,
            request_id,
            #[cfg(test)]
            None,
        )
        .await
    }

    pub async fn read_public_product_creation_status(
        &self,
        request_id: &str,
    ) -> DbResult<Option<PublicProductCreationStatusRead>> {
        let mut tx = self.pool.begin().await?;
        let Some(job) = Self::get_product_creation_job_on(&mut tx, request_id).await? else {
            tx.rollback().await?;
            return Ok(None);
        };
        let publication = Self::product_creation_publication_on(&mut tx, &job).await?;
        tx.rollback().await?;
        Ok(Some(PublicProductCreationStatusRead { job, publication }))
    }

    async fn product_creation_publication_on(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        job: &ProductCreationJobRecord,
    ) -> DbResult<Option<ProductCreationPublicationRead>> {
        let Some(conversation_id) = &job.published_conversation_id else {
            return Ok(None);
        };
        let transcript_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)")
                .bind(conversation_id)
                .fetch_one(&mut **tx)
                .await?;
        match (transcript_exists, job.status.as_str()) {
            (true, "published") => {
                match Self::get_ordinary_product_conversation_on(tx, &job.product_conversation_id)
                    .await
                {
                    Ok(aggregate) => Ok(Some(ProductCreationPublicationRead::Available {
                        conversation_id: conversation_id.clone(),
                        aggregate: Box::new(aggregate),
                    })),
                    Err(DbError::ConversationNotFound(_)) => {
                        Ok(Some(ProductCreationPublicationRead::Historical {
                            conversation_id: conversation_id.clone(),
                        }))
                    }
                    Err(error) => Err(error),
                }
            }
            (true, _) => Ok(None),
            (false, _) => Ok(Some(ProductCreationPublicationRead::Historical {
                conversation_id: conversation_id.clone(),
            })),
        }
    }

    async fn read_product_creation_status_as_coordinator_inner(
        &self,
        transcript_id: &str,
        request_id: &str,
        #[cfg(test)] after_job: Option<
            std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>,
        >,
    ) -> DbResult<Option<ProductCreationStatusRead>> {
        let mut tx = self.pool.begin().await?;
        crate::product_conversation_rename::require_current_gc_tx(&mut tx, transcript_id).await?;
        let Some(job) = Self::get_product_creation_job_on(&mut tx, request_id).await? else {
            tx.rollback().await?;
            return Ok(None);
        };
        #[cfg(test)]
        if let Some(after_job) = after_job {
            after_job.await;
        }
        let initial_objective_execution =
            Self::product_creation_initial_execution_on(&mut tx, request_id).await?;
        let publication = Self::product_creation_publication_on(&mut tx, &job).await?;
        let status = ProductCreationStatusRead {
            request_id: job.request_id,
            product_conversation_id: job.product_conversation_id,
            status: job.status,
            provisioning_stage: job.provisioning_stage,
            stage_updated_at: job.stage_updated_at,
            retry_at: job.retry_at.or(job.delivery_retry_at),
            error: job.last_error,
            publication,
            initial_objective_execution,
        };
        tx.rollback().await?;
        Ok(Some(status))
    }

    #[allow(clippy::too_many_lines)] // One transaction reserves identity, source binding, and objective.
    pub async fn accept_product_creation(
        &self,
        request_id: &str,
        intent: &ProductCreationIntent,
    ) -> DbResult<ProductCreationAcceptOutcome> {
        normalize_product_creation_intent(intent)?;
        self.accept_product_creation_resolved(request_id, None, |_| Ok(intent.clone()))
            .await
    }

    /// Serializes caller authorization, durable winner selection, and new-intent validation.
    /// The resolver sees the existing accepted job before it is asked to prepare a candidate.
    #[allow(clippy::too_many_lines)]
    pub async fn accept_product_creation_resolved<F>(
        &self,
        request_id: &str,
        current_gc_transcript_id: Option<&str>,
        resolve: F,
    ) -> DbResult<ProductCreationAcceptOutcome>
    where
        F: FnOnce(ProductCreationResolution<'_>) -> DbResult<ProductCreationIntent>,
    {
        let accepted_product_conversation_id = ProductConversationId::new();
        let now = unix_micros_now();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(transcript_id) = current_gc_transcript_id {
            super::product_conversation_rename::require_current_gc_tx(&mut tx, transcript_id)
                .await?;
        }
        let existing = sqlx::query(
            "SELECT request_id, product_conversation_id, cwd, objective, objective_origin, objective_slack_request_id, objective_source_product_conversation_id, objective_source_transcript_id, objective_source_message_id, objective_source_tool_use_id, model, effort, llm_language, status,
                    provisioning_stage, stage_updated_at_unix_micros,
                    accepted_at_unix_micros, updated_at_unix_micros, attempt_count,
                    claim_generation, claim_worker_id, claim_token,
                    claim_lease_until_unix_micros, retry_at_unix_micros,
                    delivery_attempt_count, delivery_retry_at_unix_micros,
                    pin_exact_checkout_oid, pin_logical_base, pin_freshness,
                    staging_path, staging_repo_root, staging_exact_oid,
                    published_product_id, published_conversation_id, last_error,
                    cancelled_at_unix_micros, deletion_requested_at_unix_micros,
                    COALESCE((
                        SELECT json_group_array(json_object('ordinal', i.ordinal, 'media_type', i.media_type, 'data', i.data))
                        FROM (
                            SELECT ordinal, media_type, data
                            FROM product_creation_job_images
                            WHERE request_id = j.request_id
                            ORDER BY ordinal
                        ) i
                    ), '[]') AS images_json,
                    (SELECT COALESCE(json_group_array(json_object('ordinal', f.ordinal, 'media_type', f.media_type, 'data_sha256', f.data_sha256)), '[]') FROM
                        (SELECT ordinal, media_type, data_sha256 FROM product_creation_image_replay_identity WHERE request_id = j.request_id ORDER BY ordinal) f) AS image_identity_json,
                    EXISTS(SELECT 1 FROM product_creation_retired_images WHERE request_id = j.request_id) AS images_retired
             FROM product_creation_jobs j
             WHERE j.request_id = ?1",
        )
        .bind(request_id)
        .try_map(parse_product_creation_job_row)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(existing) = existing {
            let candidate = resolve(ProductCreationResolution::Existing(&existing))?;
            require_creation_origin_tx(&mut tx, current_gc_transcript_id, &candidate.origin)
                .await?;
            tx.rollback().await?;
            return Ok(if existing.intent.matches_retry(&candidate) {
                ProductCreationAcceptOutcome::Replayed(existing)
            } else {
                ProductCreationAcceptOutcome::Conflict(existing)
            });
        }
        let language: Option<String> =
            sqlx::query_scalar("SELECT value FROM app_settings WHERE key = 'default_llm_language'")
                .fetch_optional(&mut *tx)
                .await?;
        let intent = resolve(ProductCreationResolution::New {
            default_llm_language: language
                .as_deref()
                .map(LlmLanguage::parse_or_default)
                .unwrap_or_default(),
        })?;
        require_creation_origin_tx(&mut tx, current_gc_transcript_id, &intent.origin).await?;
        normalize_product_creation_intent(&intent)?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO product_creation_jobs (
                request_id, product_conversation_id, cwd, objective, objective_origin, objective_slack_request_id,
                objective_source_product_conversation_id, objective_source_transcript_id,
                objective_source_message_id, objective_source_tool_use_id,
                model, effort, llm_language, status,
                accepted_at_unix_micros, updated_at_unix_micros, stage_updated_at_unix_micros
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?14, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'accepted', ?13, ?13, ?13)",
        )
        .bind(request_id)
        .bind(accepted_product_conversation_id.as_str())
        .bind(&intent.cwd)
        .bind(&intent.objective)
        .bind(intent.origin.as_str())
        .bind(match &intent.origin {
            ProductCreationOrigin::InternalConversation { product_conversation_id, .. } => Some(product_conversation_id.as_str()),
            ProductCreationOrigin::UnknownHistorical | ProductCreationOrigin::UserApi | ProductCreationOrigin::SlackGateway { .. } => None,
        })
        .bind(match &intent.origin {
            ProductCreationOrigin::InternalConversation { transcript_id, .. } => Some(transcript_id.as_str()),
            ProductCreationOrigin::UnknownHistorical | ProductCreationOrigin::UserApi | ProductCreationOrigin::SlackGateway { .. } => None,
        })
        .bind(match &intent.origin {
            ProductCreationOrigin::InternalConversation { source_call, .. } => Some(source_call.message_id.as_str()),
            ProductCreationOrigin::UnknownHistorical | ProductCreationOrigin::UserApi | ProductCreationOrigin::SlackGateway { .. } => None,
        })
        .bind(match &intent.origin {
            ProductCreationOrigin::InternalConversation { source_call, .. } => Some(source_call.tool_use_id.as_str()),
            ProductCreationOrigin::UnknownHistorical | ProductCreationOrigin::UserApi | ProductCreationOrigin::SlackGateway { .. } => None,
        })
        .bind(&intent.model)
        .bind(intent.effort.map(ModelEffort::as_wire_name))
        .bind(intent.llm_language.as_str())
        .bind(now)
        .bind(intent.origin.input_origin().slack_request_id())
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() == 0 {
            tx.rollback().await?;
            let existing = self
                .get_product_creation_job(request_id)
                .await?
                .ok_or_else(|| {
                    DbError::Serialization("raced product creation disappeared".to_string())
                })?;
            return Ok(if existing.intent.matches_retry(&intent) {
                ProductCreationAcceptOutcome::Replayed(existing)
            } else {
                ProductCreationAcceptOutcome::Conflict(existing)
            });
        }
        if let ProductCreationOrigin::SlackGateway {
            request_id: source_id,
        } = &intent.origin
        {
            let changed = sqlx::query("UPDATE slack_gateway_bindings SET product_id=?2 WHERE creation_id=?1 AND binding_id=(SELECT binding_id FROM slack_gateway_requests WHERE request_id=?3) AND product_id IS NULL")
                .bind(request_id).bind(accepted_product_conversation_id.as_str()).bind(source_id).execute(&mut *tx).await?;
            if changed.rows_affected() != 1 {
                return Err(DbError::Serialization(
                    "Slack creation is not bound to its accepted source".into(),
                ));
            }
            sqlx::query("UPDATE slack_gateway_requests SET input_id=?2 WHERE request_id=?1 AND admission_state='accepted'").bind(source_id).bind(request_id).execute(&mut *tx).await?;
        }
        for (ordinal, image) in intent.images.iter().enumerate() {
            let ordinal_sql = i64::try_from(ordinal)
                .map_err(|_| DbError::Serialization("too many images".to_string()))?;
            sqlx::query(
                "INSERT INTO product_creation_job_images (request_id, ordinal, media_type, data)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(request_id)
            .bind(ordinal_sql)
            .bind(&image.media_type)
            .bind(&image.data)
            .execute(&mut *tx)
            .await?;
            sqlx::query("INSERT INTO product_creation_image_replay_identity (request_id, ordinal, media_type, data_sha256) VALUES (?1, ?2, ?3, ?4)")
                .bind(request_id).bind(ordinal_sql)
                .bind(&image.media_type).bind(ImageReplayIdentity::of(ordinal, image).data_sha256)
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        let record = self
            .get_product_creation_job(request_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("accepted product creation job missing".to_string())
            })?;
        Ok(ProductCreationAcceptOutcome::Accepted(record))
    }

    pub async fn claim_product_creation(
        &self,
        request_id: &str,
        worker_id: &str,
        token: &str,
        now: DateTime<Utc>,
        lease_duration: chrono::Duration,
    ) -> DbResult<Option<ClaimedProductCreationJob>> {
        let now_str = datetime_to_unix_micros(now);
        let lease_until = datetime_to_unix_micros(now + lease_duration);
        let job = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'claimed', claim_generation = claim_generation + 1,
                 claim_worker_id = ?2, claim_token = ?3, claim_lease_until_unix_micros = ?4,
                 retry_at_unix_micros = NULL, updated_at_unix_micros = ?1
             WHERE request_id = ?5 AND (
                 status = 'accepted'
                 OR (status = 'retry_scheduled' AND retry_at_unix_micros <= ?1)
                 OR (status = 'claimed' AND claim_lease_until_unix_micros <= ?1)
             ) AND (published_conversation_id IS NULL OR EXISTS (
                 SELECT 1 FROM conversations c WHERE c.id = product_creation_jobs.published_conversation_id
             )) RETURNING request_id, product_conversation_id, cwd, objective, objective_origin, objective_slack_request_id, objective_source_product_conversation_id, objective_source_transcript_id, objective_source_message_id, objective_source_tool_use_id, model, effort,
                 llm_language, status, provisioning_stage, stage_updated_at_unix_micros,
                 accepted_at_unix_micros, updated_at_unix_micros,
                 attempt_count, claim_generation, claim_worker_id, claim_token,
                 claim_lease_until_unix_micros, retry_at_unix_micros, delivery_attempt_count,
                 delivery_retry_at_unix_micros, pin_exact_checkout_oid, pin_logical_base,
                 pin_freshness, staging_path, staging_repo_root, staging_exact_oid,
                 published_product_id, published_conversation_id, last_error,
                 cancelled_at_unix_micros, deletion_requested_at_unix_micros,
                 COALESCE((
                     SELECT json_group_array(json_object('ordinal', image.ordinal, 'media_type', image.media_type, 'data', image.data))
                     FROM (
                         SELECT ordinal, media_type, data FROM product_creation_job_images
                         WHERE product_creation_job_images.request_id = product_creation_jobs.request_id
                         ORDER BY ordinal
                     ) image
                 ), '[]') AS images_json,
                 (SELECT COALESCE(json_group_array(json_object('ordinal', f.ordinal, 'media_type', f.media_type, 'data_sha256', f.data_sha256)), '[]') FROM
                     (SELECT ordinal, media_type, data_sha256 FROM product_creation_image_replay_identity WHERE request_id = product_creation_jobs.request_id ORDER BY ordinal) f) AS image_identity_json,
                 EXISTS(SELECT 1 FROM product_creation_retired_images WHERE request_id = product_creation_jobs.request_id) AS images_retired",
        )
        .bind(now_str)
        .bind(worker_id)
        .bind(token)
        .bind(lease_until)
        .bind(request_id)
        .try_map(parse_product_creation_job_row)
        .fetch_optional(&self.pool)
        .await?;
        let Some(job) = job else {
            return Ok(None);
        };
        let generation = job.claim_generation;
        Ok(Some(ClaimedProductCreationJob {
            claim: ProductCreationClaim {
                worker_id: worker_id.to_string(),
                token: token.to_string(),
                generation,
                lease_until: unix_micros_to_datetime(lease_until)?,
            },
            job,
        }))
    }

    pub async fn claim_next_product_creation(
        &self,
        worker_id: &str,
        token: &str,
        now: DateTime<Utc>,
        lease_duration: chrono::Duration,
    ) -> DbResult<Option<ClaimedProductCreationJob>> {
        let now_str = datetime_to_unix_micros(now);
        let lease_until = datetime_to_unix_micros(now + lease_duration);
        let job = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'claimed',
                 claim_generation = claim_generation + 1,
                 claim_worker_id = ?1,
                 claim_token = ?2,
                 claim_lease_until_unix_micros = ?3,
                 retry_at_unix_micros = NULL,
                 updated_at_unix_micros = ?4
             WHERE request_id = (
                 SELECT request_id FROM product_creation_jobs
                 WHERE (status = 'accepted'
                    OR (status = 'retry_scheduled' AND retry_at_unix_micros <= ?4)
                    OR (status = 'claimed' AND claim_lease_until_unix_micros <= ?4))
                   AND (published_conversation_id IS NULL OR EXISTS (
                     SELECT 1 FROM conversations c WHERE c.id = product_creation_jobs.published_conversation_id
                   ))
                 ORDER BY CASE status WHEN 'accepted' THEN accepted_at_unix_micros ELSE retry_at_unix_micros END ASC,
                          accepted_at_unix_micros ASC, request_id ASC
                 LIMIT 1
             )
             RETURNING request_id, product_conversation_id, cwd, objective, objective_origin, objective_slack_request_id, objective_source_product_conversation_id, objective_source_transcript_id, objective_source_message_id, objective_source_tool_use_id, model, effort,
                 llm_language, status, provisioning_stage, stage_updated_at_unix_micros,
                 accepted_at_unix_micros, updated_at_unix_micros,
                 attempt_count, claim_generation, claim_worker_id, claim_token,
                 claim_lease_until_unix_micros, retry_at_unix_micros, delivery_attempt_count,
                 delivery_retry_at_unix_micros, pin_exact_checkout_oid, pin_logical_base,
                 pin_freshness, staging_path, staging_repo_root, staging_exact_oid,
                 published_product_id, published_conversation_id, last_error,
                 cancelled_at_unix_micros, deletion_requested_at_unix_micros,
                 COALESCE((
                     SELECT json_group_array(json_object('ordinal', image.ordinal, 'media_type', image.media_type, 'data', image.data))
                     FROM (
                         SELECT ordinal, media_type, data FROM product_creation_job_images
                         WHERE product_creation_job_images.request_id = product_creation_jobs.request_id
                         ORDER BY ordinal
                     ) image
                 ), '[]') AS images_json,
                 (SELECT COALESCE(json_group_array(json_object('ordinal', f.ordinal, 'media_type', f.media_type, 'data_sha256', f.data_sha256)), '[]') FROM
                     (SELECT ordinal, media_type, data_sha256 FROM product_creation_image_replay_identity WHERE request_id = product_creation_jobs.request_id ORDER BY ordinal) f) AS image_identity_json,
                 EXISTS(SELECT 1 FROM product_creation_retired_images WHERE request_id = product_creation_jobs.request_id) AS images_retired",
        )
        .bind(worker_id)
        .bind(token)
        .bind(lease_until)
        .bind(now_str)
        .try_map(parse_product_creation_job_row)
        .fetch_optional(&self.pool)
        .await?;
        let Some(job) = job else {
            return Ok(None);
        };
        Ok(Some(ClaimedProductCreationJob {
            claim: ProductCreationClaim {
                worker_id: worker_id.to_string(),
                token: token.to_string(),
                generation: job.claim_generation,
                lease_until: unix_micros_to_datetime(lease_until)?,
            },
            job,
        }))
    }

    pub async fn pin_product_creation_once(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        exact_checkout_oid: &str,
        logical_base: &str,
        freshness: &str,
    ) -> DbResult<ProductCreationPinOutcome> {
        let existing = self
            .get_product_creation_job(request_id)
            .await?
            .ok_or_else(|| DbError::Sqlx(sqlx::Error::RowNotFound))?;
        if existing.pin_exact_checkout_oid.is_some()
            || existing.pin_logical_base.is_some()
            || existing.pin_freshness.is_some()
        {
            if existing.pin_exact_checkout_oid.as_deref() == Some(exact_checkout_oid)
                && existing.pin_logical_base.as_deref() == Some(logical_base)
                && existing.pin_freshness.as_deref() == Some(freshness)
            {
                return Ok(ProductCreationPinOutcome::Same(existing));
            }
            return Ok(ProductCreationPinOutcome::Conflict(existing));
        }
        let now = unix_micros_now();
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET pin_exact_checkout_oid = ?2,
                 pin_logical_base = ?3,
                 pin_freshness = ?4,
                 updated_at_unix_micros = ?5
             WHERE request_id = ?1
               AND status = 'claimed'
               AND claim_generation = ?6
               AND claim_worker_id = ?7
               AND claim_token = ?8
               AND claim_lease_until_unix_micros > ?5
               AND pin_exact_checkout_oid IS NULL
               AND pin_logical_base IS NULL
               AND pin_freshness IS NULL",
        )
        .bind(request_id)
        .bind(exact_checkout_oid)
        .bind(logical_base)
        .bind(freshness)
        .bind(now)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .execute(&self.pool)
        .await?;
        let current = self
            .get_product_creation_job(request_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("pinned product creation job missing".to_string())
            })?;
        if updated.rows_affected() == 1 {
            Ok(ProductCreationPinOutcome::Pinned(current))
        } else {
            Ok(ProductCreationPinOutcome::Conflict(current))
        }
    }

    pub async fn advance_product_creation_provisioning_stage(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        stage: ProductCreationProvisioningStage,
    ) -> DbResult<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = unix_micros_now();
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET stage_updated_at_unix_micros = CASE
                     WHEN provisioning_stage = ?1 THEN stage_updated_at_unix_micros ELSE ?2 END,
                 updated_at_unix_micros = CASE
                     WHEN provisioning_stage = ?1 THEN updated_at_unix_micros
                     ELSE MAX(updated_at_unix_micros, ?2) END,
                 provisioning_stage = ?1
             WHERE request_id = ?3 AND status IN ('claimed', 'delivery_pending')
               AND claim_generation = ?4 AND claim_worker_id = ?5 AND claim_token = ?6
               AND claim_lease_until_unix_micros > ?2
               AND stage_updated_at_unix_micros <= ?2
               AND (provisioning_stage = ?1 OR
                    (provisioning_stage = ?7 AND
                     ?1 NOT IN ('bootstrap_initial_turn', 'finalize_publication')))",
        )
        .bind(stage.as_str())
        .bind(now)
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .bind(
            stage
                .predecessor()
                .map(ProductCreationProvisioningStage::as_str),
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn product_creation_claim_is_live(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM product_creation_jobs
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?4
               AND claim_lease_until_unix_micros > ?5",
        )
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .bind(datetime_to_unix_micros(now))
        .fetch_one(&self.pool)
        .await?;
        Ok(exists == 1)
    }

    pub async fn schedule_product_creation_retry(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        error: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let current_attempt_count: Option<i64> = sqlx::query_scalar(
            "SELECT attempt_count FROM product_creation_jobs
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?4
               AND claim_lease_until_unix_micros > ?5",
        )
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .bind(datetime_to_unix_micros(now))
        .fetch_optional(&self.pool)
        .await?;
        let Some(current_attempt_count) = current_attempt_count else {
            return Ok(false);
        };
        let Some(retry_at) = next_product_creation_retry_at(current_attempt_count, now) else {
            let updated = sqlx::query(
                "UPDATE product_creation_jobs
                 SET status = 'failed', claim_worker_id = NULL, claim_token = NULL,
                     claim_lease_until_unix_micros = NULL, retry_at_unix_micros = NULL,
                     last_error = ?4, updated_at_unix_micros = ?5
                 WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?2
                   AND claim_worker_id = ?3 AND claim_token = ?6
                   AND claim_lease_until_unix_micros > ?5",
            )
            .bind(request_id)
            .bind(claim.generation)
            .bind(&claim.worker_id)
            .bind(error)
            .bind(datetime_to_unix_micros(now))
            .bind(&claim.token)
            .execute(&self.pool)
            .await?;
            return Ok(updated.rows_affected() == 1);
        };
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'retry_scheduled', attempt_count = attempt_count + 1,
                 claim_worker_id = NULL, claim_token = NULL,
                 claim_lease_until_unix_micros = NULL, retry_at_unix_micros = ?4,
                 last_error = ?5, updated_at_unix_micros = ?6
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?7
               AND claim_lease_until_unix_micros > ?6
               AND attempt_count = ?8 AND attempt_count < ?9",
        )
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(datetime_to_unix_micros(retry_at))
        .bind(error)
        .bind(datetime_to_unix_micros(now))
        .bind(&claim.token)
        .bind(current_attempt_count)
        .bind(PRODUCT_CREATION_MAX_ATTEMPTS)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn append_product_creation_steering_entry(
        &self,
        delivery: &ClaimedProductCreationJob,
        entry: &phoenix_core::domain::sm_event::SteerEntry,
        request_fingerprint: &str,
    ) -> DbResult<usize> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = unix_micros_now();
        let conversation_id: Option<String> = sqlx::query_scalar(
            "SELECT j.published_conversation_id FROM product_creation_jobs j
             JOIN conversations c ON c.id = j.published_conversation_id
             WHERE j.request_id = ?1 AND j.status = 'delivery_pending'
               AND j.claim_generation = ?2 AND j.claim_worker_id = ?3 AND j.claim_token = ?4
               AND j.claim_lease_until_unix_micros > ?5
               AND j.product_conversation_id = ?6 AND j.published_product_id = ?6
               AND c.product_conversation_id = ?6 AND j.published_conversation_id = ?7",
        )
        .bind(&delivery.job.request_id)
        .bind(delivery.claim.generation)
        .bind(&delivery.claim.worker_id)
        .bind(&delivery.claim.token)
        .bind(now)
        .bind(delivery.job.product_conversation_id.as_str())
        .bind(&delivery.job.published_conversation_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(conversation_id) = conversation_id else {
            return Err(DbError::ProductCreationClaimLost);
        };
        if entry.message_id != delivery.job.request_id {
            return Err(DbError::ProductCreationClaimLost);
        }
        let position = self
            .append_steering_entry_tx(&mut tx, &conversation_id, entry, request_fingerprint)
            .await?;
        tx.commit().await?;
        Ok(position)
    }

    pub async fn renew_product_creation_claim(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        now: DateTime<Utc>,
        lease_duration: chrono::Duration,
    ) -> DbResult<bool> {
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET claim_lease_until_unix_micros = ?1,
                 updated_at_unix_micros = ?2
             WHERE request_id = ?3 AND status IN ('claimed', 'delivery_pending')
               AND claim_generation = ?4
               AND claim_worker_id = ?5 AND claim_token = ?6
               AND claim_lease_until_unix_micros > ?2
               AND (status = 'claimed' OR EXISTS (SELECT 1 FROM conversations c
                    WHERE c.id = product_creation_jobs.published_conversation_id))",
        )
        .bind(datetime_to_unix_micros(now + lease_duration))
        .bind(datetime_to_unix_micros(now))
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn schedule_product_creation_delivery_retry(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        delivery_error: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let Some(record) = self.get_product_creation_job(request_id).await? else {
            return Ok(false);
        };
        if record.status != "delivery_pending" {
            return Ok(false);
        }
        let exhausted = record.delivery_attempt_count >= PRODUCT_CREATION_MAX_DELIVERY_ATTEMPTS;
        let next_attempt = if exhausted {
            record.delivery_attempt_count
        } else {
            record.delivery_attempt_count + 1
        };
        let delay_seconds = match next_attempt {
            2 => 2,
            3 => 10,
            _ => 30,
        };
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET delivery_attempt_count = ?2,
                 status = CASE WHEN ?3 THEN 'delivery_failed' ELSE status END,
                 delivery_retry_at_unix_micros = CASE WHEN ?3 THEN NULL ELSE ?4 END,
                 claim_worker_id = NULL,
                 claim_token = NULL,
                 claim_lease_until_unix_micros = NULL,
                 last_error = ?5,
                 updated_at_unix_micros = ?6
             WHERE request_id = ?1 AND status = 'delivery_pending'
               AND EXISTS (SELECT 1 FROM conversations c WHERE c.id = product_creation_jobs.published_conversation_id)
               AND delivery_attempt_count = ?7 AND claim_generation = ?8
               AND claim_worker_id = ?9 AND claim_token = ?10
               AND claim_lease_until_unix_micros > ?6",
        )
        .bind(request_id)
        .bind(next_attempt)
        .bind(exhausted)
        .bind(datetime_to_unix_micros(
            now + chrono::Duration::seconds(delay_seconds),
        ))
        .bind(delivery_error)
        .bind(datetime_to_unix_micros(now))
        .bind(record.delivery_attempt_count)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn mark_product_creation_cleanup_ambiguous(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        cleanup_error: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'cleanup_ambiguous', claim_worker_id = NULL, claim_token = NULL,
                 claim_lease_until_unix_micros = NULL, retry_at_unix_micros = NULL,
                 last_error = ?4, updated_at_unix_micros = ?5
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?6
               AND claim_lease_until_unix_micros > ?5",
        )
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(cleanup_error)
        .bind(datetime_to_unix_micros(now))
        .bind(&claim.token)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    #[allow(clippy::too_many_lines)]
    pub async fn publish_product_creation_atomically(
        &self,
        input: &ProductCreationPublishInput,
    ) -> DbResult<bool> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = unix_micros_now();
        let claimed: Option<(String, String)> = sqlx::query_as(
            "SELECT request_id, product_conversation_id FROM product_creation_jobs
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?4
               AND claim_lease_until_unix_micros > ?5
               AND provisioning_stage = 'commit_metadata'
               AND stage_updated_at_unix_micros <= ?5",
        )
        .bind(&input.request_id)
        .bind(input.claim.generation)
        .bind(&input.claim.worker_id)
        .bind(&input.claim.token)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((_, stored_product_conversation_id)) = claimed else {
            tx.rollback().await?;
            return Ok(false);
        };
        if input.conversation.product_conversation_id.as_str() != stored_product_conversation_id {
            tx.rollback().await?;
            return Err(DbError::Serialization(
                "published conversation product id must match accepted product creation job"
                    .to_string(),
            ));
        }
        if matches!(
            input.environment,
            EnvironmentContext::CoordinatorHome { .. }
        ) {
            return Err(DbError::CoordinatorWorkScope(
                "Coordinator home is not an ordinary creation environment".to_string(),
            ));
        }
        let git_facts_match_environment = !matches!(
            (&input.environment, &input.git_publication),
            (EnvironmentContext::AllocatedWorktree { .. }, None)
                | (EnvironmentContext::None, Some(_))
        );
        if !git_facts_match_environment {
            tx.rollback().await?;
            return Err(DbError::Serialization(
                "allocated-worktree publication requires Git publication facts".to_string(),
            ));
        }
        let conversation_exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE id = ?1")
                .bind(&input.conversation.id)
                .fetch_one(&mut *tx)
                .await?;
        if conversation_exists != 0 {
            return Err(DbError::Serialization(
                "published conversation identity already exists".to_string(),
            ));
        }
        let scope_id = input
            .conversation
            .attached_work_scope_id
            .as_ref()
            .ok_or_else(|| {
                DbError::Serialization("published conversation missing work scope".to_string())
            })?;
        Self::insert_work_scope_tx(
            &mut tx,
            scope_id,
            input.authority_kind,
            input.environment.clone(),
            &unix_micros_to_datetime(now)?.to_rfc3339(),
        )
        .await?;
        insert_conversation_tx(&mut tx, &input.conversation).await?;
        let commit_now = unix_micros_now();
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'delivery_pending', retry_at_unix_micros = NULL, staging_path = ?2,
                 staging_repo_root = ?3, staging_exact_oid = ?4,
                 published_product_id = ?5, published_conversation_id = ?6,
                 delivery_retry_at_unix_micros = ?7, last_error = NULL,
                 provisioning_stage = 'bootstrap_initial_turn', stage_updated_at_unix_micros = ?8,
                 updated_at_unix_micros = MAX(updated_at_unix_micros, ?8)
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?9
               AND claim_worker_id = ?10 AND claim_token = ?11
               AND claim_lease_until_unix_micros > ?8
               AND provisioning_stage = 'commit_metadata'
               AND stage_updated_at_unix_micros <= ?8",
        )
        .bind(&input.request_id)
        .bind(match &input.environment {
            EnvironmentContext::AllocatedWorktree { worktree_path, .. } => {
                Some(worktree_path.as_str())
            }
            EnvironmentContext::UnownedCwd { cwd } => Some(cwd.as_str()),
            EnvironmentContext::CoordinatorHome { .. } => {
                return Err(DbError::CoordinatorWorkScope(
                    "Coordinator home cannot be published as ordinary work".to_string(),
                ));
            }
            EnvironmentContext::None => None,
        })
        .bind(
            input
                .git_publication
                .as_ref()
                .map(|facts| facts.repository_root.as_str()),
        )
        .bind(
            input
                .git_publication
                .as_ref()
                .map(|facts| facts.exact_checkout_oid.as_str()),
        )
        .bind(input.conversation.product_conversation_id.as_str())
        .bind(&input.conversation.id)
        .bind(commit_now + 2_000_000)
        .bind(commit_now)
        .bind(input.claim.generation)
        .bind(&input.claim.worker_id)
        .bind(&input.claim.token)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn record_product_creation_staging(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        staging_path: &str,
        repo_root: &str,
        exact_oid: &str,
    ) -> DbResult<bool> {
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET staging_path = ?1, staging_repo_root = ?2, staging_exact_oid = ?3,
                 updated_at_unix_micros = ?4
             WHERE request_id = ?5 AND status = 'claimed' AND claim_generation = ?6
               AND claim_worker_id = ?7 AND claim_token = ?8
               AND claim_lease_until_unix_micros > ?9",
        )
        .bind(staging_path)
        .bind(repo_root)
        .bind(exact_oid)
        .bind(unix_micros_now())
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .bind(unix_micros_now())
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn claim_next_product_creation_delivery(
        &self,
        worker_id: &str,
        token: &str,
        now: DateTime<Utc>,
        lease_duration: chrono::Duration,
    ) -> DbResult<Option<ClaimedProductCreationJob>> {
        let now_micros = datetime_to_unix_micros(now);
        let lease_until = datetime_to_unix_micros(now + lease_duration);
        let mut tx = self.pool.begin().await?;
        let candidate: Option<(String, i64)> = sqlx::query_as(
            "SELECT request_id, delivery_attempt_count
             FROM product_creation_jobs
             WHERE status = 'delivery_pending'
               AND EXISTS (SELECT 1 FROM conversations c WHERE c.id = product_creation_jobs.published_conversation_id)
               AND (delivery_retry_at_unix_micros IS NULL OR delivery_retry_at_unix_micros <= ?1)
               AND (claim_lease_until_unix_micros IS NULL OR claim_lease_until_unix_micros <= ?1)
             ORDER BY delivery_retry_at_unix_micros, updated_at_unix_micros, request_id
             LIMIT 1",
        )
        .bind(now_micros)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((request_id, delivery_attempt_count)) = candidate else {
            tx.rollback().await?;
            return Ok(None);
        };
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET claim_generation = claim_generation + 1,
                 claim_worker_id = ?1,
                 claim_token = ?2,
                 claim_lease_until_unix_micros = ?3,
                 updated_at_unix_micros = ?4
             WHERE request_id = ?5 AND status = 'delivery_pending'
               AND EXISTS (SELECT 1 FROM conversations c WHERE c.id = product_creation_jobs.published_conversation_id)
               AND (delivery_retry_at_unix_micros IS NULL OR delivery_retry_at_unix_micros <= ?4)
               AND (claim_lease_until_unix_micros IS NULL OR claim_lease_until_unix_micros <= ?4)
               AND delivery_attempt_count = ?6",
        )
        .bind(worker_id)
        .bind(token)
        .bind(lease_until)
        .bind(now_micros)
        .bind(&request_id)
        .bind(delivery_attempt_count)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(None);
        }
        tx.commit().await?;
        let job = self
            .get_product_creation_job(&request_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("claimed product creation delivery job missing".to_string())
            })?;
        Ok(Some(ClaimedProductCreationJob {
            claim: ProductCreationClaim {
                worker_id: worker_id.to_string(),
                token: token.to_string(),
                generation: job.claim_generation,
                lease_until: unix_micros_to_datetime(lease_until)?,
            },
            job,
        }))
    }

    pub async fn retry_failed_product_creation_delivery(&self, request_id: &str) -> DbResult<bool> {
        let now = unix_micros_now();
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'delivery_pending', delivery_attempt_count = 1,
                 delivery_retry_at_unix_micros = ?1, updated_at_unix_micros = ?1
             WHERE request_id = ?2 AND status = 'delivery_failed'
               AND EXISTS (SELECT 1 FROM conversations c WHERE c.id = product_creation_jobs.published_conversation_id)
               AND EXISTS (
                 SELECT 1 FROM product_conversations product
                 WHERE product.id = product_creation_jobs.published_product_id
                   AND product.kind = 'ordinary'
                   AND product.ordinary_lifecycle = 'open'
               )
               AND NOT EXISTS (
                 SELECT 1 FROM close_obligations obligation
                 WHERE obligation.product_conversation_id = product_creation_jobs.published_product_id
                   AND (obligation.phase <> 'completed' OR obligation.close_outcome = 'close_incomplete')
               )",
        )
        .bind(now)
        .bind(request_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn product_creation_delivery_retryable(&self, request_id: &str) -> DbResult<bool> {
        let retryable: bool = sqlx::query_scalar(
            "SELECT CASE WHEN j.status = 'delivery_failed'
               AND EXISTS (SELECT 1 FROM conversations c WHERE c.id = j.published_conversation_id)
               AND EXISTS (
               SELECT 1 FROM product_conversations product
               WHERE product.id = j.published_product_id
                 AND product.kind = 'ordinary'
                 AND product.ordinary_lifecycle = 'open'
             ) AND NOT EXISTS (
               SELECT 1 FROM close_obligations obligation
               WHERE obligation.product_conversation_id = j.published_product_id
                 AND (obligation.phase <> 'completed' OR obligation.close_outcome = 'close_incomplete')
             ) THEN 1 ELSE 0 END
             FROM product_creation_jobs j
             WHERE j.request_id = ?1",
        )
        .bind(request_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(retryable)
    }

    pub async fn list_product_creation_recovery_jobs(
        &self,
        cursor: Option<(i64, String)>,
    ) -> DbResult<Vec<ProductCreationRecoveryJobRecord>> {
        sqlx::query(
            "SELECT request_id, cwd, objective, objective_origin, objective_slack_request_id, objective_source_product_conversation_id, objective_source_transcript_id, objective_source_message_id, objective_source_tool_use_id, model, effort, llm_language, status,
                    published_product_id, updated_at_unix_micros, last_error,
                    CASE WHEN j.status = 'delivery_failed' AND EXISTS (
                      SELECT 1 FROM product_conversations product
                      WHERE product.id = j.published_product_id
                        AND product.kind = 'ordinary'
                        AND product.ordinary_lifecycle = 'open'
                    ) AND NOT EXISTS (
                      SELECT 1 FROM close_obligations obligation
                      WHERE obligation.product_conversation_id = j.published_product_id
                        AND (obligation.phase <> 'completed' OR obligation.close_outcome = 'close_incomplete')
                    ) THEN 1 ELSE 0 END AS delivery_retryable,
                    COALESCE((SELECT json_group_array(json_object('ordinal', ordered.ordinal, 'media_type', ordered.media_type, 'data', ordered.data))
                              FROM (SELECT ordinal, media_type, data FROM product_creation_job_images
                                    WHERE request_id = j.request_id ORDER BY ordinal) ordered), '[]') AS images_json,
                    (SELECT COALESCE(json_group_array(json_object('ordinal', f.ordinal, 'media_type', f.media_type, 'data_sha256', f.data_sha256)), '[]') FROM
                        (SELECT ordinal, media_type, data_sha256 FROM product_creation_image_replay_identity WHERE request_id = j.request_id ORDER BY ordinal) f) AS image_identity_json,
                    EXISTS(SELECT 1 FROM product_creation_retired_images WHERE request_id = j.request_id) AS images_retired
             FROM product_creation_jobs j
             WHERE status IN (
                    'accepted',
                    'claimed',
                    'retry_scheduled',
                    'cancelling',
                    'failed',
                    'cancelled',
                    'delivery_failed',
                    'delivery_pending',
                    'cleanup_ambiguous'
                  )
               AND (status NOT IN ('delivery_pending', 'delivery_failed') OR EXISTS (
                    SELECT 1 FROM conversations c WHERE c.id = j.published_conversation_id
               ))
               AND (?1 IS NULL OR updated_at_unix_micros < ?1
                    OR (updated_at_unix_micros = ?1 AND request_id < ?2))
             ORDER BY updated_at_unix_micros DESC, request_id DESC
             LIMIT 51",
        )
        .bind(cursor.as_ref().map(|(updated_at, _)| *updated_at))
        .bind(cursor.as_ref().map(|(_, request_id)| request_id))
        .try_map(|row: SqliteRow| {
            let images_json: String = row.try_get("images_json")?;
            let images = decode_stored_creation_images(&row, &images_json)?;
            let StoredProductCreationImages::Live(images) = images else {
                return Err(sqlx::Error::Decode("retired creation cannot be a recovery job".into()));
            };
            Ok(ProductCreationRecoveryJobRecord {
                request_id: row.try_get("request_id")?,
                status: row.try_get("status")?,
                delivery_retryable: row.try_get("delivery_retryable")?,
                published_product_conversation_id: row.try_get("published_product_id")?,
                intent: ProductCreationIntent {
                    cwd: row.try_get("cwd")?,
                    objective: row.try_get("objective")?,
                    origin: ProductCreationOrigin::from_row(&row)?,
                    model: row.try_get("model")?,
                    effort: row
                        .try_get::<Option<String>, _>("effort")?
                        .map(|value| {
                            ModelEffort::from_str(&value)
                                .map_err(|error| sqlx::Error::Decode(error.into()))
                        })
                        .transpose()?,
                    llm_language: LlmLanguage::parse_or_default(
                        &row.try_get::<String, _>("llm_language")?,
                    ),
                    images,
                },
                updated_at: unix_micros_to_datetime(row.try_get("updated_at_unix_micros")?)?,
                last_error: row.try_get("last_error")?,
            })
        })
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::from)
    }

    pub async fn complete_product_creation_delivery(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        steering_fingerprint: &SteeringAcceptanceFingerprint,
        intent: &ProductCreationIntent,
    ) -> DbResult<bool> {
        if !self
            .product_creation_objective_already_durably_accepted(
                request_id,
                Some(steering_fingerprint),
                intent,
            )
            .await?
        {
            return Ok(false);
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = unix_micros_now();
        let published_conversation_id: Option<String> = sqlx::query_scalar(
            "SELECT published_conversation_id
             FROM product_creation_jobs
             WHERE request_id = ?1 AND status = 'delivery_pending'
               AND EXISTS (SELECT 1 FROM conversations c WHERE c.id = product_creation_jobs.published_conversation_id)
               AND claim_generation = ?2 AND claim_worker_id = ?3 AND claim_token = ?4
               AND claim_lease_until_unix_micros > ?5
               AND provisioning_stage = 'bootstrap_initial_turn'
               AND stage_updated_at_unix_micros <= ?5",
        )
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(published_conversation_id) = published_conversation_id else {
            tx.rollback().await?;
            return Ok(false);
        };
        if !crate::close_foundation::admit_product_conversation_operation_tx(
            &mut tx,
            &published_conversation_id,
        )
        .await?
        .is_accepted()
        {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query("UPDATE conversations SET archived = 0 WHERE id = ?1")
            .bind(&published_conversation_id)
            .execute(&mut *tx)
            .await?;
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'published', claim_worker_id = NULL, claim_token = NULL,
                 claim_lease_until_unix_micros = NULL, delivery_retry_at_unix_micros = NULL,
                 provisioning_stage = 'finalize_publication', stage_updated_at_unix_micros = ?1,
                 updated_at_unix_micros = MAX(updated_at_unix_micros, ?1)
             WHERE request_id = ?2 AND status = 'delivery_pending'
               AND claim_generation = ?3 AND claim_worker_id = ?4 AND claim_token = ?5
               AND claim_lease_until_unix_micros > ?1
               AND provisioning_stage = 'bootstrap_initial_turn'
               AND stage_updated_at_unix_micros <= ?1",
        )
        .bind(unix_micros_now())
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn get_product_creation_resource_reservations(
        &self,
        request_id: &str,
    ) -> DbResult<Vec<ProductCreationResourceReservation>> {
        sqlx::query(
            "SELECT id, request_id, generation, repository_identity, resource_identity, status,
                    ownership_token
             FROM product_creation_resource_reservations
             WHERE request_id = ?1
             ORDER BY id",
        )
        .bind(request_id)
        .try_map(|row| parse_product_creation_resource_reservation_row(&row))
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::from)
    }

    pub async fn reserve_product_creation_resource(
        &self,
        reservation_id: &str,
        request_id: &str,
        claim: &ProductCreationClaim,
        repository_identity: &str,
        resource_identity: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let inserted = sqlx::query(
            "INSERT INTO product_creation_resource_reservations (
                id, request_id, generation, repository_identity, resource_identity,
                status, created_at_unix_micros, updated_at_unix_micros
             ) SELECT ?1, ?2, ?3, ?4, ?5, 'reserved', ?6, ?6
               WHERE EXISTS (
                   SELECT 1 FROM product_creation_jobs j
                   WHERE j.request_id = ?2 AND j.status = 'claimed'
                     AND j.claim_generation = ?3 AND j.claim_worker_id = ?7 AND j.claim_token = ?8
                     AND j.claim_lease_until_unix_micros > ?6
                     AND j.published_product_id IS NULL AND j.published_conversation_id IS NULL
               )
             ON CONFLICT(request_id, resource_identity) DO UPDATE SET
                generation = excluded.generation,
                repository_identity = excluded.repository_identity,
                status = CASE
                    WHEN product_creation_resource_reservations.status = 'present'
                        THEN 'present'
                    ELSE 'reserved'
                END,
                updated_at_unix_micros = excluded.updated_at_unix_micros",
        )
        .bind(reservation_id)
        .bind(request_id)
        .bind(claim.generation)
        .bind(repository_identity)
        .bind(resource_identity)
        .bind(datetime_to_unix_micros(now))
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .execute(&self.pool)
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    pub async fn record_product_creation_resource_ownership(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        resource_identity: &str,
        ownership_token: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        if ownership_token.trim().is_empty() {
            return Err(DbError::Serialization(
                "product creation ownership token must not be empty".to_string(),
            ));
        }
        let updated = sqlx::query(
            "UPDATE product_creation_resource_reservations
             SET ownership_token = ?7, updated_at_unix_micros = ?1
             WHERE request_id = ?2 AND generation = ?3 AND resource_identity = ?4
               AND (
                   (status = 'reserved' AND (ownership_token IS NULL OR ownership_token = ?7))
                   OR (status = 'present' AND ownership_token = ?7)
               )
               AND EXISTS (
                   SELECT 1 FROM product_creation_jobs j
                   WHERE j.request_id = ?2 AND j.status = 'claimed'
                     AND j.claim_generation = ?3 AND j.claim_worker_id = ?5 AND j.claim_token = ?6
                     AND j.claim_lease_until_unix_micros > ?1
               )",
        )
        .bind(datetime_to_unix_micros(now))
        .bind(request_id)
        .bind(claim.generation)
        .bind(resource_identity)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .bind(ownership_token)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn mark_product_creation_resource_present(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        resource_identity: &str,
        ownership_token: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        if ownership_token.trim().is_empty() {
            return Err(DbError::Serialization(
                "product creation ownership token must not be empty".to_string(),
            ));
        }
        let updated = sqlx::query(
            "UPDATE product_creation_resource_reservations
             SET status = 'present', ownership_token = ?7, updated_at_unix_micros = ?1
             WHERE request_id = ?2 AND generation = ?3 AND resource_identity = ?4
               AND status = 'reserved'
               AND EXISTS (
                   SELECT 1 FROM product_creation_jobs j
                   WHERE j.request_id = ?2 AND j.status = 'claimed'
                     AND j.claim_generation = ?3 AND j.claim_worker_id = ?5 AND j.claim_token = ?6
                     AND j.claim_lease_until_unix_micros > ?1
                     AND j.published_product_id IS NULL AND j.published_conversation_id IS NULL
               )",
        )
        .bind(datetime_to_unix_micros(now))
        .bind(request_id)
        .bind(claim.generation)
        .bind(resource_identity)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .bind(ownership_token)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn reset_product_creation_resource_after_cleanup(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        resource_identity: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let updated = sqlx::query(
            "UPDATE product_creation_resource_reservations
             SET status = 'reserved', ownership_token = NULL, updated_at_unix_micros = ?1
             WHERE request_id = ?2 AND generation = ?3 AND resource_identity = ?4
               AND status IN ('reserved', 'present')
               AND EXISTS (
                   SELECT 1 FROM product_creation_jobs job
                   WHERE job.request_id = ?2 AND job.status = 'claimed'
                     AND job.claim_generation = ?3 AND job.claim_worker_id = ?5
                     AND job.claim_token = ?6 AND job.claim_lease_until_unix_micros > ?1
               )",
        )
        .bind(datetime_to_unix_micros(now))
        .bind(request_id)
        .bind(claim.generation)
        .bind(resource_identity)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn mark_product_creation_resource_cleanup_conflict(
        &self,
        cleanup: &ProductCreationCleanupJob,
        reservation_id: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        self.reconcile_product_creation_resource_cleanup(
            cleanup,
            reservation_id,
            ProductCreationResourceCleanupDisposition::Conflict,
            now,
        )
        .await
    }

    pub async fn release_product_creation_resource(
        &self,
        cleanup: &ProductCreationCleanupJob,
        reservation_id: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        self.reconcile_product_creation_resource_cleanup(
            cleanup,
            reservation_id,
            ProductCreationResourceCleanupDisposition::Released,
            now,
        )
        .await
    }

    pub async fn cancel_product_creation(
        &self,
        request_id: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let now_micros = datetime_to_unix_micros(now);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let generation: Option<i64> = sqlx::query_scalar(
            "UPDATE product_creation_jobs
             SET status = 'cancelling', claim_generation = claim_generation + 1,
                 claim_worker_id = NULL, claim_token = NULL, claim_lease_until_unix_micros = NULL,
                 retry_at_unix_micros = NULL,
                 cleanup_worker_id = NULL, cleanup_token = NULL, cleanup_lease_until_unix_micros = NULL,
                 last_error = NULL, cancelled_at_unix_micros = NULL,
                 published_product_id = NULL, published_conversation_id = NULL,
                 deletion_requested_at_unix_micros = NULL, updated_at_unix_micros = ?2
             WHERE request_id = ?1
               AND (status IN ('accepted', 'claimed', 'retry_scheduled')
                    OR (status = 'delivery_pending' AND EXISTS (
                        SELECT 1 FROM conversations c
                        JOIN product_conversations p ON p.id = c.product_conversation_id
                        WHERE c.id = product_creation_jobs.published_conversation_id
                          AND p.id = product_creation_jobs.product_conversation_id
                          AND p.id = product_creation_jobs.published_product_id
                          AND p.kind = 'ordinary' AND p.ordinary_lifecycle = 'open'
                          AND c.archived = 1 AND c.runtime_role = 'user'
                          AND c.parent_conversation_id IS NULL
                          AND NOT EXISTS (SELECT 1 FROM conversations other
                              WHERE other.product_conversation_id = p.id AND other.id <> c.id)
                          AND NOT EXISTS (SELECT 1 FROM steering_acceptance_receipts receipt
                              WHERE receipt.conversation_id = c.id))))
             RETURNING claim_generation",
        )
        .bind(request_id)
        .bind(now_micros)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(generation) = generation else {
            return Ok(false);
        };
        sqlx::query(
            "UPDATE product_creation_resource_reservations
             SET generation = ?3, status = 'cleanup_required', updated_at_unix_micros = ?2
             WHERE request_id = ?1 AND status IN ('reserved', 'present', 'cleanup_required')",
        )
        .bind(request_id)
        .bind(now_micros)
        .bind(generation)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn request_product_creation_deletion(
        &self,
        request_id: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let now_micros = datetime_to_unix_micros(now);
        let mut tx = self.pool.begin().await?;
        let job: Option<i64> = sqlx::query_scalar(
            "SELECT claim_generation FROM product_creation_jobs
             WHERE request_id = ?1
               AND status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling', 'cancelled', 'failed', 'cleanup_ambiguous')
               AND published_product_id IS NULL AND published_conversation_id IS NULL",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(generation) = job else {
            tx.rollback().await?;
            return Ok(false);
        };
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'deletion_pending', claim_generation = claim_generation + 1,
                 claim_worker_id = NULL, claim_token = NULL, claim_lease_until_unix_micros = NULL,
                 retry_at_unix_micros = NULL,
                 cleanup_worker_id = NULL, cleanup_token = NULL, cleanup_lease_until_unix_micros = NULL,
                 last_error = NULL, cancelled_at_unix_micros = NULL,
                 deletion_requested_at_unix_micros = ?2, updated_at_unix_micros = ?2
             WHERE request_id = ?1 AND claim_generation = ?3
               AND status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling', 'cancelled', 'failed', 'cleanup_ambiguous')
               AND published_product_id IS NULL AND published_conversation_id IS NULL",
        )
        .bind(request_id)
        .bind(now_micros)
        .bind(generation)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query(
            "UPDATE product_creation_resource_reservations
             SET generation = ?3, status = 'cleanup_required', updated_at_unix_micros = ?2
             WHERE request_id = ?1 AND status IN ('reserved', 'present', 'cleanup_required', 'conflict')",
        )
        .bind(request_id)
        .bind(now_micros)
        .bind(generation + 1)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn claim_next_product_creation_cleanup(
        &self,
        worker_id: &str,
        token: &str,
        now: DateTime<Utc>,
        lease_duration: chrono::Duration,
    ) -> DbResult<Option<ProductCreationCleanupJob>> {
        let now_micros = datetime_to_unix_micros(now);
        let lease_until = now + lease_duration;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT request_id, product_conversation_id, cwd, objective, objective_origin, objective_slack_request_id, objective_source_product_conversation_id, objective_source_transcript_id, objective_source_message_id, objective_source_tool_use_id, model, effort, llm_language, status,
                    provisioning_stage, stage_updated_at_unix_micros,
                    accepted_at_unix_micros, updated_at_unix_micros, attempt_count,
                    claim_generation, claim_worker_id, claim_token, claim_lease_until_unix_micros,
                    retry_at_unix_micros, cleanup_worker_id, cleanup_token, cleanup_lease_until_unix_micros,
                    delivery_attempt_count, delivery_retry_at_unix_micros,
                    pin_exact_checkout_oid, pin_logical_base, pin_freshness,
                    staging_path, staging_repo_root, staging_exact_oid,
                    published_product_id, published_conversation_id, last_error,
                    cancelled_at_unix_micros, deletion_requested_at_unix_micros,
                    COALESCE((SELECT json_group_array(json_object('ordinal', i.ordinal, 'media_type', i.media_type, 'data', i.data))
                              FROM (SELECT ordinal, media_type, data FROM product_creation_job_images
                                    WHERE request_id = j.request_id ORDER BY ordinal) i), '[]') AS images_json,
                    (SELECT COALESCE(json_group_array(json_object('ordinal', f.ordinal, 'media_type', f.media_type, 'data_sha256', f.data_sha256)), '[]') FROM
                        (SELECT ordinal, media_type, data_sha256 FROM product_creation_image_replay_identity WHERE request_id = j.request_id ORDER BY ordinal) f) AS image_identity_json,
                    EXISTS(SELECT 1 FROM product_creation_retired_images WHERE request_id = j.request_id) AS images_retired
             FROM product_creation_jobs j
             WHERE updated_at_unix_micros <= ?1
               AND (cleanup_lease_until_unix_micros IS NULL OR cleanup_lease_until_unix_micros <= ?1)
               AND status IN ('cancelling', 'deletion_pending')
             ORDER BY updated_at_unix_micros, request_id
             LIMIT 1",
        )
        .bind(now_micros)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(None);
        };
        let request_id: String = row.try_get("request_id")?;
        let generation: i64 = row.try_get("claim_generation")?;
        let claimed = sqlx::query(
            "UPDATE product_creation_jobs
             SET cleanup_worker_id = ?1, cleanup_token = ?2,
                 cleanup_lease_until_unix_micros = ?3
             WHERE request_id = ?4 AND claim_generation = ?5
               AND (cleanup_lease_until_unix_micros IS NULL OR cleanup_lease_until_unix_micros <= ?6)",
        )
        .bind(worker_id)
        .bind(token)
        .bind(datetime_to_unix_micros(lease_until))
        .bind(&request_id)
        .bind(generation)
        .bind(now_micros)
        .execute(&mut *tx)
        .await?;
        if claimed.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(None);
        }
        tx.commit().await?;
        let mut job = self
            .get_product_creation_job(&request_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("claimed product cleanup job missing".to_string())
            })?;
        job.claim_generation = generation;
        let reservations = self
            .get_product_creation_resource_reservations(&request_id)
            .await?;
        Ok(Some(ProductCreationCleanupJob {
            job,
            claim: ProductCreationCleanupClaim {
                worker_id: worker_id.to_string(),
                token: token.to_string(),
                generation,
                lease_until,
            },
            reservations,
        }))
    }

    pub async fn mark_claimed_product_creation_cleanup_ambiguous(
        &self,
        cleanup: &ProductCreationCleanupJob,
        cleanup_error: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let result = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'cleanup_ambiguous', retry_at_unix_micros = NULL,
                 deletion_requested_at_unix_micros = NULL,
                 cleanup_worker_id = NULL, cleanup_token = NULL,
                 cleanup_lease_until_unix_micros = NULL,
                 last_error = ?1, updated_at_unix_micros = ?2
             WHERE request_id = ?3 AND claim_generation = ?4
               AND status IN ('cancelling', 'deletion_pending')
               AND cleanup_worker_id = ?5 AND cleanup_token = ?6
               AND cleanup_lease_until_unix_micros > ?2",
        )
        .bind(cleanup_error)
        .bind(datetime_to_unix_micros(now))
        .bind(&cleanup.job.request_id)
        .bind(cleanup.claim.generation)
        .bind(&cleanup.claim.worker_id)
        .bind(&cleanup.claim.token)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn schedule_product_creation_cleanup_retry(
        &self,
        cleanup: &ProductCreationCleanupJob,
        next_attempt_at: DateTime<Utc>,
    ) -> DbResult<bool> {
        let result = sqlx::query(
            "UPDATE product_creation_jobs
             SET updated_at_unix_micros = ?1,
                 cleanup_worker_id = NULL, cleanup_token = NULL, cleanup_lease_until_unix_micros = NULL
             WHERE request_id = ?2 AND claim_generation = ?3
               AND status IN ('cancelling', 'deletion_pending')
               AND cleanup_worker_id = ?4 AND cleanup_token = ?5",
        )
        .bind(datetime_to_unix_micros(next_attempt_at))
        .bind(&cleanup.job.request_id)
        .bind(cleanup.claim.generation)
        .bind(&cleanup.claim.worker_id)
        .bind(&cleanup.claim.token)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn reconcile_product_creation_resource_cleanup(
        &self,
        cleanup: &ProductCreationCleanupJob,
        reservation_id: &str,
        disposition: ProductCreationResourceCleanupDisposition,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let status = match disposition {
            ProductCreationResourceCleanupDisposition::Released => "released",
            ProductCreationResourceCleanupDisposition::Conflict => "conflict",
        };
        let result = sqlx::query(
            "UPDATE product_creation_resource_reservations
             SET status = ?7, updated_at_unix_micros = ?1
             WHERE id = ?2 AND request_id = ?3 AND generation = ?4
               AND (status = 'cleanup_required' OR status = 'conflict')
               AND EXISTS (
                   SELECT 1 FROM product_creation_jobs j
                   WHERE j.request_id = ?3 AND j.claim_generation = ?4
                     AND j.cleanup_worker_id = ?5 AND j.cleanup_token = ?6
                     AND j.cleanup_lease_until_unix_micros > ?1
               )",
        )
        .bind(datetime_to_unix_micros(now))
        .bind(reservation_id)
        .bind(&cleanup.job.request_id)
        .bind(cleanup.claim.generation)
        .bind(&cleanup.claim.worker_id)
        .bind(&cleanup.claim.token)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    #[allow(clippy::too_many_lines)]
    pub async fn finish_product_creation_cleanup(
        &self,
        cleanup: &ProductCreationCleanupJob,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let now_micros = datetime_to_unix_micros(now);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let authoritative: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT product_conversation_id, published_product_id, published_conversation_id
             FROM product_creation_jobs
             WHERE request_id = ?1 AND status = ?2 AND claim_generation = ?3
               AND cleanup_worker_id = ?4 AND cleanup_token = ?5
               AND cleanup_lease_until_unix_micros > ?6",
        )
        .bind(&cleanup.job.request_id)
        .bind(&cleanup.job.status)
        .bind(cleanup.claim.generation)
        .bind(&cleanup.claim.worker_id)
        .bind(&cleanup.claim.token)
        .bind(now_micros)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((reserved_id, product_id, conversation_id)) = authoritative else {
            return Ok(false);
        };
        if reserved_id != cleanup.job.product_conversation_id.as_str()
            || product_id.as_deref()
                != cleanup
                    .job
                    .published_product_id
                    .as_ref()
                    .map(ProductConversationId::as_str)
            || conversation_id != cleanup.job.published_conversation_id
        {
            return Err(DbError::Serialization(
                "product creation cleanup identity mismatch".to_string(),
            ));
        }
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM product_creation_resource_reservations
             WHERE request_id = ?1 AND status != 'released'",
        )
        .bind(&cleanup.job.request_id)
        .fetch_one(&mut *tx)
        .await?;
        if remaining != 0 {
            tx.rollback().await?;
            return Err(DbError::Serialization(
                "product creation cleanup still has unreconciled resources".to_string(),
            ));
        }
        if product_id.is_some() || conversation_id.is_some() {
            return Err(DbError::Serialization(
                "product creation cleanup retained published identity".to_string(),
            ));
        }
        let staged_product_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_conversations WHERE id = ?1)")
                .bind(&reserved_id)
                .fetch_one(&mut *tx)
                .await?;
        if staged_product_exists {
            let aggregate: Option<(String, String)> = sqlx::query_as(
                "SELECT c.id, c.work_scope_id FROM conversations c
                 JOIN product_conversations p ON p.id = c.product_conversation_id
                 JOIN product_conversation_work_scopes attachment
                   ON attachment.product_conversation_id = p.id
                  AND attachment.work_scope_id = c.work_scope_id
                 WHERE p.id = ?1 AND p.kind = 'ordinary'
                   AND p.ordinary_lifecycle = 'open' AND c.archived = 1
                   AND c.runtime_role = 'user' AND c.parent_conversation_id IS NULL
                   AND NOT EXISTS (SELECT 1 FROM conversations other
                       WHERE other.id <> c.id AND (other.product_conversation_id = p.id
                           OR other.work_scope_id = c.work_scope_id
                           OR other.parent_conversation_id = c.id))
                   AND NOT EXISTS (SELECT 1 FROM product_conversation_work_scopes other_attachment
                       WHERE other_attachment.product_conversation_id = p.id
                         AND other_attachment.work_scope_id <> c.work_scope_id)
                   AND NOT EXISTS (SELECT 1 FROM product_conversation_work_scopes other_owner
                       WHERE other_owner.work_scope_id = c.work_scope_id
                         AND other_owner.product_conversation_id <> p.id)
                   AND NOT EXISTS (SELECT 1 FROM steering_acceptance_receipts receipt
                       WHERE receipt.conversation_id = c.id)",
            )
            .bind(&reserved_id)
            .fetch_optional(&mut *tx)
            .await?;
            let Some((conversation_id, scope_id)) = aggregate else {
                return Err(DbError::Serialization(
                    "product creation cleanup staged aggregate mismatch or accepted input"
                        .to_string(),
                ));
            };
            let deleted = Self::hard_delete_conversation_tx(
                &mut tx,
                &conversation_id,
                sqlite_telemetry::ParentSqliteObserver::UninstrumentedNested,
                None,
            )
            .await?;
            if deleted != (true, true) {
                return Err(DbError::Serialization(
                    "staged aggregate was not deleted".to_string(),
                ));
            }
            Self::delete_work_scope_if_empty(&mut tx, &scope_id).await?;
            let scope_remains: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM work_scopes WHERE id = ?1)")
                    .bind(&scope_id)
                    .fetch_one(&mut *tx)
                    .await?;
            if scope_remains {
                return Err(DbError::Serialization(
                    "staged work scope was not deleted".to_string(),
                ));
            }
        }
        match cleanup.job.status.as_str() {
            "cancelling" => {
                let updated = sqlx::query(
                    "UPDATE product_creation_jobs
                     SET status = 'cancelled', cancelled_at_unix_micros = ?1,
                         updated_at_unix_micros = ?1,
                         cleanup_worker_id = NULL, cleanup_token = NULL,
                         cleanup_lease_until_unix_micros = NULL
                     WHERE request_id = ?2 AND status = 'cancelling' AND claim_generation = ?3
                       AND cleanup_worker_id = ?4 AND cleanup_token = ?5
                       AND cleanup_lease_until_unix_micros > ?1",
                )
                .bind(now_micros)
                .bind(&cleanup.job.request_id)
                .bind(cleanup.claim.generation)
                .bind(&cleanup.claim.worker_id)
                .bind(&cleanup.claim.token)
                .execute(&mut *tx)
                .await?;
                if updated.rows_affected() != 1 {
                    tx.rollback().await?;
                    return Ok(false);
                }
            }
            "deletion_pending" => {
                let deleted = sqlx::query(
                    "DELETE FROM product_creation_jobs
                     WHERE request_id = ?1 AND status = 'deletion_pending' AND claim_generation = ?2
                       AND cleanup_worker_id = ?3 AND cleanup_token = ?4
                       AND cleanup_lease_until_unix_micros > ?5",
                )
                .bind(&cleanup.job.request_id)
                .bind(cleanup.claim.generation)
                .bind(&cleanup.claim.worker_id)
                .bind(&cleanup.claim.token)
                .bind(now_micros)
                .execute(&mut *tx)
                .await?;
                if deleted.rows_affected() != 1 {
                    tx.rollback().await?;
                    return Ok(false);
                }
            }
            other => {
                tx.rollback().await?;
                return Err(DbError::Serialization(format!(
                    "unexpected product creation cleanup status: {other}"
                )));
            }
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn next_product_creation_deadline(&self) -> DbResult<Option<DateTime<Utc>>> {
        let deadline: Option<i64> = sqlx::query_scalar(
            "SELECT MIN(deadline) FROM (
                 SELECT accepted_at_unix_micros AS deadline
                 FROM product_creation_jobs WHERE status = 'accepted'
                 UNION ALL SELECT retry_at_unix_micros
                 FROM product_creation_jobs WHERE status = 'retry_scheduled'
                 UNION ALL SELECT claim_lease_until_unix_micros FROM product_creation_jobs WHERE status = 'claimed'
                 UNION ALL SELECT CASE
                     WHEN delivery_retry_at_unix_micros IS NULL THEN claim_lease_until_unix_micros
                     WHEN claim_lease_until_unix_micros IS NULL THEN delivery_retry_at_unix_micros
                     WHEN delivery_retry_at_unix_micros > claim_lease_until_unix_micros THEN delivery_retry_at_unix_micros
                     ELSE claim_lease_until_unix_micros
                 END FROM product_creation_jobs
                 WHERE status = 'delivery_pending'
                   AND EXISTS (
                       SELECT 1 FROM conversations c
                       WHERE c.id = product_creation_jobs.published_conversation_id
                   )
                 UNION ALL
                 SELECT CASE
                            WHEN cleanup_lease_until_unix_micros IS NOT NULL
                            THEN cleanup_lease_until_unix_micros
                            ELSE updated_at_unix_micros
                        END AS deadline
                 FROM product_creation_jobs
                 WHERE status IN ('cancelling', 'deletion_pending')
             )",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(deadline.map(unix_micros_to_datetime).transpose()?)
    }

    pub async fn recent_distinct_management_roots(&self, limit: usize) -> DbResult<Vec<String>> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT COALESCE(project.canonical_path, job.staging_repo_root) AS management_root
             FROM conversations conversation
             JOIN product_conversations product
               ON product.id = conversation.product_conversation_id
              AND product.kind = 'ordinary'
              AND product.ordinary_lifecycle IN ('open', 'history')
             JOIN work_scopes scope ON scope.id = conversation.work_scope_id
             LEFT JOIN projects project ON project.id = conversation.project_id
             LEFT JOIN product_creation_jobs job
               ON job.published_conversation_id = conversation.id
              AND job.status = 'published'
             WHERE conversation.user_initiated = 1 AND conversation.runtime_role = 'user'
               AND COALESCE(project.canonical_path, job.staging_repo_root) IS NOT NULL
             GROUP BY management_root
             ORDER BY COUNT(DISTINCT conversation.product_conversation_id) DESC,
                      MAX(conversation.updated_at) DESC,
                      management_root DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().take(limit).collect())
    }
}

#[cfg(test)]
mod product_creation_tests {
    use super::*;
    use phoenix_core::domain::db_schema::{Message, MessageContent, UserContent};
    use phoenix_core::domain::sm_state::ConvState;
    use phoenix_core::llm_language::LlmLanguage;

    async fn advance_to_metadata(db: &Database, request_id: &str, claim: &ProductCreationClaim) {
        use ProductCreationProvisioningStage as Stage;
        for stage in [
            Stage::ResolveRepository,
            Stage::ResolveStartingPin,
            Stage::MaterializeWorktree,
            Stage::FinalizeAttachments,
            Stage::CommitMetadata,
        ] {
            assert!(db
                .advance_product_creation_provisioning_stage(request_id, claim, stage)
                .await
                .unwrap());
        }
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "Checks retained identity through publication, deletion and replay"
    )]
    async fn coordinator_creation_history_survives_target_deletion_and_replays_identity() {
        use phoenix_core::domain::db_schema::SourceToolCall;

        let db = Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        let mut accepted_intent = intent("/repo/history", "historical objective");
        accepted_intent.origin = ProductCreationOrigin::InternalConversation {
            product_conversation_id: coordinator.product_conversation_id.clone(),
            transcript_id: coordinator.id.clone(),
            source_call: SourceToolCall {
                message_id: "history-message".into(),
                tool_use_id: "history-call".into(),
            },
        };
        db.accept_product_creation_resolved(&request_id, Some(&coordinator.id), |_| {
            Ok(accepted_intent.clone())
        })
        .await
        .unwrap();
        let claimed = db
            .claim_next_product_creation(
                "history-worker",
                "history-token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let job = db
            .get_product_creation_job(&request_id)
            .await
            .unwrap()
            .unwrap();
        let conversation_id = format!("history-{}", uuid::Uuid::new_v4());
        advance_to_metadata(&db, &request_id, &claimed.claim).await;
        assert!(db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: request_id.clone(),
                claim: claimed.claim,
                conversation: published_conversation(
                    &conversation_id,
                    job.product_conversation_id.clone(),
                    WorkScopeId::new(),
                    "/repo/history",
                ),
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/history".into(),
                },
                git_publication: None,
            })
            .await
            .unwrap());

        db.delete_conversation(&conversation_id).await.unwrap();
        let snapshot = db
            .read_product_creation_status_as_coordinator(&coordinator.id, &request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.request_id, request_id);
        assert_eq!(
            snapshot.product_conversation_id,
            job.product_conversation_id
        );
        let retained = db
            .get_product_creation_job(&request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.intent.objective, "historical objective");
        assert_eq!(retained.intent.origin, accepted_intent.origin);
        assert!(matches!(
            snapshot.publication,
            Some(ProductCreationPublicationRead::Historical { conversation_id: id }) if id == conversation_id
        ));
        assert_eq!(
            snapshot.initial_objective_execution,
            ProductCreationInitialExecution::Unavailable
        );
        let ProductCreationAcceptOutcome::Replayed(replay) = db
            .accept_product_creation_resolved(&request_id, Some(&coordinator.id), |_| {
                Ok(accepted_intent.clone())
            })
            .await
            .unwrap()
        else {
            panic!("identical request must replay historical creation");
        };
        assert_eq!(replay.product_conversation_id, job.product_conversation_id);
        let mut changed = accepted_intent;
        changed.objective = "changed objective".into();
        assert!(matches!(
            db.accept_product_creation_resolved(&request_id, Some(&coordinator.id), |_| {
                Ok(changed.clone())
            })
            .await
            .unwrap(),
            ProductCreationAcceptOutcome::Conflict(_)
        ));
        assert!(db
            .claim_next_product_creation(
                "history-replay-worker",
                "history-replay-token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn coordinator_creation_history_deletion_fences_queued_delivery_and_recovery() {
        let db = Database::open_in_memory().await.unwrap();
        let delivery = staged_delivery(&db, "coordinator_creation_history_delivery").await;
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let pending = db
            .read_product_creation_status_as_coordinator(&coordinator.id, &delivery.job.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.status, "delivery_pending");
        assert!(pending.publication.is_none());
        let conversation_id = delivery
            .job
            .published_conversation_id
            .clone()
            .expect("staged job has a published transcript");
        assert!(db.next_product_creation_deadline().await.unwrap().is_some());
        db.delete_conversation(&conversation_id).await.unwrap();
        assert_eq!(db.next_product_creation_deadline().await.unwrap(), None);
        let history = db
            .read_product_creation_status_as_coordinator(&coordinator.id, &delivery.job.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(history.status, "delivery_pending");
        assert!(matches!(history.publication,
            Some(ProductCreationPublicationRead::Historical { conversation_id: original })
                if original == conversation_id));
        assert!(db
            .append_product_creation_steering_entry(
                &delivery,
                &delivery_entry(&delivery),
                &format!("product-create:{}", delivery.job.request_id),
            )
            .await
            .is_err());

        assert!(!db
            .renew_product_creation_claim(
                &delivery.job.request_id,
                &delivery.claim,
                Utc::now(),
                chrono::Duration::minutes(1),
            )
            .await
            .unwrap());
        assert!(!db
            .complete_product_creation_delivery(
                &delivery.job.request_id,
                &delivery.claim,
                &SteeringAcceptanceFingerprint::Exact(format!(
                    "product-create:{}",
                    delivery.job.request_id
                )),
                &delivery.job.intent.live_intent().unwrap(),
            )
            .await
            .unwrap());
        assert!(!db
            .schedule_product_creation_delivery_retry(
                &delivery.job.request_id,
                &delivery.claim,
                "target deleted",
                Utc::now(),
            )
            .await
            .unwrap());
        assert!(db
            .claim_next_product_creation_delivery(
                "history-recovery",
                "history-token",
                Utc::now(),
                chrono::Duration::minutes(1),
            )
            .await
            .unwrap()
            .is_none());
        assert!(!db
            .retry_failed_product_creation_delivery(&delivery.job.request_id)
            .await
            .unwrap());
        assert!(!db
            .product_creation_delivery_retryable(&delivery.job.request_id)
            .await
            .unwrap());
        assert!(!db
            .list_product_creation_recovery_jobs(None)
            .await
            .unwrap()
            .iter()
            .any(|job| job.request_id == delivery.job.request_id));
    }

    #[tokio::test]
    async fn coordinator_creation_status_decodes_ordered_image_rows() {
        let db = Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let mut request = intent("/tmp", "ordered images");
        request.images = vec![
            ProductCreationImage {
                media_type: "image/png".into(),
                data: "first".into(),
            },
            ProductCreationImage {
                media_type: "image/jpeg".into(),
                data: "second".into(),
            },
        ];
        let request_id = uuid::Uuid::new_v4().to_string();
        db.accept_product_creation(&request_id, &request)
            .await
            .unwrap();
        let read = db
            .get_product_creation_job_as_coordinator(&coordinator.id, &request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            read.intent.images,
            StoredProductCreationImages::Live(request.images)
        );
        assert_eq!(read.intent.origin, ProductCreationOrigin::UserApi);
        assert!(matches!(
            db.get_product_creation_job_as_coordinator("not-current-gc", &request_id)
                .await,
            Err(DbError::CurrentCoordinatorRequired)
        ));
    }

    #[tokio::test]
    async fn coordinator_creation_snapshot_denies_predecessor_and_revoked_heads() {
        let db = Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        db.accept_product_creation(&request_id, &intent("/tmp", "snapshot authority"))
            .await
            .unwrap();
        db.update_conversation_state(
            &coordinator.id,
            &ConvState::ContextExhausted {
                summary: "continue coordinator".into(),
            },
        )
        .await
        .unwrap();
        let crate::ContinueOutcome::Created(successor) =
            db.continue_conversation(&coordinator.id).await.unwrap()
        else {
            panic!("expected a Coordinator successor");
        };
        for request in [request_id.as_str(), "missing-request"] {
            for caller in [coordinator.id.as_str(), "ordinary-or-missing"] {
                assert!(matches!(
                    db.read_product_creation_status_as_coordinator(caller, request)
                        .await,
                    Err(DbError::CurrentCoordinatorRequired)
                ));
            }
        }
        let status = db
            .read_product_creation_status_as_coordinator(&successor.id, &request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.status, "accepted");
        assert_eq!(status.request_id, request_id);
        assert!(db
            .read_product_creation_status_as_coordinator(&successor.id, "missing-request")
            .await
            .unwrap()
            .is_none());
        sqlx::query("UPDATE conversations SET coordinator_head = 0 WHERE id = ?1")
            .bind(&successor.id)
            .execute(db.pool())
            .await
            .unwrap();
        for request in [request_id.as_str(), "missing-request"] {
            assert!(matches!(
                db.read_product_creation_status_as_coordinator(&successor.id, request)
                    .await,
                Err(DbError::CurrentCoordinatorRequired)
            ));
        }
    }

    async fn complete_queued_creation(db: &Database, delivery: &ClaimedProductCreationJob) {
        let fingerprint = format!("product-create:{}", delivery.job.request_id);
        db.append_product_creation_steering_entry(
            delivery,
            &delivery_entry(delivery),
            &fingerprint,
        )
        .await
        .unwrap();
        assert!(db
            .complete_product_creation_delivery(
                &delivery.job.request_id,
                &delivery.claim,
                &SteeringAcceptanceFingerprint::Exact(fingerprint),
                &delivery.job.intent.live_intent().unwrap(),
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn public_creation_status_retains_original_after_deletion() {
        let db = Database::open_in_memory().await.unwrap();
        assert!(db
            .read_public_product_creation_status("unknown")
            .await
            .unwrap()
            .is_none());
        let request_id = uuid::Uuid::new_v4().to_string();
        let delivery = staged_delivery(&db, &request_id).await;
        let original = delivery.job.published_conversation_id.clone().unwrap();
        let staged = db
            .read_public_product_creation_status(&request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(staged.job.status, "delivery_pending");
        assert!(staged.publication.is_none());

        complete_queued_creation(&db, &delivery).await;
        let live = db
            .read_public_product_creation_status(&request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(live.job.status, "published");
        assert!(
            matches!(live.publication, Some(ProductCreationPublicationRead::Available {
            conversation_id, ..
        }) if conversation_id == original)
        );

        db.delete_conversation(&original).await.unwrap();
        let historical = db
            .read_public_product_creation_status(&request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(historical.job.request_id, request_id);
        assert_eq!(
            historical.job.product_conversation_id,
            delivery.job.product_conversation_id
        );
        assert!(
            matches!(historical.publication, Some(ProductCreationPublicationRead::Historical {
            conversation_id
        }) if conversation_id == original)
        );
    }

    #[tokio::test]
    async fn coordinator_creation_snapshot_does_not_mix_acceptance_with_committed_publication() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("creation-snapshot.db").to_str().unwrap())
            .await
            .unwrap();
        crate::run_pending_migrations(db.pool()).await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        db.accept_product_creation(&request_id, &intent("/repo/a", "inspect repository"))
            .await
            .unwrap();
        let snapshot = db
            .read_product_creation_status_as_coordinator_inner(
                &coordinator.id,
                &request_id,
                Some(Box::pin(async {
                    let delivery = staged_delivery(&db, &request_id).await;
                    complete_queued_creation(&db, &delivery).await;
                })),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.status, "accepted");
        assert_eq!(
            snapshot.provisioning_stage,
            ProductCreationProvisioningStage::ValidateIntent
        );
        assert!(snapshot.publication.is_none());
        assert_eq!(
            snapshot.initial_objective_execution,
            ProductCreationInitialExecution::Unavailable
        );
        let fresh = db
            .read_product_creation_status_as_coordinator(&coordinator.id, &request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fresh.status, "published");
        assert_eq!(
            fresh.provisioning_stage,
            ProductCreationProvisioningStage::FinalizePublication
        );
        assert!(fresh.publication.is_some());
        assert_eq!(
            fresh.initial_objective_execution,
            ProductCreationInitialExecution::Queued
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn coordinator_creation_snapshot_keeps_execution_route_and_authority_coherent() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("execution-snapshot.db").to_str().unwrap())
            .await
            .unwrap();
        crate::run_pending_migrations(db.pool()).await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        let delivery = staged_delivery(&db, &request_id).await;
        let pending = db
            .read_product_creation_status_as_coordinator(&coordinator.id, &request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.status, "delivery_pending");
        assert!(pending.publication.is_none());
        assert_eq!(
            pending.initial_objective_execution,
            ProductCreationInitialExecution::Unavailable
        );
        complete_queued_creation(&db, &delivery).await;
        let conversation_id = delivery.job.published_conversation_id.clone().unwrap();
        let old_slug = db.get_conversation(&conversation_id).await.unwrap().slug;
        let snapshot = db
            .read_product_creation_status_as_coordinator_inner(
                &coordinator.id,
                &request_id,
                Some(Box::pin(async {
                    let content = MessageContent::User(UserContent::new(delivery.job.intent.objective.clone()));
                    let message = Message {
                        message_id: request_id.clone(),
                        conversation_id: conversation_id.clone(),
                        sequence_id: 1,
                        origin: delivery.job.intent.origin.input_origin(),
                        message_type: content.message_type(),
                        content,
                        display_data: None,
                        usage_data: None,
                        created_at: Utc::now(),
                    };
                    db.commit_steering_drain(
                        &conversation_id,
                        &[message],
                        &ConvState::LlmRequesting { attempt: 0 },
                        Utc::now(),
                    )
                    .await
                    .unwrap();
                    let mut writer = db.pool.begin().await.unwrap();
                    sqlx::query("UPDATE conversations SET slug = 'snapshot-renamed', name_revision = name_revision + 1 WHERE id = ?1")
                        .bind(&conversation_id)
                        .execute(&mut *writer)
                        .await
                        .unwrap();
                    sqlx::query("UPDATE conversations SET coordinator_head = 0 WHERE id = ?1")
                        .bind(&coordinator.id)
                        .execute(&mut *writer)
                        .await
                        .unwrap();
                    writer.commit().await.unwrap();
                })),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.status, "published");
        assert_eq!(
            snapshot.initial_objective_execution,
            ProductCreationInitialExecution::Queued
        );
        let ProductCreationPublicationRead::Available {
            conversation_id: published_conversation_id,
            aggregate,
        } = snapshot.publication.unwrap()
        else {
            panic!("live publication must include its aggregate");
        };
        assert_eq!(published_conversation_id, conversation_id);
        assert_eq!(aggregate.root.conversation.slug, old_slug);
        assert!(matches!(aggregate.root.conversation.state, ConvState::Idle));
        assert_eq!(
            db.product_creation_initial_execution(&request_id)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Running
        );
        assert_eq!(
            db.get_conversation(&conversation_id)
                .await
                .unwrap()
                .slug
                .as_deref(),
            Some("snapshot-renamed")
        );
        assert!(matches!(
            db.read_product_creation_status_as_coordinator(&coordinator.id, &request_id)
                .await,
            Err(DbError::CurrentCoordinatorRequired)
        ));
    }

    fn intent(request_cwd: &str, objective: &str) -> ProductCreationIntent {
        ProductCreationIntent {
            origin: ProductCreationOrigin::UserApi,
            cwd: request_cwd.to_string(),
            objective: objective.to_string(),
            model: Some("gpt-5".to_string()),
            effort: Some(ModelEffort::High),
            images: vec![ProductCreationImage {
                media_type: "image/png".to_string(),
                data: "abc123".to_string(),
            }],
            llm_language: LlmLanguage::Caveman,
        }
    }

    fn published_conversation(
        id: &str,
        product_id: ProductConversationId,
        scope: WorkScopeId,
        cwd: &str,
    ) -> Conversation {
        Conversation {
            id: id.to_string(),
            product_conversation_id: product_id,
            slug: Some(id.to_string()),
            title: Some(id.to_string()),
            cwd: cwd.to_string(),
            parent_conversation_id: None,
            user_initiated: true,
            state: ConvState::Idle,
            state_updated_at: Utc::now(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            archived: false,
            model: Some("gpt-5".to_string()),
            effort: Some(ModelEffort::High),
            service_tier: ServiceTier::Standard,
            project_id: None,
            conv_mode: ConvMode::Direct,
            runtime_role: RuntimeRole::User,
            attached_work_scope_id: Some(scope),
            desired_base_branch: None,
            message_count: 0,
            transcript_generation: 1,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,

            llm_language: LlmLanguage::Caveman,
            spawned_from_conversation_id: None,
        }
    }

    async fn seed_staged_dependents(db: &Database, conversation_id: &str) {
        let pool = db.pool();
        sqlx::query("INSERT INTO workflows (workflow_id, profile_kind, profile_version, runtime_acceptance_enabled, external_acceptance_enabled, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at) VALUES (52, 'wake', 1, 1, 0, 1, 0, 'Active', 'wake', 1, X'00', 1, 1)")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_effects (workflow_id, effect_id, declared_workflow_version, family, kind, intent_codec_family, intent_codec_version, intent_payload, generation, role, capability_kind, status) VALUES (52, 1, 0, 'wake', 'observe', 'wake', 1, X'01', 0, 'Required', 'ReclaimableObservation', 'Receipted')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO wake_bindings (workflow_id, conversation_id, contract_id, profile_kind, profile_version, work_scope_id, resource_kind, bash_handle_id, registering_tool_use_id, expires_at, resolved_at, prepared_fingerprint, observe_effect_id, created_at) VALUES (52, ?1, 'contract', 'wake', 1, (SELECT work_scope_id FROM conversations WHERE id = ?1), 'Bash', 'b-52', 'tool-52', 100, NULL, 'fingerprint', 1, 1)")
            .bind(conversation_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO messages
            (message_id, conversation_id, sequence_id, message_type, content, created_at)
            VALUES ('staged-message', ?1, 1, 'system', 'staged metadata', '2026-01-01')",
        )
        .bind(conversation_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO message_fts(rowid, text) VALUES (52, 'staged metadata')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO message_fts_rows
            (fts_rowid, message_id, chunk_ordinal, conversation_id, message_type, created_at, content_hash)
            VALUES (52, 'staged-message', 0, ?1, 'system', '2026-01-01', 'hash')")
            .bind(conversation_id).execute(pool).await.unwrap();
    }

    async fn assert_staged_dependents(db: &Database, conversation_id: &str, expected: i64) {
        let counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM messages WHERE conversation_id = ?1),
                (SELECT COUNT(*) FROM message_fts_rows WHERE conversation_id = ?1),
                (SELECT COUNT(*) FROM message_fts WHERE rowid = 52),
                (SELECT COUNT(*) FROM workflows WHERE workflow_id = 52),
                (SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = 52),
                (SELECT COUNT(*) FROM wake_bindings WHERE conversation_id = ?1)",
        )
        .bind(conversation_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            counts,
            (expected, expected, expected, expected, expected, expected)
        );
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .is_empty());
    }

    async fn staged_delivery(db: &Database, request_id: &str) -> ClaimedProductCreationJob {
        staged_delivery_with_intent(db, request_id, intent("/repo/a", "inspect repository")).await
    }

    async fn staged_delivery_with_intent(
        db: &Database,
        request_id: &str,
        creation_intent: ProductCreationIntent,
    ) -> ClaimedProductCreationJob {
        db.accept_product_creation(request_id, &creation_intent)
            .await
            .unwrap();
        let claimed = db
            .claim_product_creation(
                request_id,
                "worker",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        advance_to_metadata(db, request_id, &claimed.claim).await;
        let mut conversation = published_conversation(
            &format!("conversation-{request_id}"),
            claimed.job.product_conversation_id.clone(),
            WorkScopeId::new(),
            "/repo/a",
        );
        conversation.archived = true;
        assert!(db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: request_id.to_string(),
                claim: claimed.claim.clone(),
                conversation,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/a".to_string()
                },
                git_publication: None,
            })
            .await
            .unwrap());
        ClaimedProductCreationJob {
            claim: claimed.claim,
            job: db
                .get_product_creation_job(request_id)
                .await
                .unwrap()
                .unwrap(),
        }
    }

    async fn initial_execution_fixture(watched: bool) -> (Database, String, String) {
        initial_execution_fixture_in(Database::open_in_memory().await.unwrap(), watched, None).await
    }

    async fn initial_execution_fixture_in(
        db: Database,
        watched: bool,
        images: Option<Vec<ProductCreationImage>>,
    ) -> (Database, String, String) {
        let request_id = uuid::Uuid::new_v4().to_string();
        let mut creation_intent = intent("/repo/a", "inspect repository");
        if let Some(images) = images {
            creation_intent.images = images;
        }
        let delivery = staged_delivery_with_intent(&db, &request_id, creation_intent).await;
        let conversation_id = delivery.job.published_conversation_id.clone().unwrap();
        let fingerprint = format!("product-create:{request_id}");
        db.append_product_creation_steering_entry(
            &delivery,
            &delivery_entry(&delivery),
            &fingerprint,
        )
        .await
        .unwrap();
        assert!(db
            .complete_product_creation_delivery(
                &request_id,
                &delivery.claim,
                &SteeringAcceptanceFingerprint::Exact(fingerprint),
                &delivery.job.intent.live_intent().unwrap()
            )
            .await
            .unwrap());
        assert_eq!(
            db.product_creation_initial_execution(&request_id)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Queued
        );
        if watched {
            db.watch_product_conversation(&delivery.job.product_conversation_id)
                .await
                .unwrap();
        }
        let content = MessageContent::User(UserContent::new(delivery.job.intent.objective.clone()));
        let initial_message = Message {
            message_id: request_id.clone(),
            conversation_id: conversation_id.clone(),
            sequence_id: 1,
            origin: delivery.job.intent.origin.input_origin(),
            message_type: content.message_type(),
            content,
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        db.commit_steering_drain(
            &conversation_id,
            &[initial_message],
            &ConvState::LlmRequesting { attempt: 0 },
            Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(
            db.product_creation_initial_execution(&request_id)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Running
        );
        (db, request_id, conversation_id)
    }

    #[tokio::test]
    async fn initial_objective_status_distinguishes_publication_live_waits_and_completion() {
        let (db, request, conversation) = initial_execution_fixture(true).await;
        for (state, expected) in [
            (
                ConvState::AwaitingUserResponse {
                    questions: Vec::new(),
                    tool_use_id: "initial-question".into(),
                    request_authority:
                        phoenix_core::domain::sm_state::QuestionRequestAuthority::new(),
                },
                ProductCreationInitialExecution::AwaitingUserResponse,
            ),
            (
                ConvState::AwaitingTaskApproval {
                    task_file: "tasks/test.md".into(),
                    title: "test".into(),
                    priority: phoenix_core::task_source::Priority::P2,
                    plan: "test".into(),
                },
                ProductCreationInitialExecution::AwaitingTaskApproval,
            ),
        ] {
            db.update_conversation_state(&conversation, &state)
                .await
                .unwrap();
            assert_eq!(
                db.product_creation_initial_execution(&request)
                    .await
                    .unwrap(),
                expected
            );
        }
        db.update_conversation_state(&conversation, &ConvState::Idle)
            .await
            .unwrap();
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Completed
        );
        db.update_conversation_state(&conversation, &ConvState::LlmRequesting { attempt: 0 })
            .await
            .unwrap();
        db.update_conversation_state(
            &conversation,
            &ConvState::Error {
                message: "unrelated later turn".into(),
                error_kind: phoenix_core::domain::db_schema::ErrorKind::InvalidRequest,
                resets_at: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Completed
        );
    }

    #[tokio::test]
    async fn initial_objective_failure_is_not_creation_failure_or_later_turn_success() {
        let (db, request, conversation) = initial_execution_fixture(true).await;
        db.update_conversation_state(
            &conversation,
            &ConvState::Error {
                message: "initial failure".into(),
                error_kind: phoenix_core::domain::db_schema::ErrorKind::InvalidRequest,
                resets_at: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            db.get_product_creation_job(&request)
                .await
                .unwrap()
                .unwrap()
                .status,
            "published"
        );
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Failed {
                reason: "initial failure".into(),
            }
        );
        db.update_conversation_state(&conversation, &ConvState::LlmRequesting { attempt: 0 })
            .await
            .unwrap();
        db.update_conversation_state(&conversation, &ConvState::Idle)
            .await
            .unwrap();
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Failed {
                reason: "initial failure".into(),
            }
        );
    }

    #[tokio::test]
    async fn recorded_cancel_classifies_initial_receipt_before_watch_and_occurrence_consumption() {
        for watched in [false, true] {
            let (db, request, conversation) = initial_execution_fixture(watched).await;
            sqlx::query("INSERT INTO execution_cancel_observations(conversation_id) VALUES (?1)")
                .bind(&conversation)
                .execute(db.pool())
                .await
                .unwrap();
            db.update_conversation_state(
                &conversation,
                &ConvState::CancellingSubAgents {
                    pending: vec![],
                    completed_results: vec![],
                    cause: phoenix_core::domain::sm_event::CancelCause::UserRequested,
                    spawn_tool_id: None,
                },
            )
            .await
            .unwrap();
            let pending: (i64, i64) = sqlx::query_as(
                "SELECT (SELECT COUNT(*) FROM execution_cancel_observations WHERE conversation_id = ?1),
                        (SELECT COUNT(*) FROM steering_execution_occurrences WHERE conversation_id = ?1)",
            )
            .bind(&conversation)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(pending, (1, 1));
            db.update_conversation_state(&conversation, &ConvState::Idle)
                .await
                .unwrap();
            let receipt: (String, Option<String>) = sqlx::query_as(
                "SELECT terminal_kind, failure_reason FROM product_creation_initial_execution_receipts WHERE request_id = ?1",
            )
            .bind(&request)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(receipt, ("Cancelled".into(), None));
            let remaining: (i64, i64) = sqlx::query_as(
                "SELECT (SELECT COUNT(*) FROM execution_cancel_observations WHERE conversation_id = ?1),
                        (SELECT COUNT(*) FROM steering_execution_occurrences WHERE conversation_id = ?1)",
            )
            .bind(&conversation)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(remaining, (0, 0));
            let events = db.pending_coordinator_watch_events(16).await.unwrap();
            assert_eq!(events.len(), usize::from(watched));
            if watched {
                assert_eq!(events[0].outcome.label(), "cancelled");
            }
            sqlx::query("DELETE FROM coordinator_watch_events WHERE source_transcript_id = ?1")
                .bind(&conversation)
                .execute(db.pool())
                .await
                .unwrap();
            db.update_conversation_state(&conversation, &ConvState::LlmRequesting { attempt: 1 })
                .await
                .unwrap();
            db.update_conversation_state(&conversation, &ConvState::Idle)
                .await
                .unwrap();
            db.delete_conversation(&conversation).await.unwrap();
            assert_eq!(
                db.product_creation_initial_execution(&request).await.unwrap(),
                ProductCreationInitialExecution::Cancelled
            );
        }
    }

    #[tokio::test]
    async fn cancelled_initial_objective_is_retained_after_occurrence_consumption() {
        let (db, request, conversation) = initial_execution_fixture(false).await;
        db.update_conversation_state(
            &conversation,
            &ConvState::CreationCancelled {
                job_id: "cancelled-creation".into(),
            },
        )
        .await
        .unwrap();
        let receipt: (String, Option<String>) = sqlx::query_as(
            "SELECT terminal_kind, failure_reason FROM product_creation_initial_execution_receipts WHERE request_id = ?1",
        )
        .bind(&request)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(receipt, ("Cancelled".into(), None));
        let occurrence_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM steering_execution_occurrences WHERE conversation_id = ?1",
        )
        .bind(&conversation)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(occurrence_count, 0);
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Cancelled
        );
        db.update_conversation_state(&conversation, &ConvState::LlmRequesting { attempt: 0 })
            .await
            .unwrap();
        db.update_conversation_state(&conversation, &ConvState::Idle)
            .await
            .unwrap();
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Cancelled
        );
    }

    #[tokio::test]
    async fn unwatched_terminal_receipt_survives_occurrence_consumption_and_file_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("initial-execution.sqlite");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::run_pending_migrations(db.pool()).await.unwrap();
        let (db, request, conversation) = initial_execution_fixture_in(db, false, None).await;
        db.update_conversation_state(
            &conversation,
            &ConvState::Error {
                message: "original objective failed".into(),
                error_kind: phoenix_core::domain::db_schema::ErrorKind::InvalidRequest,
                resets_at: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Failed {
                reason: "original objective failed".into(),
            }
        );
        db.pool().close().await;
        let reopened = Database::open(path.to_str().unwrap()).await.unwrap();
        assert_eq!(
            reopened
                .product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Failed {
                reason: "original objective failed".into(),
            }
        );
        let receipt_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM product_creation_initial_execution_receipts WHERE request_id = ?1",
        )
        .bind(&request)
        .fetch_one(reopened.pool())
        .await
        .unwrap();
        assert_eq!(receipt_count, 1);
        let occurrence_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM steering_execution_occurrences WHERE conversation_id = ?1",
        )
        .bind(&conversation)
        .fetch_one(reopened.pool())
        .await
        .unwrap();
        assert_eq!(occurrence_count, 0);
    }

    #[tokio::test]
    async fn unwatched_initial_objective_retains_exact_terminal_before_occurrence_consumption() {
        for (terminal, expected, reason) in [
            (
                ConvState::Idle,
                ProductCreationInitialExecution::Completed,
                None,
            ),
            (
                ConvState::Error {
                    message: "original objective failed".into(),
                    error_kind: phoenix_core::domain::db_schema::ErrorKind::InvalidRequest,
                    resets_at: None,
                },
                ProductCreationInitialExecution::Failed {
                    reason: "original objective failed".into(),
                },
                Some("original objective failed"),
            ),
        ] {
            let (db, request, conversation) = initial_execution_fixture(false).await;
            db.update_conversation_state(&conversation, &terminal)
                .await
                .unwrap();
            let receipt: (String, Option<String>) = sqlx::query_as(
                "SELECT terminal_kind, failure_reason FROM product_creation_initial_execution_receipts WHERE request_id = ?1",
            )
            .bind(&request)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(
                receipt.0,
                if reason.is_some() {
                    "Failed"
                } else {
                    "Completed"
                }
            );
            assert_eq!(receipt.1.as_deref(), reason);
            let occurrences: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM steering_execution_occurrences WHERE conversation_id = ?1",
            )
            .bind(&conversation)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(occurrences, 0);
            let watch_events: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM coordinator_watch_events WHERE source_transcript_id = ?1",
            )
            .bind(&conversation)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(watch_events, 0);
            db.update_conversation_state(&conversation, &ConvState::LlmRequesting { attempt: 0 })
                .await
                .unwrap();
            db.update_conversation_state(&conversation, &ConvState::Idle)
                .await
                .unwrap();
            assert_eq!(
                db.product_creation_initial_execution(&request)
                    .await
                    .unwrap(),
                expected
            );
            db.delete_conversation(&conversation).await.unwrap();
            assert_eq!(
                db.product_creation_initial_execution(&request)
                    .await
                    .unwrap(),
                expected
            );
        }
        let db = Database::open_in_memory().await.unwrap();
        assert_eq!(
            db.product_creation_initial_execution("unknown-request")
                .await
                .unwrap(),
            ProductCreationInitialExecution::Unavailable
        );
    }

    async fn assert_replay_conflicts(
        db: &Database,
        request: &str,
        changed_intents: &[ProductCreationIntent],
    ) {
        for changed in changed_intents {
            assert!(matches!(
                db.accept_product_creation(request, changed).await.unwrap(),
                ProductCreationAcceptOutcome::Conflict(_)
            ));
        }
    }

    async fn accept_and_fail_unwatched_turn(db: &Database, request: &str, conversation: &str) {
        use crate::workflow::{
            AcceptAuthoritativeTurn, PersistedConversationProjection,
            TerminalizeAuthoritativeTurnInput,
        };
        use phoenix_workflow::{
            AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp,
            TurnCommand, TurnOutcome,
        };

        let target = ConversationAuthority(conversation.to_owned());
        let payload = phoenix_core::domain::sm_event::PreparedDirectTurnPayload::from_parts(
                    phoenix_core::domain::sm_event::SubmittedDirectTurnIdentity {
                        origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
                        text: "unrelated accepted turn".into(),
                        images: Vec::new(),
                        files: Vec::new(),
                        message_id: "unrelated-turn-message".into(),
                        user_agent: None,
                        skill_invocation: None,
                        expansion_policy: phoenix_core::domain::sm_event::SubmittedDirectTurnExpansionPolicy::ExpandReferences,
                    },
                    phoenix_core::domain::sm_event::PreparedDirectTurnDelivery {
                        text: "unrelated accepted turn".into(),
                        llm_text: None,
                        images: Vec::new(),
                        files: Vec::new(),
                        user_agent: None,
                        skill_invocation: None,
                    },
                )
                .to_exact_bytes()
                .unwrap();
        let repo = db.workflow_repository();
        let accepted = repo
            .accept_authoritative_turn(&AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new("captured-initial-execution").unwrap(),
                prepared: PreparedTurn::from_exact_payload(&target, payload),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(1),
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
            panic!("expected new authoritative turn");
        };
        repo.terminalize_authoritative_turn_with_occurrence(
            &TerminalizeAuthoritativeTurnInput {
                command: TurnCommand::Fail {
                    turn_id,
                    expected_generation: 0,
                    reason: "captured objective failure".into(),
                },
                projection: Some(PersistedConversationProjection {
                    state: ConvState::Error {
                        message: "captured objective failure".into(),
                        error_kind: phoenix_core::domain::db_schema::ErrorKind::InvalidRequest,
                        resets_at: None,
                    },
                    state_updated_at: Utc::now(),
                }),
                provider_replay_settlement:
                    phoenix_core::domain::provider_replay::ProviderReplaySettlement::Preserve,
            },
            Some(request),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn captured_direct_turn_settlement_retains_unwatched_initial_outcome() {
        let (db, request, conversation) = initial_execution_fixture(false).await;
        accept_and_fail_unwatched_turn(&db, &request, &conversation).await;
        let receipt: (String, Option<String>) = sqlx::query_as(
            "SELECT terminal_kind, failure_reason FROM product_creation_initial_execution_receipts WHERE request_id = ?1",
        )
        .bind(&request)
        .fetch_one(db.pool())
        .await
        .unwrap();
        let occurrence_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM steering_execution_occurrences WHERE conversation_id = ?1",
        )
        .bind(&conversation)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(occurrence_count, 0);
        let watch_event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM coordinator_watch_events WHERE source_transcript_id = ?1",
        )
        .bind(&conversation)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(watch_event_count, 0);
        assert_eq!(
            receipt,
            ("Failed".into(), Some("captured objective failure".into()))
        );
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Failed {
                reason: "captured objective failure".into(),
            }
        );
        db.delete_conversation(&conversation).await.unwrap();
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Failed {
                reason: "captured objective failure".into(),
            }
        );
    }

    async fn assert_single_image_conflicts(
        db: &Database,
        request: &str,
        accepted: &ProductCreationIntent,
    ) {
        assert_replay_conflicts(
            db,
            request,
            &[
                ProductCreationIntent {
                    images: vec![],
                    ..accepted.clone()
                },
                ProductCreationIntent {
                    images: vec![ProductCreationImage {
                        data: "different".into(),
                        ..accepted.images[0].clone()
                    }],
                    ..accepted.clone()
                },
                ProductCreationIntent {
                    images: vec![ProductCreationImage {
                        media_type: "image/jpeg".into(),
                        ..accepted.images[0].clone()
                    }],
                    ..accepted.clone()
                },
                ProductCreationIntent {
                    images: vec![accepted.images[0].clone(), accepted.images[0].clone()],
                    ..accepted.clone()
                },
            ],
        )
        .await;
    }

    async fn assert_two_image_conflicts(
        db: &Database,
        request: &str,
        accepted: &ProductCreationIntent,
    ) {
        assert_replay_conflicts(
            db,
            request,
            &[
                ProductCreationIntent {
                    images: vec![],
                    ..accepted.clone()
                },
                ProductCreationIntent {
                    images: vec![accepted.images[0].clone()],
                    ..accepted.clone()
                },
                ProductCreationIntent {
                    images: vec![accepted.images[1].clone(), accepted.images[0].clone()],
                    ..accepted.clone()
                },
                ProductCreationIntent {
                    images: vec![
                        ProductCreationImage {
                            data: "changed-first".into(),
                            ..accepted.images[0].clone()
                        },
                        accepted.images[1].clone(),
                    ],
                    ..accepted.clone()
                },
                ProductCreationIntent {
                    images: vec![
                        ProductCreationImage {
                            media_type: "image/webp".into(),
                            ..accepted.images[0].clone()
                        },
                        accepted.images[1].clone(),
                    ],
                    ..accepted.clone()
                },
            ],
        )
        .await;
    }

    async fn assert_creation_survives_original_deletion(
        db: &Database,
        request: &str,
        original: &str,
        successor_id: &str,
        product_conversation_id: &ProductConversationId,
        job: &ProductCreationJobRecord,
    ) -> i64 {
        assert_eq!(
            db.get_conversation(successor_id).await.unwrap().id,
            successor_id
        );
        assert_eq!(product_conversation_id, &job.product_conversation_id);
        assert_eq!(
            db.product_creation_initial_execution(request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Failed {
                reason: "context exhausted".into(),
            }
        );
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let snapshot = db
            .read_product_creation_status_as_coordinator(&coordinator.id, request)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(snapshot.publication, Some(ProductCreationPublicationRead::Historical { conversation_id }) if conversation_id == original)
        );
        assert!(
            matches!(db.accept_product_creation(request, &job.intent.live_intent().unwrap()).await.unwrap(), ProductCreationAcceptOutcome::Replayed(retained) if retained.product_conversation_id == job.product_conversation_id && retained.published_conversation_id.as_deref() == Some(original))
        );
        assert_eq!(db.next_product_creation_deadline().await.unwrap(), None);
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM steering_messages WHERE conversation_id=?1 AND message_id=?2",
        )
        .bind(successor_id)
        .bind(request)
        .fetch_one(db.pool())
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn creation_images_retire_only_after_final_product_transcript_deletion() {
        let (db, request, original) = initial_execution_fixture(false).await;
        let job = db
            .get_product_creation_job(&request)
            .await
            .unwrap()
            .unwrap();
        db.update_conversation_state(
            &original,
            &ConvState::ContextExhausted {
                summary: "continue ordinary product".into(),
            },
        )
        .await
        .unwrap();
        let crate::ContinueOutcome::Created(successor) =
            db.continue_conversation(&original).await.unwrap()
        else {
            panic!("expected ordinary successor");
        };
        let original_images: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT ordinal, media_type, data FROM product_creation_job_images WHERE request_id = ?1 ORDER BY ordinal",
        )
        .bind(&request).fetch_all(db.pool()).await.unwrap();
        assert!(!original_images.is_empty());
        db.delete_conversation(&original).await.unwrap();
        let successor_inputs = assert_creation_survives_original_deletion(
            &db,
            &request,
            &original,
            &successor.id,
            &successor.product_conversation_id,
            &job,
        )
        .await;
        let retained_images: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT ordinal, media_type, data FROM product_creation_job_images WHERE request_id = ?1 ORDER BY ordinal",
        ).bind(&request).fetch_all(db.pool()).await.unwrap();
        assert_eq!(retained_images, original_images);
        assert!(matches!(
            db.get_product_creation_job(&request)
                .await
                .unwrap()
                .unwrap()
                .intent
                .images,
            StoredProductCreationImages::Live(_)
        ));
        let accepted = job.intent.live_intent().unwrap();
        db.delete_conversation(&successor.id).await.unwrap();
        let retired_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM product_creation_job_images WHERE request_id = ?1",
        )
        .bind(&request)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(retired_count, 0);
        let identity_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM product_creation_image_replay_identity WHERE request_id = ?1",
        )
        .bind(&request)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            usize::try_from(identity_count).unwrap(),
            original_images.len()
        );
        let retained = db
            .get_product_creation_job(&request)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            &retained.intent.images,
            StoredProductCreationImages::Retired(_)
        ));
        assert!(retained.intent.live_intent().is_err());
        assert!(matches!(
            db.accept_product_creation(&request, &accepted)
                .await
                .unwrap(),
            ProductCreationAcceptOutcome::Replayed(_)
        ));
        assert_single_image_conflicts(&db, &request, &accepted).await;
        assert_eq!(db.next_product_creation_deadline().await.unwrap(), None);
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .is_empty());
        assert_eq!(successor_inputs, 0);
    }

    #[tokio::test]
    async fn text_only_creation_final_deletion_replays_without_retired_images() {
        let (db, request, conversation) = initial_execution_fixture_in(
            Database::open_in_memory().await.unwrap(),
            false,
            Some(vec![]),
        )
        .await;
        let job = db
            .get_product_creation_job(&request)
            .await
            .unwrap()
            .unwrap();
        let accepted = job.intent.live_intent().unwrap();
        assert!(accepted.images.is_empty());
        db.update_conversation_state(&conversation, &ConvState::Idle)
            .await
            .unwrap();
        db.delete_conversation(&conversation).await.unwrap();

        let retained = db
            .get_product_creation_job(&request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            retained.intent.images,
            StoredProductCreationImages::Live(vec![])
        );
        assert_eq!(retained.intent.live_intent().unwrap(), accepted);
        let retired_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM product_creation_retired_images WHERE request_id = ?1",
        )
        .bind(&request)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(retired_count, 0);
        let ProductCreationAcceptOutcome::Replayed(replayed) = db
            .accept_product_creation(&request, &accepted)
            .await
            .unwrap()
        else {
            panic!("expected historical text-only replay");
        };
        assert_eq!(replayed.product_conversation_id, job.product_conversation_id);
        assert_eq!(replayed.published_product_id, job.published_product_id);
        assert_eq!(
            replayed.published_conversation_id.as_deref(),
            Some(conversation.as_str())
        );
        assert_eq!(replayed.intent.live_intent().unwrap(), accepted);
        let mut changed = accepted.clone();
        changed.images.push(ProductCreationImage {
            media_type: "image/png".into(),
            data: "added-image".into(),
        });
        assert_replay_conflicts(&db, &request, &[changed]).await;
        assert_eq!(db.next_product_creation_deadline().await.unwrap(), None);
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Completed
        );
        let live_targets: (i64, i64, i64) = sqlx::query_as(
            "SELECT
             (SELECT COUNT(*) FROM product_conversations WHERE id = ?1),
             (SELECT COUNT(*) FROM conversations WHERE id = ?2),
             (SELECT COUNT(*) FROM steering_messages WHERE message_id = ?3)",
        )
        .bind(job.product_conversation_id.as_str())
        .bind(&conversation)
        .bind(&request)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(live_targets, (0, 0, 0));
    }

    #[tokio::test]
    async fn two_retired_creation_images_preserve_order_bytes_and_conflict_identity() {
        let images = vec![
            ProductCreationImage {
                media_type: "image/png".into(),
                data: "first-image-bytes".into(),
            },
            ProductCreationImage {
                media_type: "image/jpeg".into(),
                data: "second-image-bytes".into(),
            },
        ];
        let (db, request, original) = initial_execution_fixture_in(
            Database::open_in_memory().await.unwrap(),
            false,
            Some(images),
        )
        .await;
        let job = db
            .get_product_creation_job(&request)
            .await
            .unwrap()
            .unwrap();
        db.update_conversation_state(
            &original,
            &ConvState::ContextExhausted {
                summary: "continue ordinary product".into(),
            },
        )
        .await
        .unwrap();
        let crate::ContinueOutcome::Created(successor) =
            db.continue_conversation(&original).await.unwrap()
        else {
            panic!("expected ordinary successor");
        };
        db.delete_conversation(&original).await.unwrap();
        db.delete_conversation(&successor.id).await.unwrap();

        let retired_rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT ordinal, media_type, data_sha256 FROM product_creation_image_replay_identity WHERE request_id = ?1 ORDER BY ordinal",
        )
        .bind(&request)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            retired_rows,
            vec![
                (
                    0,
                    "image/png".into(),
                    "8134BF8B15730236857BC533E6FB018A7E3F20FC05CC8A1EC1DE70FFC66906F3".into()
                ),
                (
                    1,
                    "image/jpeg".into(),
                    "3B164D76A96163B96FB2AA5E4450A0E825B41C29D428796B57E496CB6CD7C48A".into()
                ),
            ]
        );
        let live_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM product_creation_job_images WHERE request_id = ?1",
        )
        .bind(&request)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(live_count, 0);
        let retained = db
            .get_product_creation_job(&request)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            retained.intent.images,
            StoredProductCreationImages::Retired(_)
        ));
        let accepted = job.intent.live_intent().unwrap();
        assert!(matches!(
            db.accept_product_creation(&request, &accepted)
                .await
                .unwrap(),
            ProductCreationAcceptOutcome::Replayed(_)
        ));
        assert_two_image_conflicts(&db, &request, &accepted).await;
    }

    #[tokio::test]
    async fn missing_live_creation_image_payload_is_not_treated_as_empty() {
        let (db, request, _) = initial_execution_fixture(false).await;
        sqlx::query("DELETE FROM product_creation_job_images WHERE request_id = ?1")
            .bind(&request)
            .execute(db.pool())
            .await
            .unwrap();
        assert!(db.get_product_creation_job(&request).await.is_err());
    }

    #[tokio::test]
    async fn legacy_watched_initial_failure_keeps_exact_reason_after_receipt_absence() {
        let (db, request, conversation) = initial_execution_fixture(true).await;
        db.update_conversation_state(
            &conversation,
            &ConvState::Error {
                message: "legacy objective diagnostic".into(),
                error_kind: phoenix_core::domain::db_schema::ErrorKind::InvalidRequest,
                resets_at: None,
            },
        )
        .await
        .unwrap();
        sqlx::query(
            "DELETE FROM product_creation_initial_execution_receipts WHERE request_id = ?1",
        )
        .bind(&request)
        .execute(db.pool())
        .await
        .unwrap();
        let expected = ProductCreationInitialExecution::Failed {
            reason: "legacy objective diagnostic".into(),
        };
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            expected
        );
        db.delete_conversation(&conversation).await.unwrap();
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            expected
        );
    }

    #[tokio::test]
    async fn creation_history_keeps_exact_terminal_evidence_after_target_deletion() {
        for (terminal, expected) in [
            (ConvState::Idle, ProductCreationInitialExecution::Completed),
            (
                ConvState::Error {
                    message: "initial failure".into(),
                    error_kind: phoenix_core::domain::db_schema::ErrorKind::InvalidRequest,
                    resets_at: None,
                },
                ProductCreationInitialExecution::Failed {
                    reason: "initial failure".into(),
                },
            ),
        ] {
            let (db, request, conversation) = initial_execution_fixture(true).await;
            db.update_conversation_state(&conversation, &terminal)
                .await
                .unwrap();
            assert_eq!(
                db.product_creation_initial_execution(&request)
                    .await
                    .unwrap(),
                expected
            );
            db.delete_conversation(&conversation).await.unwrap();
            assert_eq!(
                db.product_creation_initial_execution(&request)
                    .await
                    .unwrap(),
                expected
            );
        }
        let (db, request, conversation) = initial_execution_fixture(false).await;
        db.delete_conversation(&conversation).await.unwrap();
        assert_eq!(
            db.product_creation_initial_execution(&request)
                .await
                .unwrap(),
            ProductCreationInitialExecution::Unavailable
        );
    }

    fn delivery_entry(
        delivery: &ClaimedProductCreationJob,
    ) -> phoenix_core::domain::sm_event::SteerEntry {
        phoenix_core::domain::sm_event::SteerEntry {
            text: delivery.job.intent.objective.clone(),
            origin: delivery.job.intent.origin.input_origin(),
            message_id: delivery.job.request_id.clone(),
            llm_text: None,
            images: Vec::new(),
            files: Vec::new(),
            user_agent: None,
            skill_invocation: None,
        }
    }

    #[tokio::test]
    async fn delivery_sink_rejects_every_stale_claim_and_ordinary_bypass_without_side_effects() {
        let db = Database::open_in_memory().await.unwrap();
        let delivery = staged_delivery(&db, "sink-fence").await;
        let conversation_id = delivery.job.published_conversation_id.as_deref().unwrap();
        let entry = delivery_entry(&delivery);
        for field in 0..5 {
            let mut stale = delivery.clone();
            match field {
                0 => stale.claim.generation += 1,
                1 => stale.claim.worker_id.push_str("-wrong"),
                2 => stale.claim.token.push_str("-wrong"),
                3 => stale.job.request_id.push_str("-wrong"),
                _ => stale.job.published_conversation_id = Some("wrong".to_string()),
            }
            assert!(matches!(
                db.append_product_creation_steering_entry(&stale, &entry, "fp")
                    .await,
                Err(DbError::ProductCreationClaimLost)
            ));
        }
        assert!(matches!(
            db.append_steering_entry(conversation_id, &entry, "fp")
                .await,
            Err(DbError::ProductCreationClaimLost)
        ));
        let mut unrelated_entry = entry.clone();
        unrelated_entry.message_id = "ordinary-message-cannot-bypass-staging".to_string();
        assert!(matches!(
            db.append_steering_entry(conversation_id, &unrelated_entry, "fp")
                .await,
            Err(DbError::ProductCreationClaimLost)
        ));
        assert!(matches!(
            db.append_product_creation_steering_entry(&delivery, &unrelated_entry, "fp")
                .await,
            Err(DbError::ProductCreationClaimLost)
        ));
        assert!(matches!(
            db.update_steering_queue(conversation_id, std::slice::from_ref(&entry))
                .await,
            Err(DbError::ProductCreationClaimLost)
        ));
        assert!(matches!(
            db.update_steering_queue(conversation_id, std::slice::from_ref(&unrelated_entry))
                .await,
            Err(DbError::ProductCreationClaimLost)
        ));
        sqlx::query("UPDATE product_creation_jobs SET claim_lease_until_unix_micros = ?1 WHERE request_id = ?2")
            .bind(Utc::now().timestamp_micros()).bind(&delivery.job.request_id)
            .execute(db.pool()).await.unwrap();
        assert!(matches!(
            db.append_product_creation_steering_entry(&delivery, &entry, "fp")
                .await,
            Err(DbError::ProductCreationClaimLost)
        ));
        assert!(db
            .get_steering_queue(conversation_id)
            .await
            .unwrap()
            .is_empty());
        assert!(db
            .get_steering_acceptance_fingerprint(conversation_id, &entry.message_id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn cancellation_before_delivery_fences_sink_and_renewal_then_removes_staged_aggregate() {
        let db = Database::open_in_memory().await.unwrap();
        let delivery = staged_delivery(&db, "cancel-before-sink").await;
        let conversation_id = delivery.job.published_conversation_id.as_deref().unwrap();
        let scope_id = db
            .get_conversation(conversation_id)
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        seed_staged_dependents(&db, conversation_id).await;
        assert_staged_dependents(&db, conversation_id, 1).await;
        assert!(db
            .cancel_product_creation(&delivery.job.request_id, Utc::now())
            .await
            .unwrap());
        let cancelling = db
            .get_product_creation_job(&delivery.job.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelling.status, "cancelling");
        assert!(cancelling.published_product_id.is_none());
        assert!(cancelling.published_conversation_id.is_none());
        assert_eq!(cancelling.claim_generation, delivery.claim.generation + 1);
        assert!(db.get_conversation(conversation_id).await.unwrap().archived);
        assert_staged_dependents(&db, conversation_id, 1).await;
        assert!(db
            .list_ordinary_product_conversation_projections()
            .await
            .unwrap()
            .is_empty());
        for reference in [
            delivery.job.product_conversation_id.as_str(),
            conversation_id,
        ] {
            assert!(matches!(
                db.resolve_ordinary_product_conversation(reference).await,
                Err(DbError::ConversationNotFound(_))
            ));
            assert!(matches!(
                db.read_ordinary_product_conversation_snapshot(reference, None, None, 10)
                    .await,
                Err(DbError::ConversationNotFound(_))
            ));
        }
        assert!(matches!(
            db.get_ordinary_product_conversation(&delivery.job.product_conversation_id)
                .await,
            Err(DbError::ConversationNotFound(_))
        ));
        let mut ordinary_entry = delivery_entry(&delivery);
        ordinary_entry.message_id = "cancelled-aggregate-cannot-accept".to_string();
        assert!(matches!(
            db.append_steering_entry(conversation_id, &ordinary_entry, "fp")
                .await,
            Err(DbError::ProductCreationClaimLost)
        ));
        assert!(matches!(
            db.update_steering_queue(conversation_id, &[ordinary_entry])
                .await,
            Err(DbError::ProductCreationClaimLost)
        ));
        assert!(matches!(
            db.append_product_creation_steering_entry(&delivery, &delivery_entry(&delivery), "fp")
                .await,
            Err(DbError::ProductCreationClaimLost)
        ));
        assert!(!db
            .renew_product_creation_claim(
                &delivery.job.request_id,
                &delivery.claim,
                Utc::now(),
                chrono::Duration::seconds(30)
            )
            .await
            .unwrap());
        assert!(db
            .get_steering_queue(conversation_id)
            .await
            .unwrap()
            .is_empty());
        assert!(db
            .get_steering_acceptance_fingerprint(conversation_id, &delivery.job.request_id)
            .await
            .unwrap()
            .is_none());
        let cleanup = db
            .claim_next_product_creation_cleanup(
                "cleaner",
                "cleanup-token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .finish_product_creation_cleanup(&cleanup, Utc::now())
            .await
            .unwrap());
        let job = db
            .get_product_creation_job(&delivery.job.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, "cancelled");
        assert!(job.published_product_id.is_none());
        assert!(job.published_conversation_id.is_none());
        let remnants: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM conversations WHERE id = ?1),
             (SELECT COUNT(*) FROM product_conversations WHERE id = ?2),
             (SELECT COUNT(*) FROM work_scopes WHERE id = ?3),
             (SELECT COUNT(*) FROM product_conversation_work_scopes
                 WHERE product_conversation_id = ?2 OR work_scope_id = ?3)",
        )
        .bind(conversation_id)
        .bind(delivery.job.product_conversation_id.as_str())
        .bind(scope_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(remnants, (0, 0, 0, 0));
        assert_staged_dependents(&db, conversation_id, 0).await;
    }

    #[tokio::test]
    async fn deleting_a_cancelled_staged_creation_removes_the_hidden_aggregate_too() {
        let db = Database::open_in_memory().await.unwrap();
        let delivery = staged_delivery(&db, "cancel-then-delete").await;
        let conversation_id = delivery.job.published_conversation_id.as_deref().unwrap();
        let scope_id = db
            .get_conversation(conversation_id)
            .await
            .unwrap()
            .attached_work_scope_id
            .unwrap();
        seed_staged_dependents(&db, conversation_id).await;
        assert!(db
            .cancel_product_creation(&delivery.job.request_id, Utc::now())
            .await
            .unwrap());
        assert!(db
            .request_product_creation_deletion(&delivery.job.request_id, Utc::now())
            .await
            .unwrap());
        let cleanup = db
            .claim_next_product_creation_cleanup(
                "cleaner",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cleanup.job.status, "deletion_pending");
        assert!(db
            .finish_product_creation_cleanup(&cleanup, Utc::now())
            .await
            .unwrap());
        assert!(db
            .get_product_creation_job(&delivery.job.request_id)
            .await
            .unwrap()
            .is_none());
        let remnants: (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM conversations WHERE id = ?1),
             (SELECT COUNT(*) FROM product_conversations WHERE id = ?2),
             (SELECT COUNT(*) FROM work_scopes WHERE id = ?3)",
        )
        .bind(conversation_id)
        .bind(delivery.job.product_conversation_id.as_str())
        .bind(scope_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(remnants, (0, 0, 0));
        assert_staged_dependents(&db, conversation_id, 0).await;
    }

    #[tokio::test]
    async fn staged_cancellation_rejects_mismatched_root_and_any_steering_receipt() {
        let db = Database::open_in_memory().await.unwrap();
        let delivery = staged_delivery(&db, "cancel-mismatch").await;
        let conversation_id = delivery.job.published_conversation_id.as_deref().unwrap();
        sqlx::query("UPDATE conversations SET archived = 0 WHERE id = ?1")
            .bind(conversation_id)
            .execute(db.pool())
            .await
            .unwrap();
        assert!(!db
            .cancel_product_creation(&delivery.job.request_id, Utc::now())
            .await
            .unwrap());
        sqlx::query("UPDATE conversations SET archived = 1 WHERE id = ?1")
            .bind(conversation_id)
            .execute(db.pool())
            .await
            .unwrap();
        let wrong_product = ProductConversationId::new();
        sqlx::query(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
            VALUES (?1, 'ordinary', 'open')",
        )
        .bind(wrong_product.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "UPDATE product_creation_jobs SET published_product_id = ?1 WHERE request_id = ?2",
        )
        .bind(wrong_product.as_str())
        .bind(&delivery.job.request_id)
        .execute(db.pool())
        .await
        .unwrap();
        assert!(!db
            .cancel_product_creation(&delivery.job.request_id, Utc::now())
            .await
            .unwrap());
        sqlx::query(
            "UPDATE product_creation_jobs SET published_product_id = ?1 WHERE request_id = ?2",
        )
        .bind(delivery.job.product_conversation_id.as_str())
        .bind(&delivery.job.request_id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO steering_acceptance_receipts (conversation_id, message_id, request_fingerprint)
            VALUES (?1, 'not-the-creation-request', 'fp')")
            .bind(conversation_id).execute(db.pool()).await.unwrap();
        assert!(!db
            .cancel_product_creation(&delivery.job.request_id, Utc::now())
            .await
            .unwrap());
        let unchanged = db
            .get_product_creation_job(&delivery.job.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.status, "delivery_pending");
        assert_eq!(unchanged.claim_generation, delivery.claim.generation);
        assert_eq!(
            unchanged.published_product_id,
            delivery.job.published_product_id
        );
        assert_eq!(
            unchanged.published_conversation_id,
            delivery.job.published_conversation_id
        );
    }

    #[tokio::test]
    async fn staged_failure_and_cancelled_cleanup_ambiguity_fit_existing_constraints() {
        let db = Database::open_in_memory().await.unwrap();
        let delivery = staged_delivery(&db, "staged-failure").await;
        sqlx::query(
            "UPDATE product_creation_jobs SET delivery_attempt_count = ?1 WHERE request_id = ?2",
        )
        .bind(PRODUCT_CREATION_MAX_DELIVERY_ATTEMPTS)
        .bind(&delivery.job.request_id)
        .execute(db.pool())
        .await
        .unwrap();
        assert!(db
            .schedule_product_creation_delivery_retry(
                &delivery.job.request_id,
                &delivery.claim,
                "injected delivery failure",
                Utc::now()
            )
            .await
            .unwrap());
        let failed = db
            .get_product_creation_job(&delivery.job.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.status, "delivery_failed");
        assert_eq!(
            failed.published_product_id,
            delivery.job.published_product_id
        );
        assert_eq!(
            failed.published_conversation_id,
            delivery.job.published_conversation_id
        );
        assert!(db
            .list_ordinary_product_conversation_projections()
            .await
            .unwrap()
            .is_empty());

        let delivery = staged_delivery(&db, "staged-ambiguity").await;
        let conversation_id = delivery.job.published_conversation_id.as_deref().unwrap();
        assert!(db
            .cancel_product_creation(&delivery.job.request_id, Utc::now())
            .await
            .unwrap());
        let cleanup = db
            .claim_next_product_creation_cleanup(
                "cleaner",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .mark_claimed_product_creation_cleanup_ambiguous(
                &cleanup,
                "ownership conflict",
                Utc::now()
            )
            .await
            .unwrap());
        let ambiguous = db
            .get_product_creation_job(&delivery.job.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ambiguous.status, "cleanup_ambiguous");
        assert!(ambiguous.published_product_id.is_none());
        assert!(ambiguous.published_conversation_id.is_none());
        assert!(db.get_conversation(conversation_id).await.unwrap().archived);
        assert!(db
            .list_ordinary_product_conversation_projections()
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn failed_enqueue_rolls_back_receipt_so_cancellation_can_still_win() {
        let db = Database::open_in_memory().await.unwrap();
        let delivery = staged_delivery(&db, "sink-rollback").await;
        let conversation_id = delivery.job.published_conversation_id.as_deref().unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_creation_enqueue BEFORE INSERT ON steering_messages
            BEGIN SELECT RAISE(ABORT, 'injected enqueue failure'); END",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert!(matches!(
            db.append_product_creation_steering_entry(&delivery, &delivery_entry(&delivery), "fp")
                .await,
            Err(DbError::Sqlx(_))
        ));
        assert!(db
            .get_steering_queue(conversation_id)
            .await
            .unwrap()
            .is_empty());
        assert!(db
            .get_steering_acceptance_fingerprint(conversation_id, &delivery.job.request_id)
            .await
            .unwrap()
            .is_none());
        assert!(db
            .cancel_product_creation(&delivery.job.request_id, Utc::now())
            .await
            .unwrap());
        assert_eq!(
            db.get_product_creation_job(&delivery.job.request_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "cancelling"
        );
    }

    #[tokio::test]
    async fn delivery_receipt_defeats_cancellation_even_after_queue_drain() {
        let db = Database::open_in_memory().await.unwrap();
        let delivery = staged_delivery(&db, "sink-before-cancel").await;
        let conversation_id = delivery.job.published_conversation_id.as_deref().unwrap();
        assert_eq!(
            db.append_product_creation_steering_entry(&delivery, &delivery_entry(&delivery), "fp")
                .await
                .unwrap(),
            0
        );
        assert!(db
            .remove_steering_entry(conversation_id, &delivery.job.request_id)
            .await
            .unwrap());
        assert!(!db
            .cancel_product_creation(&delivery.job.request_id, Utc::now())
            .await
            .unwrap());
        assert_eq!(
            db.get_product_creation_job(&delivery.job.request_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "delivery_pending"
        );
    }

    #[tokio::test]
    async fn file_backed_cancellation_and_sink_have_exactly_one_winner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory
            .path()
            .join("delivery-race.db")
            .to_string_lossy()
            .to_string();
        let db = Database::open(&path).await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();
        let peer = Database::open(&path).await.unwrap();
        for index in 0..4 {
            let request_id = format!("sink-race-{index}");
            let delivery = staged_delivery(&db, &request_id).await;
            let entry = delivery_entry(&delivery);
            let (cancelled, enqueued) = tokio::join!(
                peer.cancel_product_creation(&request_id, Utc::now()),
                db.append_product_creation_steering_entry(&delivery, &entry, "fp"),
            );
            let cancelled = cancelled.unwrap();
            match enqueued {
                Ok(0) => assert!(!cancelled),
                Err(DbError::ProductCreationClaimLost) => assert!(cancelled),
                other => panic!("unexpected sink race outcome: {other:?}"),
            }
            let conversation_id = delivery.job.published_conversation_id.as_deref().unwrap();
            assert_eq!(
                db.get_steering_queue(conversation_id).await.unwrap().len(),
                usize::from(!cancelled)
            );
            assert_eq!(
                db.get_steering_acceptance_fingerprint(conversation_id, &request_id)
                    .await
                    .unwrap()
                    .is_some(),
                !cancelled
            );
            let job = db
                .get_product_creation_job(&request_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(job.published_product_id.is_none(), cancelled);
            assert_eq!(job.published_conversation_id.is_none(), cancelled);
        }
    }

    #[tokio::test]
    async fn staged_reserved_identity_is_unreadable_until_delivery_completes() {
        let db = Database::open_in_memory().await.unwrap();
        let delivery = staged_delivery(&db, "hidden-staged").await;
        let product_id = &delivery.job.product_conversation_id;
        let conversation_id = delivery.job.published_conversation_id.as_deref().unwrap();
        seed_staged_dependents(&db, conversation_id).await;
        assert!(db
            .list_archived_conversations()
            .await
            .unwrap()
            .iter()
            .all(|conversation| conversation.id != conversation_id));
        assert!(!db
            .list_conversation_search_ids()
            .await
            .unwrap()
            .contains(&conversation_id.to_string()));
        assert!(!db
            .get_conversation_search_metadata(&[conversation_id.to_string()])
            .await
            .unwrap()
            .contains_key(conversation_id));
        for reference in [product_id.as_str(), conversation_id] {
            assert!(matches!(
                db.resolve_ordinary_product_conversation(reference).await,
                Err(DbError::ConversationNotFound(_))
            ));
            assert!(matches!(
                db.read_ordinary_product_conversation_snapshot(reference, None, None, 10)
                    .await,
                Err(DbError::ConversationNotFound(_))
            ));
        }
        assert!(matches!(
            db.get_ordinary_product_conversation(product_id).await,
            Err(DbError::ConversationNotFound(_))
        ));
        let fingerprint = format!("product-create:{}", delivery.job.request_id);
        db.append_product_creation_steering_entry(
            &delivery,
            &delivery_entry(&delivery),
            &fingerprint,
        )
        .await
        .unwrap();
        assert!(db
            .complete_product_creation_delivery(
                &delivery.job.request_id,
                &delivery.claim,
                &SteeringAcceptanceFingerprint::Exact(fingerprint),
                &delivery.job.intent.live_intent().unwrap()
            )
            .await
            .unwrap());
        assert!(db
            .resolve_ordinary_product_conversation(product_id.as_str())
            .await
            .is_ok());
        assert!(db
            .get_ordinary_product_conversation(product_id)
            .await
            .is_ok());
        assert!(db
            .read_ordinary_product_conversation_snapshot(product_id.as_str(), None, None, 10)
            .await
            .is_ok());
        assert!(db
            .list_conversation_search_ids()
            .await
            .unwrap()
            .contains(&conversation_id.to_string()));
        assert!(db
            .get_conversation_search_metadata(&[conversation_id.to_string()])
            .await
            .unwrap()
            .contains_key(conversation_id));
    }

    #[tokio::test]
    async fn staged_cancellation_cleanup_fails_closed_and_rolls_back_partial_deletion() {
        let db = Database::open_in_memory().await.unwrap();
        let delivery = staged_delivery(&db, "cleanup-rollback").await;
        let conversation_id = delivery.job.published_conversation_id.as_deref().unwrap();
        seed_staged_dependents(&db, conversation_id).await;
        assert!(db
            .cancel_product_creation(&delivery.job.request_id, Utc::now())
            .await
            .unwrap());
        let cleanup = db
            .claim_next_product_creation_cleanup(
                "cleaner",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let mut mismatched = cleanup.clone();
        mismatched.job.published_conversation_id = Some("another-conversation".to_string());
        assert!(db
            .finish_product_creation_cleanup(&mismatched, Utc::now())
            .await
            .is_err());
        mismatched = cleanup.clone();
        mismatched.job.product_conversation_id = ProductConversationId::new();
        assert!(db
            .finish_product_creation_cleanup(&mismatched, Utc::now())
            .await
            .is_err());
        sqlx::query("UPDATE conversations SET archived = 0 WHERE id = ?1")
            .bind(conversation_id)
            .execute(db.pool())
            .await
            .unwrap();
        assert!(db
            .finish_product_creation_cleanup(&cleanup, Utc::now())
            .await
            .is_err());
        sqlx::query("UPDATE conversations SET archived = 1 WHERE id = ?1")
            .bind(conversation_id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO steering_acceptance_receipts (conversation_id, message_id, request_fingerprint)
            VALUES (?1, ?2, 'fp')").bind(conversation_id).bind(&delivery.job.request_id)
            .execute(db.pool()).await.unwrap();
        assert!(db
            .finish_product_creation_cleanup(&cleanup, Utc::now())
            .await
            .is_err());
        sqlx::query("DELETE FROM steering_acceptance_receipts WHERE conversation_id = ?1")
            .bind(conversation_id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "CREATE TRIGGER refuse_staged_scope_delete BEFORE DELETE ON work_scopes
            BEGIN SELECT RAISE(ABORT, 'injected cleanup failure'); END",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert!(db
            .finish_product_creation_cleanup(&cleanup, Utc::now())
            .await
            .is_err());
        let pending = db
            .get_product_creation_job(&delivery.job.request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.status, "cancelling");
        assert!(pending.published_product_id.is_none());
        assert!(pending.published_conversation_id.is_none());
        assert!(db.get_conversation(conversation_id).await.unwrap().archived);
        assert_staged_dependents(&db, conversation_id, 1).await;
        sqlx::query("DROP TRIGGER refuse_staged_scope_delete")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(db
            .finish_product_creation_cleanup(&cleanup, Utc::now())
            .await
            .unwrap());
        assert_staged_dependents(&db, conversation_id, 0).await;
    }

    #[tokio::test]
    async fn accepted_jobs_and_expired_claims_have_durable_scheduler_deadlines() {
        let db = Database::open_in_memory().await.unwrap();
        assert!(db.next_product_creation_deadline().await.unwrap().is_none());
        db.accept_product_creation("deadline-request", &intent("/repo/deadline", "inspect"))
            .await
            .unwrap();
        let accepted = db
            .get_product_creation_job("deadline-request")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            db.next_product_creation_deadline().await.unwrap(),
            Some(accepted.accepted_at)
        );
        let expired_at = DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap()
            - chrono::Duration::seconds(1);
        let old = db
            .claim_product_creation(
                "deadline-request",
                "old-worker",
                "old-token",
                expired_at - chrono::Duration::seconds(30),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            db.next_product_creation_deadline().await.unwrap(),
            Some(expired_at)
        );
        let recovered = db
            .claim_next_product_creation(
                "recovery-worker",
                "recovery-token",
                Utc::now(),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered.job.request_id, accepted.request_id);
        assert!(recovered.claim.generation > old.claim.generation);
        assert_eq!(
            recovered.job.product_conversation_id,
            accepted.product_conversation_id
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn provisioning_stage_is_relational_claim_fenced_and_retained_on_failure() {
        use ProductCreationProvisioningStage as Stage;
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("stage-request", &intent("/repo/stage", "inspect"))
            .await
            .unwrap();
        let accepted = db
            .get_product_creation_job("stage-request")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            accepted.provisioning_stage,
            ProductCreationProvisioningStage::ValidateIntent
        );
        assert_eq!(accepted.stage_updated_at, accepted.accepted_at);
        let now = DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap();
        let claimed = db
            .claim_product_creation(
                "stage-request",
                "worker",
                "token",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        let mut stale = claimed.claim.clone();
        stale.generation += 1;
        assert!(!db
            .advance_product_creation_provisioning_stage(
                "stage-request",
                &stale,
                ProductCreationProvisioningStage::ResolveRepository,
            )
            .await
            .unwrap());
        for stage in [Stage::ResolveStartingPin, Stage::CommitMetadata] {
            assert!(!db
                .advance_product_creation_provisioning_stage("stage-request", &claimed.claim, stage)
                .await
                .unwrap());
        }
        assert_eq!(
            db.get_product_creation_job("stage-request")
                .await
                .unwrap()
                .unwrap(),
            claimed.job,
        );
        for stage in [Stage::ResolveRepository, Stage::ResolveStartingPin] {
            assert!(db
                .advance_product_creation_provisioning_stage("stage-request", &claimed.claim, stage)
                .await
                .unwrap());
        }
        let advanced = db
            .get_product_creation_job("stage-request")
            .await
            .unwrap()
            .unwrap();
        assert!(advanced.stage_updated_at >= accepted.stage_updated_at);
        assert!(!db
            .advance_product_creation_provisioning_stage(
                "stage-request",
                &claimed.claim,
                Stage::ResolveRepository,
            )
            .await
            .unwrap());
        assert!(db
            .advance_product_creation_provisioning_stage(
                "stage-request",
                &claimed.claim,
                Stage::ResolveStartingPin,
            )
            .await
            .unwrap());
        assert_eq!(
            db.get_product_creation_job("stage-request")
                .await
                .unwrap()
                .unwrap(),
            advanced,
        );
        assert!(db
            .schedule_product_creation_retry("stage-request", &claimed.claim, "fetch failed", now)
            .await
            .unwrap());
        let failed = db
            .get_product_creation_job("stage-request")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            failed.provisioning_stage,
            ProductCreationProvisioningStage::ResolveStartingPin
        );
        assert_eq!(failed.stage_updated_at, advanced.stage_updated_at);
        assert_eq!(failed.last_error.as_deref(), Some("fetch failed"));
        for statement in [
            "UPDATE product_creation_jobs SET provisioning_stage = 'invented' WHERE request_id = 'stage-request'",
            "UPDATE product_creation_jobs SET provisioning_stage = NULL WHERE request_id = 'stage-request'",
            "UPDATE product_creation_jobs SET stage_updated_at_unix_micros = 'not-a-timestamp' WHERE request_id = 'stage-request'",
            "UPDATE product_creation_jobs SET stage_updated_at_unix_micros = -1 WHERE request_id = 'stage-request'",
        ] {
            assert!(sqlx::query(statement).execute(db.pool()).await.is_err());
        }
    }

    #[tokio::test]
    async fn stage_advancement_rejects_expired_unreplaced_claim_and_future_stage_timestamp() {
        use ProductCreationProvisioningStage as Stage;
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("stage-clock", &intent("/repo/stage", "inspect"))
            .await
            .unwrap();
        let now = Utc::now();
        let claimed = db
            .claim_product_creation(
                "stage-clock",
                "worker",
                "token",
                now,
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let future = now + chrono::Duration::minutes(1);
        sqlx::query(
            "UPDATE product_creation_jobs SET stage_updated_at_unix_micros = ?1
             WHERE request_id = 'stage-clock'",
        )
        .bind(future.timestamp_micros())
        .execute(db.pool())
        .await
        .unwrap();
        let before = db
            .get_product_creation_job("stage-clock")
            .await
            .unwrap()
            .unwrap();
        assert!(!db
            .advance_product_creation_provisioning_stage(
                "stage-clock",
                &claimed.claim,
                Stage::ResolveRepository
            )
            .await
            .unwrap());
        assert_eq!(
            db.get_product_creation_job("stage-clock")
                .await
                .unwrap()
                .unwrap(),
            before
        );
        sqlx::query(
            "UPDATE product_creation_jobs SET stage_updated_at_unix_micros = ?1,
                 claim_lease_until_unix_micros = ?2 WHERE request_id = 'stage-clock'",
        )
        .bind(claimed.job.stage_updated_at.timestamp_micros())
        .bind((now - chrono::Duration::seconds(1)).timestamp_micros())
        .execute(db.pool())
        .await
        .unwrap();
        let expired = db
            .get_product_creation_job("stage-clock")
            .await
            .unwrap()
            .unwrap();
        for stage in [Stage::ValidateIntent, Stage::ResolveRepository] {
            assert!(!db
                .advance_product_creation_provisioning_stage("stage-clock", &claimed.claim, stage)
                .await
                .unwrap());
        }
        assert_eq!(
            db.get_product_creation_job("stage-clock")
                .await
                .unwrap()
                .unwrap(),
            expired
        );
        assert_eq!(expired.claim_generation, claimed.claim.generation);
    }

    #[tokio::test]
    async fn publication_allocates_duplicate_human_names_without_publishing_input_slugs() {
        let db = Database::open_in_memory().await.unwrap();
        for (request, expected) in [("name-one", "über-東京"), ("name-two", "über-東京-2")] {
            db.accept_product_creation(request, &intent("/tmp", "name projection"))
                .await
                .unwrap();
            let claimed = db
                .claim_product_creation(
                    request,
                    "worker",
                    "token",
                    Utc::now(),
                    chrono::Duration::minutes(5),
                )
                .await
                .unwrap()
                .unwrap();
            advance_to_metadata(&db, request, &claimed.claim).await;
            let mut conversation = published_conversation(
                request,
                claimed.job.product_conversation_id.clone(),
                WorkScopeId::new(),
                "/tmp",
            );
            conversation.title = Some("  Über 東京  ".to_string());
            conversation.slug = Some("unrelated-input-slug".to_string());
            let published = db
                .publish_product_creation_atomically(&ProductCreationPublishInput {
                    request_id: request.to_string(),
                    claim: claimed.claim,
                    conversation,
                    authority_kind: AuthorityKind::Direct,
                    environment: EnvironmentContext::UnownedCwd {
                        cwd: "/tmp".to_string(),
                    },
                    git_publication: None,
                })
                .await
                .unwrap();
            assert!(published);
            let stored = db.get_conversation(request).await.unwrap();
            assert_eq!(stored.title.as_deref(), Some("Über 東京"));
            assert_eq!(stored.slug.as_deref(), Some(expected));
            assert_eq!(
                stored.product_conversation_id,
                claimed.job.product_conversation_id
            );
        }
    }

    #[tokio::test]
    async fn pending_product_identity_is_reserved_from_conversation_routes_until_publication() {
        let db = Database::open_in_memory().await.unwrap();
        let existing = db
            .create_conversation("existing-route", "existing", "/tmp", true, None, None)
            .await
            .unwrap();
        let accepted = db
            .accept_product_creation("reserved-route", &intent("/tmp", "reserve route"))
            .await
            .unwrap();
        let ProductCreationAcceptOutcome::Accepted(accepted) = accepted else {
            panic!("new creation should be accepted");
        };
        let reserved_slug = accepted.product_conversation_id.to_string();

        db.set_ordinary_product_conversation_title(&existing.id, &reserved_slug)
            .await
            .unwrap();
        let renamed = db.get_conversation(&existing.id).await.unwrap();
        let expected_existing_slug = format!("{reserved_slug}-2");
        assert_eq!(renamed.title.as_deref(), Some(reserved_slug.as_str()));
        assert_eq!(
            renamed.slug.as_deref(),
            Some(expected_existing_slug.as_str())
        );

        let claimed = db
            .claim_product_creation(
                "reserved-route",
                "worker",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        advance_to_metadata(&db, "reserved-route", &claimed.claim).await;
        let scope = WorkScopeId::new();
        let published = db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "reserved-route".to_string(),
                claim: claimed.claim,
                conversation: published_conversation(
                    "reserved-conversation",
                    accepted.product_conversation_id.clone(),
                    scope.clone(),
                    "/tmp",
                ),
                authority_kind: AuthorityKind::Direct,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/tmp".to_string(),
                },
                git_publication: None,
            })
            .await
            .unwrap();
        assert!(published);

        let existing_after = db.get_conversation(&existing.id).await.unwrap();
        assert_eq!(
            existing_after.slug.as_deref(),
            Some(expected_existing_slug.as_str())
        );
        assert_eq!(
            db.resolve_ordinary_product_conversation(&expected_existing_slug)
                .await
                .unwrap()
                .product_conversation_id,
            existing.product_conversation_id
        );
        assert!(db
            .resolve_ordinary_product_conversation(&reserved_slug)
            .await
            .is_err());
        sqlx::query(
            "UPDATE product_creation_jobs SET status = 'published',
                 claim_worker_id = NULL, claim_token = NULL, claim_lease_until_unix_micros = NULL,
                 delivery_retry_at_unix_micros = NULL, provisioning_stage = 'finalize_publication'
             WHERE request_id = 'reserved-route' AND status = 'delivery_pending'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert_eq!(
            db.resolve_ordinary_product_conversation(&reserved_slug)
                .await
                .unwrap()
                .product_conversation_id,
            accepted.product_conversation_id
        );
        let owner: (String, String) = sqlx::query_as(
            "SELECT product_conversation_id, work_scope_id FROM product_conversation_work_scopes WHERE product_conversation_id = ?1",
        )
        .bind(accepted.product_conversation_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(owner.0, accepted.product_conversation_id.to_string());
        assert_eq!(owner.1, scope.to_string());
    }

    #[tokio::test]
    async fn publication_requires_metadata_predecessor_and_monotonic_stage_timestamp() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("stage-publish", &intent("/repo/direct", "inspect"))
            .await
            .unwrap();
        let claimed = db
            .claim_product_creation(
                "stage-publish",
                "worker",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let input = ProductCreationPublishInput {
            request_id: "stage-publish".to_string(),
            claim: claimed.claim.clone(),
            conversation: published_conversation(
                "stage-published-conversation",
                claimed.job.product_conversation_id.clone(),
                WorkScopeId::new(),
                "/repo/direct",
            ),
            authority_kind: AuthorityKind::Direct,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/direct".to_string(),
            },
            git_publication: None,
        };
        assert!(!db
            .publish_product_creation_atomically(&input)
            .await
            .unwrap());
        assert_eq!(
            db.get_product_creation_job("stage-publish")
                .await
                .unwrap()
                .unwrap(),
            claimed.job
        );
        advance_to_metadata(&db, "stage-publish", &claimed.claim).await;
        assert!(!db
            .advance_product_creation_provisioning_stage(
                "stage-publish",
                &claimed.claim,
                ProductCreationProvisioningStage::BootstrapInitialTurn,
            )
            .await
            .unwrap());
        let ready = db
            .get_product_creation_job("stage-publish")
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "UPDATE product_creation_jobs SET stage_updated_at_unix_micros = ?1
             WHERE request_id = 'stage-publish'",
        )
        .bind((Utc::now() + chrono::Duration::minutes(1)).timestamp_micros())
        .execute(db.pool())
        .await
        .unwrap();
        assert!(!db
            .publish_product_creation_atomically(&input)
            .await
            .unwrap());
        sqlx::query(
            "UPDATE product_creation_jobs SET stage_updated_at_unix_micros = ?1
             WHERE request_id = 'stage-publish'",
        )
        .bind(ready.stage_updated_at.timestamp_micros())
        .execute(db.pool())
        .await
        .unwrap();
        assert!(db
            .publish_product_creation_atomically(&input)
            .await
            .unwrap());
        let published = db
            .get_product_creation_job("stage-publish")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            published.provisioning_stage,
            ProductCreationProvisioningStage::BootstrapInitialTurn
        );
        assert!(published.stage_updated_at >= ready.stage_updated_at);
        assert!(!db
            .advance_product_creation_provisioning_stage(
                "stage-publish",
                &claimed.claim,
                ProductCreationProvisioningStage::FinalizePublication,
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn replaced_or_cancelled_claim_cannot_publish_late_results_or_advance_stage() {
        let db = Database::open_in_memory().await.unwrap();
        for cancelled in [false, true] {
            let request_id = if cancelled {
                "cancelled-late"
            } else {
                "replaced-late"
            };
            db.accept_product_creation(request_id, &intent("/repo/late", "inspect"))
                .await
                .unwrap();
            let now = DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap();
            let old = db
                .claim_product_creation(
                    request_id,
                    "old-worker",
                    "old-token",
                    now - chrono::Duration::seconds(60),
                    chrono::Duration::seconds(30),
                )
                .await
                .unwrap()
                .unwrap();
            let current = db
                .claim_product_creation(
                    request_id,
                    "current-worker",
                    "current-token",
                    now,
                    chrono::Duration::seconds(30),
                )
                .await
                .unwrap()
                .unwrap();
            if cancelled {
                assert!(db.cancel_product_creation(request_id, now).await.unwrap());
            }
            let claim = if cancelled {
                &current.claim
            } else {
                &old.claim
            };
            let before = db
                .get_product_creation_job(request_id)
                .await
                .unwrap()
                .unwrap();
            assert!(!db
                .advance_product_creation_provisioning_stage(
                    request_id,
                    claim,
                    ProductCreationProvisioningStage::ResolveRepository,
                )
                .await
                .unwrap());
            let scope = WorkScopeId::new();
            let input = ProductCreationPublishInput {
                request_id: request_id.to_string(),
                claim: claim.clone(),
                conversation: published_conversation(
                    request_id,
                    before.product_conversation_id.clone(),
                    scope.clone(),
                    "/repo/late",
                ),
                authority_kind: AuthorityKind::Direct,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/late".to_string(),
                },
                git_publication: None,
            };
            assert!(!db
                .publish_product_creation_atomically(&input)
                .await
                .unwrap());
            assert_eq!(
                db.get_product_creation_job(request_id)
                    .await
                    .unwrap()
                    .unwrap(),
                before
            );
            let conversation_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE id = ?1")
                    .bind(request_id)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(conversation_count, 0, "late result inserted a conversation");
            let scope_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM work_scopes WHERE id = ?1")
                    .bind(scope.as_str())
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(scope_count, 0, "late result inserted a work scope");
        }
    }

    #[tokio::test]
    async fn creation_origin_survives_storage_and_historical_replay() {
        let db = Database::open_in_memory().await.unwrap();
        let api = intent("/repo/origin", "objective");
        db.accept_product_creation("origin-request", &api)
            .await
            .unwrap();
        assert_eq!(
            db.get_product_creation_job("origin-request")
                .await
                .unwrap()
                .unwrap()
                .intent
                .origin,
            ProductCreationOrigin::UserApi
        );
        sqlx::query("UPDATE product_creation_jobs SET objective_origin = 'unknown_historical' WHERE request_id = 'origin-request'").execute(db.pool()).await.unwrap();
        let replay = db
            .accept_product_creation("origin-request", &api)
            .await
            .unwrap();
        let ProductCreationAcceptOutcome::Replayed(job) = replay else {
            panic!("expected historical replay")
        };
        assert_eq!(job.intent.origin, ProductCreationOrigin::UnknownHistorical);
        assert_eq!(
            job.intent.origin.input_origin(),
            phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical
        );
    }

    #[tokio::test]
    async fn internal_creation_replay_accepts_new_locator_and_retains_first_source() {
        use phoenix_core::domain::db_schema::SourceToolCall;
        let db = Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let product_conversation_id = coordinator.product_conversation_id;
        let mut accepted_intent = intent("/repo/origin", "objective");
        accepted_intent.origin = ProductCreationOrigin::InternalConversation {
            product_conversation_id: product_conversation_id.clone(),
            transcript_id: coordinator.id.clone(),
            source_call: SourceToolCall {
                message_id: "message-a".to_string(),
                tool_use_id: "call-a".to_string(),
            },
        };
        db.accept_product_creation_resolved("internal-origin", Some(&coordinator.id), |_| {
            Ok(accepted_intent.clone())
        })
        .await
        .unwrap();

        let mut replay_intent = accepted_intent.clone();
        replay_intent.origin = ProductCreationOrigin::InternalConversation {
            product_conversation_id,
            transcript_id: coordinator.id.clone(),
            source_call: SourceToolCall {
                message_id: "message-b".to_string(),
                tool_use_id: "call-b".to_string(),
            },
        };
        let ProductCreationAcceptOutcome::Replayed(job) = db
            .accept_product_creation_resolved("internal-origin", Some(&coordinator.id), |_| {
                Ok(replay_intent.clone())
            })
            .await
            .unwrap()
        else {
            panic!("expected same-owner replay")
        };
        assert_eq!(job.intent.origin, accepted_intent.origin);
    }

    #[tokio::test]
    async fn internal_creation_requires_trusted_current_coordinator_admission() {
        use phoenix_core::domain::db_schema::SourceToolCall;
        let db = Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let mut candidate = intent("/repo/origin", "objective");
        candidate.origin = ProductCreationOrigin::InternalConversation {
            product_conversation_id: coordinator.product_conversation_id,
            transcript_id: coordinator.id.clone(),
            source_call: SourceToolCall {
                message_id: "message".into(),
                tool_use_id: "call".into(),
            },
        };
        assert!(matches!(
            db.accept_product_creation("untrusted-origin", &candidate)
                .await,
            Err(DbError::CurrentCoordinatorRequired)
        ));
        assert!(matches!(
            db.accept_product_creation_resolved("wrong-head-origin", Some("not-current"), |_| Ok(
                candidate.clone()
            ))
            .await,
            Err(DbError::CurrentCoordinatorRequired)
        ));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM product_creation_jobs WHERE request_id IN ('untrusted-origin', 'wrong-head-origin')")
            .fetch_one(db.pool()).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn resolved_acceptance_uses_existing_job_before_candidate_validation() {
        let db = Database::open_in_memory().await.unwrap();
        let original = intent("/repo/origin", "objective");
        db.accept_product_creation("resolved-winner", &original)
            .await
            .unwrap();
        let mut resolver_saw_job = false;
        let result = db
            .accept_product_creation_resolved("resolved-winner", None, |existing| {
                let ProductCreationResolution::Existing(existing) = existing else {
                    panic!("durable winner is provided to resolver");
                };
                resolver_saw_job = true;
                let mut unvalidated = existing.intent.live_intent().unwrap();
                unvalidated.cwd.clear();
                Ok(unvalidated)
            })
            .await
            .unwrap();
        assert!(resolver_saw_job);
        assert!(matches!(result, ProductCreationAcceptOutcome::Conflict(_)));
    }

    #[tokio::test]
    async fn product_creation_accept_replay_and_conflict() {
        let db = Database::open_in_memory().await.unwrap();
        let first = db
            .accept_product_creation("req-1", &intent("/repo/a", "ship it"))
            .await
            .unwrap();
        assert!(matches!(first, ProductCreationAcceptOutcome::Accepted(_)));
        let replay = db
            .accept_product_creation("req-1", &intent("/repo/a", "ship it"))
            .await
            .unwrap();
        assert!(matches!(replay, ProductCreationAcceptOutcome::Replayed(_)));
        let conflict = db
            .accept_product_creation("req-1", &intent("/repo/a", "different"))
            .await
            .unwrap();
        assert!(matches!(
            conflict,
            ProductCreationAcceptOutcome::Conflict(_)
        ));
    }

    #[tokio::test]
    async fn product_creation_accept_allocates_server_owned_product_id_and_allows_image_only() {
        let db = Database::open_in_memory().await.unwrap();
        let image_only = ProductCreationIntent {
            origin: ProductCreationOrigin::UserApi,
            cwd: "/repo/img".to_string(),
            objective: "   ".to_string(),
            model: None,
            effort: None,
            images: vec![ProductCreationImage {
                media_type: "image/png".to_string(),
                data: "abc123".to_string(),
            }],
            llm_language: LlmLanguage::Caveman,
        };
        let accepted = match db
            .accept_product_creation("req-img", &image_only)
            .await
            .unwrap()
        {
            ProductCreationAcceptOutcome::Accepted(job) => job,
            other @ (ProductCreationAcceptOutcome::Replayed(_)
            | ProductCreationAcceptOutcome::Conflict(_)) => {
                panic!("expected accepted, got {other:?}")
            }
        };
        assert_eq!(accepted.intent, image_only);
        assert_eq!(accepted.attempt_count, 1);
        assert!(!accepted.product_conversation_id.as_str().is_empty());

        let replay = match db
            .accept_product_creation("req-img", &image_only)
            .await
            .unwrap()
        {
            ProductCreationAcceptOutcome::Replayed(job) => job,
            other @ (ProductCreationAcceptOutcome::Accepted(_)
            | ProductCreationAcceptOutcome::Conflict(_)) => {
                panic!("expected replay, got {other:?}")
            }
        };
        assert_eq!(
            replay.product_conversation_id,
            accepted.product_conversation_id
        );

        let err = db
            .accept_product_creation(
                "req-empty",
                &ProductCreationIntent {
                    origin: ProductCreationOrigin::UserApi,
                    cwd: "/repo/empty".to_string(),
                    objective: " ".to_string(),
                    model: None,
                    effort: None,
                    images: vec![],
                    llm_language: LlmLanguage::Caveman,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::Serialization(_)));
    }

    #[tokio::test]
    async fn product_creation_initial_objective_replay_classification_respects_fingerprint_authority(
    ) {
        let db = Database::open_in_memory().await.unwrap();
        let accepted_intent = intent("/repo/a", "ship it");
        db.accept_product_creation("req-replay", &accepted_intent)
            .await
            .unwrap();
        assert!(db
            .product_creation_objective_already_durably_accepted(
                "req-replay",
                Some(&SteeringAcceptanceFingerprint::Exact(
                    "product-create:req-replay".to_string(),
                )),
                &accepted_intent,
            )
            .await
            .unwrap());
        assert!(!db
            .product_creation_objective_already_durably_accepted(
                "req-replay",
                Some(&SteeringAcceptanceFingerprint::LegacyUnknown),
                &accepted_intent,
            )
            .await
            .unwrap());
        assert!(!db
            .product_creation_objective_already_durably_accepted(
                "req-replay",
                Some(&SteeringAcceptanceFingerprint::Exact(
                    "different-payload".to_string(),
                )),
                &accepted_intent,
            )
            .await
            .unwrap());
        assert!(!db
            .product_creation_objective_already_durably_accepted(
                "req-replay",
                Some(&SteeringAcceptanceFingerprint::Exact(
                    "product-create:req-replay".to_string(),
                )),
                &intent("/repo/a", "different"),
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn product_creation_retry_schedule_is_bounded_to_2_10_30_then_failed() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-retry", &intent("/repo/a", "retry"))
            .await
            .unwrap();
        let now = Utc::now();
        let mut claim = db
            .claim_next_product_creation("worker", "token-1", now, chrono::Duration::minutes(5))
            .await
            .unwrap()
            .unwrap();
        for (expected_attempt_count, expected_delay_secs) in [(2, 2_i64), (3, 10), (4, 30)] {
            assert!(db
                .schedule_product_creation_retry(
                    "req-retry",
                    &claim.claim,
                    "temporary failure",
                    now
                )
                .await
                .unwrap());
            let job = db
                .get_product_creation_job("req-retry")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(job.status, "retry_scheduled");
            assert_eq!(job.attempt_count, expected_attempt_count);
            let retry_at_micros = datetime_to_unix_micros(job.retry_at.unwrap());
            assert_eq!(
                retry_at_micros - datetime_to_unix_micros(now),
                expected_delay_secs * 1_000_000
            );
            claim = db
                .claim_product_creation(
                    "req-retry",
                    "worker",
                    &format!("reclaim-{expected_attempt_count}"),
                    job.retry_at.unwrap(),
                    chrono::Duration::minutes(5),
                )
                .await
                .unwrap()
                .unwrap();
        }
        let final_claim = claim;
        assert!(db
            .schedule_product_creation_retry("req-retry", &final_claim.claim, "final failure", now)
            .await
            .unwrap());
        let final_job = db
            .get_product_creation_job("req-retry")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(final_job.status, "failed");
        assert_eq!(final_job.attempt_count, 4);
        assert!(final_job.retry_at.is_none());
        assert_eq!(final_job.last_error.as_deref(), Some("final failure"));
    }

    #[tokio::test]
    async fn product_creation_retry_uses_attempt_count_not_claim_generation() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-attempts", &intent("/repo/a", "retry"))
            .await
            .unwrap();
        let now = Utc::now();
        let claim = db
            .claim_next_product_creation("worker", "token", now, chrono::Duration::seconds(30))
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "UPDATE product_creation_jobs SET claim_generation = claim_generation + 5 WHERE request_id = 'req-attempts'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let mut claim = claim;
        claim.claim.generation += 5;
        let current = db
            .get_product_creation_job("req-attempts")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.attempt_count, 1);
        assert!(claim.claim.generation > current.attempt_count);
        assert!(db
            .schedule_product_creation_retry(
                "req-attempts",
                &claim.claim,
                "retry after generation drift",
                now
            )
            .await
            .unwrap());
        let scheduled = db
            .get_product_creation_job("req-attempts")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(scheduled.attempt_count, 2);
        assert_eq!(
            scheduled.retry_at.map(datetime_to_unix_micros),
            Some(datetime_to_unix_micros(now) + 2_000_000)
        );
    }

    #[tokio::test]
    async fn cleanup_ambiguous_requires_a_live_claim() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-cleanup-lease", &intent("/repo/a", "cleanup"))
            .await
            .unwrap();
        let now = Utc::now();
        let claimed = db
            .claim_product_creation(
                "req-cleanup-lease",
                "worker",
                "token",
                now,
                chrono::Duration::seconds(1),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(!db
            .mark_product_creation_cleanup_ambiguous(
                "req-cleanup-lease",
                &claimed.claim,
                "cleanup reason",
                now + chrono::Duration::seconds(1),
            )
            .await
            .unwrap());
        assert_eq!(
            db.get_product_creation_job("req-cleanup-lease")
                .await
                .unwrap()
                .unwrap()
                .status,
            "claimed"
        );

        db.accept_product_creation("req-cleanup-reason", &intent("/repo/a", "cleanup reason"))
            .await
            .unwrap();
        let live_claim = db
            .claim_product_creation(
                "req-cleanup-reason",
                "worker-live",
                "token-live",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .mark_product_creation_cleanup_ambiguous(
                "req-cleanup-reason",
                &live_claim.claim,
                "owner marker does not match durable token",
                now + chrono::Duration::seconds(1),
            )
            .await
            .unwrap());
        let ambiguous = db
            .get_product_creation_job("req-cleanup-reason")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ambiguous.status, "cleanup_ambiguous");
        assert_eq!(
            ambiguous.last_error.as_deref(),
            Some("owner marker does not match durable token")
        );
    }

    #[tokio::test]
    async fn product_creation_publication_clears_stale_provisioning_error() {
        let db = Database::open_in_memory().await.unwrap();
        let accepted_intent = intent("/repo/a", "ship it");
        db.accept_product_creation("req-language", &accepted_intent)
            .await
            .unwrap();
        let stored = db
            .get_product_creation_job("req-language")
            .await
            .unwrap()
            .expect("accepted job");
        assert_eq!(stored.intent.llm_language, LlmLanguage::Caveman);

        let now = Utc::now();
        let claimed = db
            .claim_product_creation(
                "req-language",
                "worker-a",
                "token-a",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .expect("claim accepted job");
        assert!(db
            .schedule_product_creation_retry(
                "req-language",
                &claimed.claim,
                "transient provisioning failure",
                now,
            )
            .await
            .unwrap());
        let retried = db
            .get_product_creation_job("req-language")
            .await
            .unwrap()
            .expect("retried job");
        assert_eq!(
            retried.last_error.as_deref(),
            Some("transient provisioning failure")
        );

        let publish_claim = db
            .claim_product_creation(
                "req-language",
                "worker-b",
                "token-b",
                retried.retry_at.expect("retry deadline"),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .expect("claim retry");
        let conversation = published_conversation(
            "conv-language",
            retried.product_conversation_id,
            WorkScopeId::new(),
            "/repo/a",
        );
        advance_to_metadata(&db, "req-language", &publish_claim.claim).await;
        assert!(db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-language".to_string(),
                claim: publish_claim.claim,
                conversation,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/a".to_string(),
                },
                git_publication: Some(ProductCreationGitPublicationFacts {
                    exact_checkout_oid: "repo-id".to_string(),
                    repository_root: "/repo/a".to_string(),
                }),
            })
            .await
            .unwrap());
        let delivery_pending = db
            .get_product_creation_job("req-language")
            .await
            .unwrap()
            .expect("published job");
        assert_eq!(delivery_pending.status, "delivery_pending");
        assert_eq!(delivery_pending.intent.llm_language, LlmLanguage::Caveman);
        assert!(delivery_pending.last_error.is_none());
    }

    #[tokio::test]
    async fn product_creation_claim_can_be_renewed_only_while_live() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-renew", &intent("/repo/a", "renew"))
            .await
            .unwrap();
        let now = Utc::now();
        let claim = db
            .claim_next_product_creation("worker", "token", now, chrono::Duration::seconds(30))
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .product_creation_claim_is_live("req-renew", &claim.claim, now)
            .await
            .unwrap());
        assert!(db
            .renew_product_creation_claim(
                "req-renew",
                &claim.claim,
                now + chrono::Duration::seconds(5),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap());
        let job = db
            .get_product_creation_job("req-renew")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            job.claim_lease_until,
            DateTime::<Utc>::from_timestamp_micros(
                (now + chrono::Duration::seconds(35)).timestamp_micros()
            )
        );
        assert!(!db
            .renew_product_creation_claim(
                "req-renew",
                &claim.claim,
                now + chrono::Duration::seconds(36),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap());
        assert!(!db
            .product_creation_claim_is_live(
                "req-renew",
                &claim.claim,
                now + chrono::Duration::seconds(36),
            )
            .await
            .unwrap());
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn product_creation_delivery_retry_is_bounded_and_not_immediately_requeued() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-delivery", &intent("/repo/a", "delivery"))
            .await
            .unwrap();
        let claim = db
            .claim_next_product_creation(
                "worker",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let accepted = db
            .get_product_creation_job("req-delivery")
            .await
            .unwrap()
            .unwrap();
        let conv = published_conversation(
            "conv-delivery",
            accepted.product_conversation_id,
            WorkScopeId::new(),
            "/repo/a",
        );
        advance_to_metadata(&db, "req-delivery", &claim.claim).await;
        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-delivery".to_string(),
            claim: claim.claim.clone(),
            conversation: conv,
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/a".to_string(),
            },
            git_publication: Some(ProductCreationGitPublicationFacts {
                exact_checkout_oid: "repo-id".to_string(),
                repository_root: "/repo/a".to_string(),
            }),
        })
        .await
        .unwrap();
        assert!(db
            .claim_next_product_creation_delivery(
                "delivery-worker-early",
                "delivery-token-early",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .is_none());
        let now = Utc::now();
        let mut delivery_claim = claim.claim;
        assert!(db
            .schedule_product_creation_delivery_retry(
                "req-delivery",
                &delivery_claim,
                "delivery failed",
                now,
            )
            .await
            .unwrap());
        let pending = db
            .get_product_creation_job("req-delivery")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.delivery_attempt_count, 2);
        assert_eq!(
            pending.delivery_retry_at,
            DateTime::<Utc>::from_timestamp_micros(
                (now + chrono::Duration::seconds(2)).timestamp_micros()
            )
        );
        assert!(db
            .claim_next_product_creation_delivery(
                "delivery-worker-mid",
                "delivery-token-mid",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .is_none());
        for (worker, token, seconds) in [
            ("delivery-worker-2", "delivery-token-2", 3),
            ("delivery-worker-3", "delivery-token-3", 11),
        ] {
            delivery_claim = db
                .claim_next_product_creation_delivery(
                    worker,
                    token,
                    now + chrono::Duration::seconds(seconds),
                    chrono::Duration::minutes(5),
                )
                .await
                .unwrap()
                .unwrap()
                .claim;
            assert!(db
                .schedule_product_creation_delivery_retry(
                    "req-delivery",
                    &delivery_claim,
                    "delivery failed",
                    now,
                )
                .await
                .unwrap());
        }
        let fourth = db
            .get_product_creation_job("req-delivery")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fourth.delivery_attempt_count, 4);
        assert_eq!(fourth.status, "delivery_pending");
        assert_eq!(
            fourth.delivery_retry_at,
            DateTime::<Utc>::from_timestamp_micros(
                (now + chrono::Duration::seconds(30)).timestamp_micros()
            )
        );
        delivery_claim = db
            .claim_next_product_creation_delivery(
                "delivery-worker-4",
                "delivery-token-4",
                now + chrono::Duration::seconds(31),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap()
            .claim;
        assert!(db
            .schedule_product_creation_delivery_retry(
                "req-delivery",
                &delivery_claim,
                "delivery failed",
                now,
            )
            .await
            .unwrap());
        let exhausted = db
            .get_product_creation_job("req-delivery")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(exhausted.delivery_attempt_count, 4);
        assert_eq!(exhausted.status, "delivery_failed");
        assert!(exhausted.delivery_retry_at.is_none());
        assert_eq!(exhausted.last_error.as_deref(), Some("delivery failed"));
        assert!(!db
            .schedule_product_creation_delivery_retry(
                "req-delivery",
                &delivery_claim,
                "delivery failed",
                now,
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn persisted_resource_ownership_revalidation_is_fenced_by_live_claim() {
        let db = Database::open_in_memory().await.unwrap();
        let now = Utc::now();
        db.accept_product_creation("req-owner-fence", &intent("/repo/a", "owner fence"))
            .await
            .unwrap();
        let claimed = db
            .claim_product_creation(
                "req-owner-fence",
                "worker",
                "token",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        let resource = "/repo/a/.phoenix/worktrees/owner-fence";
        assert!(db
            .reserve_product_creation_resource(
                "reservation-owner-fence",
                "req-owner-fence",
                &claimed.claim,
                "/repo/a",
                resource,
                now,
            )
            .await
            .unwrap());
        assert!(db
            .record_product_creation_resource_ownership(
                "req-owner-fence",
                &claimed.claim,
                resource,
                "owner-token",
                now,
            )
            .await
            .unwrap());
        assert!(db
            .record_product_creation_resource_ownership(
                "req-owner-fence",
                &claimed.claim,
                resource,
                "owner-token",
                now + chrono::Duration::seconds(1),
            )
            .await
            .unwrap());

        assert!(db
            .mark_product_creation_resource_present(
                "req-owner-fence",
                &claimed.claim,
                resource,
                "owner-token",
                now + chrono::Duration::seconds(1),
            )
            .await
            .unwrap());
        assert!(db
            .record_product_creation_resource_ownership(
                "req-owner-fence",
                &claimed.claim,
                resource,
                "owner-token",
                now + chrono::Duration::seconds(1),
            )
            .await
            .unwrap());
        assert!(!db
            .record_product_creation_resource_ownership(
                "req-owner-fence",
                &claimed.claim,
                resource,
                "different-owner-token",
                now + chrono::Duration::seconds(1),
            )
            .await
            .unwrap());

        assert!(db
            .cancel_product_creation("req-owner-fence", now + chrono::Duration::seconds(2))
            .await
            .unwrap());
        assert!(!db
            .record_product_creation_resource_ownership(
                "req-owner-fence",
                &claimed.claim,
                resource,
                "owner-token",
                now + chrono::Duration::seconds(3),
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn product_creation_cancellation_revokes_claim_and_finishes_cancelled() {
        let db = Database::open_in_memory().await.unwrap();
        let now = Utc::now();
        db.accept_product_creation("req-cancel", &intent("/repo/a", "cancel"))
            .await
            .unwrap();
        let claimed = db
            .claim_product_creation(
                "req-cancel",
                "worker",
                "token",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();

        assert!(db
            .cancel_product_creation("req-cancel", now + chrono::Duration::seconds(1))
            .await
            .unwrap());
        let cancelling = db
            .get_product_creation_job("req-cancel")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelling.status, "cancelling");
        assert_eq!(cancelling.claim_generation, claimed.claim.generation + 1);
        assert!(cancelling.claim_token.is_none());

        let cleanup = db
            .claim_next_product_creation_cleanup(
                "cleanup-worker",
                "cleanup-token",
                now + chrono::Duration::seconds(1),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .finish_product_creation_cleanup(&cleanup, now + chrono::Duration::seconds(2))
            .await
            .unwrap());
        let cancelled = db
            .get_product_creation_job("req-cancel")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert!(cancelled.cancelled_at.is_some());
    }

    #[tokio::test]
    async fn conflicted_cleanup_can_be_released_after_occupant_becomes_absent() {
        let db = Database::open_in_memory().await.unwrap();
        let now = Utc::now();
        db.accept_product_creation("req-resolved", &intent("/repo/a", "resolved"))
            .await
            .unwrap();
        let claimed = db
            .claim_product_creation(
                "req-resolved",
                "worker",
                "token",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .reserve_product_creation_resource(
                "reservation-resolved",
                "req-resolved",
                &claimed.claim,
                "/repo/a",
                "/repo/a/.phoenix/worktrees/resolved",
                now,
            )
            .await
            .unwrap());
        assert!(db
            .cancel_product_creation("req-resolved", now + chrono::Duration::seconds(1))
            .await
            .unwrap());
        let cleanup = db
            .claim_next_product_creation_cleanup(
                "cleanup-worker",
                "cleanup-token",
                now + chrono::Duration::seconds(1),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .mark_product_creation_resource_cleanup_conflict(
                &cleanup,
                "reservation-resolved",
                now + chrono::Duration::seconds(2),
            )
            .await
            .unwrap());
        assert!(db
            .release_product_creation_resource(
                &cleanup,
                "reservation-resolved",
                now + chrono::Duration::seconds(3),
            )
            .await
            .unwrap());
        assert!(db
            .finish_product_creation_cleanup(&cleanup, now + chrono::Duration::seconds(3))
            .await
            .unwrap());
        assert_eq!(
            db.get_product_creation_job("req-resolved")
                .await
                .unwrap()
                .unwrap()
                .status,
            "cancelled"
        );
    }

    #[tokio::test]
    async fn product_creation_cleanup_conflict_is_durable_and_blocks_finalization() {
        let db = Database::open_in_memory().await.unwrap();
        let now = Utc::now();
        db.accept_product_creation("req-conflict", &intent("/repo/a", "conflict"))
            .await
            .unwrap();
        let claimed = db
            .claim_product_creation(
                "req-conflict",
                "worker",
                "token",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .reserve_product_creation_resource(
                "reservation-conflict",
                "req-conflict",
                &claimed.claim,
                "/repo/a",
                "/repo/a/.phoenix/worktrees/conflict",
                now,
            )
            .await
            .unwrap());
        assert!(db
            .mark_product_creation_resource_present(
                "req-conflict",
                &claimed.claim,
                "/repo/a/.phoenix/worktrees/conflict",
                "owner-token",
                now,
            )
            .await
            .unwrap());
        assert!(db
            .cancel_product_creation("req-conflict", now + chrono::Duration::seconds(1))
            .await
            .unwrap());
        let cleanup = db
            .claim_next_product_creation_cleanup(
                "cleanup-worker",
                "cleanup-token",
                now + chrono::Duration::seconds(1),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            cleanup.reservations[0].ownership_token.as_deref(),
            Some("owner-token")
        );
        assert!(db
            .mark_product_creation_resource_cleanup_conflict(
                &cleanup,
                "reservation-conflict",
                now + chrono::Duration::seconds(2),
            )
            .await
            .unwrap());
        assert!(db
            .mark_claimed_product_creation_cleanup_ambiguous(
                &cleanup,
                "ownership conflict",
                now + chrono::Duration::seconds(2),
            )
            .await
            .unwrap());

        let reservation = db
            .get_product_creation_resource_reservations("req-conflict")
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(reservation.status, "conflict");
        assert_eq!(reservation.ownership_token.as_deref(), Some("owner-token"));
        let ambiguous = db
            .get_product_creation_job("req-conflict")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ambiguous.status, "cleanup_ambiguous");
        assert_eq!(ambiguous.last_error.as_deref(), Some("ownership conflict"));
        assert!(!db
            .finish_product_creation_cleanup(&cleanup, now + chrono::Duration::seconds(3))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn persisted_cleanup_conflict_is_idempotent_and_requeued_for_deletion() {
        let db = Database::open_in_memory().await.unwrap();
        let now = Utc::now();
        db.accept_product_creation("req-requeue", &intent("/repo/a", "requeue"))
            .await
            .unwrap();
        let claimed = db
            .claim_product_creation(
                "req-requeue",
                "worker",
                "token",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .reserve_product_creation_resource(
                "reservation-requeue",
                "req-requeue",
                &claimed.claim,
                "/repo/a",
                "/repo/a/.phoenix/worktrees/requeue",
                now,
            )
            .await
            .unwrap());
        assert!(db
            .cancel_product_creation("req-requeue", now + chrono::Duration::seconds(1))
            .await
            .unwrap());
        let cleanup = db
            .claim_next_product_creation_cleanup(
                "cleanup-worker",
                "cleanup-token",
                now + chrono::Duration::seconds(1),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .mark_product_creation_resource_cleanup_conflict(
                &cleanup,
                "reservation-requeue",
                now + chrono::Duration::seconds(2),
            )
            .await
            .unwrap());
        assert!(db
            .mark_product_creation_resource_cleanup_conflict(
                &cleanup,
                "reservation-requeue",
                now + chrono::Duration::seconds(2),
            )
            .await
            .unwrap());
        assert!(db
            .mark_claimed_product_creation_cleanup_ambiguous(
                &cleanup,
                "ownership conflict",
                now + chrono::Duration::seconds(2),
            )
            .await
            .unwrap());
        assert!(db
            .request_product_creation_deletion("req-requeue", now + chrono::Duration::seconds(3),)
            .await
            .unwrap());
        let deletion = db
            .claim_next_product_creation_cleanup(
                "deletion-worker",
                "deletion-token",
                now + chrono::Duration::seconds(3),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .release_product_creation_resource(
                &deletion,
                "reservation-requeue",
                now + chrono::Duration::seconds(4),
            )
            .await
            .unwrap());
        assert!(db
            .finish_product_creation_cleanup(&deletion, now + chrono::Duration::seconds(4))
            .await
            .unwrap());
        assert!(db
            .get_product_creation_job("req-requeue")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn product_creation_deletion_tombstone_is_physically_removed_after_cleanup() {
        let db = Database::open_in_memory().await.unwrap();
        let now = Utc::now();
        db.accept_product_creation("req-delete", &intent("/repo/a", "delete"))
            .await
            .unwrap();

        assert!(db
            .request_product_creation_deletion("req-delete", now)
            .await
            .unwrap());
        let tombstone = db
            .get_product_creation_job("req-delete")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tombstone.status, "deletion_pending");
        assert!(tombstone.deletion_requested_at.is_some());

        let cleanup = db
            .claim_next_product_creation_cleanup(
                "cleanup-worker",
                "cleanup-token",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .finish_product_creation_cleanup(&cleanup, now + chrono::Duration::seconds(1))
            .await
            .unwrap());
        assert!(db
            .get_product_creation_job("req-delete")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn delivery_claim_is_exclusive_across_two_database_handles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("product-delivery-claim.sqlite");
        let path = path.to_string_lossy().to_string();
        let db1 = Database::open(&path).await.unwrap();
        crate::migrations::run_pending_migrations(db1.pool())
            .await
            .unwrap();
        let db2 = Database::open(&path).await.unwrap();
        crate::migrations::run_pending_migrations(db2.pool())
            .await
            .unwrap();

        db1.accept_product_creation("req-delivery-claim", &intent("/repo/a", "delivery"))
            .await
            .unwrap();
        let claim = db1
            .claim_next_product_creation(
                "worker",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let accepted = db1
            .get_product_creation_job("req-delivery-claim")
            .await
            .unwrap()
            .unwrap();
        let conv = published_conversation(
            "conv-delivery-claim",
            accepted.product_conversation_id,
            WorkScopeId::new(),
            "/repo/a",
        );
        let delivery_claim_at = claim.claim.lease_until + chrono::Duration::seconds(1);
        advance_to_metadata(&db1, "req-delivery-claim", &claim.claim).await;
        db1.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-delivery-claim".to_string(),
            claim: claim.claim,
            conversation: conv,
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/a".to_string(),
            },
            git_publication: Some(ProductCreationGitPublicationFacts {
                exact_checkout_oid: "repo-id".to_string(),
                repository_root: "/repo/a".to_string(),
            }),
        })
        .await
        .unwrap();

        let now = delivery_claim_at;
        let first = db1
            .claim_next_product_creation_delivery(
                "delivery-worker-a",
                "delivery-token-a",
                now,
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap();
        let second = db2
            .claim_next_product_creation_delivery(
                "delivery-worker-b",
                "delivery-token-b",
                now,
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap();
        assert!(first.is_some());
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn delivery_completion_requires_current_claim_and_exact_acceptance() {
        assert_delivery_completion_admission(None).await;
    }

    #[tokio::test]
    async fn delivery_completion_preserves_archived_history() {
        assert_delivery_completion_admission(Some("archived")).await;
    }

    #[tokio::test]
    async fn delivery_completion_preserves_archived_cleanup_attention_history() {
        assert_delivery_completion_admission(Some("archived_cleanup_attention")).await;
    }

    #[tokio::test]
    async fn delivery_completion_respects_open_close_incomplete_fence() {
        assert_delivery_completion_admission(Some("close_incomplete")).await;
    }

    #[allow(clippy::too_many_lines)]
    async fn assert_delivery_completion_admission(close_outcome: Option<&str>) {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-delivery-complete", &intent("/repo/a", "delivery"))
            .await
            .unwrap();
        let claim = db
            .claim_next_product_creation(
                "worker",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let accepted = db
            .get_product_creation_job("req-delivery-complete")
            .await
            .unwrap()
            .unwrap();
        let mut conv = published_conversation(
            "conv-delivery-complete",
            accepted.product_conversation_id.clone(),
            WorkScopeId::new(),
            "/repo/a",
        );
        conv.archived = true;

        advance_to_metadata(&db, "req-delivery-complete", &claim.claim).await;
        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-delivery-complete".to_string(),
            claim: claim.claim.clone(),
            conversation: conv,
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/a".to_string(),
            },
            git_publication: Some(ProductCreationGitPublicationFacts {
                exact_checkout_oid: "repo-id".to_string(),
                repository_root: "/repo/a".to_string(),
            }),
        })
        .await
        .unwrap();
        sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'delivery_failed', delivery_retry_at_unix_micros = NULL,
                 claim_worker_id = NULL, claim_token = NULL, claim_lease_until_unix_micros = NULL
             WHERE request_id = 'req-delivery-complete'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let retryable_lifecycle: String = sqlx::query_scalar(
            "SELECT ordinary_lifecycle FROM product_conversations WHERE id = ?1",
        )
        .bind(accepted.product_conversation_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(retryable_lifecycle, "open");
        assert!(db
            .retry_failed_product_creation_delivery("req-delivery-complete")
            .await
            .unwrap());
        let delivery = db
            .claim_next_product_creation_delivery(
                "delivery-worker",
                "delivery-token",
                claim.claim.lease_until + chrono::Duration::seconds(1),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(!db
            .complete_product_creation_delivery(
                "req-delivery-complete",
                &delivery.claim,
                &SteeringAcceptanceFingerprint::Exact("wrong-fingerprint".to_string()),
                &delivery.job.intent.live_intent().unwrap(),
            )
            .await
            .unwrap());
        db.append_product_creation_steering_entry(
            &delivery,
            &phoenix_core::domain::sm_event::SteerEntry {
                origin: phoenix_core::domain::db_schema::InputOrigin::UnknownHistorical,
                text: delivery.job.intent.objective.clone(),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                message_id: "req-delivery-complete".to_string(),
                user_agent: None,
                skill_invocation: None,
            },
            "product-create:req-delivery-complete",
        )
        .await
        .unwrap();
        let stale_claim = ProductCreationClaim {
            worker_id: delivery.claim.worker_id.clone(),
            token: "stale-token".to_string(),
            generation: delivery.claim.generation,
            lease_until: delivery.claim.lease_until,
        };
        assert!(!db
            .complete_product_creation_delivery(
                "req-delivery-complete",
                &stale_claim,
                &SteeringAcceptanceFingerprint::Exact(
                    "product-create:req-delivery-complete".to_string(),
                ),
                &delivery.job.intent.live_intent().unwrap(),
            )
            .await
            .unwrap());
        if let Some(outcome) = close_outcome {
            let archived = outcome != "close_incomplete";
            let lifecycle = if archived { "history" } else { "open" };
            sqlx::query("UPDATE product_conversations SET ordinary_lifecycle = ?2 WHERE id = ?1")
                .bind(accepted.product_conversation_id.as_str())
                .bind(lifecycle)
                .execute(db.pool())
                .await
                .unwrap();
            sqlx::query(
                "UPDATE conversations SET archived = ?1 WHERE id = 'conv-delivery-complete'",
            )
            .bind(archived)
            .execute(db.pool())
            .await
            .unwrap();
            sqlx::query("DROP TRIGGER close_obligations_require_admission_phase_on_insert")
                .execute(db.pool())
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO close_obligations (
                     attempt_id, product_conversation_id, phase, created_at, updated_at,
                     completed_at, close_outcome
                 ) VALUES (
                     'delivery-close-won', ?1, 'completed', '2025-01-01T00:00:02Z',
                     '2025-01-01T00:00:02Z', '2025-01-01T00:00:02Z', ?2
                 )",
            )
            .bind(accepted.product_conversation_id.as_str())
            .bind(outcome)
            .execute(db.pool())
            .await
            .unwrap();
            assert!(!db
                .complete_product_creation_delivery(
                    "req-delivery-complete",
                    &delivery.claim,
                    &SteeringAcceptanceFingerprint::Exact(
                        "product-create:req-delivery-complete".to_string(),
                    ),
                    &delivery.job.intent.live_intent().unwrap(),
                )
                .await
                .unwrap());
            assert_eq!(
                db.get_conversation("conv-delivery-complete")
                    .await
                    .unwrap()
                    .archived,
                archived
            );
            let actual_lifecycle: String = sqlx::query_scalar(
                "SELECT ordinary_lifecycle FROM product_conversations WHERE id = ?1",
            )
            .bind(accepted.product_conversation_id.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(actual_lifecycle, lifecycle);
            let job = db
                .get_product_creation_job("req-delivery-complete")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(job.status, "delivery_pending");
            assert_eq!(job, delivery.job);
            let actual_outcome: String = sqlx::query_scalar(
                "SELECT close_outcome FROM close_obligations WHERE attempt_id = 'delivery-close-won'",
            )
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(actual_outcome, outcome);
            return;
        }

        let before_completion = db
            .get_product_creation_job("req-delivery-complete")
            .await
            .unwrap()
            .unwrap();
        for (stage, timestamp) in [
            (
                "commit_metadata",
                before_completion.stage_updated_at.timestamp_micros(),
            ),
            (
                "bootstrap_initial_turn",
                (Utc::now() + chrono::Duration::minutes(1)).timestamp_micros(),
            ),
        ] {
            sqlx::query(
                "UPDATE product_creation_jobs SET provisioning_stage = ?1,
                     stage_updated_at_unix_micros = ?2 WHERE request_id = 'req-delivery-complete'",
            )
            .bind(stage)
            .bind(timestamp)
            .execute(db.pool())
            .await
            .unwrap();
            let before = db
                .get_product_creation_job("req-delivery-complete")
                .await
                .unwrap()
                .unwrap();
            assert!(!db
                .complete_product_creation_delivery(
                    "req-delivery-complete",
                    &delivery.claim,
                    &SteeringAcceptanceFingerprint::Exact(
                        "product-create:req-delivery-complete".to_string()
                    ),
                    &delivery.job.intent.live_intent().unwrap(),
                )
                .await
                .unwrap());
            assert_eq!(
                db.get_product_creation_job("req-delivery-complete")
                    .await
                    .unwrap()
                    .unwrap(),
                before
            );
            assert!(
                db.get_conversation("conv-delivery-complete")
                    .await
                    .unwrap()
                    .archived
            );
        }
        sqlx::query(
            "UPDATE product_creation_jobs SET provisioning_stage = 'bootstrap_initial_turn',
                 stage_updated_at_unix_micros = ?1 WHERE request_id = 'req-delivery-complete'",
        )
        .bind(before_completion.stage_updated_at.timestamp_micros())
        .execute(db.pool())
        .await
        .unwrap();

        assert!(db
            .complete_product_creation_delivery(
                "req-delivery-complete",
                &delivery.claim,
                &SteeringAcceptanceFingerprint::Exact(
                    "product-create:req-delivery-complete".to_string(),
                ),
                &delivery.job.intent.live_intent().unwrap(),
            )
            .await
            .unwrap());
        assert!(
            !db.get_conversation("conv-delivery-complete")
                .await
                .unwrap()
                .archived
        );
        let lifecycle: String = sqlx::query_scalar(
            "SELECT ordinary_lifecycle FROM product_conversations WHERE id = ?1",
        )
        .bind(delivery.job.product_conversation_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(lifecycle, "open");
        let completed = db
            .get_product_creation_job("req-delivery-complete")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, "published");
        assert!(completed.claim_worker_id.is_none());
        assert!(completed.claim_token.is_none());
        assert!(completed.claim_lease_until.is_none());
        assert!(completed.delivery_retry_at.is_none());
        assert_eq!(
            completed.provisioning_stage,
            ProductCreationProvisioningStage::FinalizePublication
        );
        assert!(completed.stage_updated_at >= before_completion.stage_updated_at);
    }

    #[tokio::test]
    async fn published_product_creation_rejects_cancel_and_pending_delete() {
        let db = Database::open_in_memory().await.unwrap();
        let now = Utc::now();
        db.accept_product_creation("req-published-lifecycle", &intent("/repo/a", "published"))
            .await
            .unwrap();
        let claimed = db
            .claim_product_creation(
                "req-published-lifecycle",
                "worker",
                "token",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        advance_to_metadata(&db, "req-published-lifecycle", &claimed.claim).await;
        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-published-lifecycle".to_string(),
            claim: claimed.claim.clone(),
            conversation: published_conversation(
                "conv-published-lifecycle",
                claimed.job.product_conversation_id.clone(),
                WorkScopeId::new(),
                "/repo/a",
            ),
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/a".to_string(),
            },
            git_publication: Some(ProductCreationGitPublicationFacts {
                repository_root: "/repo/a".to_string(),
                exact_checkout_oid: "abc123".to_string(),
            }),
        })
        .await
        .unwrap();

        let delivery = ClaimedProductCreationJob {
            claim: claimed.claim,
            job: db
                .get_product_creation_job("req-published-lifecycle")
                .await
                .unwrap()
                .unwrap(),
        };
        db.append_product_creation_steering_entry(
            &delivery,
            &delivery_entry(&delivery),
            "product-create:req-published-lifecycle",
        )
        .await
        .unwrap();

        assert!(!db
            .cancel_product_creation("req-published-lifecycle", now)
            .await
            .unwrap());
        assert!(!db
            .request_product_creation_deletion("req-published-lifecycle", now)
            .await
            .unwrap());
        assert!(!db
            .retry_failed_product_creation_delivery("req-published-lifecycle")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn delivery_retryable_excludes_non_completed_close_obligations() {
        let db = Database::open_in_memory().await.unwrap();
        let now = Utc::now();
        db.accept_product_creation("req-delivery-close", &intent("/repo/a", "delivery-close"))
            .await
            .unwrap();
        let claimed = db
            .claim_product_creation(
                "req-delivery-close",
                "worker",
                "token",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        let product_id = claimed.job.product_conversation_id.clone();
        advance_to_metadata(&db, "req-delivery-close", &claimed.claim).await;
        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-delivery-close".to_string(),
            claim: claimed.claim,
            conversation: published_conversation(
                "conv-delivery-close",
                product_id.clone(),
                WorkScopeId::new(),
                "/repo/a",
            ),
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/a".to_string(),
            },
            git_publication: Some(ProductCreationGitPublicationFacts {
                repository_root: "/repo/a".to_string(),
                exact_checkout_oid: "abc123".to_string(),
            }),
        })
        .await
        .unwrap();
        sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'delivery_failed', delivery_retry_at_unix_micros = NULL,
                 claim_worker_id = NULL, claim_token = NULL, claim_lease_until_unix_micros = NULL
             WHERE request_id = 'req-delivery-close'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("DROP TRIGGER close_obligations_require_admission_phase_on_insert")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, product_conversation_id, phase, created_at, updated_at, topology_sealed
             ) VALUES (
                 'close-noncompleted', ?1, 'awaiting_retirement_inspection',
                 '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 1
             )",
        )
        .bind(product_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();

        assert!(!db
            .retry_failed_product_creation_delivery("req-delivery-close")
            .await
            .unwrap());
        let rows = db.list_product_creation_recovery_jobs(None).await.unwrap();
        let delivery = rows
            .iter()
            .find(|row| row.request_id == "req-delivery-close")
            .unwrap();
        assert!(!delivery.delivery_retryable);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn list_product_creation_recovery_jobs_filters_and_orders_user_visible_rows() {
        let db = Database::open_in_memory().await.unwrap();
        let now = Utc::now();

        db.accept_product_creation("req-accepted", &intent("/repo/accepted", "accepted"))
            .await
            .unwrap();
        db.accept_product_creation("req-claimed", &intent("/repo/claimed", "claimed"))
            .await
            .unwrap();
        db.claim_product_creation(
            "req-claimed",
            "worker",
            "token",
            now,
            chrono::Duration::seconds(30),
        )
        .await
        .unwrap();
        db.accept_product_creation("req-retry", &intent("/repo/retry", "retry"))
            .await
            .unwrap();
        let retry_claim = db
            .claim_product_creation(
                "req-retry",
                "worker-retry",
                "token-retry",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .schedule_product_creation_retry("req-retry", &retry_claim.claim, "boom", now)
            .await
            .unwrap());

        db.accept_product_creation("req-failed", &intent("/repo/failed", "failed"))
            .await
            .unwrap();
        let failed_claim = db
            .claim_product_creation(
                "req-failed",
                "worker-failed",
                "token-failed",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE product_creation_jobs SET attempt_count = ?2 WHERE request_id = ?1")
            .bind("req-failed")
            .bind(PRODUCT_CREATION_MAX_ATTEMPTS)
            .execute(db.pool())
            .await
            .unwrap();
        assert!(db
            .schedule_product_creation_retry("req-failed", &failed_claim.claim, "failed badly", now)
            .await
            .unwrap());

        db.accept_product_creation("req-cancelled", &intent("/repo/cancelled", "cancelled"))
            .await
            .unwrap();
        assert!(db
            .cancel_product_creation("req-cancelled", now)
            .await
            .unwrap());
        let cleanup = db
            .claim_next_product_creation_cleanup(
                "cleanup-worker",
                "cleanup-token",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(db
            .finish_product_creation_cleanup(&cleanup, now)
            .await
            .unwrap());

        db.accept_product_creation("req-delivery", &intent("/repo/delivery", "delivery"))
            .await
            .unwrap();
        let delivery_claim = db
            .claim_product_creation(
                "req-delivery",
                "worker-delivery",
                "token-delivery",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        let accepted = db
            .get_product_creation_job("req-delivery")
            .await
            .unwrap()
            .unwrap();
        let expected_published_product_id = accepted.product_conversation_id.to_string();
        advance_to_metadata(&db, "req-delivery", &delivery_claim.claim).await;
        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-delivery".to_string(),
            claim: delivery_claim.claim.clone(),
            conversation: published_conversation(
                "conv-delivery",
                accepted.product_conversation_id,
                WorkScopeId::new(),
                "/repo/delivery",
            ),
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/delivery".to_string(),
            },
            git_publication: Some(ProductCreationGitPublicationFacts {
                repository_root: "/repo/delivery".to_string(),
                exact_checkout_oid: "abc123".to_string(),
            }),
        })
        .await
        .unwrap();
        let delivery_claim = db
            .claim_next_product_creation_delivery(
                "delivery-worker",
                "delivery-token",
                delivery_claim.claim.lease_until + chrono::Duration::seconds(1),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let mut active_delivery_claim = delivery_claim.claim;
        for i in 1..=4 {
            assert!(db
                .schedule_product_creation_delivery_retry(
                    "req-delivery",
                    &active_delivery_claim,
                    &format!("delivery failure {i}"),
                    now + chrono::Duration::seconds(i),
                )
                .await
                .unwrap());
            if i < 4 {
                active_delivery_claim = db
                    .claim_next_product_creation_delivery(
                        &format!("delivery-worker-{i}"),
                        &format!("delivery-token-{i}"),
                        now + chrono::Duration::seconds(i + 40),
                        chrono::Duration::minutes(5),
                    )
                    .await
                    .unwrap()
                    .unwrap()
                    .claim;
            }
        }

        db.accept_product_creation("req-published", &intent("/repo/published", "published"))
            .await
            .unwrap();
        let published_claim = db
            .claim_product_creation(
                "req-published",
                "worker-published",
                "token-published",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        advance_to_metadata(&db, "req-published", &published_claim.claim).await;
        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-published".to_string(),
            claim: published_claim.claim,
            conversation: published_conversation(
                "conv-published",
                db.get_product_creation_job("req-published")
                    .await
                    .unwrap()
                    .unwrap()
                    .product_conversation_id,
                WorkScopeId::new(),
                "/repo/published",
            ),
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/published".to_string(),
            },
            git_publication: Some(ProductCreationGitPublicationFacts {
                repository_root: "/repo/published".to_string(),
                exact_checkout_oid: "def456".to_string(),
            }),
        })
        .await
        .unwrap();

        db.accept_product_creation("req-delete-hidden", &intent("/repo/delete", "delete"))
            .await
            .unwrap();
        assert!(db
            .request_product_creation_deletion("req-delete-hidden", now)
            .await
            .unwrap());

        let rows = db.list_product_creation_recovery_jobs(None).await.unwrap();
        let statuses: std::collections::HashMap<_, _> = rows
            .iter()
            .map(|row| (row.request_id.as_str(), row.status.as_str()))
            .collect();
        assert_eq!(statuses.len(), 7);
        assert_eq!(statuses.get("req-delivery"), Some(&"delivery_failed"));
        assert_eq!(statuses.get("req-published"), Some(&"delivery_pending"));
        assert_eq!(statuses.get("req-cancelled"), Some(&"cancelled"));
        assert_eq!(statuses.get("req-failed"), Some(&"failed"));
        assert_eq!(statuses.get("req-retry"), Some(&"retry_scheduled"));
        assert_eq!(statuses.get("req-claimed"), Some(&"claimed"));
        assert_eq!(statuses.get("req-accepted"), Some(&"accepted"));
        let failed = rows
            .iter()
            .find(|row| row.request_id == "req-failed")
            .unwrap();
        assert_eq!(failed.last_error.as_deref(), Some("failed badly"));
        let delivery = rows
            .iter()
            .find(|row| row.request_id == "req-delivery")
            .unwrap();
        assert_eq!(delivery.intent.cwd, "/repo/delivery");
        assert_eq!(delivery.intent.objective, "delivery");
        assert_eq!(delivery.last_error.as_deref(), Some("delivery failure 4"));
        assert_eq!(
            delivery.published_product_conversation_id.as_deref(),
            Some(expected_published_product_id.as_str())
        );
        let accepted = rows
            .iter()
            .find(|row| row.request_id == "req-accepted")
            .unwrap();
        assert_eq!(accepted.published_product_conversation_id, None);
        assert!(rows.iter().all(|row| row.request_id != "req-delete-hidden"));
    }

    #[tokio::test]
    async fn product_creation_pin_once() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-2", &intent("/repo/a", "pin"))
            .await
            .unwrap();
        let claim = db
            .claim_next_product_creation(
                "worker",
                "token",
                Utc::now(),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            db.pin_product_creation_once("req-2", &claim.claim, "oid", "main", "fresh")
                .await
                .unwrap(),
            ProductCreationPinOutcome::Pinned(_)
        ));
        assert!(matches!(
            db.pin_product_creation_once("req-2", &claim.claim, "oid", "main", "fresh")
                .await
                .unwrap(),
            ProductCreationPinOutcome::Same(_)
        ));
        assert!(matches!(
            db.pin_product_creation_once("req-2", &claim.claim, "other", "main", "fresh")
                .await
                .unwrap(),
            ProductCreationPinOutcome::Conflict(_)
        ));
    }

    #[tokio::test]
    async fn product_creation_pin_requires_live_claim() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-pin-live", &intent("/repo/a", "pin"))
            .await
            .unwrap();
        let now = Utc::now();
        let claim = db
            .claim_next_product_creation("worker", "token", now, chrono::Duration::seconds(30))
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "UPDATE product_creation_jobs SET claim_lease_until_unix_micros = ?2 WHERE request_id = ?1",
        )
        .bind("req-pin-live")
        .bind(now.timestamp_micros())
        .execute(db.pool())
        .await
        .unwrap();
        let expired = ProductCreationClaim {
            lease_until: now,
            ..claim.claim.clone()
        };
        assert!(matches!(
            db.pin_product_creation_once("req-pin-live", &expired, "oid", "main", "fresh")
                .await
                .unwrap(),
            ProductCreationPinOutcome::Conflict(_)
        ));
    }

    #[tokio::test]
    async fn product_creation_publish_rollback_no_publication() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-3", &intent("/repo/a", "rollback"))
            .await
            .unwrap();
        let claimed = db
            .claim_next_product_creation(
                "worker",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let accepted = db.get_product_creation_job("req-3").await.unwrap().unwrap();
        let scope = WorkScopeId::new();
        let conv = published_conversation(
            "conv-dup",
            accepted.product_conversation_id,
            scope,
            "/repo/a",
        );
        db.create_conversation("conv-dup", "conv-dup", "/repo/original", true, None, None)
            .await
            .unwrap();
        advance_to_metadata(&db, "req-3", &claimed.claim).await;
        let outcome = db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-3".to_string(),
                claim: claimed.claim,
                conversation: conv.clone(),
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/a".to_string(),
                },
                git_publication: None,
            })
            .await;
        assert!(outcome.is_err());
        assert!(db
            .get_product_creation_job("req-3")
            .await
            .unwrap()
            .unwrap()
            .published_conversation_id
            .is_none());
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM product_conversation_work_scopes WHERE product_conversation_id = ?1")
            .bind(conv.product_conversation_id.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(exists, 0);
    }

    #[tokio::test]
    async fn product_creation_publication_requires_live_claim() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-live-publish", &intent("/repo/a", "publish"))
            .await
            .unwrap();
        let now = Utc::now();
        let claim = db
            .claim_next_product_creation("worker", "token", now, chrono::Duration::seconds(30))
            .await
            .unwrap()
            .unwrap();
        let accepted = db
            .get_product_creation_job("req-live-publish")
            .await
            .unwrap()
            .unwrap();
        let conv = published_conversation(
            "conv-live-publish",
            accepted.product_conversation_id,
            WorkScopeId::new(),
            "/repo/a",
        );
        advance_to_metadata(&db, "req-live-publish", &claim.claim).await;
        sqlx::query(
            "UPDATE product_creation_jobs SET claim_lease_until_unix_micros = ?2 WHERE request_id = ?1",
        )
        .bind("req-live-publish")
        .bind(now.timestamp_micros())
        .execute(db.pool())
        .await
        .unwrap();
        let expired_claim = ProductCreationClaim {
            lease_until: now,
            ..claim.claim
        };
        let ok = db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-live-publish".to_string(),
                claim: expired_claim,
                conversation: conv,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/a".to_string(),
                },
                git_publication: None,
            })
            .await
            .unwrap();
        assert!(!ok);
    }

    #[tokio::test]
    async fn product_creation_publish_records_atomic_owner() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-4", &intent("/repo/a", "owner"))
            .await
            .unwrap();
        let claimed = db
            .claim_next_product_creation(
                "worker",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let accepted = db.get_product_creation_job("req-4").await.unwrap().unwrap();
        let product_id = accepted.product_conversation_id.clone();
        let scope = WorkScopeId::new();
        let conv =
            published_conversation("conv-owner", product_id.clone(), scope.clone(), "/repo/a");
        advance_to_metadata(&db, "req-4", &claimed.claim).await;
        let ok = db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-4".to_string(),
                claim: claimed.claim,
                conversation: conv,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/a".to_string(),
                },
                git_publication: None,
            })
            .await
            .unwrap();
        assert!(ok);
        let owner: (String, String) = sqlx::query_as("SELECT product_conversation_id, work_scope_id FROM product_conversation_work_scopes WHERE product_conversation_id = ?1")
            .bind(product_id.as_str())
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(owner.0, product_id.as_str());
        assert_eq!(owner.1, scope.as_str());
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "Checks foundation lifecycle and retained creation receipt through publication and deletion"
    )]
    async fn git_product_creation_publish_keeps_repository_foundation_dormant() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-cascade", &intent("/repo/hidden", "owner"))
            .await
            .unwrap();
        let claim = db
            .claim_next_product_creation(
                "worker",
                "token",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let accepted = db
            .get_product_creation_job("req-cascade")
            .await
            .unwrap()
            .unwrap();
        let scope = WorkScopeId::new();
        let product_id = accepted.product_conversation_id.clone();
        let conv = published_conversation(
            "conv-cascade",
            product_id.clone(),
            scope.clone(),
            "/repo/hidden",
        );
        advance_to_metadata(&db, "req-cascade", &claim.claim).await;
        assert!(db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-cascade".to_string(),
                claim: claim.claim,
                conversation: conv,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::AllocatedWorktree {
                    cwd: "/repo/hidden/.phoenix/worktrees/conv-cascade".to_string(),
                    worktree_path: "/repo/hidden/.phoenix/worktrees/conv-cascade".to_string(),
                    branch_name: None,
                    base_branch: Some("main".to_string()),
                },
                git_publication: Some(ProductCreationGitPublicationFacts {
                    exact_checkout_oid: "0123456789abcdef".to_string(),
                    repository_root: "/repo/hidden".to_string(),
                }),
            })
            .await
            .unwrap());
        let environment: (String, String, String) = sqlx::query_as(
            "SELECT environment_kind, cwd, worktree_path
             FROM work_scope_environments WHERE work_scope_id = ?1",
        )
        .bind(scope.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(environment.0, "allocated_worktree");
        assert_eq!(
            environment.1,
            "/repo/hidden/.phoenix/worktrees/conv-cascade"
        );
        assert_eq!(
            environment.2,
            "/repo/hidden/.phoenix/worktrees/conv-cascade"
        );
        let staging_facts: (String, String) = sqlx::query_as(
            "SELECT staging_repo_root, staging_exact_oid
             FROM product_creation_jobs WHERE request_id = 'req-cascade'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(staging_facts.0, "/repo/hidden");
        assert_eq!(staging_facts.1, "0123456789abcdef");
        let repository_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM git_repositories")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let attachment_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM work_scope_git_repositories")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let locator_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM git_repository_locator_observations")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(repository_count, 0);
        assert_eq!(attachment_count, 0);
        assert_eq!(locator_count, 0);
        db.delete_conversation("conv-cascade").await.unwrap();
        let retained = db
            .get_product_creation_job("req-cascade")
            .await
            .unwrap()
            .expect("published creation receipt survives target deletion");
        assert_eq!(retained.product_conversation_id, product_id);
        assert_eq!(
            retained.published_conversation_id.as_deref(),
            Some("conv-cascade")
        );
        let replay = db
            .accept_product_creation("req-cascade", &intent("/repo/hidden", "owner"))
            .await
            .unwrap();
        let ProductCreationAcceptOutcome::Replayed(replay) = replay else {
            panic!("same creation intent must replay the retained receipt");
        };
        assert_eq!(replay.product_conversation_id, product_id);
        assert!(db
            .claim_next_product_creation(
                "worker-replay",
                "token-replay",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .is_none());
        assert!(matches!(
            db.accept_product_creation("req-cascade", &intent("/repo/hidden", "changed"))
                .await
                .unwrap(),
            ProductCreationAcceptOutcome::Conflict(_)
        ));
    }

    #[tokio::test]
    async fn product_creation_cross_owner_rejected() {
        let db = Database::open_in_memory().await.unwrap();
        let shared_scope = WorkScopeId::new();
        db.accept_product_creation("req-a", &intent("/repo/a", "a"))
            .await
            .unwrap();
        let accepted_a = db.get_product_creation_job("req-a").await.unwrap().unwrap();
        let conv_a = published_conversation(
            "conv-a-owner",
            accepted_a.product_conversation_id.clone(),
            shared_scope.clone(),
            "/repo/a",
        );
        let claim_a = db
            .claim_next_product_creation(
                "worker-a",
                "token-a",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        advance_to_metadata(&db, "req-a", &claim_a.claim).await;
        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-a".to_string(),
            claim: claim_a.claim,
            conversation: conv_a,
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/a".to_string(),
            },
            git_publication: None,
        })
        .await
        .unwrap();
        db.accept_product_creation("req-b", &intent("/repo/b", "b"))
            .await
            .unwrap();
        let claim_b = db
            .claim_next_product_creation(
                "worker-b",
                "token-b",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let accepted_b = db.get_product_creation_job("req-b").await.unwrap().unwrap();
        let conv_b = published_conversation(
            "conv-b-owner",
            accepted_b.product_conversation_id.clone(),
            shared_scope.clone(),
            "/repo/b",
        );
        advance_to_metadata(&db, "req-b", &claim_b.claim).await;
        let err = db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-b".to_string(),
                claim: claim_b.claim,
                conversation: conv_b,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/b".to_string(),
                },
                git_publication: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::Sqlx(_)));
    }

    #[tokio::test]
    async fn recent_management_roots_include_pre_creation_scope_and_current_project_locator() {
        let db = Database::open_in_memory().await.unwrap();
        let project = db.find_or_create_project("/repo/current").await.unwrap();
        let conversation = db
            .create_conversation_with_project(
                "pre-creation-recents",
                "pre-creation-recents",
                "/repo/current/.phoenix/worktrees/old",
                true,
                None,
                None,
                Some(&project.id),
                &ConvMode::Explore {
                    worktree_path: Some(
                        phoenix_core::domain::db_schema::NonEmptyString::new(
                            "/repo/current/.phoenix/worktrees/old",
                        )
                        .unwrap(),
                    ),
                    next_taskmd_id_hint: None,
                },
                None,
                None,
                None,
                LlmLanguage::default(),
            )
            .await
            .unwrap();
        assert!(conversation.attached_work_scope_id.is_some());

        sqlx::query("UPDATE projects SET canonical_path = '/repo/moved' WHERE id = ?1")
            .bind(&project.id)
            .execute(db.pool())
            .await
            .unwrap();

        assert_eq!(
            db.recent_distinct_management_roots(5).await.unwrap(),
            vec!["/repo/moved".to_string()]
        );
    }

    #[tokio::test]
    async fn current_project_locator_wins_over_stale_creation_staging_root() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-current-root", &intent("/repo/stale", "publish"))
            .await
            .unwrap();
        let claim = db
            .claim_next_product_creation(
                "worker-current-root",
                "token-current-root",
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap()
            .unwrap();
        let accepted = db
            .get_product_creation_job("req-current-root")
            .await
            .unwrap()
            .unwrap();
        let conversation_id = "conv-current-root";
        advance_to_metadata(&db, "req-current-root", &claim.claim).await;
        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-current-root".to_string(),
            claim: claim.claim,
            conversation: published_conversation(
                conversation_id,
                accepted.product_conversation_id,
                WorkScopeId::new(),
                "/repo/stale/.phoenix/worktrees/one",
            ),
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::AllocatedWorktree {
                cwd: "/repo/stale/.phoenix/worktrees/one".to_string(),
                worktree_path: "/repo/stale/.phoenix/worktrees/one".into(),
                branch_name: None,
                base_branch: None,
            },
            git_publication: Some(ProductCreationGitPublicationFacts {
                exact_checkout_oid: "stale-oid".to_string(),
                repository_root: "/repo/stale".to_string(),
            }),
        })
        .await
        .unwrap();
        sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'published', claim_worker_id = NULL, claim_token = NULL,
                 claim_lease_until_unix_micros = NULL, delivery_retry_at_unix_micros = NULL
             WHERE request_id = 'req-current-root'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let project = db.find_or_create_project("/repo/current").await.unwrap();
        sqlx::query("UPDATE conversations SET project_id = ?1 WHERE id = ?2")
            .bind(&project.id)
            .bind(conversation_id)
            .execute(db.pool())
            .await
            .unwrap();

        assert_eq!(
            db.recent_distinct_management_roots(5).await.unwrap(),
            vec!["/repo/current".to_string()]
        );
        let repositories: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM git_repositories")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let attachments: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM work_scope_git_repositories")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(repositories, 0);
        assert_eq!(attachments, 0);
    }

    #[tokio::test]
    async fn product_creation_recent_management_roots_rank_by_support_then_recency() {
        let db = Database::open_in_memory().await.unwrap();
        for (idx, cwd) in [(1, "/repo/a"), (2, "/repo/b"), (3, "/repo/a")] {
            let req = format!("req-rec-{idx}");
            db.accept_product_creation(&req, &intent(cwd, "publish"))
                .await
                .unwrap();
            let claim = db
                .claim_next_product_creation(
                    &format!("worker-{idx}"),
                    &format!("token-{idx}"),
                    Utc::now(),
                    chrono::Duration::minutes(5),
                )
                .await
                .unwrap()
                .unwrap();
            let accepted = db.get_product_creation_job(&req).await.unwrap().unwrap();
            let conv = published_conversation(
                &format!("conv-rec-{idx}"),
                accepted.product_conversation_id,
                WorkScopeId::new(),
                cwd,
            );
            advance_to_metadata(&db, &req, &claim.claim).await;
            db.publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: req,
                claim: claim.claim,
                conversation: conv,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: cwd.to_string(),
                },
                git_publication: Some(ProductCreationGitPublicationFacts {
                    exact_checkout_oid: format!("oid-{idx}"),
                    repository_root: cwd.to_string(),
                }),
            })
            .await
            .unwrap();
            sqlx::query(
                "UPDATE product_creation_jobs
                 SET status = 'published', claim_worker_id = NULL, claim_token = NULL,
                     claim_lease_until_unix_micros = NULL, delivery_retry_at_unix_micros = NULL
                 WHERE request_id = ?1",
            )
            .bind(format!("req-rec-{idx}"))
            .execute(db.pool())
            .await
            .unwrap();
            // test-timing-allow: recency ordering is the behavior under test and SQLite stores the publication wall clock.
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let cwds = db.recent_distinct_management_roots(5).await.unwrap();
        assert_eq!(cwds, vec!["/repo/a".to_string(), "/repo/b".to_string()]);
    }
}
