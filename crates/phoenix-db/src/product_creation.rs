#![allow(clippy::missing_errors_doc)]

use super::*;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_core::llm_language::LlmLanguage;

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
pub struct ProductCreationIntent {
    pub cwd: String,
    pub objective: String,
    pub model: Option<String>,
    pub effort: Option<ModelEffort>,
    pub llm_language: LlmLanguage,
    pub images: Vec<ProductCreationImage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationJobRecord {
    pub request_id: String,
    pub product_conversation_id: ProductConversationId,
    pub intent: ProductCreationIntent,
    pub status: String,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationCleanupJob {
    pub job: ProductCreationJobRecord,
    pub claim: ProductCreationCleanupClaim,
    pub reservations: Vec<ProductCreationResourceReservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationRepositoryAttachment {
    pub repository_id: Option<String>,
    pub exact_checkout_oid: String,
    pub repository_root: String,
    pub git_common_dir: String,
}

#[derive(Debug, Clone)]
pub struct ProductCreationPublishInput {
    pub request_id: String,
    pub claim: ProductCreationClaim,
    pub conversation: Conversation,
    pub authority_kind: AuthorityKind,
    pub environment: EnvironmentContext,
    pub repository_attachment: Option<ProductCreationRepositoryAttachment>,
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
    })
}

#[allow(clippy::needless_pass_by_value)]
fn parse_product_creation_job_row(row: SqliteRow) -> Result<ProductCreationJobRecord, sqlx::Error> {
    let request_id: String = row.try_get("request_id")?;
    let images_json: String = row.try_get("images_json")?;
    let images: Vec<ProductCreationImage> =
        serde_json::from_str(&images_json).map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    Ok(ProductCreationJobRecord {
        request_id,
        product_conversation_id: ProductConversationId::parse(
            &row.try_get::<String, _>("product_conversation_id")?,
        )
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
        intent: ProductCreationIntent {
            cwd: row.try_get("cwd")?,
            objective: row.try_get("objective")?,
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
        sqlx::query(
            "SELECT j.request_id, j.product_conversation_id, j.cwd, j.objective, j.model, j.effort, j.llm_language, j.status,
                    j.accepted_at_unix_micros, j.updated_at_unix_micros, j.attempt_count,
                    j.claim_generation, j.claim_worker_id, j.claim_token,
                    j.claim_lease_until_unix_micros, j.retry_at_unix_micros,
                    j.delivery_attempt_count, j.delivery_retry_at_unix_micros,
                    j.pin_exact_checkout_oid, j.pin_logical_base, j.pin_freshness,
                    j.staging_path, j.staging_repo_root, j.staging_exact_oid,
                    j.published_product_id, j.published_conversation_id, j.last_error,
                    j.cancelled_at_unix_micros, j.deletion_requested_at_unix_micros,
                    COALESCE((
                        SELECT json_group_array(json_object('media_type', i.media_type, 'data', i.data))
                        FROM product_creation_job_images i
                        WHERE i.request_id = j.request_id
                        ORDER BY i.ordinal
                    ), '[]') AS images_json
             FROM product_creation_jobs j
             WHERE j.request_id = ?1",
        )
        .bind(request_id)
        .try_map(parse_product_creation_job_row)
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::Sqlx)
    }

    pub async fn accept_product_creation(
        &self,
        request_id: &str,
        intent: &ProductCreationIntent,
    ) -> DbResult<ProductCreationAcceptOutcome> {
        normalize_product_creation_intent(intent)?;
        let accepted_product_conversation_id = ProductConversationId::new();
        let now = unix_micros_now();
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query(
            "SELECT request_id, product_conversation_id, cwd, objective, model, effort, llm_language, status,
                    accepted_at_unix_micros, updated_at_unix_micros, attempt_count,
                    claim_generation, claim_worker_id, claim_token,
                    claim_lease_until_unix_micros, retry_at_unix_micros,
                    delivery_attempt_count, delivery_retry_at_unix_micros,
                    pin_exact_checkout_oid, pin_logical_base, pin_freshness,
                    staging_path, staging_repo_root, staging_exact_oid,
                    published_product_id, published_conversation_id, last_error,
                    cancelled_at_unix_micros, deletion_requested_at_unix_micros,
                    COALESCE((
                        SELECT json_group_array(json_object('media_type', i.media_type, 'data', i.data))
                        FROM product_creation_job_images i
                        WHERE i.request_id = j.request_id
                        ORDER BY i.ordinal
                    ), '[]') AS images_json
             FROM product_creation_jobs j
             WHERE j.request_id = ?1",
        )
        .bind(request_id)
        .try_map(parse_product_creation_job_row)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(existing) = existing {
            tx.rollback().await?;
            return Ok(if existing.intent == *intent {
                ProductCreationAcceptOutcome::Replayed(existing)
            } else {
                ProductCreationAcceptOutcome::Conflict(existing)
            });
        }
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO product_creation_jobs (
                request_id, product_conversation_id, cwd, objective, model, effort, llm_language, status,
                accepted_at_unix_micros, updated_at_unix_micros
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'accepted', ?8, ?8)",
        )
        .bind(request_id)
        .bind(accepted_product_conversation_id.as_str())
        .bind(&intent.cwd)
        .bind(&intent.objective)
        .bind(&intent.model)
        .bind(intent.effort.map(ModelEffort::as_wire_name))
        .bind(intent.llm_language.as_str())
        .bind(now)
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
            return Ok(if existing.intent == *intent {
                ProductCreationAcceptOutcome::Replayed(existing)
            } else {
                ProductCreationAcceptOutcome::Conflict(existing)
            });
        }
        for (ordinal, image) in intent.images.iter().enumerate() {
            sqlx::query(
                "INSERT INTO product_creation_job_images (request_id, ordinal, media_type, data)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(request_id)
            .bind(
                i64::try_from(ordinal)
                    .map_err(|_| DbError::Serialization("too many images".to_string()))?,
            )
            .bind(&image.media_type)
            .bind(&image.data)
            .execute(&mut *tx)
            .await?;
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
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'claimed', claim_generation = claim_generation + 1,
                 claim_worker_id = ?2, claim_token = ?3, claim_lease_until_unix_micros = ?4,
                 retry_at_unix_micros = NULL, updated_at_unix_micros = ?1
             WHERE request_id = ?5 AND (
                 status = 'accepted'
                 OR (status = 'retry_scheduled' AND retry_at_unix_micros <= ?1)
                 OR (status = 'claimed' AND claim_lease_until_unix_micros <= ?1)
             )",
        )
        .bind(now_str)
        .bind(worker_id)
        .bind(token)
        .bind(lease_until)
        .bind(request_id)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        let job = self
            .get_product_creation_job(request_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("claimed product creation job missing".to_string())
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

    pub async fn claim_next_product_creation(
        &self,
        worker_id: &str,
        token: &str,
        now: DateTime<Utc>,
        lease_duration: chrono::Duration,
    ) -> DbResult<Option<ClaimedProductCreationJob>> {
        let now_str = datetime_to_unix_micros(now);
        let lease_until = datetime_to_unix_micros(now + lease_duration);
        let mut tx = self.pool.begin().await?;
        let candidate: Option<(String, String)> = sqlx::query_as(
            "SELECT request_id, status
             FROM product_creation_jobs
             WHERE status = 'accepted'
                OR (status = 'retry_scheduled' AND retry_at_unix_micros <= ?1)
                OR (status = 'claimed' AND claim_lease_until_unix_micros <= ?1)
             ORDER BY CASE status WHEN 'accepted' THEN accepted_at_unix_micros ELSE retry_at_unix_micros END ASC, accepted_at_unix_micros ASC, request_id ASC
             LIMIT 1",
        )
        .bind(now_str)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((request_id, status)) = candidate else {
            tx.rollback().await?;
            return Ok(None);
        };
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'claimed',
                 claim_generation = claim_generation + 1,
                 claim_worker_id = ?1,
                 claim_token = ?2,
                 claim_lease_until_unix_micros = ?3,
                 retry_at_unix_micros = NULL,
                 updated_at_unix_micros = ?4
             WHERE request_id = ?5 AND status = ?6
               AND (
                    status = 'accepted'
                    OR (status = 'retry_scheduled' AND retry_at_unix_micros <= ?4)
                    OR (status = 'claimed' AND claim_lease_until_unix_micros <= ?4)
               )",
        )
        .bind(worker_id)
        .bind(token)
        .bind(lease_until)
        .bind(now_str)
        .bind(&request_id)
        .bind(&status)
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
                DbError::Serialization("claimed product creation job missing".to_string())
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
             WHERE request_id = ?3 AND status = 'claimed' AND claim_generation = ?4
               AND claim_worker_id = ?5 AND claim_token = ?6
               AND claim_lease_until_unix_micros > ?2",
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
                 updated_at_unix_micros = ?5
             WHERE request_id = ?1 AND status = 'delivery_pending'
               AND delivery_attempt_count = ?6",
        )
        .bind(request_id)
        .bind(next_attempt)
        .bind(exhausted)
        .bind(datetime_to_unix_micros(
            now + chrono::Duration::seconds(delay_seconds),
        ))
        .bind(datetime_to_unix_micros(now))
        .bind(record.delivery_attempt_count)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn mark_product_creation_cleanup_ambiguous(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'cleanup_ambiguous', claim_worker_id = NULL, claim_token = NULL,
                 claim_lease_until_unix_micros = NULL, retry_at_unix_micros = NULL,
                 updated_at_unix_micros = ?4
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?5
               AND claim_lease_until_unix_micros > ?4",
        )
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
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
        let now = unix_micros_now();
        let mut tx = self.pool.begin().await?;
        let claimed: Option<(String, String)> = sqlx::query_as(
            "SELECT request_id, product_conversation_id FROM product_creation_jobs
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?4
               AND claim_lease_until_unix_micros > ?5",
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
        let attachment_matches_environment = !matches!(
            (&input.environment, &input.repository_attachment),
            (EnvironmentContext::AllocatedWorktree { .. }, None)
                | (EnvironmentContext::None, Some(_))
        );
        if !attachment_matches_environment {
            tx.rollback().await?;
            return Err(DbError::Serialization(
                "allocated-worktree publication requires a repository attachment".to_string(),
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
        if let Some(attachment) = input.repository_attachment.as_ref() {
            let repository_root = attachment.repository_root.as_str();
            let git_common_dir = attachment.git_common_dir.as_str();
            let repository_id = attachment
                .repository_id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            sqlx::query("INSERT OR IGNORE INTO git_repositories (id) VALUES (?1)")
                .bind(&repository_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "INSERT INTO work_scope_git_repositories (work_scope_id, repository_id)
                 VALUES (?1, ?2)",
            )
            .bind(scope_id.as_str())
            .bind(&repository_id)
            .execute(&mut *tx)
            .await?;
            for (locator_kind, path) in [
                ("management_root", repository_root),
                ("common_dir", git_common_dir),
            ] {
                sqlx::query(
                    "INSERT INTO git_repository_locator_observations (
                        repository_id, locator_kind, status, path, observed_at_unix_micros
                     ) VALUES (?1, ?2, 'present', ?3, ?4)
                     ON CONFLICT(repository_id, locator_kind)
                     DO UPDATE SET status = excluded.status, path = excluded.path,
                                   observed_at_unix_micros = excluded.observed_at_unix_micros",
                )
                .bind(&repository_id)
                .bind(locator_kind)
                .bind(path)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
        }
        sqlx::query(
            "INSERT INTO product_conversation_work_scopes (product_conversation_id, work_scope_id)
             VALUES (?1, ?2)",
        )
        .bind(input.conversation.product_conversation_id.as_str())
        .bind(scope_id.as_str())
        .execute(&mut *tx)
        .await?;
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'delivery_pending', retry_at_unix_micros = NULL, staging_path = ?2,
                 staging_repo_root = ?3, staging_exact_oid = ?4,
                 published_product_id = ?5, published_conversation_id = ?6,
                 delivery_retry_at_unix_micros = ?7, updated_at_unix_micros = ?8
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?9
               AND claim_worker_id = ?10 AND claim_token = ?11",
        )
        .bind(&input.request_id)
        .bind(match &input.environment {
            EnvironmentContext::AllocatedWorktree { worktree_path, .. } => {
                Some(worktree_path.as_str())
            }
            EnvironmentContext::UnownedCwd { cwd } => Some(cwd.as_str()),
            EnvironmentContext::None => None,
        })
        .bind(
            input
                .repository_attachment
                .as_ref()
                .map(|attachment| attachment.repository_root.as_str()),
        )
        .bind(
            input
                .repository_attachment
                .as_ref()
                .map(|attachment| attachment.exact_checkout_oid.as_str()),
        )
        .bind(input.conversation.product_conversation_id.as_str())
        .bind(&input.conversation.id)
        .bind(now + 2_000_000)
        .bind(now)
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
             WHERE request_id = ?2 AND status = 'delivery_failed'",
        )
        .bind(now)
        .bind(request_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
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
        let now = unix_micros_now();
        let mut tx = self.pool.begin().await?;
        let published_conversation_id: Option<String> = sqlx::query_scalar(
            "SELECT published_conversation_id FROM product_creation_jobs
             WHERE request_id = ?1 AND status = 'delivery_pending'
               AND claim_generation = ?2 AND claim_worker_id = ?3 AND claim_token = ?4
               AND claim_lease_until_unix_micros > ?5",
        )
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        let Some(published_conversation_id) = published_conversation_id else {
            tx.rollback().await?;
            return Ok(false);
        };
        sqlx::query("UPDATE conversations SET archived = 0 WHERE id = ?1")
            .bind(&published_conversation_id)
            .execute(&mut *tx)
            .await?;
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'published', claim_worker_id = NULL, claim_token = NULL,
                 claim_lease_until_unix_micros = NULL, delivery_retry_at_unix_micros = NULL,
                 updated_at_unix_micros = ?1
             WHERE request_id = ?2 AND status = 'delivery_pending'
               AND claim_generation = ?3 AND claim_worker_id = ?4 AND claim_token = ?5
               AND claim_lease_until_unix_micros > ?1",
        )
        .bind(now)
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
            "SELECT id, request_id, generation, repository_identity, resource_identity, status
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

    pub async fn mark_product_creation_resource_present(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        resource_identity: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let updated = sqlx::query(
            "UPDATE product_creation_resource_reservations
             SET status = 'present', updated_at_unix_micros = ?1
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
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn cancel_product_creation(
        &self,
        request_id: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let now_micros = datetime_to_unix_micros(now);
        let mut tx = self.pool.begin().await?;
        let job: Option<(i64, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT claim_generation, published_product_id, published_conversation_id
             FROM product_creation_jobs
             WHERE request_id = ?1
               AND status IN ('accepted', 'claimed', 'retry_scheduled')",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((generation, published_product_id, published_conversation_id)) = job else {
            tx.rollback().await?;
            return Ok(false);
        };
        if published_product_id.is_some() || published_conversation_id.is_some() {
            tx.rollback().await?;
            return Ok(false);
        }
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'cancelling', claim_generation = claim_generation + 1,
                 claim_worker_id = NULL, claim_token = NULL, claim_lease_until_unix_micros = NULL,
                 retry_at_unix_micros = NULL,
                 cleanup_worker_id = NULL, cleanup_token = NULL, cleanup_lease_until_unix_micros = NULL,
                 last_error = NULL, cancelled_at_unix_micros = NULL,
                 deletion_requested_at_unix_micros = NULL, updated_at_unix_micros = ?2
             WHERE request_id = ?1 AND claim_generation = ?3
               AND status IN ('accepted', 'claimed', 'retry_scheduled')
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
             WHERE request_id = ?1 AND status IN ('reserved', 'present')",
        )
        .bind(request_id)
        .bind(now_micros)
        .bind(generation + 1)
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
             WHERE request_id = ?1 AND status IN ('reserved', 'present')",
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
            "SELECT request_id, product_conversation_id, cwd, objective, model, effort, llm_language, status,
                    accepted_at_unix_micros, updated_at_unix_micros, attempt_count,
                    claim_generation, claim_worker_id, claim_token, claim_lease_until_unix_micros,
                    retry_at_unix_micros, cleanup_worker_id, cleanup_token, cleanup_lease_until_unix_micros,
                    delivery_attempt_count, delivery_retry_at_unix_micros,
                    pin_exact_checkout_oid, pin_logical_base, pin_freshness,
                    staging_path, staging_repo_root, staging_exact_oid,
                    published_product_id, published_conversation_id, last_error,
                    cancelled_at_unix_micros, deletion_requested_at_unix_micros,
                    COALESCE((SELECT json_group_array(json_object('media_type', i.media_type, 'data', i.data))
                              FROM product_creation_job_images i WHERE i.request_id = j.request_id
                              ORDER BY i.ordinal), '[]') AS images_json
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

    pub async fn release_product_creation_resource(
        &self,
        cleanup: &ProductCreationCleanupJob,
        reservation_id: &str,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let result = sqlx::query(
            "UPDATE product_creation_resource_reservations
             SET status = 'released', updated_at_unix_micros = ?1
             WHERE id = ?2 AND request_id = ?3 AND generation = ?4
               AND status = 'cleanup_required'
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
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn finish_product_creation_cleanup(
        &self,
        cleanup: &ProductCreationCleanupJob,
        now: DateTime<Utc>,
    ) -> DbResult<bool> {
        let now_micros = datetime_to_unix_micros(now);
        let mut tx = self.pool.begin().await?;
        let authoritative: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM product_creation_jobs
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
        if authoritative.is_none() {
            tx.rollback().await?;
            return Ok(false);
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
                 SELECT retry_at_unix_micros AS deadline FROM product_creation_jobs WHERE status = 'retry_scheduled'
                 UNION ALL SELECT claim_lease_until_unix_micros FROM product_creation_jobs WHERE status = 'claimed'
                 UNION ALL SELECT CASE
                     WHEN delivery_retry_at_unix_micros IS NULL THEN claim_lease_until_unix_micros
                     WHEN claim_lease_until_unix_micros IS NULL THEN delivery_retry_at_unix_micros
                     WHEN delivery_retry_at_unix_micros > claim_lease_until_unix_micros THEN delivery_retry_at_unix_micros
                     ELSE claim_lease_until_unix_micros
                 END FROM product_creation_jobs WHERE status = 'delivery_pending'
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

    pub async fn repository_management_root_for_work_scope(
        &self,
        work_scope_id: &WorkScopeId,
    ) -> DbResult<Option<String>> {
        sqlx::query_scalar(
            "SELECT locator.path
             FROM work_scope_git_repositories attachment
             JOIN git_repository_locator_observations locator
               ON locator.repository_id = attachment.repository_id
              AND locator.locator_kind = 'management_root' AND locator.status = 'present'
             WHERE attachment.work_scope_id = ?1",
        )
        .bind(work_scope_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::from)
    }

    pub async fn work_scope_has_git_repository(
        &self,
        work_scope_id: &WorkScopeId,
    ) -> DbResult<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_scope_git_repositories WHERE work_scope_id = ?1",
        )
        .bind(work_scope_id.as_str())
        .fetch_one(&self.pool)
        .await?;
        Ok(count == 1)
    }

    pub async fn recent_distinct_published_product_creation_cwds(
        &self,
        limit: usize,
    ) -> DbResult<Vec<String>> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT locator.path
             FROM conversations conversation
             JOIN product_conversations product
               ON product.id = conversation.product_conversation_id
              AND product.kind = 'ordinary'
              AND product.ordinary_lifecycle IN ('open', 'history')
             JOIN work_scope_git_repositories attachment
               ON attachment.work_scope_id = conversation.work_scope_id
             JOIN git_repository_locator_observations locator
               ON locator.repository_id = attachment.repository_id
              AND locator.locator_kind = 'management_root' AND locator.status = 'present'
             WHERE conversation.user_initiated = 1 AND conversation.runtime_role = 'user'
               AND NOT EXISTS (
                   SELECT 1 FROM product_creation_jobs job
                   WHERE job.published_conversation_id = conversation.id
                     AND job.status <> 'published'
               )
             GROUP BY attachment.repository_id, locator.path
             ORDER BY COUNT(DISTINCT conversation.id) DESC,
                      MAX(conversation.updated_at) DESC,
                      locator.path DESC
             LIMIT ?1",
        )
        .bind(
            i64::try_from(limit)
                .map_err(|_| DbError::Serialization("limit overflow".to_string()))?,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod product_creation_tests {
    use super::*;
    use phoenix_core::llm_language::LlmLanguage;

    fn intent(request_cwd: &str, objective: &str) -> ProductCreationIntent {
        ProductCreationIntent {
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
            chain_name: None,
            llm_language: LlmLanguage::Caveman,
            spawned_from_conversation_id: None,
        }
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
    }

    #[tokio::test]
    async fn product_creation_snapshots_llm_language_and_retains_last_error() {
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
        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-delivery".to_string(),
            claim: claim.claim,
            conversation: conv,
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/a".to_string(),
            },
            repository_attachment: Some(ProductCreationRepositoryAttachment {
                repository_id: None,
                exact_checkout_oid: "repo-id".to_string(),
                repository_root: "/repo/a".to_string(),
                git_common_dir: "/repo/a/.git".to_string(),
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
        assert!(db
            .schedule_product_creation_delivery_retry("req-delivery", now)
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
        assert!(db
            .schedule_product_creation_delivery_retry("req-delivery", now)
            .await
            .unwrap());
        assert!(db
            .schedule_product_creation_delivery_retry("req-delivery", now)
            .await
            .unwrap());
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
        assert!(db
            .schedule_product_creation_delivery_retry("req-delivery", now)
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
        assert!(!db
            .schedule_product_creation_delivery_retry("req-delivery", now)
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
        db1.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-delivery-claim".to_string(),
            claim: claim.claim,
            conversation: conv,
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/a".to_string(),
            },
            repository_attachment: Some(ProductCreationRepositoryAttachment {
                repository_id: None,
                exact_checkout_oid: "repo-id".to_string(),
                repository_root: "/repo/a".to_string(),
                git_common_dir: "/repo/a/.git".to_string(),
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

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn delivery_completion_requires_current_claim_and_exact_acceptance() {
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
            accepted.product_conversation_id,
            WorkScopeId::new(),
            "/repo/a",
        );
        conv.archived = true;

        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-delivery-complete".to_string(),
            claim: claim.claim.clone(),
            conversation: conv,
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/a".to_string(),
            },
            repository_attachment: Some(ProductCreationRepositoryAttachment {
                repository_id: None,
                exact_checkout_oid: "repo-id".to_string(),
                repository_root: "/repo/a".to_string(),
                git_common_dir: "/repo/a/.git".to_string(),
            }),
        })
        .await
        .unwrap();
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
                &delivery.job.intent,
            )
            .await
            .unwrap());
        db.append_steering_entry(
            "conv-delivery-complete",
            &phoenix_core::domain::sm_event::SteerEntry {
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
                &delivery.job.intent,
            )
            .await
            .unwrap());
        assert!(db
            .complete_product_creation_delivery(
                "req-delivery-complete",
                &delivery.claim,
                &SteeringAcceptanceFingerprint::Exact(
                    "product-create:req-delivery-complete".to_string(),
                ),
                &delivery.job.intent,
            )
            .await
            .unwrap());
        assert!(
            !db.get_conversation("conv-delivery-complete")
                .await
                .unwrap()
                .archived
        );
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
        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-published-lifecycle".to_string(),
            claim: claimed.claim,
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
            repository_attachment: Some(ProductCreationRepositoryAttachment {
                repository_id: None,
                repository_root: "/repo/a".to_string(),
                git_common_dir: "/repo/a/.git".to_string(),
                exact_checkout_oid: "abc123".to_string(),
            }),
        })
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
        let outcome = db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-3".to_string(),
                claim: claimed.claim,
                conversation: conv.clone(),
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/a".to_string(),
                },
                repository_attachment: None,
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
                repository_attachment: None,
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
        let ok = db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-4".to_string(),
                claim: claimed.claim,
                conversation: conv,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/a".to_string(),
                },
                repository_attachment: None,
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
    async fn product_creation_publish_attaches_hidden_repository_and_cascade_delete_clears_job() {
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
        assert!(db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-cascade".to_string(),
                claim: claim.claim,
                conversation: conv,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/hidden".to_string(),
                },
                repository_attachment: Some(ProductCreationRepositoryAttachment {
                    repository_id: Some("repo-hidden-id".to_string()),
                    exact_checkout_oid: "0123456789abcdef".to_string(),
                    repository_root: "/repo/hidden".to_string(),
                    git_common_dir: "/repo/hidden/.git".to_string(),
                }),
            })
            .await
            .unwrap());
        let attached: (String, String) = sqlx::query_as(
            "SELECT work_scope_id, repository_id FROM work_scope_git_repositories WHERE work_scope_id = ?1",
        )
        .bind(scope.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(attached.0, scope.as_str());
        assert_eq!(attached.1, "repo-hidden-id");
        let locators: Vec<(String, String)> = sqlx::query_as(
            "SELECT locator_kind, path FROM git_repository_locator_observations
             WHERE repository_id = 'repo-hidden-id' ORDER BY locator_kind",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            locators,
            vec![
                ("common_dir".to_string(), "/repo/hidden/.git".to_string()),
                ("management_root".to_string(), "/repo/hidden".to_string()),
            ]
        );
        sqlx::query("DELETE FROM product_conversations WHERE id = ?1")
            .bind(product_id.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        assert!(db
            .get_product_creation_job("req-cascade")
            .await
            .unwrap()
            .is_none());
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
        db.publish_product_creation_atomically(&ProductCreationPublishInput {
            request_id: "req-a".to_string(),
            claim: claim_a.claim,
            conversation: conv_a,
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: "/repo/a".to_string(),
            },
            repository_attachment: None,
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
        let err = db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-b".to_string(),
                claim: claim_b.claim,
                conversation: conv_b,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/b".to_string(),
                },
                repository_attachment: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::Sqlx(_)));
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
            db.publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: req,
                claim: claim.claim,
                conversation: conv,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: cwd.to_string(),
                },
                repository_attachment: Some(ProductCreationRepositoryAttachment {
                    repository_id: Some(format!("repo-{}", cwd.replace('/', "-"))),
                    exact_checkout_oid: format!("oid-{idx}"),
                    repository_root: cwd.to_string(),
                    git_common_dir: format!("{cwd}/.git"),
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
        let cwds = db
            .recent_distinct_published_product_creation_cwds(5)
            .await
            .unwrap();
        assert_eq!(cwds, vec!["/repo/a".to_string(), "/repo/b".to_string()]);
    }
}
