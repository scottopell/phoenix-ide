//! Owner-facing configuration and inspectability, not the execution interface.
use super::Config;
use crate::api::{handlers::AppError, AppState};
use axum::{extract::State, Json};
pub(crate) async fn get(State(s): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    super::status(&s.db)
        .await
        .map(Json)
        .map_err(AppError::Internal)
}
pub(crate) async fn put(
    State(s): State<AppState>,
    Json(c): Json<Config>,
) -> Result<Json<serde_json::Value>, AppError> {
    c.validate().map_err(AppError::BadRequest)?;
    // A pass must finish using its retained configuration before a timezone
    // or account update is evaluated against its completed durable progress.
    let _access = s.slack_gateway_access.lock().await;
    let value = serde_json::to_string(&c).map_err(|e| AppError::Internal(e.to_string()))?;
    s.db.set_slack_configuration(&value).await.map_err(|e| {
        if let phoenix_db::DbError::Serialization(message) = e {
            AppError::BadRequest(message)
        } else {
            AppError::Internal(e.to_string())
        }
    })?;
    super::status(&s.db)
        .await
        .map(Json)
        .map_err(AppError::Internal)
}

pub(crate) async fn source(
    State(s): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::response::Redirect, AppError> {
    let url: Option<String> =
        sqlx::query_scalar("SELECT source_url FROM slack_gateway_requests WHERE request_id=?1")
            .bind(id)
            .fetch_optional(s.db.pool())
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
    let url = url.ok_or_else(|| AppError::NotFound("Slack request not found".into()))?;
    let parsed = reqwest::Url::parse(&url)
        .map_err(|_| AppError::Internal("Invalid persisted Slack source URL".into()))?;
    if parsed.scheme() != "https" || !parsed.host_str().is_some_and(|s| s.ends_with(".slack.com")) {
        return Err(AppError::Internal(
            "Invalid persisted Slack source host".into(),
        ));
    }
    Ok(axum::response::Redirect::temporary(&url))
}

pub(crate) async fn capabilities(State(s): State<AppState>) -> Json<serde_json::Value> {
    let tools=s.mcp_manager.tool_definitions().await.into_iter().filter(|(_,t)|t.name.contains("slack")).map(|(server,t)|serde_json::json!({"server":server,"name":t.name,"input_schema":t.input_schema})).collect::<Vec<_>>();
    Json(serde_json::json!({"tools":tools}))
}

pub(crate) async fn inspect(
    State(s): State<AppState>,
    Json(c): Json<Config>,
) -> Result<Json<serde_json::Value>, AppError> {
    c.validate().map_err(AppError::BadRequest)?;
    let mut budget = s.slack_gateway_access.lock().await;
    s.db.slack_baseline_complete(&c.workspace, &c.owner)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if super::retry_deadline(&s.db, &c)
        .await
        .map_err(AppError::Internal)?
        .is_some()
    {
        return Err(AppError::BadRequest(
            "Slack requested a retry delay; try the read-access check later".into(),
        ));
    }
    let slack = super::slack::Slack::with_budget(s.mcp_manager.clone(), &c, budget.current());
    if !slack.can_call() {
        return Err(AppError::BadRequest(
            "The Slack call allowance is exhausted; try the read-access check later".into(),
        ));
    }
    let result = inspect_reads(&c, &slack).await;
    super::record_backoff(&s.db, &c, &slack, result.is_ok())
        .await
        .map_err(AppError::Internal)?;
    result
}

async fn inspect_reads(
    c: &Config,
    slack: &super::slack::Slack,
) -> Result<Json<serde_json::Value>, AppError> {
    slack.verify_owner(c).await.map_err(AppError::BadRequest)?;
    let tz: chrono_tz::Tz = c
        .timezone
        .parse()
        .map_err(|_| AppError::BadRequest("Unknown timezone".into()))?;
    let after =
        (chrono::Utc::now().with_timezone(&tz).date_naive() - chrono::Days::new(4)).to_string();
    let mut checks = Vec::new();
    let mut reaction_read_verified = false;
    for reactions in [false, true] {
        let page = slack
            .search(c, reactions, None, &after)
            .await
            .map_err(AppError::BadRequest)?;
        let mut verified = 0;
        for hit in &page.hits {
            if let Some(source) = slack
                .source(c, hit, reactions)
                .await
                .map_err(AppError::BadRequest)?
            {
                verified += 1;
                if source.reaction_read_verified {
                    reaction_read_verified = true;
                } else if !reaction_read_verified {
                    reaction_read_verified = slack
                        .inspect_reaction_access(c, hit)
                        .await
                        .map_err(AppError::BadRequest)?;
                }
            }
        }
        checks.push(serde_json::json!({"query":if reactions {"fire reactions"} else {"owner fire messages"},"first_page_hits":page.hits.len(),"verified_sources":verified,"more_pages":page.cursor.is_some()}));
    }
    Ok(Json(
        serde_json::json!({"owner_verified":true,"checks":checks,"reaction_read_verified":reaction_read_verified,"calls":slack.calls.load(std::sync::atomic::Ordering::Relaxed),"gateway_activated":false}),
    ))
}
