use super::{Database, DbResult};
use phoenix_core::domain::request_capacity::{RequestCapacityPlan, SupersededInputCount};
use serde::{de::DeserializeOwned, Serialize};
use sqlx::Row;

fn label(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .expect("fact enum serializes")
        .as_str()
        .expect("fact enum string")
        .to_owned()
}
fn parse<T: DeserializeOwned>(value: String) -> DbResult<T> {
    serde_json::from_value(serde_json::Value::String(value))
        .map_err(|e| super::DbError::Serialization(e.to_string()))
}

#[derive(Debug)]
pub struct CapacityDiagnosticRow {
    pub plan: RequestCapacityPlan,
    pub created_at: String,
    pub usage_recorded: bool,
    pub attempt_outcome: Option<String>,
    pub provider: Option<String>,
    pub provider_duration_ms: Option<i64>,
    pub provider_ttft_ms: Option<i64>,
    pub actual_input_tokens: Option<i64>,
    pub actual_output_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
}

impl Database {
    /// Persists facts only; no provider attempt or token usage is manufactured.
    ///
    /// # Errors
    /// Returns an error if `SQLite` rejects the facts or cannot commit them.
    pub async fn insert_request_capacity_plan(&self, plan: &RequestCapacityPlan) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query("INSERT INTO request_capacity_plans (plan_id,request_id,retry_attempt,conversation_id,root_conversation_id,kind,model,route_class,connection_id,context_window_tokens,output_reservation_tokens,local_prediction_tokens,provider_input_tokens,counting_method,count_fallback,reduction_action,reduced_local_tokens,outcome,planning_duration_ms,prefix_comparison,superseded_local_prediction_tokens,superseded_provider_input_tokens,superseded_counting_method,superseded_count_fallback,created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(plan_id) DO NOTHING")
            .bind(&plan.plan_id).bind(&plan.request_id).bind(plan.retry_attempt).bind(&plan.conversation_id).bind(&plan.root_conversation_id)
            .bind(label(&plan.kind)).bind(&plan.model).bind(label(&plan.route_class)).bind(&plan.connection_id)
            .bind(plan.context_window_tokens).bind(plan.output_reservation_tokens).bind(plan.local_prediction_tokens).bind(plan.provider_input_tokens)
            .bind(label(&plan.counting_method)).bind(plan.count_fallback.map(|v| label(&v))).bind(label(&plan.reduction_action))
            .bind(plan.reduced_local_tokens).bind(label(&plan.outcome)).bind(plan.planning_duration_ms).bind(label(&plan.prefix_comparison)).bind(plan.superseded_count.as_ref().map(|v| v.local_prediction_tokens)).bind(plan.superseded_count.as_ref().and_then(|v| v.provider_input_tokens)).bind(plan.superseded_count.as_ref().map(|v| label(&v.counting_method))).bind(plan.superseded_count.as_ref().and_then(|v| v.count_fallback.map(|f| label(&f)))).bind(chrono::Utc::now().to_rfc3339())
            .execute(&mut *tx).await?;
        if result.rows_affected() != 0 {
            for (ordinal, cause) in plan.prefix_change_causes.iter().enumerate() {
                let ordinal = i64::try_from(ordinal)
                    .map_err(|_| super::DbError::Serialization("Too many prefix causes".into()))?;
                sqlx::query("INSERT OR IGNORE INTO request_prefix_change_causes(plan_id,ordinal,cause) VALUES (?,?,?)")
                    .bind(&plan.plan_id).bind(ordinal).bind(label(cause)).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Bounded content-free projection. Legacy usage has no identity and never joins by time.
    ///
    /// # Errors
    /// Returns an error if `SQLite` cannot read the facts or a row cannot be decoded.
    pub async fn usage_capacity_diagnostics(
        &self,
        root: Option<&str>,
        since: &str,
    ) -> DbResult<Vec<CapacityDiagnosticRow>> {
        let rows = sqlx::query("SELECT p.*, u.id AS usage_id, m.outcome AS attempt_outcome,m.provider,m.total_duration_ms,m.dispatch_to_first_generation_event_ms,
          CASE WHEN u.input_total_observed = 1 THEN u.input_tokens + u.cache_creation_tokens + u.cache_read_tokens END AS actual_input_tokens,
          CASE WHEN u.output_observed = 1 THEN u.output_tokens END AS actual_output_tokens,
          CASE WHEN u.cache_write_observed = 1 THEN u.cache_creation_tokens END AS observed_cache_write,
          CASE WHEN u.cache_read_observed = 1 THEN u.cache_read_tokens END AS observed_cache_read
          FROM request_capacity_plans p
          LEFT JOIN llm_request_metrics m ON m.capacity_plan_id=p.plan_id AND m.request_id=p.request_id AND m.retry_attempt=p.retry_attempt
          LEFT JOIN turn_usage u ON u.capacity_plan_id=p.plan_id AND u.request_id=p.request_id AND u.retry_attempt=p.retry_attempt
          WHERE (? IS NULL OR p.root_conversation_id=?) AND p.created_at >= ? ORDER BY p.created_at DESC,p.request_id DESC,p.retry_attempt DESC,p.plan_id DESC LIMIT 200")
          .bind(root).bind(root).bind(since).fetch_all(&self.pool).await?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let plan_id: String = row.try_get("plan_id")?;
            let request_id: String = row.try_get("request_id")?;
            let retry_attempt: u32 = row.try_get("retry_attempt")?;
            let causes: Vec<String> = sqlx::query_scalar(
                "SELECT cause FROM request_prefix_change_causes WHERE plan_id=? ORDER BY ordinal",
            )
            .bind(&plan_id)
            .fetch_all(&self.pool)
            .await?;
            result.push(CapacityDiagnosticRow {
                plan: RequestCapacityPlan {
                    plan_id,
                    request_id,
                    retry_attempt,
                    conversation_id: row.try_get("conversation_id")?,
                    root_conversation_id: row.try_get("root_conversation_id")?,
                    kind: parse(row.try_get("kind")?)?,
                    model: row.try_get("model")?,
                    route_class: parse(row.try_get("route_class")?)?,
                    connection_id: row.try_get("connection_id")?,
                    context_window_tokens: row.try_get("context_window_tokens")?,
                    output_reservation_tokens: row.try_get("output_reservation_tokens")?,
                    local_prediction_tokens: row.try_get("local_prediction_tokens")?,
                    provider_input_tokens: row.try_get("provider_input_tokens")?,
                    superseded_count: row
                        .try_get::<Option<u32>, _>("superseded_local_prediction_tokens")?
                        .map(|local_prediction_tokens| -> DbResult<_> {
                            Ok(SupersededInputCount {
                                local_prediction_tokens,
                                provider_input_tokens: row
                                    .try_get("superseded_provider_input_tokens")?,
                                counting_method: parse(row.try_get("superseded_counting_method")?)?,
                                count_fallback: row
                                    .try_get::<Option<String>, _>("superseded_count_fallback")?
                                    .map(parse)
                                    .transpose()?,
                            })
                        })
                        .transpose()?,
                    counting_method: parse(row.try_get("counting_method")?)?,
                    count_fallback: row
                        .try_get::<Option<String>, _>("count_fallback")?
                        .map(parse)
                        .transpose()?,
                    reduction_action: parse(row.try_get("reduction_action")?)?,
                    reduced_local_tokens: row.try_get("reduced_local_tokens")?,
                    outcome: parse(row.try_get("outcome")?)?,
                    planning_duration_ms: row.try_get("planning_duration_ms")?,
                    prefix_comparison: parse(row.try_get("prefix_comparison")?)?,
                    prefix_change_causes: causes.into_iter().map(parse).collect::<DbResult<_>>()?,
                },
                usage_recorded: row.try_get::<Option<i64>, _>("usage_id")?.is_some(),
                created_at: row.try_get("created_at")?,
                attempt_outcome: row.try_get("attempt_outcome")?,
                provider: row.try_get("provider")?,
                provider_duration_ms: row.try_get("total_duration_ms")?,
                provider_ttft_ms: row.try_get("dispatch_to_first_generation_event_ms")?,
                actual_input_tokens: row.try_get("actual_input_tokens")?,
                actual_output_tokens: row.try_get("actual_output_tokens")?,
                cache_write_tokens: row.try_get("observed_cache_write")?,
                cache_read_tokens: row.try_get("observed_cache_read")?,
            });
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::llm_types::{
        EffectiveEffort, ServiceTier, Usage, UsageAttempt, UsageObservation,
    };
    use phoenix_core::domain::request_capacity::{
        CountFallbackReason, InputCountingMethod, InputReductionAction, PrefixChangeCause,
        PrefixComparison, RequestPlanKind, RequestPlanOutcome, SafeRouteClass,
    };
    fn plan(request: &str, retry: u32) -> RequestCapacityPlan {
        RequestCapacityPlan {
            plan_id: format!("{request}:{retry}"),
            request_id: request.into(),
            retry_attempt: retry,
            conversation_id: "capacity".into(),
            root_conversation_id: "capacity".into(),
            kind: RequestPlanKind::Ordinary,
            model: "test".into(),
            route_class: SafeRouteClass::Unknown,
            connection_id: None,
            context_window_tokens: 272_000,
            output_reservation_tokens: 128_000,
            local_prediction_tokens: 100,
            provider_input_tokens: None,
            superseded_count: None,
            counting_method: InputCountingMethod::ConservativeLocalEstimate,
            count_fallback: Some(CountFallbackReason::Unavailable),
            reduction_action: InputReductionAction::None,
            reduced_local_tokens: 0,
            outcome: RequestPlanOutcome::CapacityConflict,
            planning_duration_ms: 7,
            prefix_comparison: PrefixComparison::Unknown,
            prefix_change_causes: vec![
                PrefixChangeCause::Clearing,
                PrefixChangeCause::GuidanceRefresh,
            ],
        }
    }
    async fn database() -> Database {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("capacity", "capacity", "/tmp", true, None, None)
            .await
            .unwrap();
        db
    }
    #[tokio::test]
    async fn capacity_diagnostics_local_refusal_has_no_attempt_or_fabricated_usage() {
        let db = database().await;
        db.insert_request_capacity_plan(&plan("local", 1))
            .await
            .unwrap();
        let rows = db.usage_capacity_diagnostics(None, "").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].attempt_outcome.is_none());
        assert!(!rows[0].usage_recorded);
        assert_eq!(rows[0].actual_input_tokens, None);
        assert_eq!(rows[0].plan.prefix_change_causes.len(), 2);
        let attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM llm_request_metrics")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(attempts, 0);
    }
    #[tokio::test]
    async fn capacity_diagnostics_exact_usage_identity_and_raw_presence_are_authoritative() {
        let db = database().await;
        for retry in 1..=3 {
            db.insert_request_capacity_plan(&plan("same", retry))
                .await
                .unwrap();
        }
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 900,
            cache_creation_tokens: 20,
            cache_read_tokens: 70,
            reasoning_tokens: None,
        };
        // A legacy row with equal numeric values never joins a plan by timestamp.
        db.insert_turn_usage(
            "capacity",
            "capacity",
            "test",
            EffectiveEffort::native_unknown(),
            ServiceTier::Standard,
            &usage,
            None,
            None,
        )
        .await
        .unwrap();
        for (retry, observation) in [
            (
                1,
                UsageObservation {
                    input_total: true,
                    output: true,
                    cache_write: true,
                    cache_read: true,
                },
            ),
            (2, UsageObservation::default()),
        ] {
            let attempt = UsageAttempt {
                capacity_plan_id: Some(format!("same:{retry}")),
                request_id: "same".into(),
                retry_attempt: retry,
                observation,
            };
            db.insert_turn_usage(
                "capacity",
                "capacity",
                "test",
                EffectiveEffort::native_unknown(),
                ServiceTier::Standard,
                &usage,
                None,
                Some(&attempt),
            )
            .await
            .unwrap();
            // Repeated finalization keeps the first row; distinct retry identities remain separate.
            db.insert_turn_usage(
                "capacity",
                "capacity",
                "test",
                EffectiveEffort::native_unknown(),
                ServiceTier::Standard,
                &Usage::default(),
                None,
                Some(&attempt),
            )
            .await
            .unwrap();
        }
        let rows = db.usage_capacity_diagnostics(None, "").await.unwrap();
        let observed = rows.iter().find(|r| r.plan.retry_attempt == 1).unwrap();
        assert_eq!(observed.actual_input_tokens, Some(100)); // excludes all output, includes cached input
        assert_eq!(observed.actual_output_tokens, Some(900));
        let unknown = rows.iter().find(|r| r.plan.retry_attempt == 2).unwrap();
        assert!(unknown.usage_recorded);
        assert_eq!(unknown.actual_input_tokens, None);
        let missing = rows.iter().find(|r| r.plan.retry_attempt == 3).unwrap();
        assert!(!missing.usage_recorded);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM turn_usage")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn capacity_diagnostics_reprepared_same_retry_never_joins_old_plan_to_new_usage() {
        use phoenix_core::domain::llm_types::{
            LlmAttemptMetrics, LlmAttemptOutcome, LlmTransport, ProviderStreamTelemetry,
        };
        let db = database().await;
        let mut old = plan("resumed", 1);
        old.outcome = RequestPlanOutcome::Ready;
        let mut resumed = old.clone();
        resumed.plan_id = "new-prepared-request".into();
        resumed.local_prediction_tokens = 250;
        resumed.outcome = RequestPlanOutcome::Ready;
        db.insert_request_capacity_plan(&old).await.unwrap();
        db.insert_request_capacity_plan(&resumed).await.unwrap();
        let metrics = LlmAttemptMetrics {
            capacity_plan_id: Some(old.plan_id.clone()),
            conversation_id: "capacity".into(),
            root_conversation_id: "capacity".into(),
            request_id: old.request_id.clone(),
            retry_attempt: 1,
            provider: "test".into(),
            model: "test".into(),
            transport: LlmTransport::HttpJson,
            total_duration_ms: 99,
            stream: ProviderStreamTelemetry::non_streaming(),
            outcome: LlmAttemptOutcome::NetworkError,
        };
        db.upsert_llm_request_metrics(&metrics).await.unwrap();
        let mut later = metrics.clone();
        later.capacity_plan_id = Some(resumed.plan_id.clone());
        later.outcome = LlmAttemptOutcome::Success;
        db.upsert_llm_request_metrics(&later).await.unwrap();
        let usage = Usage {
            input_tokens: 200,
            output_tokens: 50,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            reasoning_tokens: None,
        };
        let attempt = UsageAttempt {
            capacity_plan_id: Some(resumed.plan_id.clone()),
            request_id: resumed.request_id.clone(),
            retry_attempt: 1,
            observation: UsageObservation {
                input_total: true,
                output: true,
                cache_write: true,
                cache_read: true,
            },
        };
        db.insert_turn_usage(
            "capacity",
            "capacity",
            "test",
            EffectiveEffort::native_unknown(),
            ServiceTier::Standard,
            &usage,
            None,
            Some(&attempt),
        )
        .await
        .unwrap();
        let rows = db.usage_capacity_diagnostics(None, "").await.unwrap();
        assert_eq!(rows.len(), 2);
        let original = rows
            .iter()
            .find(|row| row.plan.plan_id == old.plan_id)
            .unwrap();
        assert!(original.attempt_outcome.is_some());
        assert!(!original.usage_recorded);
        assert_eq!(original.actual_input_tokens, None);
        let current = rows
            .iter()
            .find(|row| row.plan.plan_id == resumed.plan_id)
            .unwrap();
        assert!(current.attempt_outcome.is_none());
        assert!(current.usage_recorded);
        assert_eq!(current.actual_input_tokens, Some(200));
        assert_eq!(current.plan.local_prediction_tokens, 250);
        assert_eq!(
            db.llm_request_metrics_for_request("resumed").await.unwrap()[0]
                .metrics
                .capacity_plan_id
                .as_deref(),
            Some(old.plan_id.as_str())
        );
    }

    #[tokio::test]
    async fn capacity_diagnostics_schema_rejects_partial_identity_and_inconsistent_counting() {
        let db = database().await;
        let result=sqlx::query("INSERT INTO turn_usage(conversation_id,root_conversation_id,model,created_at,request_id) VALUES ('capacity','capacity','test','now','partial')").execute(&db.pool).await;
        assert!(result.is_err());
        let mut invalid = plan("invalid", 1);
        invalid.provider_input_tokens = Some(1);
        assert!(db.insert_request_capacity_plan(&invalid).await.is_err());
        invalid.counting_method = InputCountingMethod::OpenAiCompleteInput;
        assert!(db.insert_request_capacity_plan(&invalid).await.is_err());
        invalid.count_fallback = None;
        db.insert_request_capacity_plan(&invalid).await.unwrap();
    }
}
