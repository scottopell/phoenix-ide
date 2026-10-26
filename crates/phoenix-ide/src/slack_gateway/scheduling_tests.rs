use super::*;
use serde_json::json;
use sqlx::Row;
use std::sync::{atomic::Ordering, Arc, Mutex};

fn config() -> Config {
    Config {
        enabled: true,
        mcp_server: "fixture".into(),
        owner: "UOWNER".into(),
        workspace: "fixture.slack.com".into(),
        timezone: "America/New_York".into(),
        phoenix_url: "https://localhost:8043".into(),
        notification_command: None,
    }
}

#[test]
fn configuration_accepts_enterprise_owner_and_requires_a_phoenix_origin() {
    let mut c = config();
    for owner in ["UOWNER", "WOWNER"] {
        c.owner = owner.into();
        assert!(c.validate().is_ok());
    }
    for url in [
        "https://phoenix.example",
        "https://phoenix.example/",
        "http://localhost:8031",
    ] {
        c.phoenix_url = url.into();
        assert!(c.validate().is_ok(), "{url}");
    }
    for url in [
        "https://phoenix.example/c/id",
        "https://phoenix.example/?view=x",
        "https://phoenix.example/#view",
        "https://user:password@phoenix.example",
        "ftp://phoenix.example",
    ] {
        c.phoenix_url = url.into();
        assert!(c.validate().is_err(), "{url}");
    }
}

#[tokio::test]
async fn read_access_inspection_shares_the_automatic_gate_allowance_and_backoff() {
    let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    let c = config();
    let clone = state.clone();
    assert!(Arc::ptr_eq(
        &state.slack_gateway_access,
        &clone.slack_gateway_access
    ));
    let mut automatic_pass = state.slack_gateway_access.lock().await;
    let mut slack =
        slack::Slack::with_budget(state.mcp_manager.clone(), &c, automatic_pass.current());
    slack.fixture = Some(Arc::new(|_, _| {
        Ok(json!({"result":"User ID: UOWNER\nBot: No"}))
    }));
    // Calls charged by an automatic service exhaust the inspection allowance too.
    for _ in 0..16 {
        slack
            .call("slack_read_user_profile", json!({}))
            .await
            .unwrap();
    }
    let inspection = http::inspect(axum::extract::State(clone), axum::Json(c.clone()));
    tokio::pin!(inspection);
    tokio::select! {
        biased;
        _ = &mut inspection => panic!("inspection bypassed the active automatic service"),
        () = tokio::task::yield_now() => {}
    }
    drop(automatic_pass);
    let result = inspection.await.err().unwrap();
    assert!(
        matches!(result, crate::api::handlers::AppError::BadRequest(message) if message.contains("allowance is exhausted"))
    );
    // Even a fresh process allowance cannot bypass a persisted provider delay.
    let fresh = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    fresh
        .db
        .slack_baseline_complete(&c.workspace, &c.owner)
        .await
        .unwrap();
    sqlx::query("UPDATE slack_gateway_accounts SET retry_at_us=?1")
        .bind(chrono::Utc::now().timestamp_micros() + 120_000_000)
        .execute(fresh.db.pool())
        .await
        .unwrap();
    let result = http::inspect(axum::extract::State(fresh), axum::Json(c))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(result, crate::api::handlers::AppError::BadRequest(message) if message.contains("retry delay"))
    );
}

#[tokio::test]
async fn changing_accounts_does_not_inherit_the_previous_retry_deadline() {
    let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    let original = config();
    state
        .db
        .set_slack_configuration(&serde_json::to_string(&original).unwrap())
        .await
        .unwrap();
    state
        .db
        .slack_baseline_complete(&original.workspace, &original.owner)
        .await
        .unwrap();
    sqlx::query("UPDATE slack_gateway_accounts SET retry_at_us=?1")
        .bind(chrono::Utc::now().timestamp_micros() + 3_600_000_000)
        .execute(state.db.pool())
        .await
        .unwrap();
    let mut other = original.clone();
    other.workspace = "other.slack.com".into();
    other.owner = "WOTHER".into();
    state
        .db
        .set_slack_configuration(&serde_json::to_string(&other).unwrap())
        .await
        .unwrap();
    assert!(retry_deadline(&state.db, &other).await.unwrap().is_none());
    assert!(retry_deadline(&state.db, &original)
        .await
        .unwrap()
        .is_some());
    let error = http::inspect(axum::extract::State(state), axum::Json(other))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(error, crate::api::handlers::AppError::BadRequest(message) if !message.contains("retry delay"))
    );
}

