//! Slack-owned discovery and delivery; native services own all agent execution.
mod driver;
pub(crate) mod http;
mod notification;
pub(crate) mod reply;
#[cfg(test)]
mod scheduling_tests;
mod slack;
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub enabled: bool,
    pub mcp_server: String,
    pub owner: String,
    /// Exact Slack permalink host, used to reject other workspaces.
    pub workspace: String,
    pub timezone: String,
    pub phoenix_url: String,
    /// Owner-configured executable and arguments; notification content uses stdin.
    pub notification_command: Option<Vec<String>>,
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if !self.owner.starts_with('U') || !self.owner.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("Expected a Slack owner user ID".into());
        }
        if !self.workspace.ends_with(".slack.com") || self.workspace.contains('/') {
            return Err("workspace must be an exact Slack permalink host".into());
        }
        self.timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| "Unknown IANA timezone")?;
        let url = reqwest::Url::parse(&self.phoenix_url).map_err(|_| "Invalid Phoenix URL")?;
        if !matches!(url.scheme(), "https" | "http")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("Expected an HTTP(S) Phoenix URL without credentials".into());
        }
        if self.mcp_server.trim().is_empty() {
            return Err("Expected an existing MCP server name".into());
        }
        if let Some(command) = &self.notification_command {
            if command.is_empty()
                || command.len() > 16
                || !std::path::Path::new(&command[0]).is_absolute()
                || command
                    .iter()
                    .any(|arg| arg.contains('\0') || arg.len() > 4096)
            {
                return Err(
                    "Expected an absolute notification executable and at most 15 arguments".into(),
                );
            }
        }
        Ok(())
    }
}
pub(crate) async fn has_explicit_binding(
    db: &crate::db::Database,
    product: &str,
) -> Result<bool, String> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM slack_gateway_bindings WHERE product_id=?1 AND kind='explicit')").bind(product).fetch_one(db.pool()).await.map_err(|e|e.to_string())
}
pub(crate) async fn config(db: &crate::db::Database) -> Result<Option<Config>, String> {
    db.get_app_setting("slack_gateway")
        .await
        .map_err(|e| e.to_string())?
        .map(|v| serde_json::from_str(&v).map_err(|e| e.to_string()))
        .transpose()
}
pub(crate) async fn status(db: &crate::db::Database) -> Result<serde_json::Value, String> {
    let conf = config(db).await?;
    let Some(c) = conf else {
        return Ok(
            serde_json::json!({"config":null,"status":"disabled","requests":[],"returns":[]}),
        );
    };
    let account=sqlx::query("SELECT status,last_scan_at,last_error,baseline_complete,calls_last_pass,pages_last_pass,retry_at_us FROM slack_gateway_accounts WHERE workspace=?1 AND owner=?2").bind(&c.workspace).bind(&c.owner).fetch_optional(db.pool()).await.map_err(|e|e.to_string())?;
    let scan=account.map(|r|serde_json::json!({"status":r.get::<String,_>("status"),"last_scan_at_unix_micros":r.get::<Option<i64>,_>("last_scan_at"),"last_error":r.get::<Option<String>,_>("last_error"),"baseline_complete":r.get::<bool,_>("baseline_complete"),"calls_last_pass":r.get::<i64,_>("calls_last_pass"),"pages_last_pass":r.get::<i64,_>("pages_last_pass"),"retry_at_us":r.get::<Option<i64>,_>("retry_at_us")}));
    Ok(
        serde_json::json!({"config":c,"scan":scan,"requests":db.slack_requests(&c.workspace,&c.owner).await.map_err(|e|e.to_string())?,"returns":db.slack_returns(&c.workspace,&c.owner).await.map_err(|e|e.to_string())?}),
    )
}
#[derive(Clone, Copy)]
pub(super) enum Pass {
    Discovery { reactions_first: bool },
    Delivery,
}

struct BudgetWindow {
    started: tokio::time::Instant,
    calls: std::sync::Arc<std::sync::atomic::AtomicU32>,
}
impl BudgetWindow {
    fn new() -> Self {
        Self {
            started: tokio::time::Instant::now(),
            calls: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }
    fn current(&mut self) -> std::sync::Arc<std::sync::atomic::AtomicU32> {
        if self.started.elapsed() >= std::time::Duration::from_secs(60) {
            self.started = tokio::time::Instant::now();
            self.calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        }
        self.calls.clone()
    }
}
pub(crate) fn start(state: crate::api::AppState) {
    tokio::spawn(async move {
        if let Err(e) = state.db.slack_recover_returns().await {
            tracing::error!(error=%e,"Slack return recovery failed");
            return;
        }
        let mut budget = BudgetWindow::new();
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut recovery = tokio::time::interval(std::time::Duration::from_secs(2));
        recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let wakeup = state.db.slack_gateway_wakeup();
        let mut reactions_first = false;
        loop {
            let pass = tokio::select! {
                biased;
                _ = timer.tick() => {
                    reactions_first = !reactions_first;
                    Pass::Discovery { reactions_first }
                },
                () = wakeup.notified() => Pass::Delivery,
                _ = recovery.tick() => Pass::Delivery,
            };
            let Ok(_authority) = state.runtime.acquire_local_authority_pass() else {
                return;
            };
            let c = match config(&state.db).await {
                Ok(Some(c)) => c,
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!(error=%e,"Slack gateway configuration unavailable");
                    continue;
                }
            };
            let slack = slack::Slack::with_budget(state.mcp_manager.clone(), &c, budget.current());
            let started = std::time::Instant::now();
            if let Err(error) = driver::tick(&state, &c, &slack, pass).await {
                tracing::warn!(%error,"Slack gateway pass failed");
                let _ = state
                    .db
                    .slack_scan_status(&c.workspace, &c.owner, false, "backing_off", Some(&error))
                    .await;
            }
            let calls = slack.calls.load(std::sync::atomic::Ordering::Relaxed);
            if calls > 0 {
                tracing::info!(
                    discovery = matches!(pass, Pass::Discovery { .. }),
                    calls,
                    pages = slack.pages.load(std::sync::atomic::Ordering::Relaxed),
                    elapsed_ms = started.elapsed().as_millis(),
                    "Slack gateway pass completed"
                );
            }
        }
    });
}
