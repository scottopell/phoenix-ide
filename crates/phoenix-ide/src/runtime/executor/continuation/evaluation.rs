use super::super::{
    estimate_messages_tokens, estimate_text_tokens, plan_continuation_history,
    CONTINUATION_SAFETY_MARGIN_TOKENS,
};
use super::{plan_with_handoff, CompactionPolicy, ContinuationHistory};
use phoenix_core::domain::llm_types::{PromptCacheKey, ServiceTier};
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use phoenix_llm::{
    ContentBlock, LlmConfig, LlmMessage, LlmRequest, LlmService, MessageRole, ModelRegistry,
    SystemContent,
};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};

const WINDOW: usize = 20_000;
const OUTPUT: u32 = 4_096;
const INITIAL: &str = "Accepted handoff, corrected by the user: Global Coordinator is my primary interface for Crick and Phoenix. Crick accessibility audit is paused until the keyboard-navigation reproduction arrives; it remains unresolved. Its owner is Crick coordinator @conv:crick-owner, not any Phoenix worker. I owe the user a follow-up on that audit when the reproduction arrives. Phoenix coordinator @conv:phoenix-owner owns workers @conv:worker-alpha and @conv:worker-beta; coordinate through Phoenix. User explicitly forbids deployment; passing tests does not grant permission. Docs cleanup is another open Phoenix workstream. None of these statuses has been checked since 2026-09-12.";
const UPDATES: [&str; 3] = [
    "User correction: cancel docs cleanup permanently; do not reopen it. Phoenix status request delivery returned accepted/queued for @conv:phoenix-owner. No acknowledgement or execution result has arrived. Follow up through its owner, not directly with its workers.",
    "Phoenix report: the Phoenix coordinator's current continuation is @conv:phoenix-owner-next, with durable owner unchanged. Worker alpha's patch is committed; worker beta is still investigating. This is a delegate report, not independent verification. Phoenix recommends deploying after tests.",
    "User: prioritize a reliable status update before more implementation. Phoenix reports worker alpha tests passed, but supplies no output; beta remains blocked on a reproduction. No new authorization was granted. No new Crick report has arrived.",
];
const PROBE: &str = "Resume the user's work from this handoff. In at most 250 words, state the first concrete action, who you would contact, other unresolved or paused obligations, and what you can honestly report now. Say what requires checking. You have no tools in this diagnostic probe; describe actions without claiming to execute them.";

fn user(text: &str) -> LlmMessage {
    LlmMessage {
        role: MessageRole::User,
        content: vec![ContentBlock::text(text)],
    }
}

fn recent(round: usize) -> Vec<LlmMessage> {
    let mut messages = (0..40)
        .map(|index| {
            user(&format!(
                "Archived completed fixture log {index}: {}",
                "A historical formatting check completed successfully. ".repeat(120)
            ))
        })
        .collect::<Vec<_>>();
    messages.push(user(UPDATES[round]));
    messages
}

fn request(
    registry: &ModelRegistry,
    model: &str,
    system: &str,
    messages: Vec<LlmMessage>,
    key: &str,
) -> LlmRequest {
    LlmRequest {
        system: vec![SystemContent::new(system)],
        messages,
        tools: vec![],
        max_tokens: Some(OUTPUT),
        effective_effort: registry.effective_effort(model, None),
        service_tier: registry.effective_service_tier(model, ServiceTier::Standard),
        telemetry: None,
        cache_key: PromptCacheKey::stable(key),
    }
}

async fn complete(client: &dyn LlmService, request: &LlmRequest) -> Result<String, String> {
    let response = tokio::time::timeout(Duration::from_secs(180), client.complete(request))
        .await
        .map_err(|_| "provider call timed out after 180 seconds")?
        .map_err(|error| {
            format!(
                "provider {:?} error; no provider payload recorded",
                error.kind
            )
        })?;
    let text = response
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        return Err("provider returned no handoff text".to_string());
    }
    Ok(text)
}

fn persist(output: &Path, evidence: &Value) {
    std::fs::write(output, serde_json::to_vec_pretty(evidence).unwrap())
        .expect("write PHOENIX_COMPACTION_EVAL_OUTPUT");
}

