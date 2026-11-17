//! Owner-facing configuration and inspectability, not the execution interface.
use super::Config;
use crate::api::{handlers::AppError, AppState};
use axum::{extract::State, Json};
pub(crate) async fn get(State(s): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    super::status(&s.db, &s.mcp_manager)
        .await
        .map(Json)
        .map_err(AppError::Internal)
}
pub(crate) async fn put(
    State(s): State<AppState>,
    Json(c): Json<Config>,
) -> Result<Json<serde_json::Value>, AppError> {
    let c = c.normalized().map_err(AppError::BadRequest)?;
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
    super::status(&s.db, &s.mcp_manager)
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
    let catalogs = s.mcp_manager.cached_tool_catalogs().await;
    let freshness = catalogs
        .iter()
        .map(|(server, tools)| serde_json::json!({"server":server,"current":tools.is_some()}))
        .collect::<Vec<_>>();
    let tools = catalogs
        .into_iter()
        .flat_map(|(server, tools)| {
            tools.unwrap_or_default().into_iter()
                .filter(|t| t.name.contains("slack"))
                .map(move |t| serde_json::json!({"server":server,"name":t.name,"input_schema":t.input_schema}))
        })
        .collect::<Vec<_>>();
    Json(serde_json::json!({"tools":tools,"catalogs":freshness}))
}

pub(crate) async fn inspect(
    State(s): State<AppState>,
    Json(c): Json<Config>,
) -> Result<Json<serde_json::Value>, AppError> {
    let c = c.normalized().map_err(AppError::BadRequest)?;
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
    let mut queries = Vec::new();
    if c.inbox_channel.is_some() {
        queries.push(super::QueryKind::Inbox);
    }
    queries.extend([super::QueryKind::Prefix, super::QueryKind::Reaction]);
    if c.saved_enabled {
        queries.push(super::QueryKind::Saved);
    }
    let mut budget_limited = false;
    for kind in queries {
        if !slack.can_sample() {
            budget_limited = true;
            break;
        }
        let page = slack
            .search(c, kind, None, &after)
            .await
            .map_err(AppError::BadRequest)?;
        let mut verified = 0;
        for hit in &page.hits {
            if let Some(source) = slack
                .source(c, hit, kind)
                .await
                .map_err(AppError::BadRequest)?
            {
                verified += 1;
                if source.reaction_read_verified {
                    reaction_read_verified = true;
                } else if kind != super::QueryKind::Saved && !reaction_read_verified {
                    reaction_read_verified = slack
                        .inspect_reaction_access(c, hit)
                        .await
                        .map_err(AppError::BadRequest)?;
                }
            }
        }
        checks.push(serde_json::json!({"query":match kind { super::QueryKind::Prefix => "owner emoji-prefixed messages", super::QueryKind::Reaction => "configured emoji reactions", super::QueryKind::Saved => "Saved messages", super::QueryKind::Inbox => "submission channel messages" },"first_page_hits":page.hits.len(),"verified_sources":verified,"more_pages":page.cursor.is_some()}));
    }
    Ok(Json(
        serde_json::json!({"owner_verified":true,"checks":checks,"reaction_read_verified":reaction_read_verified,"calls":slack.calls.load(std::sync::atomic::Ordering::Relaxed),"budget_limited":budget_limited,"gateway_activated":false}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn read_inspection_stops_before_a_partial_workflow_exceeds_shared_allowance() {
        // REQ-SG-017/020
        let c:Config=serde_json::from_value(serde_json::json!({"enabled":false,"saved_enabled":true,"inbox_channel":"CINBOX","mcp_server":"fixture","workspace":"fixture.slack.com","owner":"UOWNER","timezone":"UTC","phoenix_url":"https://phoenix.example","trigger_emoji":"fire"})).unwrap();
        let allowance = Arc::new(AtomicU32::new(12));
        let mut slack = super::super::slack::Slack::with_budget(
            Arc::new(phoenix_tools::mcp::McpClientManager::new()),
            &c,
            allowance.clone(),
        );
        slack.fixture = Some(Arc::new(|tool, args| match tool {
            "slack_read_user_profile" => {
                Ok(serde_json::json!({"result":"User ID: UOWNER\nBot: No"}))
            }
            "slack_search_public_and_private" if args["filters"] == "after:1970-01-01" => Ok(
                serde_json::json!({"results":"### Result 1 of 1\nChannel: proof (ID: CPROOF)\nMessage_ts: 100.000001\nPermalink: [link](https://fixture.slack.com/archives/CPROOF/p100000001)\nText: proof","pagination_info":"End of results"}),
            ),
            _ => {
                panic!("Cannot start another workflow sample with only two calls remaining: {tool}")
            }
        }));
        let axum::Json(result) = inspect_reads(&c, &slack).await.unwrap();
        assert_eq!(result["owner_verified"], true);
        assert_eq!(result["budget_limited"], true);
        assert_eq!(result["checks"], serde_json::json!([]));
        assert_eq!(result["gateway_activated"], false);
        assert_eq!(allowance.load(Ordering::Relaxed), 14);
    }
}
