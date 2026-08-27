#![allow(clippy::missing_errors_doc)]

use super::*;
use phoenix_core::domain::product_conversation::ProductConversationId;

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
    pub images: Vec<ProductCreationImage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCreationJobRecord {
    pub request_id: String,
    pub intent: ProductCreationIntent,
    pub status: String,
    pub accepted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub claim_generation: i64,
    pub claim_worker_id: Option<String>,
    pub claim_token: Option<String>,
    pub claim_lease_until: Option<DateTime<Utc>>,
    pub retry_at: Option<DateTime<Utc>>,
    pub pin_exact_checkout_oid: Option<String>,
    pub pin_logical_base: Option<String>,
    pub pin_freshness: Option<String>,
    pub staging_path: Option<String>,
    pub staging_repo_root: Option<String>,
    pub staging_exact_oid: Option<String>,
    pub published_product_id: Option<ProductConversationId>,
    pub published_conversation_id: Option<String>,
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

#[derive(Debug, Clone)]
pub struct ProductCreationPublishInput {
    pub request_id: String,
    pub claim: ProductCreationClaim,
    pub conversation: Conversation,
    pub authority_kind: AuthorityKind,
    pub environment: EnvironmentContext,
    pub repository_id: Option<String>,
    pub repository_root: Option<String>,
}

fn normalize_product_creation_intent(intent: &ProductCreationIntent) -> DbResult<()> {
    if intent.cwd.trim().is_empty() {
        return Err(DbError::Serialization(
            "product creation cwd must not be empty".to_string(),
        ));
    }
    if intent.objective.trim().is_empty() {
        return Err(DbError::Serialization(
            "product creation objective must not be empty".to_string(),
        ));
    }
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
fn parse_product_creation_job_row(row: SqliteRow) -> Result<ProductCreationJobRecord, sqlx::Error> {
    let request_id: String = row.try_get("request_id")?;
    let images_json: String = row.try_get("images_json")?;
    let images: Vec<ProductCreationImage> =
        serde_json::from_str(&images_json).map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    Ok(ProductCreationJobRecord {
        request_id,
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
            images,
        },
        status: row.try_get("status")?,
        accepted_at: parse_datetime(&row.try_get::<String, _>("accepted_at")?),
        updated_at: parse_datetime(&row.try_get::<String, _>("updated_at")?),
        claim_generation: row.try_get("claim_generation")?,
        claim_worker_id: row.try_get("claim_worker_id")?,
        claim_token: row.try_get("claim_token")?,
        claim_lease_until: row
            .try_get::<Option<String>, _>("claim_lease_until")?
            .as_deref()
            .map(parse_datetime),
        retry_at: row
            .try_get::<Option<String>, _>("retry_at")?
            .as_deref()
            .map(parse_datetime),
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
    })
}

impl Database {
    pub async fn get_product_creation_job(
        &self,
        request_id: &str,
    ) -> DbResult<Option<ProductCreationJobRecord>> {
        sqlx::query(
            "SELECT j.request_id, j.cwd, j.objective, j.model, j.effort, j.status,
                    j.accepted_at, j.updated_at, j.claim_generation, j.claim_worker_id,
                    j.claim_token, j.claim_lease_until, j.retry_at,
                    j.pin_exact_checkout_oid, j.pin_logical_base, j.pin_freshness,
                    j.staging_path, j.staging_repo_root, j.staging_exact_oid,
                    j.published_product_id, j.published_conversation_id,
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
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query(
            "SELECT request_id, cwd, objective, model, effort, status,
                    accepted_at, updated_at, claim_generation, claim_worker_id,
                    claim_token, claim_lease_until, retry_at,
                    pin_exact_checkout_oid, pin_logical_base, pin_freshness,
                    staging_path, staging_repo_root, staging_exact_oid,
                    published_product_id, published_conversation_id,
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
                request_id, cwd, objective, model, effort, status, accepted_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'accepted', ?6, ?6)",
        )
        .bind(request_id)
        .bind(&intent.cwd)
        .bind(&intent.objective)
        .bind(&intent.model)
        .bind(intent.effort.map(ModelEffort::as_wire_name))
        .bind(&now)
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
        let now_str = now.to_rfc3339();
        let lease_until = (now + lease_duration).to_rfc3339();
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'claimed', claim_generation = claim_generation + 1,
                 claim_worker_id = ?2, claim_token = ?3, claim_lease_until = ?4,
                 retry_at = NULL, updated_at = ?1
             WHERE request_id = ?5 AND (
                 status = 'accepted'
                 OR (status = 'retry_scheduled' AND retry_at <= ?1)
                 OR (status = 'claimed' AND claim_lease_until <= ?1)
             )",
        )
        .bind(&now_str)
        .bind(worker_id)
        .bind(token)
        .bind(&lease_until)
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
                lease_until: parse_datetime(&lease_until),
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
        let now_str = now.to_rfc3339();
        let lease_until = (now + lease_duration).to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let candidate: Option<(String, String)> = sqlx::query_as(
            "SELECT request_id, status
             FROM product_creation_jobs
             WHERE status = 'accepted'
                OR (status = 'retry_scheduled' AND retry_at <= ?1)
                OR (status = 'claimed' AND claim_lease_until <= ?1)
             ORDER BY CASE status WHEN 'accepted' THEN accepted_at ELSE retry_at END ASC, accepted_at ASC, request_id ASC
             LIMIT 1",
        )
        .bind(&now_str)
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
                 claim_lease_until = ?3,
                 retry_at = NULL,
                 updated_at = ?4
             WHERE request_id = ?5 AND status = ?6
               AND (
                    status = 'accepted'
                    OR (status = 'retry_scheduled' AND retry_at <= ?4)
                    OR (status = 'claimed' AND claim_lease_until <= ?4)
               )",
        )
        .bind(worker_id)
        .bind(token)
        .bind(&lease_until)
        .bind(&now_str)
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
                lease_until: parse_datetime(&lease_until),
            },
            job,
        }))
    }

    pub async fn pin_product_creation_once(
        &self,
        request_id: &str,
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
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE product_creation_jobs
             SET pin_exact_checkout_oid = ?2,
                 pin_logical_base = ?3,
                 pin_freshness = ?4,
                 updated_at = ?5
             WHERE request_id = ?1
               AND pin_exact_checkout_oid IS NULL
               AND pin_logical_base IS NULL
               AND pin_freshness IS NULL",
        )
        .bind(request_id)
        .bind(exact_checkout_oid)
        .bind(logical_base)
        .bind(freshness)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        let current = self
            .get_product_creation_job(request_id)
            .await?
            .ok_or_else(|| {
                DbError::Serialization("pinned product creation job missing".to_string())
            })?;
        Ok(ProductCreationPinOutcome::Pinned(current))
    }

    pub async fn schedule_product_creation_retry(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
        retry_at: DateTime<Utc>,
    ) -> DbResult<bool> {
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'retry_scheduled', claim_worker_id = NULL, claim_token = NULL,
                 claim_lease_until = NULL, retry_at = ?4, updated_at = ?5
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?6",
        )
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(retry_at.to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .bind(&claim.token)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn mark_product_creation_cleanup_ambiguous(
        &self,
        request_id: &str,
        claim: &ProductCreationClaim,
    ) -> DbResult<bool> {
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'cleanup_ambiguous', claim_worker_id = NULL, claim_token = NULL,
                 claim_lease_until = NULL, retry_at = NULL, updated_at = ?4
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?5",
        )
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(Utc::now().to_rfc3339())
        .bind(&claim.token)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn publish_product_creation_atomically(
        &self,
        input: &ProductCreationPublishInput,
    ) -> DbResult<bool> {
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let claimed: Option<(String,)> = sqlx::query_as(
            "SELECT request_id FROM product_creation_jobs
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?4",
        )
        .bind(&input.request_id)
        .bind(input.claim.generation)
        .bind(&input.claim.worker_id)
        .bind(&input.claim.token)
        .fetch_optional(&mut *tx)
        .await?;
        if claimed.is_none() {
            tx.rollback().await?;
            return Ok(false);
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
            &now,
        )
        .await?;
        insert_conversation_tx(&mut tx, &input.conversation).await?;
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
             SET status = 'delivery_pending', retry_at = NULL, staging_path = ?2,
                 staging_repo_root = ?3, staging_exact_oid = ?4,
                 published_product_id = ?5, published_conversation_id = ?6, updated_at = ?7
             WHERE request_id = ?1 AND status = 'claimed' AND claim_generation = ?8
               AND claim_worker_id = ?9 AND claim_token = ?10",
        )
        .bind(&input.request_id)
        .bind(match &input.environment {
            EnvironmentContext::AllocatedWorktree { worktree_path, .. } => {
                Some(worktree_path.as_str())
            }
            EnvironmentContext::UnownedCwd { cwd } => Some(cwd.as_str()),
            EnvironmentContext::None => None,
        })
        .bind(input.repository_root.as_deref())
        .bind(input.repository_id.as_deref())
        .bind(input.conversation.product_conversation_id.as_str())
        .bind(&input.conversation.id)
        .bind(&now)
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
             SET staging_path = ?1, staging_repo_root = ?2, staging_exact_oid = ?3, updated_at = ?4
             WHERE request_id = ?5 AND status = 'claimed' AND claim_generation = ?6
               AND claim_worker_id = ?7 AND claim_token = ?8",
        )
        .bind(staging_path)
        .bind(repo_root)
        .bind(exact_oid)
        .bind(Utc::now().to_rfc3339())
        .bind(request_id)
        .bind(claim.generation)
        .bind(&claim.worker_id)
        .bind(&claim.token)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn next_product_creation_delivery(
        &self,
    ) -> DbResult<Option<ProductCreationJobRecord>> {
        sqlx::query(
            "SELECT j.request_id, j.cwd, j.objective, j.model, j.effort, j.status,
                    j.accepted_at, j.updated_at, j.claim_generation, j.claim_worker_id,
                    j.claim_token, j.claim_lease_until, j.retry_at,
                    j.pin_exact_checkout_oid, j.pin_logical_base, j.pin_freshness,
                    j.staging_path, j.staging_repo_root, j.staging_exact_oid,
                    j.published_product_id, j.published_conversation_id,
                    COALESCE((SELECT json_group_array(json_object('media_type', i.media_type, 'data', i.data))
                              FROM product_creation_job_images i WHERE i.request_id = j.request_id
                              ORDER BY i.ordinal), '[]') AS images_json
             FROM product_creation_jobs j WHERE j.status = 'delivery_pending'
             ORDER BY j.updated_at, j.request_id LIMIT 1",
        )
        .try_map(parse_product_creation_job_row)
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::from)
    }

    pub async fn complete_product_creation_delivery(&self, request_id: &str) -> DbResult<bool> {
        let updated = sqlx::query(
            "UPDATE product_creation_jobs
             SET status = 'published', claim_worker_id = NULL, claim_token = NULL,
                 claim_lease_until = NULL, updated_at = ?1
             WHERE request_id = ?2 AND status = 'delivery_pending'",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(request_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn next_product_creation_deadline(&self) -> DbResult<Option<DateTime<Utc>>> {
        let deadline: Option<String> = sqlx::query_scalar(
            "SELECT MIN(deadline) FROM (
                 SELECT retry_at AS deadline FROM product_creation_jobs WHERE status = 'retry_scheduled'
                 UNION ALL SELECT claim_lease_until FROM product_creation_jobs WHERE status = 'claimed'
                 UNION ALL SELECT updated_at FROM product_creation_jobs WHERE status = 'delivery_pending'
             )",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(deadline.as_deref().map(parse_datetime))
    }

    pub async fn recent_distinct_published_product_creation_cwds(
        &self,
        limit: usize,
    ) -> DbResult<Vec<String>> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT cwd
             FROM (
                SELECT cwd, MAX(updated_at) AS latest_updated_at
                FROM product_creation_jobs
                WHERE status IN ('delivery_pending', 'published')
                GROUP BY cwd
             ) ranked
             ORDER BY latest_updated_at DESC, cwd DESC
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
            llm_language: LlmLanguage::default(),
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
    async fn product_creation_pin_once() {
        let db = Database::open_in_memory().await.unwrap();
        db.accept_product_creation("req-2", &intent("/repo/a", "pin"))
            .await
            .unwrap();
        assert!(matches!(
            db.pin_product_creation_once("req-2", "oid", "main", "fresh")
                .await
                .unwrap(),
            ProductCreationPinOutcome::Pinned(_)
        ));
        assert!(matches!(
            db.pin_product_creation_once("req-2", "oid", "main", "fresh")
                .await
                .unwrap(),
            ProductCreationPinOutcome::Same(_)
        ));
        assert!(matches!(
            db.pin_product_creation_once("req-2", "other", "main", "fresh")
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
        let product_id = ProductConversationId::new();
        let scope = WorkScopeId::new();
        let conv = published_conversation("conv-dup", product_id, scope, "/repo/a");
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
                repository_id: None,
                repository_root: None,
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
        let product_id = ProductConversationId::new();
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
                repository_id: None,
                repository_root: None,
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
    async fn product_creation_cross_owner_rejected() {
        let db = Database::open_in_memory().await.unwrap();
        let shared_scope = WorkScopeId::new();
        let product_a = ProductConversationId::new();
        let product_b = ProductConversationId::new();
        let conv_a = published_conversation(
            "conv-a-owner",
            product_a.clone(),
            shared_scope.clone(),
            "/repo/a",
        );
        let conv_b = published_conversation(
            "conv-b-owner",
            product_b.clone(),
            shared_scope.clone(),
            "/repo/b",
        );
        db.accept_product_creation("req-a", &intent("/repo/a", "a"))
            .await
            .unwrap();
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
            repository_id: None,
            repository_root: None,
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
        let err = db
            .publish_product_creation_atomically(&ProductCreationPublishInput {
                request_id: "req-b".to_string(),
                claim: claim_b.claim,
                conversation: conv_b,
                authority_kind: AuthorityKind::Work,
                environment: EnvironmentContext::UnownedCwd {
                    cwd: "/repo/b".to_string(),
                },
                repository_id: None,
                repository_root: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::Sqlx(_)));
    }

    #[tokio::test]
    async fn product_creation_recent_distinct_published_cwd_by_recency() {
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
            let conv = published_conversation(
                &format!("conv-rec-{idx}"),
                ProductConversationId::new(),
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
                repository_id: None,
                repository_root: None,
            })
            .await
            .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let cwds = db
            .recent_distinct_published_product_creation_cwds(5)
            .await
            .unwrap();
        assert_eq!(cwds, vec!["/repo/a".to_string(), "/repo/b".to_string()]);
    }
}