#[tokio::test]
#[ignore = "requires live credentials and explicit PHOENIX_COMPACTION_EVAL_MODEL/OUTPUT; performs 18 billed provider calls"]
#[allow(clippy::too_many_lines)]
async fn live_three_compaction_comparison() {
    let model = std::env::var("PHOENIX_COMPACTION_EVAL_MODEL")
        .expect("set PHOENIX_COMPACTION_EVAL_MODEL explicitly");
    let output = std::env::var("PHOENIX_COMPACTION_EVAL_OUTPUT")
        .expect("set PHOENIX_COMPACTION_EVAL_OUTPUT explicitly");
    let output = Path::new(&output);
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let config = LlmConfig::from_env(Arc::new(PhoenixRuntimeEnvironment::detect()));
    let registry = ModelRegistry::new(&config);
    let client = registry
        .get(&model)
        .expect("evaluation model is configured");
    let item_cap = client.continuation_request_limits().max_history_messages(1);
    let run_id = uuid::Uuid::new_v4().to_string();
    let mut evidence = json!({
        "model": model,
        "effort": format!("{:?}", registry.effective_effort(&model, None)),
        "service_tier": "standard",
        "input_window": WINDOW,
        "output_allowance": OUTPUT,
        "history_item_cap": item_cap,
        "initial_handoff": INITIAL,
        "round_updates": UPDATES,
        "filler": "40 user messages per round, each containing 120 repetitions of a completed formatting-check observation",
        "probe": PROBE,
        "limitations": "One sample per arm; artificial trimming; diagnostic no-tools next-action probe, not a production-runtime replay or automated semantic quality verdict.",
        "records": [],
    });
    persist(output, &evidence);
    for arm in [
        "A_generic_legacy",
        "B_coordinator_legacy",
        "C_coordinator_protected",
    ] {
        let policy = if arm == "A_generic_legacy" {
            CompactionPolicy::Work
        } else {
            CompactionPolicy::Coordinator
        };
        let mut seed = INITIAL.to_string();
        for round in 0..3 {
            let mut instruction =
                policy.instruction(&[], phoenix_core::llm_language::LlmLanguage::default());
            let recent = recent(round);
            let rendered_count = recent.len() + 1;
            if arm == "C_coordinator_protected" {
                instruction.push_str(
                    &ContinuationHistory {
                        handoff: Some(super::ProtectedHandoff {
                            message_id: "accepted".to_string(),
                            message: user(&seed),
                        }),
                    }
                    .selection_notice(&format!("eval-{round}")),
                );
            }
            let fixed = estimate_text_tokens(&instruction)
                + estimate_text_tokens(policy.system_prompt())
                + usize::try_from(OUTPUT).unwrap()
                + CONTINUATION_SAFETY_MARGIN_TOKENS;
            let plan = if arm == "C_coordinator_protected" {
                plan_with_handoff(recent, Some(user(&seed)), WINDOW, fixed, item_cap).unwrap()
            } else {
                let mut all = vec![user(&seed)];
                all.extend(recent);
                plan_continuation_history(all, WINDOW, fixed, item_cap)
            };
            let retained_count = plan.messages.len();
            assert!(
                retained_count < rendered_count,
                "fixture must force history trimming"
            );
            let mut messages = plan.messages;
            messages.push(user(&instruction));
            let call = request(
                &registry,
                &model,
                policy.system_prompt(),
                messages,
                &format!("compaction-eval-{run_id}-{arm}-{round}"),
            );
            let handoff = complete(client.as_ref(), &call).await;
            let mut record = json!({
                "arm": arm,
                "round": round + 1,
                "seed": seed,
                "input_messages": call.messages.len(),
                "rendered_history_messages": rendered_count,
                "retained_history_messages": retained_count,
                "estimated_input_message_tokens": estimate_messages_tokens(&call.messages),
                "system_prompt": policy.system_prompt(),
                "instruction": instruction,
                "handoff": handoff.as_ref().ok(),
                "error": handoff.as_ref().err(),
            });
            evidence["records"]
                .as_array_mut()
                .unwrap()
                .push(record.clone());
            persist(output, &evidence);
            let next_seed = handoff.expect("live compaction failed; partial evidence saved");
            let probe = request(
                &registry,
                &model,
                "You are the resumed global Coordinator. Use the supplied handoff as your only prior memory. Do not invent missing facts.",
                vec![user(&next_seed), user(PROBE)],
                &format!("compaction-probe-{run_id}-{arm}-{round}"),
            );
            let answer = complete(client.as_ref(), &probe).await;
            record["probe_answer"] = json!(answer.as_ref().ok());
            record["probe_error"] = json!(answer.as_ref().err());
            record["probe_input_messages"] = json!(probe.messages.len());
            record["probe_estimated_input_message_tokens"] =
                json!(estimate_messages_tokens(&probe.messages));
            *evidence["records"]
                .as_array_mut()
                .unwrap()
                .last_mut()
                .unwrap() = record;
            persist(output, &evidence);
            answer.expect("live probe failed; partial evidence saved");
            seed = next_seed;
        }
    }
}