#[tokio::test]
async fn startup_recovery_retries_a_database_failure_without_restarting_phoenix() {
    let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    let mut c = config();
    c.enabled = false;
    let source = phoenix_db::SlackSource {
        workspace: c.workspace.clone(),
        owner: c.owner.clone(),
        channel: "CFIXTURE".into(),
        message_ts: "100.000001".into(),
        thread_ts: "100.000001".into(),
        source_url: "https://fixture.slack.com/archives/CFIXTURE/p100000001".into(),
        instruction: String::new(),
        context: "fixture".into(),
        observed_at_unix_micros: 1,
        kind: phoenix_db::SlackRequestKind::Briefing,
    };
    // Seed accepted work before pausing its discovery configuration.
    state.db.accept_slack_source(&source, false).await.unwrap();
    state
        .db
        .set_slack_configuration(&serde_json::to_string(&c).unwrap())
        .await
        .unwrap();
    let r = state
        .db
        .slack_requests(&c.workspace, &c.owner)
        .await
        .unwrap()
        .remove(0);
    state
        .db
        .fail_slack_before_admission(&r.request_id, "fixture failure", "https://phoenix.example")
        .await
        .unwrap();
    state.db.slack_eyes_confirmed(&r.request_id).await.unwrap();
    let output = state
        .db
        .slack_returns(&c.workspace, &c.owner)
        .await
        .unwrap()
        .remove(0);
    state
        .db
        .slack_begin_notification_return(
            &output.return_id,
            "fixture-attempt",
            &["/fixture/notify".into()],
        )
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER refuse_fixture_recovery BEFORE UPDATE ON slack_gateway_returns WHEN OLD.state='attempting' AND NEW.state='uncertain' BEGIN SELECT RAISE(ABORT,'transient recovery fixture'); END").execute(state.db.pool()).await.unwrap();
    start(state.clone());
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM slack_gateway_accounts")
                    .fetch_optional(state.db.pool())
                    .await
                    .unwrap();
            if status.as_deref() == Some("failed") {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("recovery failure should be visible");
    sqlx::query("DROP TRIGGER refuse_fixture_recovery")
        .execute(state.db.pool())
        .await
        .unwrap();
    state.db.slack_gateway_wakeup().notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if state
                .db
                .slack_returns(&c.workspace, &c.owner)
                .await
                .unwrap()[0]
                .state
                == phoenix_db::SlackReturnState::Uncertain
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("same gateway task should retry recovery");
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Compare exhaustion and throttling against the same ready-return obligation.
async fn ready_reply_precedes_exhausting_or_throttled_eyes_retries() {
    for throttled in [false, true] {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let mut c = config();
        c.enabled = false;
        let conversation = state
            .db
            .create_conversation("eyes-priority", "eyes-priority", "/tmp", true, None, None)
            .await
            .unwrap();
        let mut first = None;
        for n in 0..14 {
            let source = phoenix_db::SlackSource {
                workspace: c.workspace.clone(),
                owner: c.owner.clone(),
                channel: "CFIXTURE".into(),
                message_ts: format!("100.{:06}", n + 1),
                thread_ts: "100.000000".into(),
                source_url: format!(
                    "https://fixture.slack.com/archives/CFIXTURE/p100{:06}",
                    n + 1
                ),
                instruction: "fixture".into(),
                context: "fixture".into(),
                observed_at_unix_micros: 1,
                kind: phoenix_db::SlackRequestKind::Explicit,
            };
            state.db.accept_slack_source(&source, false).await.unwrap();
        }
        for r in state
            .db
            .slack_requests(&c.workspace, &c.owner)
            .await
            .unwrap()
        {
            state
                .db
                .slack_bind_product(&r.binding_id, conversation.product_conversation_id.as_str())
                .await
                .unwrap();
            state
                .db
                .slack_mark_admitted(&r.request_id, &conversation.id)
                .await
                .unwrap();
            first.get_or_insert(r.binding_id);
        }
        sqlx::query("INSERT INTO slack_gateway_returns(return_id,binding_id,occurrence_key,destination,text,created_at_unix_micros) VALUES ('ready',?1,'ready','thread','ready answer',1)").bind(first.unwrap()).execute(state.db.pool()).await.unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let calls = log.clone();
        let mut slack = slack::Slack::new(state.mcp_manager.clone(), &c);
        slack.fixture = Some(Arc::new(move |tool, args| {
            calls.lock().unwrap().push(tool.to_string());
            if let Some(proof) = workspace_probe(tool, &args) {
                return Ok(proof);
            }
            match tool {
                "slack_read_user_profile" => Ok(json!({"result":"User ID: UOWNER\nBot: No"})),
                "slack_send_message" => Ok(
                    json!({"message_link":"https://fixture.slack.com/archives/CFIXTURE/p200000001?thread_ts=100.000000","message_context":{"channel_id":"CFIXTURE","message_ts":"200.000001"}}),
                ),
                "slack_add_reaction" => Err(if throttled {
                    "429 retry_after:120"
                } else {
                    "fixture eyes rejected"
                }
                .into()),
                _ => panic!("unexpected tool {tool}"),
            }
        }));
        driver::tick(&state, &c, &slack, Pass::Delivery)
            .await
            .unwrap();
        assert_eq!(
            state
                .db
                .slack_returns(&c.workspace, &c.owner)
                .await
                .unwrap()
                .into_iter()
                .find(|r| r.return_id == "ready")
                .unwrap()
                .state,
            phoenix_db::SlackReturnState::Confirmed
        );
        let log = log.lock().unwrap();
        let sent = log
            .iter()
            .position(|tool| tool == "slack_send_message")
            .unwrap();
        let eyes = log
            .iter()
            .position(|tool| tool == "slack_add_reaction")
            .unwrap();
        assert!(sent < eyes);
        assert_eq!(
            log.iter()
                .filter(|tool| tool.as_str() == "slack_send_message")
                .count(),
            1
        );
        assert_eq!(
            slack.calls.load(Ordering::Relaxed),
            if throttled { 4 } else { 16 }
        );
    }
}

fn workspace_probe(tool: &str, args: &serde_json::Value) -> Option<serde_json::Value> {
    (tool == "slack_search_public_and_private" && args["filters"] == "after:1970-01-01")
        .then(|| json!({"results":"### Result 1 of 1\nChannel: proof (ID: CPROOF)\nMessage_ts: 10.000001\nPermalink: [link](https://fixture.slack.com/archives/CPROOF/p10000001)\nText: workspace identity only","pagination_info":"End of results"}))
}

fn reaction_fixture(
    sources: Vec<phoenix_db::SlackSource>,
    log: Arc<Mutex<Vec<String>>>,
) -> Arc<slack::FixtureCall> {
    Arc::new(move |tool, args| {
        log.lock().unwrap().push(tool.to_string());
        if let Some(proof) = workspace_probe(tool, &args) {
            return Ok(proof);
        }
        match tool {
            "slack_read_user_profile" => Ok(json!({"result":"User ID: UOWNER\nBot: No"})),
            "slack_search_public_and_private" => {
                if !args["filters"].as_str().unwrap().contains("hasmy:") {
                    return Ok(
                        json!({"results":"No results found", "pagination_info":"End of results"}),
                    );
                }
                let page = args["cursor"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<usize>()
                    .unwrap();
                let s = &sources[page];
                Ok(
                    json!({"results":format!("### Result 1 of 1\nChannel: DM (ID: {})\nMessage_ts: {}\nPermalink: [link]({})\nText: fixture message",s.channel,s.message_ts,s.source_url),
                    "pagination_info":if page+1 < sources.len() {format!("Next cursor `{}`",page+1)} else {"End of results".into()}}),
                )
            }
            "slack_read_thread" => Ok(
                json!({"messages":format!("=== THREAD PARENT MESSAGE ===\nFrom: Other (UOTHER)\nTime: fixture\nMessage TS: {}\nFixture context\nReactions: fire (1)\n\nNo thread messsages\n", args["message_ts"].as_str().unwrap()),"pagination_info":"There are no more messages in this thread."}),
            ),
            "slack_get_reactions" => {
                Ok(json!({"result":"Reactions on message:\n\n:fire: × 1 — Owner (UOWNER)"}))
            }
            _ => Err(format!("Unexpected fixture tool: {tool}")),
        }
    })
}

struct Backlog {
    state: crate::api::AppState,
    config: Config,
    fixture: Arc<slack::FixtureCall>,
    log: Arc<Mutex<Vec<String>>>,
}
async fn backlog() -> Backlog {
    let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    let c = config();
    state
        .db
        .set_app_setting("slack_gateway", &serde_json::to_string(&c).unwrap())
        .await
        .unwrap();
    state
        .db
        .slack_baseline_complete(&c.workspace, &c.owner)
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    let sources: Vec<_> = (0..7)
        .map(|n| {
            let ts = format!("{}.000001", now - n * 60);
            phoenix_db::SlackSource {
                workspace: c.workspace.clone(),
                owner: c.owner.clone(),
                channel: "CFIXTURE".into(),
                thread_ts: ts.clone(),
                message_ts: ts.clone(),
                source_url: format!(
                    "https://fixture.slack.com/archives/CFIXTURE/p{}",
                    ts.replace('.', "")
                ),
                instruction: String::new(),
                context: "fixture".into(),
                observed_at_unix_micros: chrono::Utc::now().timestamp_micros(),
                kind: phoenix_db::SlackRequestKind::Briefing,
            }
        })
        .collect();
    for source in &sources[..6] {
        state.db.accept_slack_source(source, true).await.unwrap();
    }
    state
        .db
        .slack_scan_status(&c.workspace, &c.owner, true, "healthy", None)
        .await
        .unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let fixture = reaction_fixture(sources, log.clone());
    Backlog {
        state,
        config: c,
        fixture,
        log,
    }
}

// REQ-SG-003/004/005/017: an old message gets a new reaction behind six baseline sources.
#[tokio::test]
async fn deleted_hit_does_not_block_baseline_or_a_later_page() {
    let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    let c = config();
    state
        .db
        .set_app_setting("slack_gateway", &serde_json::to_string(&c).unwrap())
        .await
        .unwrap();
    let ts = format!("{}.000001", chrono::Utc::now().timestamp());
    let sources = ["CDELETED", "CLATER"].map(|channel| phoenix_db::SlackSource {
        workspace: c.workspace.clone(),
        owner: c.owner.clone(),
        channel: channel.into(),
        thread_ts: ts.clone(),
        message_ts: ts.clone(),
        source_url: format!(
            "https://fixture.slack.com/archives/{channel}/p{}",
            ts.replace('.', "")
        ),
        instruction: String::new(),
        context: "context".into(),
        observed_at_unix_micros: 1,
        kind: phoenix_db::SlackRequestKind::Briefing,
    });
    let fixture = reaction_fixture(sources.to_vec(), Arc::new(Mutex::new(Vec::new())));
    let mut slack = slack::Slack::new(state.mcp_manager.clone(), &c);
    slack.fixture = Some(Arc::new(move |tool, args| {
        if tool == "slack_read_thread" && args["channel_id"] == "CDELETED" {
            Err("message_not_found".into())
        } else {
            fixture(tool, args)
        }
    }));
    driver::discover(&state, &c, &slack).await.unwrap();
    let ready: bool = sqlx::query_scalar("SELECT baseline_complete FROM slack_gateway_accounts")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    assert!(ready);
    assert!(!state
        .db
        .slack_source_recorded(&c.workspace, &c.owner, "CDELETED", &ts)
        .await
        .unwrap());
    assert!(state
        .db
        .slack_source_recorded(&c.workspace, &c.owner, "CLATER", &ts)
        .await
        .unwrap());
    assert!(state
        .db
        .slack_requests(&c.workspace, &c.owner)
        .await
        .unwrap()
        .is_empty());
}

// REQ-SG-003/004/005/017: an old message gets a new reaction behind six baseline sources.
#[tokio::test]
async fn reaction_backlog_measurement() {
    let Backlog {
        state,
        config: c,
        fixture,
        log,
    } = backlog().await;
    let mut total = 0;
    for pass in 1..=10 {
        let mut slack = slack::Slack::new(state.mcp_manager.clone(), &c);
        slack.fixture = Some(fixture.clone());
        slack.verify_owner(&c).await.unwrap();
        driver::discover(&state, &c, &slack).await.unwrap();
        total += slack.calls.load(Ordering::Relaxed);
        let accepted = state
            .db
            .slack_requests(&c.workspace, &c.owner)
            .await
            .unwrap();
        if !accepted.is_empty() {
            let log = log.lock().unwrap();
            println!(
                "REACTION_BACKLOG passes={pass} calls={total} reads={} searches={}",
                log.iter()
                    .filter(|s| s.as_str() == "slack_read_thread")
                    .count(),
                log.iter()
                    .filter(|s| s.as_str() == "slack_search_public_and_private")
                    .count()
            );
            assert_eq!(accepted.len(), 1);
            assert_eq!(pass, 2);
            assert_eq!(total, 16);
            return;
        }
    }
    panic!("Eligible old-message reaction was starved by baseline sources");
}

// REQ-SG-017: repeated local wakeups cannot renew the network allowance.
#[tokio::test]
async fn delivery_wakeups_share_one_budget_and_idle_never_searches() {
    let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    let c = config();
    let used = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
    for attempt in 0..20 {
        let mut slack = slack::Slack::with_budget(state.mcp_manager.clone(), &c, used.clone());
        let calls = calls.clone();
        slack.fixture = Some(Arc::new(move |tool, args| {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(workspace_probe(tool, &args)
                .unwrap_or_else(|| json!({"result":"User ID: UOWNER\nBot: No"})))
        }));
        assert_eq!(slack.verify_owner(&c).await.is_ok(), attempt < 8);
    }
    assert_eq!(calls.load(Ordering::Relaxed), 16);
    assert_eq!(used.load(Ordering::Relaxed), 16);
    let mut slack = slack::Slack::new(state.mcp_manager.clone(), &c);
    slack.fixture = Some(Arc::new(|_, _| panic!("idle delivery must not call Slack")));
    let started = std::time::Instant::now();
    for _ in 0..100 {
        driver::tick(&state, &c, &slack, Pass::Delivery)
            .await
            .unwrap();
    }
    println!(
        "IDLE_DELIVERY passes=100 calls={} elapsed_ms={}",
        slack.calls.load(Ordering::Relaxed),
        started.elapsed().as_millis()
    );
    assert_eq!(slack.calls.load(Ordering::Relaxed), 0);
    assert_eq!(slack.pages.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn call_window_only_renews_after_sixty_seconds() {
    let mut window = BudgetWindow::new();
    window.current().store(16, Ordering::Relaxed);
    tokio::time::advance(std::time::Duration::from_secs(59)).await;
    assert_eq!(window.current().load(Ordering::Relaxed), 16);
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    assert_eq!(window.current().load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn pass_metrics_exclude_calls_already_charged_to_the_window() {
    let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    let c = config();
    let budget = Arc::new(std::sync::atomic::AtomicU32::new(6));
    let mut slack = slack::Slack::with_budget(state.mcp_manager.clone(), &c, budget.clone());
    slack.fixture = Some(Arc::new(|method, args| {
        if let Some(proof) = workspace_probe(method, &args) {
            return Ok(proof);
        }
        if method.contains("read_user_profile") {
            Ok(json!({"result":"User ID: UOWNER\nBot: No"}))
        } else {
            Ok(json!({"results":"No results found", "pagination_info":"End of results"}))
        }
    }));
    driver::tick(
        &state,
        &c,
        &slack,
        Pass::Discovery {
            reactions_first: false,
        },
    )
    .await
    .unwrap();
    let calls: i64 = sqlx::query_scalar("SELECT calls_last_pass FROM slack_gateway_accounts")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    assert_eq!(calls, 3);
    assert_eq!(budget.load(Ordering::Relaxed), 9);
}

// REQ-SG-003/004/017: exhaustion checkpoints a whole page and leaves delivery capacity.
#[tokio::test]
async fn budget_exhaustion_retains_cursor_and_resumes_without_source_reads() {
    let Backlog {
        state,
        config: c,
        fixture,
        log,
    } = backlog().await;
    let budget = Arc::new(std::sync::atomic::AtomicU32::new(8));
    let mut slack = slack::Slack::with_budget(state.mcp_manager.clone(), &c, budget.clone());
    slack.fixture = Some(fixture.clone());
    driver::discover_ordered(&state, &c, &slack, true)
        .await
        .unwrap();
    let cursor: String =
        sqlx::query_scalar("SELECT cursor FROM slack_gateway_discovery WHERE kind='reaction'")
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    assert_eq!(cursor, "1");
    assert_eq!(slack.calls.load(Ordering::Relaxed), 1);
    assert_eq!(budget.load(Ordering::Relaxed), 9);
    assert!(log
        .lock()
        .unwrap()
        .iter()
        .all(|t| t == "slack_search_public_and_private"));
    let mut resumed = slack::Slack::new(state.mcp_manager.clone(), &c);
    resumed.fixture = Some(fixture);
    driver::discover_ordered(&state, &c, &resumed, false)
        .await
        .unwrap();
    assert_eq!(
        state
            .db
            .slack_requests(&c.workspace, &c.owner)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(resumed.calls.load(Ordering::Relaxed) <= 12);
}

// REQ-SG-017: a server retry deadline also governs delivery wakeups.
#[tokio::test]
async fn throttled_scan_keeps_page_and_delivery_cannot_bypass_backoff() {
    let Backlog {
        state,
        config: c,
        fixture,
        ..
    } = backlog().await;
    let base = fixture.clone();
    let mut slack = slack::Slack::new(state.mcp_manager.clone(), &c);
    slack.fixture = Some(Arc::new(move |tool, args| {
        if tool == "slack_search_public_and_private" && args["cursor"] == "1" {
            Err("429 rate limit; retry_after: 120".into())
        } else {
            base(tool, args)
        }
    }));
    driver::tick(
        &state,
        &c,
        &slack,
        Pass::Discovery {
            reactions_first: false,
        },
    )
    .await
    .unwrap();
    let row = sqlx::query("SELECT retry_at_us,status FROM slack_gateway_accounts")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    assert!(row.get::<i64, _>("retry_at_us") > chrono::Utc::now().timestamp_micros());
    assert_eq!(row.get::<String, _>("status"), "backing_off");
    let cursor: String =
        sqlx::query_scalar("SELECT cursor FROM slack_gateway_discovery WHERE kind='reaction'")
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    assert_eq!(cursor, "1");
    let mut next = slack::Slack::new(state.mcp_manager.clone(), &c);
    next.fixture = Some(Arc::new(|_, _| {
        panic!("persisted backoff must prevent Slack calls")
    }));
    driver::tick(&state, &c, &next, Pass::Delivery)
        .await
        .unwrap();
    driver::tick(
        &state,
        &c,
        &next,
        Pass::Discovery {
            reactions_first: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(next.calls.load(Ordering::Relaxed), 0);
    sqlx::query("UPDATE slack_gateway_accounts SET retry_at_us=0")
        .execute(state.db.pool())
        .await
        .unwrap();
    let mut resumed = slack::Slack::new(state.mcp_manager.clone(), &c);
    resumed.fixture = Some(fixture);
    driver::tick(
        &state,
        &c,
        &resumed,
        Pass::Discovery {
            reactions_first: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        state
            .db
            .slack_requests(&c.workspace, &c.owner)
            .await
            .unwrap()
            .len(),
        1
    );
}

// REQ-SG-006/017: throttling during accepted work must remain visible even
// when the discovery burst cannot start any pages.
#[tokio::test]
async fn successful_unthrottled_pass_clears_an_expired_retry_deadline() {
    let Backlog {
        state,
        config: c,
        fixture,
        ..
    } = backlog().await;
    sqlx::query("UPDATE slack_gateway_accounts SET retry_at_us=?1,status='backing_off'")
        .bind(chrono::Utc::now().timestamp_micros() - 1)
        .execute(state.db.pool())
        .await
        .unwrap();
    let mut slack = slack::Slack::new(state.mcp_manager.clone(), &c);
    slack.fixture = Some(fixture);
    driver::tick(
        &state,
        &c,
        &slack,
        Pass::Discovery {
            reactions_first: true,
        },
    )
    .await
    .unwrap();
    let retry: Option<i64> = sqlx::query_scalar("SELECT retry_at_us FROM slack_gateway_accounts")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    assert!(retry.is_none());
    assert!(slack.calls.load(Ordering::Relaxed) > 0);
}

#[tokio::test]
async fn acknowledgment_throttle_is_reported_as_backoff() {
    let Backlog {
        state,
        config: c,
        fixture,
        ..
    } = backlog().await;
    let mut discovery = slack::Slack::new(state.mcp_manager.clone(), &c);
    discovery.fixture = Some(fixture.clone());
    driver::discover(&state, &c, &discovery).await.unwrap();
    let request = state
        .db
        .slack_requests(&c.workspace, &c.owner)
        .await
        .unwrap()
        .remove(0);
    state
        .db
        .fail_slack_before_admission(
            &request.request_id,
            "fixture refusal",
            "https://phoenix.example",
        )
        .await
        .unwrap();
    let mut slack = slack::Slack::new(state.mcp_manager.clone(), &c);
    slack.fixture = Some(Arc::new(move |tool, args| {
        if tool == "slack_add_reaction" {
            Err("429 rate limit; retry_after: 120".into())
        } else {
            fixture(tool, args)
        }
    }));
    driver::tick(
        &state,
        &c,
        &slack,
        Pass::Discovery {
            reactions_first: false,
        },
    )
    .await
    .unwrap();
    let row = sqlx::query("SELECT status,retry_at_us,last_error FROM slack_gateway_accounts")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("status"), "backing_off");
    assert!(row.get::<i64, _>("retry_at_us") > chrono::Utc::now().timestamp_micros());
    assert!(row.get::<Option<String>, _>("last_error").is_some());
    assert_eq!(slack.calls.load(Ordering::Relaxed), 3);
    assert_eq!(slack.pages.load(Ordering::Relaxed), 0);
}

// REQ-SG-003/004/005/017: a failed candidate read must not complete its page
// or establish a baseline/accepted identity for the unread source.
#[tokio::test]
async fn failed_candidate_verification_retains_page_until_retry() {
    let Backlog {
        state,
        config: c,
        fixture,
        ..
    } = backlog().await;
    let base = fixture.clone();
    let mut failing = slack::Slack::new(state.mcp_manager.clone(), &c);
    failing.fixture = Some(Arc::new(move |tool, args| {
        if tool == "slack_get_reactions" {
            Err("fixture source temporarily unavailable".into())
        } else {
            base(tool, args)
        }
    }));
    assert!(driver::discover(&state, &c, &failing).await.is_err());
    let first_observed: i64 =
        sqlx::query_scalar("SELECT first_observed_at_unix_micros FROM slack_gateway_observations")
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    let row =
        sqlx::query("SELECT cursor,complete FROM slack_gateway_discovery WHERE kind='reaction'")
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    assert_eq!(row.get::<String, _>("cursor"), "6");
    assert!(!row.get::<bool, _>("complete"));
    assert!(state
        .db
        .slack_requests(&c.workspace, &c.owner)
        .await
        .unwrap()
        .is_empty());
    let mut reporting = slack::Slack::new(state.mcp_manager.clone(), &c);
    reporting.fixture = failing.fixture.clone();
    driver::tick(
        &state,
        &c,
        &reporting,
        Pass::Discovery {
            reactions_first: true,
        },
    )
    .await
    .unwrap();
    let scan = sqlx::query("SELECT status,retry_at_us,last_error FROM slack_gateway_accounts")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    assert_eq!(scan.get::<String, _>("status"), "failed");
    assert!(scan.get::<Option<i64>, _>("retry_at_us").is_none());
    assert!(scan.get::<Option<String>, _>("last_error").is_some());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT first_observed_at_unix_micros FROM slack_gateway_observations"
        )
        .fetch_one(state.db.pool())
        .await
        .unwrap(),
        first_observed
    );
    let mut resumed = slack::Slack::new(state.mcp_manager.clone(), &c);
    resumed.fixture = Some(fixture);
    driver::discover(&state, &c, &resumed).await.unwrap();
    assert_eq!(
        state
            .db
            .slack_requests(&c.workspace, &c.owner)
            .await
            .unwrap()
            .len(),
        1
    );
    let accepted = state
        .db
        .slack_requests(&c.workspace, &c.owner)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(accepted.source.observed_at_unix_micros, first_observed);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM slack_gateway_observations")
            .fetch_one(state.db.pool())
            .await
            .unwrap(),
        0
    );
}

struct LatePrefixBacklog {
    state: crate::api::AppState,
    config: Config,
    fixture: Arc<slack::FixtureCall>,
    prefix_reads: Arc<std::sync::atomic::AtomicU32>,
}

async fn late_prefix_backlog() -> LatePrefixBacklog {
    let Backlog {
        state,
        config: c,
        fixture,
        ..
    } = backlog().await;
    let after = (chrono::Utc::now().date_naive() - chrono::Days::new(4)).to_string();
    sqlx::query("INSERT INTO slack_gateway_discovery(workspace,owner,kind,after_date,complete) VALUES (?1,?2,'prefix',?3,1), (?1,?2,'reaction',?3,0)")
        .bind(&c.workspace).bind(&c.owner).bind(after).execute(state.db.pool()).await.unwrap();
    let stamp = format!("{}.000001", chrono::Utc::now().timestamp());
    let prefix_reads = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let observed = prefix_reads.clone();
    let with_prefix: Arc<slack::FixtureCall> = Arc::new(move |tool, args| {
        if let Some(proof) = workspace_probe(tool, &args) {
            return Ok(proof);
        }
        if tool == "slack_search_public_and_private"
            && !args["filters"].as_str().unwrap().contains("hasmy:")
        {
            observed.fetch_add(1, Ordering::Relaxed);
            Ok(
                json!({"results":format!("### Result 1 of 1\nChannel: DM (ID: CLATE)\nMessage_ts: {stamp}\nPermalink: [link](https://fixture.slack.com/archives/CLATE/p{})\nText: :fire: investigate",stamp.replace('.',"")),"pagination_info":"End of results"}),
            )
        } else if tool == "slack_read_thread" && args["channel_id"] == "CLATE" {
            Ok(
                json!({"messages":format!("=== THREAD PARENT MESSAGE ===\nFrom: Owner (UOWNER)\nTime: fixture\nMessage TS: {stamp}\n:fire: investigate\nReactions: fire (1)\n\nNo thread messsages\n"),"pagination_info":"There are no more messages in this thread."}),
            )
        } else {
            fixture(tool, args)
        }
    });
    LatePrefixBacklog {
        state,
        config: c,
        fixture: with_prefix,
        prefix_reads,
    }
}

// REQ-SG-003/017: a retained sweep no longer delays the newest explicit source.
#[tokio::test]
async fn completed_prefix_query_refreshes_new_message_without_resetting_catchup() {
    let LatePrefixBacklog {
        state,
        config: c,
        fixture,
        prefix_reads,
    } = late_prefix_backlog().await;
    for pass in 1..=2 {
        let mut slack = slack::Slack::new(state.mcp_manager.clone(), &c);
        slack.fixture = Some(fixture.clone());
        slack.verify_owner(&c).await.unwrap();
        driver::discover(&state, &c, &slack).await.unwrap();
        assert_eq!(prefix_reads.load(Ordering::Relaxed), pass);
        let requests = state
            .db
            .slack_requests(&c.workspace, &c.owner)
            .await
            .unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.source.channel == "CLATE")
                .count(),
            1
        );
        if pass == 1 {
            let cursor: String = sqlx::query_scalar(
                "SELECT cursor FROM slack_gateway_discovery WHERE kind='reaction'",
            )
            .fetch_one(state.db.pool())
            .await
            .unwrap();
            assert_eq!(cursor, "3");
            assert_eq!(slack.calls.load(Ordering::Relaxed), 9);
            println!("FRESH_PREFIX passes=1 calls=9 source_reads=2 saved_reaction_cursor={cursor}");
        }
        assert!(slack.calls.load(Ordering::Relaxed) <= 16);
    }
}

// A head probe needs enough allowance for itself, one catch-up page, and delivery.
#[tokio::test]
async fn fresh_probe_yields_to_catchup_when_allowance_is_low() {
    let LatePrefixBacklog {
        state,
        config: c,
        fixture,
        prefix_reads,
    } = late_prefix_backlog().await;
    let budget = Arc::new(std::sync::atomic::AtomicU32::new(6));
    let mut slack = slack::Slack::with_budget(state.mcp_manager.clone(), &c, budget.clone());
    slack.fixture = Some(fixture);
    driver::discover(&state, &c, &slack).await.unwrap();
    assert_eq!(prefix_reads.load(Ordering::Relaxed), 0);
    let cursor: String =
        sqlx::query_scalar("SELECT cursor FROM slack_gateway_discovery WHERE kind='reaction'")
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    assert_eq!(cursor, "3");
    assert_eq!(budget.load(Ordering::Relaxed), 9);
}

#[tokio::test]
async fn initial_baseline_never_uses_the_fresh_acceptance_path() {
    let LatePrefixBacklog {
        state,
        config: c,
        fixture,
        prefix_reads,
    } = late_prefix_backlog().await;
    sqlx::query("UPDATE slack_gateway_accounts SET baseline_complete=0")
        .execute(state.db.pool())
        .await
        .unwrap();
    let mut slack = slack::Slack::new(state.mcp_manager.clone(), &c);
    slack.fixture = Some(fixture);
    driver::discover(&state, &c, &slack).await.unwrap();
    assert_eq!(prefix_reads.load(Ordering::Relaxed), 0);
    assert!(state
        .db
        .slack_requests(&c.workspace, &c.owner)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn failed_fresh_probe_does_not_discard_or_starve_saved_pages() {
    let LatePrefixBacklog {
        state,
        config: c,
        fixture,
        ..
    } = late_prefix_backlog().await;
    let mut slack = slack::Slack::new(state.mcp_manager.clone(), &c);
    slack.fixture = Some(Arc::new(move |tool, args| {
        if tool == "slack_search_public_and_private"
            && !args["filters"].as_str().unwrap().contains("hasmy:")
        {
            Err("fixture fresh search unavailable".into())
        } else {
            fixture(tool, args)
        }
    }));
    assert!(driver::discover(&state, &c, &slack).await.is_err());
    let requests = state
        .db
        .slack_requests(&c.workspace, &c.owner)
        .await
        .unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].source.channel, "CFIXTURE");
    assert!(slack.calls.load(Ordering::Relaxed) <= 16);
}
