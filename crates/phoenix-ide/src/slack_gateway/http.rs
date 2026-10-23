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
    let value = serde_json::to_string(&c).map_err(|e| AppError::Internal(e.to_string()))?;
    s.db.set_app_setting("slack_gateway", &value)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
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
    let slack = super::slack::Slack::new(s.mcp_manager.clone(), &c);
    slack.verify_owner(&c).await.map_err(AppError::BadRequest)?;
    let tz: chrono_tz::Tz = c
        .timezone
        .parse()
        .map_err(|_| AppError::BadRequest("Unknown timezone".into()))?;
    let after =
        (chrono::Utc::now().with_timezone(&tz).date_naive() - chrono::Days::new(4)).to_string();
    let mut checks = Vec::new();
    for reactions in [false, true] {
        let page = slack
            .search(&c, reactions, None, &after)
            .await
            .map_err(AppError::BadRequest)?;
        let mut verified = 0;
        for hit in &page.hits {
            if slack
                .source(&c, hit, reactions)
                .await
                .map_err(AppError::BadRequest)?
                .is_some()
            {
                verified += 1;
            }
        }
        checks.push(serde_json::json!({"query":if reactions {"fire reactions"} else {"owner fire messages"},"first_page_hits":page.hits.len(),"verified_sources":verified,"more_pages":page.cursor.is_some()}));
    }
    Ok(Json(
        serde_json::json!({"owner_verified":true,"checks":checks,"calls":slack.calls.load(std::sync::atomic::Ordering::Relaxed),"gateway_activated":false}),
    ))
}
