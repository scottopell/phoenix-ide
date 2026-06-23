#![allow(clippy::wildcard_enum_match_arm)]
//! Conversation runtime executor
//!
//! The executor loop receives inputs from:
//! - User events via `event_rx` (`UserMessage`, `UserCancel`, etc.) → routed to `transition()`
//! - Effect outcomes via the per-effect, generation-tagged channels
//!   (`llm_outcome_rx`, `tool_outcome_rx`, `retry_outcome_rx`) → routed to
//!   `process_outcome()` once the generation guard has discarded any stale,
//!   superseded outcome.
//!
//! Background tasks receive typed `oneshot::Sender<T>` for their outcome type.
//! A `Sender<ToolExecOutcome>` physically cannot send an `LlmOutcome`. The
//! forwarder stamps the dispatch generation and wraps each received outcome in
//! `EffectOutcome` for `process_outcome()`.

use super::traits::{LlmClient, StateStore, Storage, ToolExecutor};
use super::{
    SseBroadcaster, SseEvent, SubAgentCancelRequest, SubAgentSpawnRequest, TaskApprovalHandoffData,
    TaskApprovalHandoffRequest,
};

use crate::db::{MessageContent, ToolOutcome, ToolResult};
use crate::state_machine::outcome::{EffectOutcome, LlmOutcome, ToolExecOutcome};
use crate::state_machine::state::{
    SubAgentMode, SubAgentOutcome, SubAgentResult, ToolCall, ToolInput,
};
use crate::state_machine::{
    handle_outcome, tool_result_message_id, transition, CheckpointData, ConvContext, ConvState,
    Effect, Event, StepResult,
};
use crate::system_prompt::{build_system_prompt, ModeContext};
use crate::tools::{BrowserSessionManager, ToolContext};
use chrono::{DateTime, Utc};
use phoenix_llm::{
    ContentBlock, LlmMessage, LlmRequest, MessageRole, ModelRegistry, PromptCacheKey, SystemContent,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

/// Safety-net wall-clock timeout for sub-agents (REQ-SA-006).
/// Primary enforcement is max turns (REQ-PROJ-008). This catches stuck tool execution.
const DEFAULT_SUBAGENT_TIMEOUT: Duration = Duration::from_mins(20);

/// Backstop deadline forcing `CancellingTool` to a terminal-direction state
/// even when the tool task never observes its cancellation token and never
/// returns (REQ-BED-005a, spec `config.cancellation_deadline`). The cooperative
/// abort path (REQ-BED-005) is effectively immediate; this only fires for the
/// pathological "task never returns" case.
const CANCELLATION_DEADLINE: Duration = Duration::from_secs(3);

/// Last-resort backstop forcing `CancellingSubAgents` to drain to `Idle` when a
/// cancelled sub-agent never reports back. Set to 2× `CANCELLATION_DEADLINE` (6s
/// = 2 × 3s) deliberately: `CancellingSubAgents` is parent-only, so this only
/// applies to a parent waiting for its sub-agents to drain. Each sub-agent has
/// its own 3s `CancellingTool` backstop, so giving the parent 6s guarantees a
/// real cancelled result wins the race in the common case, leaving this 6s as a
/// true last resort for a sub-agent runtime that has genuinely vanished.
const CANCELLING_SUBAGENTS_DEADLINE: Duration = Duration::from_secs(6);

/// Hard byte cap on the LLM-bound text of a single tool result.
///
/// Per-tool caps are line-based (bash tail = 200 lines, `read_file` = 2000
/// lines) and do not bound bytes: a `cat` of a multi-MB single-line minified
/// file yields a handful of very long "lines" that pass every line cap, enter
/// the tool result whole, get persisted, and are resent every subsequent turn
/// until the context window is exceeded (`NotResumable` — conversation-fatal).
/// This is the final backstop applied to ALL tools at the persist choke point,
/// on top of (not instead of) the per-tool line caps. 100 KB keeps a generous
/// amount of legitimate output while making the pathological case impossible.
const MAX_TOOL_OUTPUT_BYTES: usize = 100 * 1024;

/// Maximum number of sub-agent tasks accepted in a single `spawn_agents` call.
/// Realises the `SpawnLimit` invariant in `specs/bedrock/bedrock.allium`
/// (`config.max_sub_agents_per_spawn`): a single spawn must not enqueue more
/// than this many sub-agents, bounding fan-out resource use. A call carrying
/// more tasks is rejected wholesale (no truncation) so the LLM sees an explicit
/// error rather than a silently-trimmed batch.
const MAX_SUB_AGENTS_PER_SPAWN: usize = 10;

// ── Stale tool-result clearing (specs/stale-tool-results) ───────────────────
// Old, re-queryable tool-result bodies are replaced with a placeholder once the
// request approaches the context window, governed by a monotonic per-conversation
// watermark. The model, invariants, and rationale live in the spec.

/// Recency floor: the most recent this-many tool rounds are always sent in full
/// (REQ-STR-003). Covers the agent's working set and the trailing tool result
/// the last cache breakpoint anchors on, and is >= the two rounds of visual
/// context the prior image window preserved.
const KEEP_RECENT_ROUNDS: usize = 3;

/// Clearing activates once the last turn's reported prompt size exceeds this
/// fraction (numerator/denominator) of the model context window (REQ-STR-001).
const CLEAR_TRIGGER_NUM: usize = 7;
const CLEAR_TRIGGER_DEN: usize = 10;

/// Minimum tokens a sweep must free or the watermark holds this turn, bounding
/// how often a sweep disturbs the cached prefix (REQ-STR-006).
const CLEAR_AT_LEAST_TOKENS: usize = 8192;

/// Placeholder body sent in place of a cleared tool result. The paired
/// `tool_use` block is preserved, so this is never a silent gap (REQ-STR-004).
const CLEARED_TOOL_RESULT_PLACEHOLDER: &str = "[tool result cleared to save context]";

/// Approximate wire token cost of one image block. A flattened-and-replayed
/// image still costs a fixed block of tokens; this drives both the budget
/// estimator and the clearing sweep so image-heavy rounds register their true
/// weight (REQ-STR-006, REQ-STR-011).
const IMAGE_TOKEN_ESTIMATE: usize = 1500;

/// Truncate `text` to at most [`MAX_TOOL_OUTPUT_BYTES`] bytes, keeping a head
/// and tail slice joined by a marker that records how much was dropped.
///
/// Slice boundaries are snapped down to UTF-8 char boundaries, so the result is
/// always valid UTF-8 and never splits a multi-byte char. Output already within
/// the cap is returned unchanged (no allocation of a new marker).
// string_slice: every index below is snapped to a char boundary via the
// `is_char_boundary` loops before it is used, so the slices cannot panic.
#[allow(clippy::string_slice)]
fn cap_tool_output_text(text: String) -> String {
    let total = text.len();
    if total <= MAX_TOOL_OUTPUT_BYTES {
        return text;
    }

    // The marker counts against the budget so the final string stays under the
    // cap. Its byte length depends on the decimal digits of `omitted`/`total`;
    // both are <= `total`, so an upper bound on the marker is computable up
    // front from `total` alone (omitted <= total).
    let marker_template = format!("\n…[truncated {total} bytes of {total} total]…\n");
    let marker_budget = marker_template.len();
    let content_budget = MAX_TOOL_OUTPUT_BYTES.saturating_sub(marker_budget);

    // Split the content budget head-heavy. Snap the head boundary down to a
    // char boundary.
    let head_target = (content_budget * 3 / 4).min(content_budget);
    let mut head_end = head_target.min(total);
    while head_end > 0 && !text.is_char_boundary(head_end) {
        head_end -= 1;
    }

    // Tail length is the remaining content budget; snap its start up to a char
    // boundary.
    let tail_len = content_budget - head_end;
    let mut tail_start = total - tail_len;
    while tail_start < total && !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }

    let omitted = tail_start - head_end;
    let marker = format!("\n…[truncated {omitted} bytes of {total} total]…\n");
    let mut out = String::with_capacity(head_end + (total - tail_start) + marker.len());
    out.push_str(&text[..head_end]);
    out.push_str(&marker);
    out.push_str(&text[tail_start..]);
    out
}

/// Map a producer-side [`crate::tools::ToolOutput`] onto a persisted
/// [`ToolOutcome`].
///
/// A total match on the `ToolOutput` enum — the structural payoff of making
/// it an enum: the variant, not an independently-settable `success: bool`,
/// decides the outcome, so a success can never be persisted as an error or
/// vice versa. There is deliberately no `Cancelled` arm — cancellation is
/// detected by the executor via the cancellation token before this mapping
/// is ever reached.
fn tool_output_to_outcome(out: crate::tools::ToolOutput) -> ToolOutcome {
    use crate::db::ToolContentImage;
    use crate::tools::{ToolImage, ToolOutput};
    let convert = |images: Vec<ToolImage>| -> Vec<ToolContentImage> {
        images
            .into_iter()
            .map(|img| ToolContentImage {
                media_type: img.media_type,
                data: img.data,
            })
            .collect()
    };
    match out {
        ToolOutput::Success {
            output,
            images,
            display_data,
            ..
        } => ToolOutcome::Success {
            output: cap_tool_output_text(output),
            display_data,
            images: convert(images),
        },
        ToolOutput::Error {
            output,
            images,
            display_data,
            ..
        } => ToolOutcome::Error {
            output: cap_tool_output_text(output),
            display_data,
            images: convert(images),
        },
    }
}

/// Await a tool task's oneshot outcome and forward it, generation-tagged, to
/// the executor's tool-outcome channel, mapping a dropped sender to a typed
/// `Failed` outcome.
///
/// A dropped oneshot sender (the tool task panicked or was aborted before it
/// could `send`) must NOT silently lose the outcome: with no delivery,
/// `ToolExecuting` never sees `ToolComplete` and a pending `CancellingTool`
/// waits forever, rejecting all input until restart. `Failed` →
/// `Event::ToolComplete` (an error `ToolResult`), accepted by both
/// `ToolExecuting` and `CancellingTool`, so the conversation always progresses.
///
/// The `generation` stamp lets the executor distinguish an outcome for the
/// still-in-flight tool from a stale one belonging to an intentionally aborted
/// task (see `ConversationRuntime::tool_request_generation`). An intentional
/// `Effect::AbortTool` / backstop teardown aborts the task, dropping the
/// sender, so the synthetic `Failed` looks identical to a genuine panic; the
/// executor discards any forwarded outcome whose generation is no longer
/// current, even if a later turn reuses the same `tool_use_id`.
async fn forward_tool_outcome(
    tool_rx: oneshot::Receiver<ToolExecOutcome>,
    tool_use_id: String,
    generation: u64,
    tool_outcome_tx: mpsc::Sender<(u64, ToolExecOutcome)>,
) {
    let tool_outcome = match tool_rx.await {
        Ok(tool_outcome) => tool_outcome,
        Err(_recv_error) => {
            tracing::warn!(
                tool_use_id = %tool_use_id,
                generation,
                "tool task dropped its outcome sender (panic/abort); \
                 synthesizing Failed outcome to avoid wedging the conversation"
            );
            ToolExecOutcome::Failed {
                tool_use_id,
                error: "tool task aborted or panicked".to_string(),
            }
        }
    };
    let _ = tool_outcome_tx.send((generation, tool_outcome)).await;
}

/// Run one cleared tool call to its typed `ToolExecOutcome`. Cancellation is
/// detected from the token state, NOT the output string — the state machine
/// only accepts `ToolAborted` from `CancellingTool`, entered when the
/// `AbortTool` effect cancels the token (FM-1 prevention). A missing output
/// (unknown tool) maps to `Failed`.
async fn execute_tool_to_outcome<S, T>(
    storage: S,
    tool_executor: Arc<T>,
    checked: crate::runtime::deny_gate::CheckedToolCall,
    tool_ctx: ToolContext,
    cancel_token_check: &CancellationToken,
    conv_id: String,
    tool_name: String,
    tool_use_id: String,
) -> ToolExecOutcome
where
    S: Storage + Clone + 'static,
    T: ToolExecutor + ?Sized,
{
    tracing::info!(conv_id = %conv_id, tool = %tool_name, id = %tool_use_id, "Executing tool");
    let tool_start = std::time::Instant::now();

    let output = tool_executor.execute(checked, tool_ctx).await;

    if cancel_token_check.is_cancelled() {
        tracing::info!(conv_id = %conv_id, tool = %tool_name, id = %tool_use_id, "Tool cancelled");
        ToolExecOutcome::Aborted {
            tool_use_id,
            reason: crate::state_machine::AbortReason::CancellationRequested,
        }
    } else if let Some(mut out) = output {
        if let Some(llm_usage) = out.take_llm_usage() {
            let _ = storage
                .insert_turn_usage(&conv_id, &conv_id, &llm_usage.model, &llm_usage.usage)
                .await
                .map_err(|e| {
                    tracing::warn!(conv_id = %conv_id, error = %e, "failed to record tool LLM usage");
                });
        }
        let duration_ms = u64::try_from(tool_start.elapsed().as_millis()).unwrap_or(u64::MAX);
        tracing::info!(
            conv_id = %conv_id,
            tool = %tool_name,
            id = %tool_use_id,
            duration_ms,
            success = out.is_success(),
            "Tool completed"
        );
        let outcome = tool_output_to_outcome(out);
        ToolExecOutcome::Completed(ToolResult {
            tool_use_id: tool_use_id.clone(),
            outcome,
            duration_ms: Some(duration_ms),
        })
    } else {
        tracing::warn!(conv_id = %conv_id, tool = %tool_name, id = %tool_use_id, "Tool not found");
        ToolExecOutcome::Failed {
            tool_use_id,
            error: format!("Unknown tool: {tool_name}"),
        }
    }
}

/// Forward a permission-gate denial to the conversation loop as a completed
/// tool outcome, without spawning a tool task (specs/permissions REQ-PERM-005,
/// deny-and-continue). The denial reaches the model through the same channel a
/// real tool result uses, so it can attempt an alternative.
fn forward_denial_outcome(
    tool_use_id: String,
    denial: crate::runtime::deny_gate::Denial,
    generation: u64,
    tool_outcome_tx: mpsc::Sender<(u64, ToolExecOutcome)>,
) {
    let outcome = ToolExecOutcome::Completed(ToolResult {
        tool_use_id: tool_use_id.clone(),
        outcome: tool_output_to_outcome(denial.into_tool_output()),
        duration_ms: Some(0),
    });
    let (denied_tx, denied_rx) = oneshot::channel::<ToolExecOutcome>();
    let _ = denied_tx.send(outcome);
    tokio::spawn(forward_tool_outcome(
        denied_rx,
        tool_use_id,
        generation,
        tool_outcome_tx,
    ));
}

/// Await an LLM task's oneshot outcome and forward it, generation-tagged, to
/// the executor's LLM-outcome channel, mapping a dropped sender to a typed
/// `NetworkError` outcome.
///
/// A dropped oneshot sender (the LLM task panicked or was aborted before it
/// could `send`) must NOT silently lose the outcome: with no delivery, an
/// `LlmRequesting`/`AwaitingContinuation` state hangs forever. `NetworkError`
/// is retryable, so the retry machinery recovers.
///
/// The `generation` stamp lets the executor distinguish an outcome for the
/// still-in-flight request from a stale one belonging to an intentionally
/// aborted request (see `ConversationRuntime::llm_request_generation`). An
/// intentional `Effect::AbortLlm` also drops the sender — without the tag, the
/// synthetic `NetworkError` would look identical to a genuine panic and could
/// be applied to a later, unrelated `LlmRequesting` turn, triggering a
/// spurious retry. The executor discards any forwarded outcome whose
/// generation is no longer current.
async fn forward_llm_outcome(
    llm_rx: oneshot::Receiver<LlmOutcome>,
    generation: u64,
    llm_outcome_tx: mpsc::Sender<(u64, LlmOutcome)>,
) {
    let llm_outcome = match llm_rx.await {
        Ok(llm_outcome) => llm_outcome,
        Err(_recv_error) => {
            tracing::warn!(
                generation,
                "LLM task dropped its outcome sender (panic/abort); \
                 synthesizing NetworkError outcome to avoid wedging the conversation"
            );
            LlmOutcome::NetworkError {
                message: "LLM task aborted or panicked".to_string(),
            }
        }
    };
    let _ = llm_outcome_tx.send((generation, llm_outcome)).await;
}

/// The outcome of planning a stale tool-result clearing sweep over one
/// conversation's history (specs/stale-tool-results).
#[derive(Debug, Default, Clone)]
struct ClearPlan {
    /// The watermark after this turn's (possible) advance, persisted back to
    /// the conversation.
    new_watermark: i64,
    /// Message `sequence_id`s of all clearable tool results at or before
    /// `new_watermark` — the full set to render as placeholders this turn (stable
    /// across turns). Keyed by `sequence_id`, not `tool_use_id`: the latter can be
    /// reused across turns (provider-dependent), which would otherwise clear a
    /// recent floor result that happens to share an old id.
    cleared_sequence_ids: std::collections::HashSet<i64>,
    /// Tokens freed by *this* turn's advance (newly cleared results only).
    freed_tokens: usize,
    /// Number of results newly cleared by this turn's advance.
    cleared_count: usize,
}

/// A tool-result message reduced to the facts the clearing planner decides over.
struct ToolResultFacts {
    sequence_id: i64,
    round: usize,
    clearable: bool,
    token_estimate: usize,
}

/// Image-aware token estimate for a persisted tool result: its text body plus a
/// fixed cost per image block (REQ-STR-006, REQ-STR-011).
fn estimate_tool_result_tokens(content: &str, image_count: usize) -> usize {
    estimate_text_tokens(content) + image_count * IMAGE_TOKEN_ESTIMATE
}

/// Gather the per-result facts the clearing planner decides over, in history
/// order, plus the highest round seen. A round opens at each `Agent` message
/// carrying a `ToolUse` block; every tool-result message belongs to the most
/// recently opened round (the first is numbered 0). A `tool_use` always precedes
/// its result in `sequence_id` order, so the producing-tool lookup is filled
/// inline in this single pass.
fn collect_tool_result_facts(
    db_messages: &[crate::db::Message],
    clearable_tool_names: &std::collections::HashSet<String>,
) -> (Vec<ToolResultFacts>, usize) {
    use crate::db::MessageContent;
    use phoenix_llm::ContentBlock as LlmContentBlock;

    let mut tool_name_of: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let mut results: Vec<ToolResultFacts> = Vec::new();
    let mut current_round: usize = 0;
    let mut max_round: usize = 0;
    let mut opened_a_round = false;
    for m in db_messages {
        match &m.content {
            MessageContent::Agent(blocks) => {
                let mut has_tool_use = false;
                for b in blocks {
                    if let LlmContentBlock::ToolUse { id, name, .. } = b {
                        tool_name_of.insert(id.as_str(), name.as_str());
                        has_tool_use = true;
                    }
                }
                if has_tool_use {
                    if opened_a_round {
                        current_round += 1;
                    }
                    opened_a_round = true;
                }
            }
            MessageContent::Tool(tc) => {
                let name = tool_name_of
                    .get(tc.tool_use_id.as_str())
                    .copied()
                    .unwrap_or("");
                results.push(ToolResultFacts {
                    sequence_id: m.sequence_id,
                    round: current_round,
                    clearable: clearable_tool_names.contains(name),
                    token_estimate: estimate_tool_result_tokens(&tc.content, tc.images.len()),
                });
                max_round = current_round;
            }
            _ => {}
        }
    }
    (results, max_round)
}

/// Plan a stale tool-result clearing sweep (specs/stale-tool-results.allium).
///
/// `reported_prompt_tokens` is the previous turn's actual prompt size — the
/// pressure signal; `None` (no prior turn) is treated as not under pressure.
/// When it exceeds the trigger and at least `CLEAR_AT_LEAST_TOKENS` are freeable
/// outside the recency floor, the sweep clears *everything* outside the floor in
/// one move.
fn plan_tool_result_clearing(
    db_messages: &[crate::db::Message],
    clearable_tool_names: &std::collections::HashSet<String>,
    prior_watermark: i64,
    reported_prompt_tokens: Option<i64>,
    context_window: usize,
) -> ClearPlan {
    let (results, max_round) = collect_tool_result_facts(db_messages, clearable_tool_names);

    // Recency floor: rounds at or above `floor_first_round` are never cleared.
    let floor_first_round = (max_round + 1).saturating_sub(KEEP_RECENT_ROUNDS);

    // Worthwhile-gain gate: tokens freeable by clearing every eligible result
    // (clearable, not yet cleared, outside the floor).
    let eligible_freed: usize = results
        .iter()
        .filter(|f| f.clearable && f.sequence_id > prior_watermark && f.round < floor_first_round)
        .map(|f| f.token_estimate)
        .sum();

    let trigger = context_window * CLEAR_TRIGGER_NUM / CLEAR_TRIGGER_DEN;
    let trigger = i64::try_from(trigger).unwrap_or(i64::MAX);
    let over_pressure = reported_prompt_tokens.is_some_and(|tokens| tokens > trigger);

    let mut new_watermark = prior_watermark;
    if over_pressure && eligible_freed >= CLEAR_AT_LEAST_TOKENS {
        // Maximal sweep (REQ-STR-007): advance the watermark to the newest
        // result below the floor, clearing the whole pre-floor prefix at once.
        if let Some(floor_boundary) = results
            .iter()
            .filter(|f| f.round < floor_first_round)
            .map(|f| f.sequence_id)
            .max()
        {
            new_watermark = new_watermark.max(floor_boundary);
        }
    }

    let cleared_sequence_ids: std::collections::HashSet<i64> = results
        .iter()
        .filter(|f| f.clearable && f.sequence_id <= new_watermark)
        .map(|f| f.sequence_id)
        .collect();
    let (freed_tokens, cleared_count) = results
        .iter()
        .filter(|f| {
            f.clearable && f.sequence_id > prior_watermark && f.sequence_id <= new_watermark
        })
        .fold((0usize, 0usize), |(tok, cnt), f| {
            (tok + f.token_estimate, cnt + 1)
        });

    ClearPlan {
        new_watermark,
        cleared_sequence_ids,
        freed_tokens,
        cleared_count,
    }
}

/// Message `sequence_id`s of the clearable tool results at or before `watermark`.
/// Renders a turn consistently with the durably-persisted watermark when a fresh
/// sweep cannot be recorded (REQ-STR-007).
fn clearable_sequence_ids_through_watermark(
    db_messages: &[crate::db::Message],
    clearable_tool_names: &std::collections::HashSet<String>,
    watermark: i64,
) -> std::collections::HashSet<i64> {
    collect_tool_result_facts(db_messages, clearable_tool_names)
        .0
        .into_iter()
        .filter(|f| f.clearable && f.sequence_id <= watermark)
        .map(|f| f.sequence_id)
        .collect()
}

/// Assemble the LLM-bound message list for one request, applying stale
/// tool-result clearing (specs/stale-tool-results): read the pressure signal and
/// the persisted watermark, plan a sweep, persist any advance, and render the
/// history once with the resulting cleared set.
///
/// The failure paths preserve the grow-only guarantee (REQ-STR-007):
/// - watermark read error → fall back to `watermark_cache`, the last value this
///   runtime durably read or wrote, so the cleared set never shrinks and
///   previously-cleared results are not re-exposed; only if it was never read
///   (cache empty) is the uncleared history sent;
/// - usage read error → treat as not under pressure (hold the watermark);
/// - watermark write error → render the prior watermark's set, matching what is
///   durably recorded so the next turn cannot un-clear it.
///
/// `watermark_cache` mirrors the durable watermark in memory; it is the runtime's
/// own state (the runtime is the sole writer for its conversation), so it can
/// never be staler than the database after a successful read or write.
async fn assemble_cleared_messages<S: StateStore>(
    storage: &S,
    conv_id: &str,
    db_messages: &[crate::db::Message],
    clearable_names: &std::collections::HashSet<String>,
    context_window: usize,
    watermark_cache: &std::sync::Mutex<Option<i64>>,
) -> Vec<LlmMessage> {
    let prior_watermark = match storage.get_clear_watermark(conv_id).await {
        Ok(w) => {
            *watermark_cache.lock().unwrap() = Some(w);
            w
        }
        Err(e) => {
            let cached = *watermark_cache.lock().unwrap();
            return if let Some(w) = cached {
                tracing::warn!(
                    conv_id = %conv_id, error = %e, watermark = w,
                    "failed to read clear watermark; rendering last known cleared set",
                );
                let cleared =
                    clearable_sequence_ids_through_watermark(db_messages, clearable_names, w);
                render_messages(db_messages, &cleared)
            } else {
                tracing::warn!(
                    conv_id = %conv_id, error = %e,
                    "failed to read clear watermark with none cached; sending uncleared history",
                );
                render_messages(db_messages, &std::collections::HashSet::new())
            };
        }
    };

    let reported_prompt_tokens = match storage.get_last_turn_prompt_tokens(conv_id).await {
        Ok(tokens) => tokens,
        Err(e) => {
            tracing::warn!(
                conv_id = %conv_id, error = %e,
                "failed to read last-turn usage; holding watermark this turn",
            );
            None
        }
    };

    let plan = plan_tool_result_clearing(
        db_messages,
        clearable_names,
        prior_watermark,
        reported_prompt_tokens,
        context_window,
    );

    let cleared = if plan.new_watermark == prior_watermark {
        plan.cleared_sequence_ids
    } else {
        match storage
            .set_clear_watermark(conv_id, plan.new_watermark)
            .await
        {
            Ok(()) => {
                *watermark_cache.lock().unwrap() = Some(plan.new_watermark);
                tracing::debug!(
                    conv_id = %conv_id,
                    cleared_results = plan.cleared_count,
                    freed_tokens = plan.freed_tokens,
                    watermark = plan.new_watermark,
                    "stale tool-result clearing swept old results",
                );
                plan.cleared_sequence_ids
            }
            Err(e) => {
                tracing::warn!(
                    conv_id = %conv_id, error = %e,
                    "failed to persist clear watermark; holding prior cleared set this turn",
                );
                clearable_sequence_ids_through_watermark(
                    db_messages,
                    clearable_names,
                    prior_watermark,
                )
            }
        }
    };

    render_messages(db_messages, &cleared)
}

/// Fold persisted messages into the provider-agnostic LLM message list.
///
/// A tool result whose message `sequence_id` is in `cleared_sequence_ids` is
/// rendered as a fixed placeholder with no image blocks (stale tool-result
/// clearing, specs/stale-tool-results); the paired `tool_use` block is left
/// intact, so a cleared result is never a silent gap. Every other tool result is
/// sent verbatim with its images. The persisted messages are never mutated — the
/// cleared form exists only in the returned list for this one request.
fn render_messages(
    db_messages: &[crate::db::Message],
    cleared_sequence_ids: &std::collections::HashSet<i64>,
) -> Vec<LlmMessage> {
    use crate::db::{MessageContent, ToolContent};
    use phoenix_llm::ImageSource;

    let mut messages = Vec::new();
    for msg in db_messages {
        match &msg.content {
            MessageContent::User(user_content) => {
                // Use llm_text when expansion occurred (REQ-IR-001, REQ-IR-006):
                // the model sees the fully resolved form while the DB stores the shorthand.
                let mut text_for_llm = user_content.llm_text().to_string();
                if !user_content.files.is_empty() {
                    for file in &user_content.files {
                        text_for_llm.push('\n');
                        text_for_llm.push_str(&file.llm_context_tag());
                    }
                }
                let mut content = vec![ContentBlock::text(text_for_llm)];

                // Add images (REQ-BED-013)
                for img in &user_content.images {
                    content.push(ContentBlock::Image {
                        source: img.to_image_source(),
                    });
                }

                messages.push(LlmMessage {
                    role: MessageRole::User,
                    content,
                });
            }

            MessageContent::Agent(blocks) => {
                messages.push(LlmMessage {
                    role: MessageRole::Assistant,
                    content: blocks.clone(),
                });
            }

            MessageContent::Tool(ToolContent {
                tool_use_id,
                content,
                is_error,
                images,
            }) => {
                // Cleared results render as a placeholder with no images; kept
                // results carry their body and any images verbatim (REQ-STR-004,
                // REQ-STR-011).
                let (text, image_sources): (String, Vec<ImageSource>) =
                    if cleared_sequence_ids.contains(&msg.sequence_id) {
                        (CLEARED_TOOL_RESULT_PLACEHOLDER.to_string(), Vec::new())
                    } else {
                        let sources = images
                            .iter()
                            .map(|img| ImageSource::Base64 {
                                media_type: img.media_type.clone(),
                                data: img.data.clone(),
                            })
                            .collect();
                        (content.clone(), sources)
                    };

                // Tool results go in user message
                messages.push(LlmMessage {
                    role: MessageRole::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: text,
                        images: image_sources,
                        is_error: *is_error,
                    }],
                });
            }

            // Skill messages are delivered as user-role messages (REQ-SK-002)
            MessageContent::Skill(skill_content) => {
                let mut body = skill_content.body.clone();
                for file in &skill_content.files {
                    body.push('\n');
                    body.push_str(&file.llm_context_tag());
                }
                messages.push(LlmMessage {
                    role: MessageRole::User,
                    content: vec![ContentBlock::text(body)],
                });
            }

            // Ignore system, error, and continuation messages.
            // System messages are UI-only bookkeeping (restart markers, task
            // file renames, diff snapshots). LLM-directed messages use
            // MessageContent::User with is_meta (e.g., grace turn prompt).
            MessageContent::System(_)
            | MessageContent::Error(_)
            | MessageContent::Continuation(_) => {}
        }
    }
    messages
}

/// Render a human-readable summary of sub-agent outcomes for the LLM.
///
/// Both `persist_sub_agent_results` carriers (the `spawn_agents` `tool_result`
/// and the standalone assistant text message) feed the model this same text.
fn render_sub_agent_summary(results: &[SubAgentResult]) -> String {
    let body = results
        .iter()
        .map(|r| {
            let outcome = match &r.outcome {
                SubAgentOutcome::Success { result } => format!("Result: {result}"),
                SubAgentOutcome::Failure { error, .. } => format!("Failed: {error}"),
                SubAgentOutcome::TimedOut => {
                    "Timed out: sub-agent exceeded its time limit".to_string()
                }
            };
            format!("Task: \"{}\"\n{outcome}", r.task)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("Sub-agent results ({} completed):\n\n{body}", results.len())
}

/// Decide whether `path` is inside the worktree rooted at `root`.
///
/// Three stages, each closing a class of bypass:
///
/// 1. Reject non-absolute paths. Relative overrides are ambiguous (no
///    defined resolution base) and not worth modelling.
/// 2. Canonicalise `root`. The worktree must exist; if it doesn't,
///    the comparison is meaningless and we reject (fail closed).
/// 3. Canonicalise the deepest existing ancestor of `path` and check
///    that ancestor lies inside the canonical root. `canonicalize`
///    resolves `..` and symlinks together (it's a `realpath` call),
///    so neither construct can escape:
///    `/worktree/../escape` -> ancestor `/worktree/..` -> canonical
///    `/parent_of_worktree` -> rejected.
///    `/worktree/escape/newdir` where `escape` symlinks to `/outside`
///    -> ancestor `/worktree/escape` -> canonical `/outside` ->
///    rejected.
///    Conversely, an in-worktree path that just happens to use `..`
///    to traverse internally (e.g. `/worktree/src/../tests`)
///    canonicalises back to a subpath of the worktree -> accepted.
fn path_is_within(path: &str, root: &str) -> bool {
    use std::path::{Path, PathBuf};
    let raw_path = Path::new(path);
    if !raw_path.is_absolute() {
        return false;
    }
    let Ok(canon_root) = std::fs::canonicalize(Path::new(root)) else {
        return false;
    };
    let mut anchor = PathBuf::from(raw_path);
    loop {
        if let Ok(canon) = std::fs::canonicalize(&anchor) {
            return canon == canon_root || canon.starts_with(&canon_root);
        }
        if !anchor.pop() {
            return false;
        }
    }
}

/// Default cap on consecutive LLM requests within a single parent-conversation
/// user turn. Distinct from sub-agent `max_turns`: this resets on every
/// `Event::UserMessage`, so a long conversation is never penalised — only a
/// runaway `tool_use` burst within one turn. Overridable via the
/// `PHOENIX_PARENT_TOOL_CYCLE_CAP` env var; set to `0` to disable.
///
/// Set deliberately high — this is a backup safety-net, not a budget.
/// A well-behaved agent + real user is expected to stay far below it;
/// hitting this cap means something is stuck or looping.
const DEFAULT_PARENT_TOOL_CYCLE_CAP: u32 = 1000;

/// Resolve the parent-conversation tool-use cycle cap from the environment,
/// falling back to [`DEFAULT_PARENT_TOOL_CYCLE_CAP`]. A malformed value logs
/// a warning and uses the default. Called once per runtime at construction.
fn parent_tool_cycle_cap_from_env() -> u32 {
    let Ok(raw) = std::env::var("PHOENIX_PARENT_TOOL_CYCLE_CAP") else {
        return DEFAULT_PARENT_TOOL_CYCLE_CAP;
    };
    raw.parse::<u32>().unwrap_or_else(|_| {
        tracing::warn!(
            raw = %raw,
            default = DEFAULT_PARENT_TOOL_CYCLE_CAP,
            "PHOENIX_PARENT_TOOL_CYCLE_CAP is not a non-negative integer; using default"
        );
        DEFAULT_PARENT_TOOL_CYCLE_CAP
    })
}

/// Generic conversation runtime that can work with any storage, LLM, and tool implementations
pub struct ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    context: ConvContext,
    state: ConvState,
    /// Server clock at which the conversation entered `state` — initialised
    /// from the loaded `Conversation.state_updated_at` (or `Utc::now()` for
    /// fresh runtimes) and bumped to `Utc::now()` whenever `state` is
    /// reassigned by `apply_transition`. Carried on every `SseEvent::StateChange`
    /// emission so the client's elapsed-time display can derive
    /// `now() - state_updated_at` without any per-event timestamping
    /// (specs/working-phase-visibility/ REQ-WPV-001). This same value is
    /// threaded into the DB write via `StateStore::update_state`, so the
    /// persisted `Conversation.state_updated_at` and the SSE-carried value
    /// are identical — no clock drift between the two.
    state_updated_at: DateTime<Utc>,
    storage: S,
    llm_client: Arc<L>,
    tool_executor: Arc<T>,
    /// Names of tools whose stale results may be cleared (specs/stale-tool-results).
    /// Static for the conversation's tool set, so it is computed once and only
    /// recomputed on the Explore→Work upgrade, avoiding a registry lock +
    /// `HashSet` rebuild on every LLM dispatch.
    clearable_names: Arc<std::collections::HashSet<String>>,
    /// In-memory mirror of the durable clear watermark (specs/stale-tool-results).
    /// This runtime is the sole writer of its conversation's watermark, so the
    /// cache is authoritative after any successful read/write; a transient
    /// watermark read failure falls back to it instead of un-clearing history.
    clear_watermark_cache: Arc<std::sync::Mutex<Option<i64>>>,
    /// Browser session manager for `ToolContext`
    browser_sessions: Arc<BrowserSessionManager>,
    /// Bash handle registry for `ToolContext` (REQ-BASH-014).
    bash_handles: Arc<crate::tools::BashHandleRegistry>,
    /// Tmux server registry for `ToolContext` (REQ-TMUX-013).
    tmux_registry: Arc<crate::tools::TmuxRegistry>,
    /// LLM registry for `ToolContext`
    llm_registry: Arc<ModelRegistry>,
    /// Active PTY terminal sessions — passed to `ToolContext` for `read_terminal` tool.
    terminals: crate::terminal::ActiveTerminals,
    event_rx: mpsc::Receiver<Event>,
    event_tx: mpsc::Sender<Event>,
    broadcast_tx: SseBroadcaster,
    /// Token to cancel running tool execution
    tool_cancel_token: Option<CancellationToken>,
    /// Handle to the spawned LLM task — aborted on cancel to drop the HTTP connection
    llm_task_handle: Option<tokio::task::JoinHandle<()>>,
    /// Abort handle for the in-flight retry-backoff timer spawned by
    /// `Effect::ScheduleRetry`. Kept so a transition out of the
    /// retry-scheduling state can proactively abort the timer task before it
    /// fires, but correctness does NOT depend on it: a stale `RetryTimeout`
    /// that has already raced onto the channel is rejected by the
    /// `retry_generation` match, not by handle presence (see `retry_generation`).
    retry_timer_handle: Option<tokio::task::AbortHandle>,
    /// Generation/epoch tag for the in-flight retry-backoff timer, mirroring
    /// `llm_request_generation`. Bumped each time a retry timer is scheduled
    /// (`Effect::ScheduleRetry`) and each time the conversation leaves the
    /// retry-scheduling state (cancel / response / new turn / exhaustion). The
    /// timer task stamps the dispatching generation onto its fire; the select
    /// loop discards a `RetryTimeout` whose generation != this current value.
    ///
    /// Identity guard the pure reducer cannot provide: attempt numbers reset
    /// per turn, so the reducer's `attempt == retry_attempt` check cannot tell
    /// a stale timer (cancelled-then-resent turn) from the live one. Handle
    /// presence alone is insufficient too: with an OLD `RetryTimeout` already
    /// queued AND a NEWER backoff pending (handle = `Some` for the new timer),
    /// using presence as the guard would let the old fire clear the handle, the
    /// reducer would then reject it on attempt mismatch, and the LIVE timer's
    /// later fire would be dropped as "no retry pending" — wedging the turn in
    /// backoff forever. Matching the generation honors the live fire and
    /// discards only the genuinely stale one.
    retry_generation: u64,
    /// Generation/epoch tag for the in-flight LLM request. Incremented at each
    /// LLM dispatch (`Effect::RequestLlm`) and at each intentional abort
    /// (`Effect::AbortLlm`). The LLM outcome forwarder stamps the dispatching
    /// generation onto every outcome it forwards; `process_outcome`-side
    /// routing discards any LLM outcome whose generation != this current value.
    ///
    /// Identity guard for the LLM request, mirroring `retry_timer_handle`:
    /// `Effect::AbortLlm` (user cancel / new turn) aborts the in-flight LLM
    /// task, which drops its outcome sender. The forwarder maps that drop to a
    /// retryable `NetworkError` so a genuine panic can never wedge the
    /// conversation — but an *intentional* abort produces the same drop. Since
    /// the synthetic outcome carries no request identity, a stale aborted
    /// outcome could otherwise be applied to a freshly-started `LlmRequesting`
    /// state for an unrelated turn and trigger a spurious retry. Bumping the
    /// generation on abort makes the stale outcome's generation old, so it is
    /// dropped; a genuine current-generation panic still surfaces as a failure.
    llm_request_generation: u64,
    /// Executor-internal, generation-tagged LLM-outcome channel. The forwarder
    /// sends `(dispatch_generation, outcome)`; the select loop routes the
    /// outcome to `process_outcome` only when the generation is still current.
    /// The epoch tag stays executor-side and
    /// never leaks into the pure `EffectOutcome` type.
    llm_outcome_tx: mpsc::Sender<(u64, LlmOutcome)>,
    llm_outcome_rx: mpsc::Receiver<(u64, LlmOutcome)>,
    /// Generation/epoch tag for the in-flight tool task, mirroring
    /// `llm_request_generation`. Incremented at each tool dispatch (the
    /// `tool_task_handle` spawn site) and at the `CancellingTool` backstop's
    /// forced teardown (`handle_cancelling_tool_timeout`, which aborts the
    /// wedged task and drops its sender). The tool outcome forwarder stamps the
    /// dispatching generation onto every outcome it forwards; select-loop-side
    /// routing discards any tool outcome whose generation != this current value.
    ///
    /// NOT bumped on `Effect::AbortTool`: that only cancels the token, and the
    /// cooperative tool task then produces a real `Aborted` outcome the state
    /// machine consumes to reach Idle. That outcome carries the still-current
    /// dispatch generation and must not be discarded.
    ///
    /// Identity guard the state machine's id/state gate cannot fully provide:
    /// the reducer accepts a tool outcome only when the `tool_use_id` matches
    /// the current tool AND the conversation is in a tool state. After a forced
    /// cancellation reaches Idle, an aborted tool task's forwarder can still
    /// enqueue a stale outcome; if the NEXT turn's tool call happens to REUSE
    /// the same `tool_use_id` while that stale outcome is still queued, the
    /// id/state gate would accept the stale failure as the new tool's result.
    /// `tool_use_id` uniqueness across turns is provider-dependent, not
    /// structurally enforced. Bumping the generation on the next dispatch (and
    /// on the forced teardown) makes the stale outcome's generation old, so it
    /// is dropped; the live tool's current-generation outcome still matches. See
    /// specs/bedrock REQ-BED-005a.
    tool_request_generation: u64,
    /// Executor-internal, generation-tagged tool-outcome channel, mirroring
    /// `llm_outcome_tx`. The forwarder sends `(dispatch_generation, outcome)`;
    /// the select loop routes the outcome to `process_outcome` only when the
    /// generation is still current. The epoch
    /// tag stays executor-side and never leaks into the pure `EffectOutcome`
    /// type.
    tool_outcome_tx: mpsc::Sender<(u64, ToolExecOutcome)>,
    tool_outcome_rx: mpsc::Receiver<(u64, ToolExecOutcome)>,
    /// Executor-internal, generation-tagged retry-timer channel, mirroring
    /// `llm_outcome_tx`. The backoff timer sends `(retry_generation, attempt)`;
    /// the select loop routes a `RetryTimeout` to `process_outcome` only when
    /// the generation is still current. The epoch
    /// epoch tag stays executor-side and never leaks into the pure
    /// `EffectOutcome` type.
    retry_outcome_tx: mpsc::Sender<(u64, u32)>,
    retry_outcome_rx: mpsc::Receiver<(u64, u32)>,
    /// Channel to notify parent of sub-agent completion (sub-agent only)
    parent_event_tx: Option<mpsc::Sender<Event>>,
    /// Channel to request sub-agent spawning (parent only)
    spawn_tx: Option<mpsc::Sender<SubAgentSpawnRequest>>,
    /// Channel to request sub-agent cancellation (parent only)
    cancel_tx: Option<mpsc::Sender<SubAgentCancelRequest>>,
    handoff_tx: Option<mpsc::Sender<TaskApprovalHandoffRequest>>,
    /// Buffer for `SubAgentResult` events received before entering `AwaitingSubAgents`.
    /// Pre-allocated with capacity = sub-agent count when spawning (FM-6 prevention).
    sub_agent_result_buffer: Vec<Event>,
    /// Generation/epoch tag for the current sub-agent fan-in round, mirroring
    /// `llm_request_generation`. Bumped on each entry to `AwaitingSubAgents`.
    /// Used to label the buffer-drain on round entry for observability; the
    /// drain's keep/discard decision is by `pending`-set membership (each
    /// `agent_id` is a unique UUID belonging to exactly one round), so a late
    /// result from a completed round is discarded rather than misapplied to the
    /// next round.
    sub_agent_round_gen: u64,
    /// Steering messages queued while the conversation was busy. Delivered
    /// one-at-a-time (FIFO) when the conversation next enters `Idle`.
    /// Loaded from DB at executor startup; persisted back after each enqueue
    /// or dequeue.
    steering_queue: Vec<crate::state_machine::event::SteerEntry>,
    /// Unified liveness deadline for every waiting state — the single backstop
    /// that guarantees a waiting conversation cannot wedge forever. It is armed
    /// on entering any waiting state (`AwaitingSubAgents`, `CancellingTool`,
    /// `CancellingSubAgents`) with that state's duration (see `deadline_for`) and
    /// cleared on leaving all of them. When it fires, the one select-loop arm
    /// calls `handle_deadline_expiry`, which dispatches on the current state:
    /// `AwaitingSubAgents` is the completion timeout (REQ-SA-006, inject
    /// `TimedOut`); `CancellingTool` is forced teardown (REQ-BED-005a, inject
    /// `ToolAborted`); `CancellingSubAgents` is forced teardown (REQ-BED-005a,
    /// inject `TimedOut` per still-pending sub-agent so the parent drains to Idle
    /// without waiting for stragglers).
    /// Keeping this a single field + single arm + single dispatcher makes the
    /// liveness backstop structural: adding a future waiting state means handling
    /// it in `deadline_for` and `handle_deadline_expiry`, nowhere else.
    deadline: Option<tokio::time::Instant>,
    /// Handle to the currently-spawned tool execution task. Retained so the
    /// cancellation backstop can abort an uncooperative tool task that never
    /// observes its token. Mirrors `llm_task_handle`. Replaced when a new tool
    /// spawns; aborting it drops the tool's outcome sender, but a resulting
    /// stale `Failed`/late `ToolComplete` is rejected by the state machine once
    /// the conversation has already left `CancellingTool` (id/state mismatch ->
    /// logged, harmless).
    tool_task_handle: Option<tokio::task::JoinHandle<()>>,
    /// Count of active Work-mode sub-agents for one-writer constraint (REQ-PROJ-008)
    active_work_subagents: u32,
    /// LLM turn counter for sub-agents (REQ-PROJ-008 max turns enforcement)
    llm_turn_count: u32,
    /// Whether this sub-agent has been given its grace turn (one extra LLM turn to call `submit_result`)
    grace_turn_granted: bool,
    /// LLM request counter for parent conversations. Resets on every
    /// `Event::UserMessage`, so a long conversation with many turns is fine;
    /// only runaway tool-use bursts within a single user turn trip the cap.
    /// Guards against tasks 24684 + 24680 (a provider that keeps asking for
    /// a missing tool can otherwise loop until the DB runs out of space).
    /// Task 24684 was originally numbered 24679 in commit history — see
    /// the task file for the rebase-time renumbering note.
    parent_tool_cycle_count: u32,
    /// Cap on `parent_tool_cycle_count` before the runtime halts and emits
    /// a system message. `0` disables the cap. Read once at construction
    /// time from `PHOENIX_PARENT_TOOL_CYCLE_CAP`, with
    /// [`DEFAULT_PARENT_TOOL_CYCLE_CAP`] as the fallback. Tests that want
    /// to exercise the cap deterministically use [`Self::with_parent_tool_cycle_cap`].
    parent_tool_cycle_cap: u32,
    /// Credential helper for recovery settlement (REQ-BED-030).
    /// When the state is `AwaitingRecovery`, the select loop awaits `settled.notified()`.
    credential_helper: Option<Arc<phoenix_llm::CredentialHelper>>,
    /// Named-agent catalog frozen at conversation start (parent conversations
    /// only). The same catalog renders the `spawn_agents` `agent_type` enum and
    /// resolves `agent_type` at spawn time, so the advertised choice and the
    /// runtime resolution never diverge mid-conversation (REQ-AG-004/008).
    /// Empty for sub-agents (which cannot spawn).
    agent_catalog: Arc<[phoenix_agents::AgentDefinition]>,
    /// Sender to the single serialized fork-resolution consumer, used solely to
    /// retire this conversation's still-pending fork proposals when it reaches a
    /// terminal state (`ForkProposalsRetiredOnOriginTerminal`, REQ-PROJ-035). Set
    /// by the runtime manager for parent conversations that can propose forks;
    /// `None` for sub-agents and in tests that don't exercise the
    /// terminal-retirement path. Routing through the consumer (rather than a raw
    /// DB handle) keeps retirement serialized with approve/request-changes so a
    /// terminal transition can't tear down an in-flight resolve's worktree.
    fork_cmd_tx: Option<mpsc::Sender<super::fork_resolve::ForkCommand>>,
    /// Watch sender that publishes the current `ConvState` on every transition.
    /// The paired `watch::Receiver` lives in `ConversationHandle` so HTTP
    /// handlers can read live runtime state without holding the `runtimes` lock.
    /// `None` in tests that do not exercise the live-state authority path.
    state_watcher: Option<watch::Sender<ConvState>>,
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: ConvContext,
        state: ConvState,
        storage: S,
        llm_client: L,
        tool_executor: T,
        browser_sessions: Arc<BrowserSessionManager>,
        bash_handles: Arc<crate::tools::BashHandleRegistry>,
        tmux_registry: Arc<crate::tools::TmuxRegistry>,
        llm_registry: Arc<ModelRegistry>,
        terminals: crate::terminal::ActiveTerminals,
        event_rx: mpsc::Receiver<Event>,
        event_tx: mpsc::Sender<Event>,
        broadcast_tx: SseBroadcaster,
    ) -> Self {
        // Generation-tagged, per-effect outcome channels. Background tasks send
        // typed outcomes (LlmOutcome, ToolExecOutcome, retry fires) through
        // oneshot channels; forwarders stamp the dispatch generation and relay
        // them here, where the select loop discards superseded outcomes before
        // routing live ones into `process_outcome`.
        let (llm_outcome_tx, llm_outcome_rx) = mpsc::channel::<(u64, LlmOutcome)>(64);
        let (tool_outcome_tx, tool_outcome_rx) = mpsc::channel::<(u64, ToolExecOutcome)>(64);
        let (retry_outcome_tx, retry_outcome_rx) = mpsc::channel::<(u64, u32)>(64);

        let tool_executor = Arc::new(tool_executor);
        let clearable_names = Arc::new(tool_executor.clearable_tool_names());

        Self {
            context,
            state,
            state_updated_at: Utc::now(),
            storage,
            llm_client: Arc::new(llm_client),
            tool_executor,
            clearable_names,
            clear_watermark_cache: Arc::new(std::sync::Mutex::new(None)),
            browser_sessions,
            bash_handles,
            tmux_registry,
            llm_registry,
            terminals,
            event_rx,
            event_tx,
            broadcast_tx,
            tool_cancel_token: None,
            llm_task_handle: None,
            retry_timer_handle: None,
            retry_generation: 0,
            llm_request_generation: 0,
            llm_outcome_tx,
            llm_outcome_rx,
            tool_request_generation: 0,
            tool_outcome_tx,
            tool_outcome_rx,
            retry_outcome_tx,
            retry_outcome_rx,
            parent_event_tx: None,
            spawn_tx: None,
            cancel_tx: None,
            handoff_tx: None,
            sub_agent_result_buffer: Vec::new(),
            sub_agent_round_gen: 0,
            steering_queue: Vec::new(),
            deadline: None,
            tool_task_handle: None,
            active_work_subagents: 0,
            llm_turn_count: 0,
            grace_turn_granted: false,
            parent_tool_cycle_count: 0,
            parent_tool_cycle_cap: parent_tool_cycle_cap_from_env(),
            credential_helper: None,
            agent_catalog: Arc::from(Vec::new()),
            fork_cmd_tx: None,
            state_watcher: None,
        }
    }

    /// Provide the fork-resolution consumer sender used to retire still-pending
    /// fork proposals when this conversation reaches a terminal state
    /// (REQ-PROJ-035). Set by the runtime manager for fork-proposing parent
    /// conversations.
    pub fn with_fork_command_sender(
        mut self,
        tx: mpsc::Sender<super::fork_resolve::ForkCommand>,
    ) -> Self {
        self.fork_cmd_tx = Some(tx);
        self
    }

    /// Attach a watch sender so the runtime publishes its current `ConvState`
    /// on every transition. The paired receiver lives in `ConversationHandle`
    /// and lets HTTP handlers read live state without consulting the DB.
    pub fn with_state_watcher(mut self, tx: watch::Sender<ConvState>) -> Self {
        self.state_watcher = Some(tx);
        self
    }

    /// Set the credential helper for recovery settlement (REQ-BED-030).
    pub fn with_credential_helper(
        mut self,
        helper: Option<Arc<phoenix_llm::CredentialHelper>>,
    ) -> Self {
        self.credential_helper = helper;
        self
    }

    /// Override the parent tool-use cycle cap. Test-only: production code
    /// relies on the env-var default set in [`Self::new`].
    #[cfg(test)]
    pub fn with_parent_tool_cycle_cap(mut self, cap: u32) -> Self {
        self.parent_tool_cycle_cap = cap;
        self
    }

    /// Override the initial `state_updated_at` from the loaded DB row so
    /// the very first `SseEvent::StateChange` after resume carries the
    /// real entry timestamp rather than the runtime-construction time.
    /// New (never-persisted) conversations don't call this — the
    /// constructor default (`Utc::now()`) is correct for them.
    pub fn with_state_updated_at(mut self, ts: DateTime<Utc>) -> Self {
        self.state_updated_at = ts;
        self
    }

    /// Set the parent event channel (for sub-agents)
    pub fn with_parent(mut self, parent_tx: mpsc::Sender<Event>) -> Self {
        self.parent_event_tx = Some(parent_tx);
        self
    }

    /// Freeze the named-agent catalog used to render the `spawn_agents` schema
    /// and resolve `agent_type` at spawn time. Set by the runtime manager for
    /// parent conversations so both surfaces share one catalog (REQ-AG-008).
    pub fn with_agent_catalog(mut self, catalog: Arc<[phoenix_agents::AgentDefinition]>) -> Self {
        self.agent_catalog = catalog;
        self
    }

    /// Set the spawn/cancel channels (for parent conversations)
    pub fn with_spawn_channels(
        mut self,
        spawn_tx: mpsc::Sender<SubAgentSpawnRequest>,
        cancel_tx: mpsc::Sender<SubAgentCancelRequest>,
    ) -> Self {
        self.spawn_tx = Some(spawn_tx);
        self.cancel_tx = Some(cancel_tx);
        self
    }

    pub fn with_task_handoff_channel(
        mut self,
        handoff_tx: mpsc::Sender<TaskApprovalHandoffRequest>,
    ) -> Self {
        self.handoff_tx = Some(handoff_tx);
        self
    }

    /// Initialise the steering queue from a previously-persisted snapshot
    /// (loaded from `conversations.steering_queue` at executor startup).
    pub fn with_steering_queue(
        mut self,
        queue: Vec<crate::state_machine::event::SteerEntry>,
    ) -> Self {
        self.steering_queue = queue;
        self
    }

    #[allow(clippy::too_many_lines)] // Sequential event loop; splitting hurts readability
    pub async fn run(mut self) {
        tracing::info!(conv_id = %self.context.conversation_id, "Starting conversation runtime");

        // Check if we need to resume an interrupted operation
        // This handles crash recovery for in-flight LLM requests
        if let ConvState::LlmRequesting { .. } | ConvState::SeededLlmRequesting { .. } = &self.state
        {
            tracing::info!(conv_id = %self.context.conversation_id, "Resuming interrupted LLM request");
            if let Err(e) = self.execute_effect(Effect::RequestLlm).await {
                tracing::error!(error = %e, "Failed to resume LLM request");
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::Error {
                    sequence_id: seq,
                    error: crate::runtime::user_facing_error::UserFacingError::with_action(
                        "resume the LLM request",
                    ),
                });
            }
        }

        // REQ-BED-030: crash recovery for AwaitingRecovery.
        // If the credential helper is still running, the select loop will pick it up.
        // If it already settled, handle it immediately.
        if matches!(self.state, ConvState::AwaitingRecovery { .. }) {
            if let Some(ref helper) = self.credential_helper {
                let status = helper.credential_status().await;
                if !matches!(
                    status,
                    phoenix_llm::credential_helper::CredentialStatus::Running
                ) {
                    self.handle_credential_settlement().await;
                }
            } else {
                // No credential helper available after restart — fall through to error.
                if let Err(e) = self
                    .process_event(Event::CredentialHelperFailed {
                        message: "Credential helper not available after restart".to_string(),
                    })
                    .await
                {
                    tracing::error!(error = %e, "Error handling post-restart credential recovery");
                }
            }
        }

        // Arm the liveness deadline if the runtime starts directly in a waiting
        // state. Production normally resets transient states to Idle on startup,
        // so this only matters when a runtime is constructed straight into a
        // waiting state — but arming here (not only on transition) is what makes
        // the backstop structural rather than dependent on the entry path.
        if let Some(d) = Self::deadline_for(&self.state) {
            self.deadline = Some(tokio::time::Instant::now() + d);
        }

        // Process events and outcomes in a loop - no recursion
        // Input sources:
        //   event_rx          — user events + legacy executor events (continuation, sub-agent results)
        //   llm_outcome_rx    — generation-tagged LLM outcomes (stale ones discarded)
        //   tool_outcome_rx   — generation-tagged tool outcomes (stale ones discarded)
        //   retry_outcome_rx  — generation-tagged retry-timer fires (stale ones discarded)
        //   deadline          — liveness backstop for waiting states (REQ-SA-006, REQ-BED-005a)
        //   recovery          — credential helper settlement (REQ-BED-030)
        loop {
            // Copy deadline before select to avoid borrow conflict
            let deadline = self.deadline;
            let awaiting_recovery = matches!(self.state, ConvState::AwaitingRecovery { .. });

            tokio::select! {
                Some(event) = self.event_rx.recv() => {
                    // Eviction shutdown signal — exit cleanly so the broadcaster
                    // is dropped and connected SSE clients detect the closed
                    // stream and trigger a reconnect to the new runtime.
                    if matches!(event, Event::Shutdown) {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            "Runtime shutdown signal received; exiting executor loop"
                        );
                        return;
                    }
                    if let Err(e) = self.process_event(event).await {
                        // process_event already broadcast a typed
                        // SseEvent::Error at the source if appropriate
                        // (task 24682). No double-broadcast here.
                        tracing::error!(error = %e, "Error handling event");
                    }
                    // FM-5 prevention: terminal states exit the loop explicitly.
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        self.emit_terminal_lifecycle_event().await;
                        return;
                    }
                }
                Some((generation, llm_outcome)) = self.llm_outcome_rx.recv() => {
                    // Generation guard for the LLM request, mirroring the
                    // `RetryTimeout` guard above. A `forward_llm_outcome` send
                    // whose generation has been superseded (by a later dispatch
                    // or an intentional `Effect::AbortLlm`) is stale: its
                    // synthetic NetworkError must not be applied to the current,
                    // unrelated `LlmRequesting` turn. Drop it.
                    if self.llm_outcome_is_stale(generation) {
                        tracing::debug!(
                            outcome_generation = generation,
                            current_generation = self.llm_request_generation,
                            state = self.state.variant_name(),
                            "Ignoring stale LLM outcome — request superseded (abort/new dispatch)"
                        );
                    } else if let Err(e) =
                        self.process_outcome(EffectOutcome::Llm(llm_outcome)).await
                    {
                        tracing::warn!(error = %e, "Outcome rejected by state machine");
                    }
                    // FM-5 prevention: terminal states exit the loop explicitly.
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        self.emit_terminal_lifecycle_event().await;
                        return;
                    }
                }
                Some((generation, tool_outcome)) = self.tool_outcome_rx.recv() => {
                    // Generation guard for the tool task, mirroring the
                    // `llm_outcome_rx` guard above. A `forward_tool_outcome`
                    // send whose generation has been superseded (by a later
                    // dispatch or the forced backstop teardown) is stale: its
                    // outcome must not be applied to a later, unrelated tool
                    // round — even if that round happens to reuse the same
                    // `tool_use_id`. Drop it. The state machine's id/state gate
                    // remains as defense in depth.
                    if self.tool_outcome_is_stale(generation) {
                        tracing::debug!(
                            outcome_generation = generation,
                            current_generation = self.tool_request_generation,
                            state = self.state.variant_name(),
                            "Ignoring stale tool outcome — task superseded (abort/new dispatch)"
                        );
                    } else if let Err(e) =
                        self.process_outcome(EffectOutcome::Tool(tool_outcome)).await
                    {
                        tracing::warn!(error = %e, "Outcome rejected by state machine");
                    }
                    // FM-5 prevention: terminal states exit the loop explicitly.
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        self.emit_terminal_lifecycle_event().await;
                        return;
                    }
                }
                Some((generation, attempt)) = self.retry_outcome_rx.recv() => {
                    // Generation guard for the retry-backoff timer, mirroring the
                    // LLM-outcome guard above. A fire whose generation has been
                    // superseded (a newer `Effect::ScheduleRetry`, or leaving the
                    // retry-scheduling state) is stale and must not double-dispatch
                    // the LLM. Crucially, this is generation-based, not
                    // handle-presence-based: an OLD fire arriving while a NEW timer
                    // is pending is discarded here without disturbing the new
                    // timer, so the live fire is still honored when it arrives.
                    if self.retry_timeout_is_stale(generation) {
                        tracing::debug!(
                            outcome_generation = generation,
                            current_generation = self.retry_generation,
                            state = self.state.variant_name(),
                            "Ignoring stale RetryTimeout — timer superseded (cancel/new dispatch)"
                        );
                    } else if let Err(e) =
                        self.process_outcome(EffectOutcome::RetryTimeout { attempt }).await
                    {
                        tracing::warn!(error = %e, "Outcome rejected by state machine");
                    }
                    // FM-5 prevention: terminal states exit the loop explicitly.
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        self.emit_terminal_lifecycle_event().await;
                        return;
                    }
                }
                // Unified liveness backstop (REQ-SA-006, REQ-BED-005a): one arm
                // for every waiting state's deadline. `handle_deadline_expiry`
                // dispatches on the current state to the right teardown.
                () = async {
                    match deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending::<()>().await,
                    }
                }, if deadline.is_some() => {
                    self.handle_deadline_expiry().await;
                    // FM-5 prevention: terminal states exit the loop explicitly.
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        self.emit_terminal_lifecycle_event().await;
                        return;
                    }
                }
                // REQ-BED-030: credential helper settled while awaiting recovery
                () = async {
                    match &self.credential_helper {
                        Some(helper) => helper.wait_for_settlement().await,
                        None => std::future::pending::<()>().await,
                    }
                }, if awaiting_recovery && self.credential_helper.is_some() => {
                    self.handle_credential_settlement().await;
                    if let StepResult::Terminal(outcome) = self.state.step_result() {
                        tracing::info!(
                            conv_id = %self.context.conversation_id,
                            ?outcome,
                            "Conversation reached terminal state, exiting executor loop"
                        );
                        self.emit_terminal_lifecycle_event().await;
                        return;
                    }
                }
                else => break,
            }
        }

        tracing::info!(conv_id = %self.context.conversation_id, "Conversation runtime stopped");
    }

    /// REQ-BED-030: credential helper settled while in `AwaitingRecovery`.
    /// Check the helper's new status and inject the appropriate event.
    async fn handle_credential_settlement(&mut self) {
        let Some(ref helper) = self.credential_helper else {
            return;
        };
        let status = helper.credential_status().await;
        let event = if status == phoenix_llm::credential_helper::CredentialStatus::Valid {
            tracing::info!("Credential helper succeeded, retrying LLM request");
            Event::CredentialBecameAvailable
        } else {
            tracing::info!(
                ?status,
                "Credential helper settled without valid credential"
            );
            Event::CredentialHelperFailed {
                message: "Authentication failed — click Retry to try again".to_string(),
            }
        };
        if let Err(e) = self.process_event(event).await {
            tracing::error!(error = %e, "Error handling credential settlement event");
        }
    }

    /// Broadcast `ConversationBecameTerminal` to all SSE subscribers, clean up
    /// any lingering worktree, and retire this conversation's still-pending fork
    /// proposals (REQ-PROJ-035).
    ///
    /// Send errors (no active receivers) are intentionally ignored.
    async fn emit_terminal_lifecycle_event(&self) {
        self.cleanup_worktree_if_present();
        self.retire_fork_proposals_on_terminal().await;
        let _ = self
            .broadcast_tx
            .send_seq(|seq| SseEvent::ConversationBecameTerminal { sequence_id: seq });
    }

    /// `ForkProposalsRetiredOnOriginTerminal` (REQ-PROJ-035): when this origin
    /// conversation becomes terminal, dismiss its still-pending fork proposals
    /// and clean any deterministic spawn/promote orphan a crashed approve/promote
    /// left behind. Enqueues a `RetireForOrigin` command on the single serialized
    /// fork-resolution consumer and awaits its best-effort completion. No-op when
    /// this runtime has no fork-resolution sender (sub-agents).
    async fn retire_fork_proposals_on_terminal(&self) {
        let Some(tx) = self.fork_cmd_tx.as_ref() else {
            return;
        };
        let (reply, reply_rx) = oneshot::channel();
        if tx
            .send(super::fork_resolve::ForkCommand::RetireForOrigin {
                origin_id: self.context.conversation_id.clone(),
                reply,
            })
            .await
            .is_err()
        {
            tracing::warn!(
                conv_id = %self.context.conversation_id,
                "fork retirement on terminal: consumer gone; skipped"
            );
            return;
        }
        let _ = reply_rx.await;
    }

    /// Remove the conversation's worktree if it still exists on disk.
    ///
    /// REQ-PROJ-028 creates worktrees at first message. If the user never
    /// approved a task (Explore mode), the worktree and temp branch leak.
    /// Work/Branch conversations clean up via mark-merged/abandon before
    /// reaching `ConvState::Terminal`, so this is a no-op for them in the
    /// normal case.
    ///
    /// REQ-BED-031 / approved-task handoff: `ContextExhausted` and
    /// `HandedOff` are terminal states whose worktree must be preserved for a
    /// successor conversation. Skip cleanup in those states; reconcile /
    /// abandon / mark-merged are the only paths permitted to remove the worktree.
    fn cleanup_worktree_if_present(&self) {
        if matches!(
            self.state,
            ConvState::ContextExhausted { .. } | ConvState::HandedOff { .. }
        ) {
            tracing::debug!(
                conv_id = %self.context.conversation_id,
                "skipping terminal worktree cleanup for successor-owned worktree"
            );
            return;
        }

        // Direct mode and legacy Managed have working_dir == repo_root, so fall
        // back to working_dir when the strict Phoenix-worktree predicate fails.
        let wd = &self.context.working_dir;
        let repo_root =
            crate::git_ops::repo_root_from_phoenix_worktree(wd).unwrap_or_else(|| wd.clone());

        let worktree_path = repo_root
            .join(".phoenix")
            .join("worktrees")
            .join(&self.context.conversation_id);

        if worktree_path.exists() {
            let worktree_str = worktree_path.to_string_lossy().to_string();
            tracing::info!(worktree = %worktree_str, "Cleaning up worktree on terminal");
            let _ = crate::git_ops::run_git(
                &repo_root,
                &["worktree", "remove", &worktree_str, "--force"],
            );
        }
    }

    /// Process a typed effect outcome from a background task.
    ///
    /// Routes through `handle_outcome()` (pure SM function). Invalid outcomes
    /// are logged and discarded — state unchanged.
    /// Whether a forwarded LLM outcome stamped with `generation` belongs to a
    /// superseded request. An outcome is stale when its generation no longer
    /// matches the current in-flight generation — either a later
    /// `Effect::RequestLlm` opened a newer generation, or an intentional
    /// `Effect::AbortLlm` bumped it. Stale outcomes (including the synthetic
    /// `NetworkError` produced when an aborted task drops its sender) are
    /// discarded so they cannot misfire on a subsequent unrelated turn.
    fn llm_outcome_is_stale(&self, generation: u64) -> bool {
        generation != self.llm_request_generation
    }

    /// Whether a forwarded tool outcome stamped with `generation` belongs to a
    /// superseded tool task. Stale when its generation no longer matches the
    /// current tool generation — a later tool dispatch opened a newer one, or the
    /// `CancellingTool` backstop's forced teardown bumped it. Closes the
    /// same-id-reuse race the state machine's id/state gate alone cannot (a stale
    /// outcome whose `tool_use_id` the next round happens to reuse). See
    /// `tool_request_generation`.
    fn tool_outcome_is_stale(&self, generation: u64) -> bool {
        generation != self.tool_request_generation
    }

    /// Whether a forwarded `RetryTimeout` stamped with `generation` belongs to a
    /// superseded retry timer. Stale when its generation no longer matches the
    /// current retry generation — a later `Effect::ScheduleRetry` opened a newer
    /// one, or leaving the retry-scheduling state (cancel / response / new turn /
    /// exhaustion) bumped it. The live timer's own current-generation fire still
    /// matches and is honored; only genuinely superseded fires are discarded.
    fn retry_timeout_is_stale(&self, generation: u64) -> bool {
        generation != self.retry_generation
    }

    async fn process_outcome(&mut self, outcome: EffectOutcome) -> Result<(), String> {
        // A `RetryTimeout` reaching this point has already passed the
        // generation guard in the select loop (`retry_timeout_is_stale`), so it
        // is the live timer firing. Its task has run to completion, leaving the
        // stored handle spent — clear it to keep "handle is Some only while a
        // timer task is in flight". (The subsequent `RequestLlm` keeps the
        // state in `LlmRequesting`, so the leave-the-scheduling-state abort in
        // `apply_transition_result` won't clear it for us.) Correctness comes
        // from the generation match, not from this handle bookkeeping.
        if matches!(outcome, EffectOutcome::RetryTimeout { .. }) {
            self.retry_timer_handle = None;
        }

        let result = match handle_outcome(&self.state, &self.context, outcome) {
            Ok(r) => r,
            Err(invalid) => {
                tracing::warn!(
                    reason = %invalid.reason,
                    state = self.state.variant_name(),
                    "Rejected invalid outcome — state unchanged"
                );
                return Err(invalid.reason);
            }
        };

        // Apply transition result and process any generated events
        let mut events_to_process = self.apply_transition_result(result).await?;

        // Process chained events (e.g., SpawnAgentsComplete from execute_effect)
        while let Some(event) = events_to_process.pop() {
            let chained_result = match transition(&self.state, &self.context, event) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "Chained event from outcome rejected");
                    continue;
                }
            };
            let more_events = self.apply_transition_result(chained_result).await?;
            events_to_process.extend(more_events);
        }

        Ok(())
    }

    async fn process_event(&mut self, event: Event) -> Result<(), String> {
        // A fresh user turn always resets the parent tool-cycle counter
        // (task 24680). Cap logic lives in the `Effect::RequestLlm` handler.
        if matches!(event, Event::UserMessage { .. }) {
            self.parent_tool_cycle_count = 0;
        }

        // Steering messages are buffered rather than fed to the state machine.
        // They are delivered as `UserMessage` when the conversation next enters `Idle`.
        if let Event::SteerMessage {
            text,
            llm_text,
            images,
            files,
            message_id,
            user_agent,
            skill_invocation,
        } = event
        {
            let entry = crate::state_machine::event::SteerEntry {
                text,
                llm_text,
                images,
                files,
                message_id: message_id.clone(),
                user_agent,
                skill_invocation,
            };
            self.steering_queue.push(entry);
            let queue_position = self.steering_queue.len() - 1;
            // Persist updated queue
            if let Err(e) = self
                .storage
                .update_steering_queue(&self.context.conversation_id, &self.steering_queue)
                .await
            {
                tracing::warn!(error = %e, "Failed to persist steering queue");
            }
            // Notify the UI so it can show the queued indicator
            let _ = self
                .broadcast_tx
                .send_seq(|seq| SseEvent::SteerMessageQueued {
                    sequence_id: seq,
                    message_id,
                    queue_position,
                });
            return Ok(());
        }

        // Cancel steering: remove entry from in-memory queue.
        // DB is already updated by the cancel handler before this event arrives.
        if let Event::CancelSteerMessage { message_id } = event {
            let before = self.steering_queue.len();
            self.steering_queue.retain(|e| e.message_id != message_id);
            tracing::info!(
                conv_id = %self.context.conversation_id,
                %message_id,
                removed = before - self.steering_queue.len(),
                "Steering message cancelled in executor"
            );
            return Ok(());
        }

        // Check if this is a SubAgentResult that needs buffering
        if let Event::SubAgentResult { .. } = &event {
            if !self.can_handle_sub_agent_result() {
                tracing::debug!("Buffering SubAgentResult, parent not in AwaitingSubAgents");
                self.sub_agent_result_buffer.push(event);
                return Ok(());
            }
        }

        // We need to process events in a loop to handle chained effects
        let mut events_to_process = vec![event];

        while let Some(current_event) = events_to_process.pop() {
            // Decrement one-writer counter when a Work sub-agent completes (REQ-PROJ-008)
            if let Event::SubAgentResult { ref agent_id, .. } = current_event {
                if let ConvState::AwaitingSubAgents { ref pending, .. }
                | ConvState::CancellingSubAgents { ref pending, .. } = self.state
                {
                    if let Some(agent) = pending.iter().find(|p| p.agent_id == *agent_id) {
                        if agent.mode == SubAgentMode::Work {
                            self.active_work_subagents =
                                self.active_work_subagents.saturating_sub(1);
                        }
                    }
                }
            }

            // Pure state transition
            let result = match transition(&self.state, &self.context, current_event) {
                Ok(r) => r,
                Err(e) => {
                    // Task 24682: surface a humanised, kind-aware error
                    // payload via SSE, never the raw `Debug` formatting.
                    // The full `TransitionError` is logged separately so
                    // operators can still diagnose it.
                    tracing::warn!(
                        error = %e,
                        state = self.state.variant_name(),
                        "Transition rejected"
                    );
                    let _ = self.broadcast_tx.send_seq(|seq| SseEvent::Error {
                        sequence_id: seq,
                        error: crate::runtime::user_facing_error::from_transition_error(&e),
                    });
                    return Err(e.to_string());
                }
            };

            let generated_events = self.apply_transition_result(result).await?;
            events_to_process.extend(generated_events);
        }

        Ok(())
    }

    /// Drain `sub_agent_result_buffer` for the round the parent is entering,
    /// returning the buffered results that belong to it. Called on entry to
    /// `AwaitingSubAgents`/`CancellingSubAgents`.
    ///
    /// A buffered `SubAgentResult` is valid for THIS fan-in round only if its
    /// agent belongs to the round being entered. `agent_id` is a fresh UUID per
    /// spawned agent, so each agent belongs to exactly one round; round
    /// membership is therefore exactly the entering round's `pending` set.
    ///
    /// Two cases must be distinguished (Finding #3):
    ///   - EARLY result for THIS round: an agent spawned in this round finished
    ///     before the parent entered `AwaitingSubAgents` (e.g. tool sequence
    ///     `[spawn_agents A, bash, spawn_agents B]`, where A's result arrives
    ///     during `bash`). Its agent is in `pending` → drain it.
    ///   - STALE result from a COMPLETED round: a timed-out/cancelled agent from
    ///     a prior round reports its real result *after* the parent left
    ///     `AwaitingSubAgents`. Its agent is NOT in this round's `pending` →
    ///     discard it. Feeding it to the reducer would either misapply it to the
    ///     wrong fan-in round or get rejected and abort the transition.
    ///
    /// `sub_agent_round_gen` tags the entering round for observability in the
    /// discard log; correctness comes from the pending-set membership test.
    fn drain_buffer_for_round(&mut self) -> Vec<Event> {
        self.sub_agent_round_gen = self.sub_agent_round_gen.wrapping_add(1);
        let round_gen = self.sub_agent_round_gen;
        let round_pending: std::collections::HashSet<&str> = match &self.state {
            ConvState::AwaitingSubAgents { pending, .. }
            | ConvState::CancellingSubAgents { pending, .. } => {
                pending.iter().map(|p| p.agent_id.as_str()).collect()
            }
            _ => std::collections::HashSet::new(),
        };
        let buffered = std::mem::take(&mut self.sub_agent_result_buffer);
        let mut to_drain = Vec::with_capacity(buffered.len());
        let mut discarded = 0usize;
        for event in buffered {
            let belongs = match &event {
                Event::SubAgentResult { agent_id, .. } => round_pending.contains(agent_id.as_str()),
                // Only SubAgentResult is ever buffered (see the push path);
                // anything else is unexpected — keep it rather than silently
                // drop, so a future buffered event type can't vanish.
                _ => true,
            };
            if belongs {
                to_drain.push(event);
            } else {
                discarded += 1;
                if let Event::SubAgentResult { agent_id, .. } = &event {
                    tracing::debug!(
                        round_gen,
                        agent_id = %agent_id,
                        "Discarding buffered SubAgentResult from a completed round — \
                         agent not pending in the entering round"
                    );
                }
            }
        }
        if !to_drain.is_empty() || discarded > 0 {
            tracing::debug!(
                round_gen,
                drained = to_drain.len(),
                discarded,
                "Drained buffered SubAgentResults on entering AwaitingSubAgents"
            );
        }
        to_drain
    }

    /// Apply a `TransitionResult` from either `transition()` or `handle_outcome()`.
    ///
    /// Updates state, drains sub-agent buffer if entering `AwaitingSubAgents`,
    /// dispatches effects. Returns any synchronously generated events
    /// (e.g., from `SpawnAgentsComplete`).
    async fn apply_transition_result(
        &mut self,
        result: crate::state_machine::transition::TransitionResult,
    ) -> Result<Vec<Event>, String> {
        let mut generated_events = Vec::new();

        // Update state. Bump the entry timestamp on phase change so every
        // SseEvent::StateChange the executor subsequently emits carries a
        // fresh, server-authoritative state_updated_at
        // (specs/working-phase-visibility/ REQ-WPV-001). `persist_state_effect`
        // threads this same value into the DB write, so the persisted row and
        // the SSE value match exactly.
        let old_state = std::mem::replace(&mut self.state, result.new_state.clone());
        // Only stamp a fresh entry time when the phase actually changes.
        // Several events absorb as no-ops (Terminal absorbs unknown events;
        // an empty steering drain re-enters the same state) and reach here
        // with new_state == old_state; bumping then would reset the client's
        // elapsed counter (REQ-WPV-001) for a phase the agent never left.
        // No-op transitions emit no PersistState effect, so the DB row keeps
        // its prior value too — gating here keeps the in-memory stamp in sync.
        if self.state != old_state {
            self.state_updated_at = Utc::now();
            // Publish early for most transitions so effective_conversation_state
            // reflects the new state before long-running effects (e.g. ApproveTask
            // git work) complete. Exception: suppress when entering Idle with
            // queued steering messages — the inline drain will immediately advance
            // the state to LlmRequesting, and exposing the transient Idle would let
            // a concurrent POST /chat route a UserMessage before the drain finishes
            // (FM-7 intermediate-Idle race). The end-of-function publish below
            // covers that suppressed case once the drain completes.
            let will_drain_from_idle = matches!(self.state, ConvState::Idle)
                && !self.steering_queue.is_empty()
                && !self.context.is_sub_agent;
            if !will_drain_from_idle {
                if let Some(tx) = &self.state_watcher {
                    let _ = tx.send(self.state.clone());
                }
            }
        }

        // Retire any pending retry-backoff timer when the conversation leaves
        // the retry-scheduling context. A retry timer is only valid while the
        // state stays `LlmRequesting`/`AwaitingContinuation` (the only states
        // that emit `Effect::ScheduleRetry`, and where the next legitimate
        // `RetryTimeout` is consumed — including same-state steer drains and
        // attempt increments). Any other destination — cancel (→ Idle /
        // CancellingTool), a response (→ ToolExecuting / Completed), or retry
        // exhaustion (→ Error) — ends this retry context.
        //
        // Bumping `retry_generation` is the identity guard the pure reducer
        // cannot provide: attempt numbers reset per turn, so a stale timer from
        // a cancelled-then-resent turn would otherwise pass the reducer's
        // `attempt == retry_attempt` check and fire a second concurrent
        // `RequestLlm` (double token cost; a duplicate response can overwrite
        // the real one). After the bump, any in-flight or already-queued fire
        // from the old timer is stale and the select loop drops it. Aborting
        // the handle is a best-effort optimization to stop the timer task
        // early; correctness does not depend on it.
        if !matches!(
            self.state,
            ConvState::LlmRequesting { .. } | ConvState::AwaitingContinuation { .. }
        ) {
            if let Some(handle) = self.retry_timer_handle.take() {
                handle.abort();
                self.retry_generation = self.retry_generation.wrapping_add(1);
            }
        }

        // Log notable state transitions at INFO. "Notable" means transitions that cross
        // a meaningful phase boundary (idle↔active, entering/leaving tool execution,
        // terminal states) are logged at DEBUG to keep steady-state noise low.
        // Variant names come from `ConvState::variant_name` so the set of
        // names is maintained in exactly one place.
        {
            let from = old_state.variant_name();
            let to = self.state.variant_name();
            if from != to {
                let notable = matches!(
                    &self.state,
                    ConvState::Idle
                        | ConvState::ToolExecuting { .. }
                        | ConvState::AwaitingSubAgents { .. }
                        | ConvState::Completed { .. }
                        | ConvState::Failed { .. }
                        | ConvState::Error { .. }
                        | ConvState::ContextExhausted { .. }
                        | ConvState::HandedOff { .. }
                        | ConvState::AwaitingTaskApproval { .. }
                        | ConvState::AwaitingUserResponse { .. }
                        | ConvState::Terminal
                );
                if notable {
                    tracing::info!(
                        conv_id = %self.context.conversation_id,
                        from,
                        to,
                        "State transition"
                    );
                } else {
                    tracing::debug!(
                        conv_id = %self.context.conversation_id,
                        from,
                        to,
                        "State transition"
                    );
                }
            }
        }

        let entering_awaiting = !matches!(
            old_state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        ) && matches!(
            self.state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        );

        // Drain buffer when entering AwaitingSubAgents.
        if entering_awaiting {
            generated_events.extend(self.drain_buffer_for_round());
        }

        // Manage the unified liveness deadline for every waiting state.
        self.manage_deadline(&old_state);

        // Steering-queue drain: process synchronously so persist effects land
        // BEFORE any RequestLlm in the current effect list. This eliminates a
        // race where a mid-turn drain would persist steers concurrently with
        // (or after) the LLM task reading the DB; if the in-flight LLM then
        // returned a no-tool response, the conversation would settle to Idle
        // with the steers persisted but unanswered.
        // An error dismissal (DismissError) enters Idle but is the user
        // clearing a banner, not a turn completing, so it must not drain the
        // steering queue. It is identified by the hidden marker it persists —
        // keyed on that effect, not on the source state, so the exclusion stays
        // correct if another `Error -> Idle` edge (that *should* drain) is ever
        // added. Mirrors specs/steering-messages DrainOnIdleEntry's guard.
        let is_error_dismissal = result.effects.iter().any(|e| {
            matches!(
                e,
                Effect::PersistHiddenSystemMarker { marker, .. }
                    if *marker == crate::state_machine::transition::ERROR_DISMISSED_MARKER
            )
        });
        if let Some(drain_event) = self.maybe_drain_steering_queue(&old_state, is_error_dismissal) {
            self.run_effects_with_inline_drain(result.effects, drain_event, &mut generated_events)
                .await?;
        } else {
            for effect in result.effects {
                if let Some(gen_event) = self.execute_effect(effect).await? {
                    generated_events.push(gen_event);
                }
            }
        }

        // Publish the final state to any live-state observer after all effects
        // (including any inline steering drain) have completed. Publishing here
        // rather than at the state-set site above prevents the intermediate
        // `Idle` from being visible to `effective_conversation_state` during
        // the await inside `run_effects_with_inline_drain`, which would cause a
        // concurrent `POST /chat` to route a `UserMessage` before the drain has
        // advanced the state back to `LlmRequesting` (FM-7).
        if let Some(tx) = &self.state_watcher {
            let _ = tx.send(self.state.clone());
        }

        Ok(generated_events)
    }

    /// Defer any `RequestLlm` in `original_effects`, run the rest, then process
    /// the drain event's persist effects inline, then run the deferred
    /// `RequestLlm`. Guarantees the spawned LLM task reads a DB that already
    /// contains the steered messages.
    async fn run_effects_with_inline_drain(
        &mut self,
        original_effects: Vec<Effect>,
        drain_event: Event,
        generated_events: &mut Vec<Event>,
    ) -> Result<(), String> {
        let mut deferred_request_llm: Option<Effect> = None;
        // Cosmetic (task 60004): when the inline drain enters from Idle, the
        // original transition's Idle state-change SSE would briefly render the
        // conversation as Idle before the drain's Idle->LlmRequesting notify.
        // Persist the Idle state to the DB but suppress its broadcast (and any
        // explicit state_change notify); the drain emits the authoritative
        // LlmRequesting state-change. Mid-turn drains enter from LlmRequesting,
        // not Idle, so their state-change is correct and not suppressed.
        let suppress_intermediate_state_change = matches!(self.state, ConvState::Idle);
        for effect in original_effects {
            if matches!(effect, Effect::RequestLlm) {
                deferred_request_llm = Some(effect);
                continue;
            }
            if suppress_intermediate_state_change {
                match &effect {
                    Effect::PersistState => {
                        self.persist_state_effect(false).await?;
                        continue;
                    }
                    Effect::NotifyStateChange => {
                        continue;
                    }
                    _ => {}
                }
            }
            if let Some(gen_event) = self.execute_effect(effect).await? {
                generated_events.push(gen_event);
            }
        }

        let Event::SteerDrainedUserMessages { entries } = drain_event else {
            unreachable!("maybe_drain_steering_queue returns only SteerDrainedUserMessages")
        };
        let drain_result = transition(
            &self.state,
            &self.context,
            Event::SteerDrainedUserMessages { entries },
        )
        .map_err(|e| format!("steering drain transition failed: {e:?}"))?;
        let drain_old_state = std::mem::replace(&mut self.state, drain_result.new_state);
        // Same gating as apply_transition_result: only stamp a fresh entry
        // time when the drain actually changes phase. A mid-turn drain
        // re-enters the same LlmRequesting state and emits only PersistState
        // (no StateChange) — bumping then would advance the persisted
        // state_updated_at with no matching SSE, so a later reconnect would
        // read a DB timestamp newer than any StateChange the client saw and
        // jump the elapsed counter (REQ-WPV-001).
        if self.state != drain_old_state {
            self.state_updated_at = Utc::now();
            // Mirror apply_transition_result: publish the new state so live-state
            // observers (e.g. effective_conversation_state) stay current after a
            // steering drain transition (Idle → LlmRequesting is the common path).
            if let Some(tx) = &self.state_watcher {
                let _ = tx.send(self.state.clone());
            }
        }
        for effect in drain_result.effects {
            if let Some(gen_event) = self.execute_effect(effect).await? {
                generated_events.push(gen_event);
            }
        }

        if let Some(effect) = deferred_request_llm {
            if let Some(gen_event) = self.execute_effect(effect).await? {
                generated_events.push(gen_event);
            }
        }
        Ok(())
    }

    /// Persist the current state to the DB. When `broadcast` is true, also
    /// emit the `StateChange` SSE; the inline-drain path persists with
    /// `broadcast = false` to suppress an intermediate Idle flicker (task
    /// 60004) since the drain emits its own authoritative state-change.
    async fn persist_state_effect(&mut self, broadcast: bool) -> Result<Option<Event>, String> {
        self.storage
            .update_state(
                &self.context.conversation_id,
                &self.state,
                self.state_updated_at,
            )
            .await?;

        if broadcast {
            let _ = self.broadcast_tx.send_seq(|seq| SseEvent::StateChange {
                sequence_id: seq,
                state: self.state.clone(),
                presentation_mode: self.state.presentation_mode().to_string(),
                state_updated_at: self.state_updated_at,
            });
        }
        Ok(None)
    }

    /// If the current state transition is a steering-queue drain hook point and
    /// the queue is non-empty, drain all entries into a single
    /// `SteerDrainedUserMessages` event. The DB queue is NOT touched here; it
    /// is updated later by `Effect::ClearSteeringQueueEntries` once the emitted
    /// event is processed and all `PersistMessage` effects succeed.
    ///
    /// Sub-agents do not have steering queues; this returns `None` for them.
    ///
    /// Hook points (parent conversations only):
    /// - Entering `Idle` from any other state (turn complete; deliver steers
    ///   into the next LLM call).
    /// - Entering `LlmRequesting` from `ToolExecuting`/`AwaitingSubAgents` (mid-
    ///   turn; the prior transition already dispatched `RequestLlm`, so steers
    ///   land in the NEXT LLM call, not the in-flight one).
    fn maybe_drain_steering_queue(
        &mut self,
        old_state: &ConvState,
        is_error_dismissal: bool,
    ) -> Option<Event> {
        if self.context.is_sub_agent {
            return None;
        }

        // An error dismissal enters Idle but is not a turn-end, so it must not
        // drain (see the call site). Draining is reserved for turn-completion
        // idle entries.
        if is_error_dismissal {
            return None;
        }

        let entering_idle =
            !matches!(old_state, ConvState::Idle) && matches!(self.state, ConvState::Idle);
        let entering_llm_requesting_from_tool_round =
            matches!(
                old_state,
                ConvState::ToolExecuting { .. }
                    | ConvState::AwaitingSubAgents { .. }
                    | ConvState::CancellingSubAgents { .. }
            ) && matches!(self.state, ConvState::LlmRequesting { .. });

        if !(entering_idle || entering_llm_requesting_from_tool_round)
            || self.steering_queue.is_empty()
        {
            return None;
        }

        let entries = std::mem::take(&mut self.steering_queue);
        tracing::debug!(
            count = entries.len(),
            entering_idle,
            mid_turn = entering_llm_requesting_from_tool_round,
            "Draining all queued steering messages"
        );
        // DB queue is updated by `Effect::ClearSteeringQueueEntries` AFTER
        // persist effects run, so a crash mid-drain leaves the queue intact
        // for idempotent re-drain on restart.
        Some(Event::SteerDrainedUserMessages { entries })
    }

    /// Check if the current state can handle `SubAgentResult` events
    fn can_handle_sub_agent_result(&self) -> bool {
        matches!(
            self.state,
            ConvState::AwaitingSubAgents { .. } | ConvState::CancellingSubAgents { .. }
        )
    }

    /// Handle the `AwaitingSubAgents` completion timeout (REQ-SA-006) by routing
    /// it through the cancellation protocol rather than racing it.
    ///
    /// Dispatched by `handle_deadline_expiry` when the liveness deadline fires in
    /// `AwaitingSubAgents`; the deadline is already cleared by the dispatcher.
    ///
    /// Injecting a single `UserCancel { cause: Timeout }` drives the
    /// `AwaitingSubAgents + UserCancel -> CancellingSubAgents` transition, which
    /// emits `Effect::CancelSubAgents` (the real cancel) and stamps
    /// `cause: Timeout` on the state. Real cancelled results then drain through
    /// `CancellingSubAgents` — each sub-agent terminates and reports within its
    /// own 3s `CancellingTool` backstop — and the drain arm labels non-success
    /// outcomes `TimedOut` per the state's cause. The 6s `CancellingSubAgents`
    /// last-resort backstop (`handle_cancelling_sub_agents_timeout`) presumes any
    /// still-pending agent dead after that. This handler no longer sends a direct
    /// cancel or fabricates per-agent results; the transition owns the cancel.
    async fn handle_sub_agent_timeout(&mut self) {
        let pending_count = if let ConvState::AwaitingSubAgents { pending, .. } = &self.state {
            pending.len()
        } else {
            // Deadline fired but state already moved on — nothing to do.
            return;
        };

        tracing::warn!(
            count = pending_count,
            "Sub-agent completion timeout reached, cancelling pending agents via cancel protocol"
        );

        let event = Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::Timeout,
        };
        if let Err(e) = self.process_event(event).await {
            tracing::warn!(error = %e, "Failed to process sub-agent timeout cancel");
        }
    }

    /// The liveness-deadline duration for a state, or `None` if the state has no
    /// deadline. This is the single place that maps a waiting state to its
    /// backstop duration — `AwaitingSubAgents` gets the 20-minute completion
    /// timeout (REQ-SA-006); `CancellingTool` gets the 3-second forced-teardown
    /// backstop (REQ-BED-005a); `CancellingSubAgents` gets a 6-second last-resort
    /// backstop (2× the 3s sub-agent `CancellingTool` deadline) so real cancelled
    /// results win the drain before the parent presumes a vanished runtime dead.
    fn deadline_for(state: &ConvState) -> Option<Duration> {
        match state {
            ConvState::AwaitingSubAgents { .. } => Some(DEFAULT_SUBAGENT_TIMEOUT),
            ConvState::CancellingTool { .. } => Some(CANCELLATION_DEADLINE),
            ConvState::CancellingSubAgents { .. } => Some(CANCELLING_SUBAGENTS_DEADLINE),
            _ => None,
        }
    }

    /// Manage the unified liveness deadline on every transition. Arm a fresh
    /// deadline whenever the conversation changes into a (different) waiting
    /// state, using that state's `deadline_for` duration; clear it when the new
    /// state has no deadline.
    ///
    /// Keying re-arming on a *variant change* preserves the exact semantics of
    /// the prior two-field design: an `AwaitingSubAgents → AwaitingSubAgents`
    /// self-transition (a sub-agent resolving while others remain) keeps the
    /// original 20-minute window rather than restarting it, and a
    /// `CancellingSubAgents → CancellingSubAgents` drain keeps its 6-second
    /// window. An `AwaitingSubAgents → CancellingSubAgents` cancel re-arms to the
    /// 6-second backstop — the variant changed, so the cancellation window
    /// replaces the long completion window.
    fn manage_deadline(&mut self, old_state: &ConvState) {
        // Leaving CancellingTool ends the tool round; drop the retained handle so
        // a later backstop can never abort an unrelated task.
        if matches!(old_state, ConvState::CancellingTool { .. })
            && !matches!(self.state, ConvState::CancellingTool { .. })
        {
            self.tool_task_handle = None;
        }

        let variant_changed =
            std::mem::discriminant(old_state) != std::mem::discriminant(&self.state);
        match Self::deadline_for(&self.state) {
            Some(duration) if variant_changed => {
                self.deadline = Some(tokio::time::Instant::now() + duration);
                tracing::debug!(
                    state = self.state.variant_name(),
                    deadline_secs = duration.as_secs(),
                    "Liveness deadline armed"
                );
            }
            Some(_) => {
                // Same waiting variant — keep the in-flight deadline running.
            }
            None => {
                self.deadline = None;
            }
        }
    }

    /// Single entry point from the select-loop deadline arm. Clears the deadline,
    /// then dispatches on the current state to the appropriate teardown. A
    /// deadline that fires after the conversation has already moved on to a state
    /// with no backstop is a no-op (the per-state handlers also guard this).
    async fn handle_deadline_expiry(&mut self) {
        self.deadline = None;
        match &self.state {
            ConvState::AwaitingSubAgents { .. } => self.handle_sub_agent_timeout().await,
            ConvState::CancellingTool { .. } => self.handle_cancelling_tool_timeout().await,
            ConvState::CancellingSubAgents { .. } => {
                self.handle_cancelling_sub_agents_timeout().await;
            }
            other => {
                tracing::debug!(
                    state = other.variant_name(),
                    "Liveness deadline fired but state has no backstop — ignoring"
                );
            }
        }
    }

    /// Handle the 6-second last-resort backstop firing in `CancellingSubAgents`
    /// (REQ-BED-005a `CancellingSubAgentsDeadlineFires`). By the time this fires,
    /// real cancelled results have had 3s+ (the sub-agent's own `CancellingTool`
    /// backstop) to arrive, so any still-pending sub-agent is presumed gone —
    /// its runtime has genuinely vanished. Inject a generic `Failure { Cancelled
    /// }` per still-pending agent; the `CancellingSubAgents + SubAgentResult`
    /// drain arm maps that by the state's recorded `cause` (a timeout-caused
    /// teardown renders `TimedOut`, a user cancel renders `Failure { Cancelled
    /// }`), so this handler does not hardcode the label. Releasing the one-writer
    /// reservation as each result drains is safe because the agent is presumed
    /// dead. The last pending result resolves `CancellingSubAgents -> Idle`.
    async fn handle_cancelling_sub_agents_timeout(&mut self) {
        let (pending_ids, cause): (Vec<String>, crate::state_machine::event::CancelCause) =
            if let ConvState::CancellingSubAgents { pending, cause, .. } = &self.state {
                (pending.iter().map(|p| p.agent_id.clone()).collect(), *cause)
            } else {
                // Deadline fired but state already moved on — nothing to do.
                return;
            };

        tracing::warn!(
            conv_id = %self.context.conversation_id,
            count = pending_ids.len(),
            deadline_secs = CANCELLING_SUBAGENTS_DEADLINE.as_secs(),
            "Cancellation last-resort backstop fired — sub-agents did not report back within the \
             deadline; presuming them terminated and forcing teardown to Idle"
        );

        // Inject a generic Failure{Cancelled} per still-pending sub-agent so the
        // parent drains out of CancellingSubAgents. The drain arm relabels by the
        // state's `cause` (Timeout -> TimedOut, UserRequested -> Failure{Cancelled})
        // and the last one resolves by cause: Timeout resumes to LlmRequesting,
        // UserRequested settles to Idle.
        for agent_id in pending_ids {
            let event = Event::SubAgentResult {
                agent_id,
                outcome: SubAgentOutcome::Failure {
                    error: "sub-agent did not report within the cancellation deadline \
                            (presumed terminated)"
                        .to_string(),
                    error_kind: crate::db::ErrorKind::Cancelled,
                },
            };
            if let Err(e) = self.process_event(event).await {
                tracing::warn!(error = %e, "Failed to process cancellation backstop SubAgentResult");
            }
        }

        // Persist+broadcast the user-visible system message AFTER the drain
        // loop commits the cancelled checkpoint, so its sequence is strictly
        // after the work it describes. History replays by `sequence_id`; a
        // lower sequence would make the notice appear before (or transiently
        // instead of) the cancelled tool/sub-agent rounds on reconnect/replay.
        //
        // Only a user-requested cancel actually stops the conversation; a
        // completion timeout drains and resumes the parent's LLM turn, so the
        // "cancellation completed" notice would be wrong there.
        if cause == crate::state_machine::event::CancelCause::UserRequested {
            self.broadcast_cancellation_backstop_message().await;
        }
    }

    /// Persist+broadcast the user-visible "cancellation completed" system
    /// message shared by both cancellation backstops. Keeps the user oriented:
    /// the conversation is now Idle even though background resource reclamation
    /// may still be in flight. Callers invoke this AFTER committing the
    /// cancelled checkpoint so the message's sequence is strictly after the
    /// work it describes (history replays by `sequence_id`).
    async fn broadcast_cancellation_backstop_message(&mut self) {
        let msg_id = uuid::Uuid::new_v4().to_string();
        let content = crate::db::MessageContent::system(
            "Cancellation completed. Aborted work may still be reclaiming resources in the \
             background."
                .to_string(),
        );
        let seq = self.broadcast_tx.next_seq();
        match self
            .storage
            .add_message_with_seq(
                &msg_id,
                &self.context.conversation_id,
                seq,
                &content,
                None,
                None,
            )
            .await
        {
            Ok(msg) => {
                let _ = self.broadcast_tx.send_message(msg);
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to persist cancellation backstop system message");
            }
        }
    }

    /// Handle the cancellation backstop firing (REQ-BED-005a
    /// `CancellingToolDeadlineFires`): a tool that never observed its
    /// cancellation token and never returned has held the conversation in
    /// `CancellingTool` past the bounded deadline. Force the cancellation to
    /// completion without waiting for the task.
    ///
    /// Dispatched by `handle_deadline_expiry` (which has already cleared the
    /// deadline). Sequence: WARN, abort the retained tool task to stop the
    /// orphaned tokio task, inject `Event::ToolAborted { tool_use_id }` — the
    /// same synthetic-result path the cooperative `CancellingTool + ToolAborted
    /// -> Idle` arm uses — to settle to Idle, then persist+broadcast the
    /// user-visible system message. The message is persisted AFTER the
    /// checkpoint commits so its `sequence_id` is strictly after the cancelled
    /// tool round it describes (history replays by sequence). A late stale
    /// outcome from the aborted task is rejected by both the tool generation
    /// guard and the state machine's id/state gate once we have left
    /// `CancellingTool`.
    async fn handle_cancelling_tool_timeout(&mut self) {
        let tool_use_id = if let ConvState::CancellingTool { tool_use_id, .. } = &self.state {
            tool_use_id.clone()
        } else {
            // Deadline fired but state already moved on — nothing to do.
            return;
        };

        tracing::warn!(
            conv_id = %self.context.conversation_id,
            tool_id = %tool_use_id,
            deadline_secs = CANCELLATION_DEADLINE.as_secs(),
            "Cancellation backstop fired — tool did not observe its token within the \
             deadline; forcing teardown to Idle without waiting for the task"
        );

        // Abort the (possibly wedged) tool task so the orphaned tokio task stops.
        // The OS child is reaped by the tool itself (kill_on_drop / cooperative
        // reaping); this stops the in-process task that never returned.
        // Bump the tool generation so the aborted task's forwarded outcome is
        // stale and discarded by the select loop rather than applied to a later
        // round that may reuse the same `tool_use_id`.
        self.tool_request_generation = self.tool_request_generation.wrapping_add(1);
        if let Some(handle) = self.tool_task_handle.take() {
            handle.abort();
        }

        // Drive the cooperative cancellation arm with a synthetic ToolAborted.
        if let Err(e) = self.process_event(Event::ToolAborted { tool_use_id }).await {
            tracing::warn!(error = %e, "Failed to process cancellation backstop ToolAborted");
        }

        // Announce completion only if we actually reached Idle. When the
        // cancelled round had spawned sub-agents, ToolAborted transitions to
        // CancellingSubAgents (not Idle) — that path's own backstop announces
        // completion when it drains to Idle, so broadcasting here would be both
        // premature (cancellation isn't done) and a duplicate. Persisted AFTER
        // the checkpoint so its sequence is strictly after the round it describes.
        if matches!(self.state, ConvState::Idle) {
            self.broadcast_cancellation_backstop_message().await;
        }
    }

    /// Handle the hard stop after grace turn (REQ-BED-026 `SubAgentTurnLimitHardStop`):
    /// extract last assistant text from conversation history and notify parent.
    ///
    /// Extract partial result from conversation history and send `GraceTurnExhausted`
    /// event to the state machine. The SM handles the transition and emits `NotifyParent`.
    async fn handle_grace_turn_hard_stop(&mut self) {
        // Extract last assistant text from conversation history (I/O — belongs in executor)
        let partial_result = match self
            .storage
            .get_messages(&self.context.conversation_id)
            .await
        {
            Ok(messages) => {
                // Walk backward to find the last assistant message with text content blocks
                let mut text = None;
                for msg in messages.iter().rev() {
                    if let MessageContent::Agent(blocks) = &msg.content {
                        let text_parts: Vec<&str> = blocks
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect();
                        if !text_parts.is_empty() {
                            text = Some(text_parts.join("\n\n"));
                            break;
                        }
                    }
                }
                text
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to read messages for partial result extraction");
                None
            }
        };

        // Send GraceTurnExhausted event to the state machine.
        // The state machine handles the transition to Completed/Failed
        // and emits NotifyParent as an effect.
        let _ = self
            .event_tx
            .send(Event::GraceTurnExhausted {
                result: partial_result,
            })
            .await;
    }

    /// Halt a parent conversation that has exceeded its tool-use cycle cap
    /// (task 24680). Persists a user-visible system message explaining what
    /// happened, then sends `Event::UserCancel` so the state machine
    /// transitions `LlmRequesting → Idle` via the normal abort path. The
    /// next user message will reset the counter and resume normal operation.
    ///
    /// `attempted` is the attempt number that tripped the guard — strictly
    /// `cap + 1` for the first trip of a turn, but the signature makes the
    /// off-by-one explicit to operators reading logs or the system message:
    /// "attempt #{attempted} exceeds cap of {cap}" reads unambiguously,
    /// while a bare "limit reached ({cap})" invites confusion about whether
    /// the counter shown elsewhere (`cap + 1`) is a bug.
    async fn halt_parent_cycle_cap(&mut self, cap: u32, attempted: u32) {
        let msg_id = uuid::Uuid::new_v4().to_string();
        let text = format!(
            "Tool-use iteration limit reached: attempted LLM call #{attempted} exceeds the cap \
             of {cap} consecutive calls without a user message. Halted to prevent a runaway \
             agent loop. Send another message to continue — the counter resets on every user \
             turn. If this keeps happening, check recent tool results for a stuck call. \
             Override via the PHOENIX_PARENT_TOOL_CYCLE_CAP env var (0 disables)."
        );
        let content = crate::db::MessageContent::system(text);

        // Pre-allocate seq from the broadcaster so this message is ordered
        // strictly after any ephemeral events (tokens, state_change) emitted
        // earlier. See PersistBeforeBroadcast in specs/sse_wire/sse_wire.allium.
        let seq = self.broadcast_tx.next_seq();
        match self
            .storage
            .add_message_with_seq(
                &msg_id,
                &self.context.conversation_id,
                seq,
                &content,
                None,
                None,
            )
            .await
        {
            Ok(msg) => {
                let _ = self.broadcast_tx.send_message(msg);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to persist parent cycle cap system message"
                );
            }
        }

        let _ = self
            .event_tx
            .send(Event::UserCancel {
                reason: Some(format!("parent_tool_cycle_cap_exceeded ({cap})")),
                cause: crate::state_machine::event::CancelCause::UserRequested,
            })
            .await;
    }

    /// Handle the `spawn_agents` tool specially:
    /// 1. Parse tasks and generate agent IDs
    /// 2. Send spawn requests to `RuntimeManager` for each task
    /// 3. Return `SpawnAgentsComplete` event
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn handle_spawn_agents_tool(
        &mut self,
        tool: ToolCall,
    ) -> Result<Option<Event>, String> {
        use crate::state_machine::state::{PendingSubAgent, SpawnAgentsInput, SubAgentSpec};

        let tool_use_id = tool.id.clone();
        let input_value = tool.input.to_value();

        // Parse the spawn_agents input
        let input: SpawnAgentsInput = match serde_json::from_value(input_value) {
            Ok(i) => i,
            Err(e) => {
                // Return error as regular tool completion
                let result = ToolResult::error(tool_use_id.clone(), format!("Invalid input: {e}"));
                return Ok(Some(Event::ToolComplete {
                    tool_use_id,
                    result,
                }));
            }
        };

        if input.tasks.is_empty() {
            let result = ToolResult::error(
                tool_use_id.clone(),
                "At least one task is required".to_string(),
            );
            return Ok(Some(Event::ToolComplete {
                tool_use_id,
                result,
            }));
        }

        if input.tasks.len() > MAX_SUB_AGENTS_PER_SPAWN {
            let result = ToolResult::error(
                tool_use_id.clone(),
                format!(
                    "Too many tasks: {} requested, but at most {MAX_SUB_AGENTS_PER_SPAWN} \
                     sub-agents can be spawned per call. Split the work across multiple \
                     spawn_agents calls.",
                    input.tasks.len(),
                ),
            );
            return Ok(Some(Event::ToolComplete {
                tool_use_id,
                result,
            }));
        }

        // Bounded buffer: hint capacity for this batch's sub-agents (FM-6
        // prevention). Use `reserve`, never reassign: a result buffered from an
        // earlier batch in the same awaiting-round (e.g. tool sequence
        // [spawn_agents A, bash, spawn_agents B], where A's result arrives while
        // bash runs) must survive this dispatch and be drained when the parent
        // enters AwaitingSubAgents. The drain (`mem::take`) empties the buffer,
        // so each round still starts clean without a destructive reassignment.
        self.sub_agent_result_buffer.reserve(input.tasks.len());

        // --- Named-agent resolution (REQ-AG-005, REQ-AG-007) ---
        // Resolve against the catalog frozen at conversation start — the same
        // one that rendered the spawn_agents schema — so the advertised
        // agent_type enum and this validation never diverge if agent files
        // change mid-conversation (REQ-AG-008). Reject an unknown agent_type and
        // resolve each task's effective mode (task field > agent default >
        // Explore) up front, so the write-capability checks below run on the
        // *resolved* mode.
        let agents: &[phoenix_agents::AgentDefinition] = &self.agent_catalog;
        let mut resolved_tasks: Vec<(Option<&phoenix_agents::AgentDefinition>, SubAgentMode)> =
            Vec::with_capacity(input.tasks.len());
        for task in &input.tasks {
            let agent = if let Some(ref agent_type) = task.agent_type {
                let Some(found) = phoenix_agents::find_agent(agents, agent_type) else {
                    let available: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
                    let result = ToolResult::error(
                        tool_use_id.clone(),
                        format!(
                            "Unknown agent_type '{}'. Available: {}",
                            agent_type,
                            if available.is_empty() {
                                "none".to_string()
                            } else {
                                available.join(", ")
                            }
                        ),
                    );
                    return Ok(Some(Event::ToolComplete {
                        tool_use_id,
                        result,
                    }));
                };
                Some(found)
            } else {
                None
            };
            let mode = task
                .mode
                .or_else(|| agent.and_then(|a| a.mode))
                .unwrap_or_default();
            resolved_tasks.push((agent, mode));
        }

        // --- Mode validation and one-writer constraint (REQ-PROJ-008) ---
        let parent_allows_work = match self.context.mode_context.as_ref() {
            Some(ModeContext::Work { .. } | ModeContext::Direct | ModeContext::Branch { .. }) => {
                true
            }
            Some(ModeContext::Explore { .. }) | None => false,
        };

        let mut work_count_in_batch = 0u32;
        for &(_, mode) in &resolved_tasks {
            if mode == SubAgentMode::Work {
                if !parent_allows_work {
                    let result = ToolResult::error(
                        tool_use_id.clone(),
                        "Work sub-agents require the parent to be in a write-capable mode \
                         (Work, Branch, or Direct). Use mode: \"explore\" or omit mode \
                         for read-only sub-agents."
                            .to_string(),
                    );
                    return Ok(Some(Event::ToolComplete {
                        tool_use_id,
                        result,
                    }));
                }
                work_count_in_batch += 1;
            }
        }

        if work_count_in_batch > 1 {
            let result = ToolResult::error(
                tool_use_id.clone(),
                "Only one Work sub-agent can be spawned per call. \
                 Split into separate spawn_agents calls if you need sequential Work sub-agents."
                    .to_string(),
            );
            return Ok(Some(Event::ToolComplete {
                tool_use_id,
                result,
            }));
        }

        if work_count_in_batch > 0 && self.active_work_subagents > 0 {
            let result = ToolResult::error(
                tool_use_id.clone(),
                "A Work sub-agent is already active. Only one Work sub-agent \
                 can run at a time per parent conversation. Wait for it to complete \
                 before spawning another."
                    .to_string(),
            );
            return Ok(Some(Event::ToolComplete {
                tool_use_id,
                result,
            }));
        }

        // cwd-scoping guard (REQ-PROJ-008): a Work sub-agent's overridden
        // `cwd` must stay inside the parent's worktree. Without this guard
        // a Work sub-agent could write outside the worktree because its
        // own runtime would see a different working_dir than the parent.
        // Direct parents have no worktree to scope against -- writes there
        // are unscoped by design -- so the check only fires for parents
        // that own a worktree (Work/Branch).
        let parent_worktree_path: Option<&str> = match self.context.mode_context.as_ref() {
            Some(
                ModeContext::Work { worktree_path, .. } | ModeContext::Branch { worktree_path, .. },
            ) => Some(worktree_path.as_str()),
            _ => None,
        };
        if let Some(worktree_root) = parent_worktree_path {
            for (task, &(_, mode)) in input.tasks.iter().zip(&resolved_tasks) {
                if mode != SubAgentMode::Work {
                    continue;
                }
                let Some(override_cwd) = task.cwd.as_deref() else {
                    continue;
                };
                if !path_is_within(override_cwd, worktree_root) {
                    let result = ToolResult::error(
                        tool_use_id.clone(),
                        format!(
                            "Work sub-agent cwd '{override_cwd}' must be inside the parent's \
                             worktree '{worktree_root}'. Omit `cwd` to inherit the worktree, \
                             or pass an absolute path that resolves under it."
                        ),
                    );
                    return Ok(Some(Event::ToolComplete {
                        tool_use_id,
                        result,
                    }));
                }
            }
        }

        // Resolve and validate every spec BEFORE sending any spawn request.
        // Model validation can fail per-task; doing it inside the send loop
        // would leave earlier tasks already spawned (and untracked, since the
        // tool call then reports failure instead of SpawnAgentsComplete) when a
        // later task's effective model is unknown. Build-and-validate first,
        // then send the whole batch.
        let parent_cwd = self.context.working_dir.to_string_lossy().to_string();
        let mut specs: Vec<SubAgentSpec> = Vec::with_capacity(input.tasks.len());

        for (task, &(agent, mode)) in input.tasks.iter().zip(&resolved_tasks) {
            let cwd = task.cwd.clone().unwrap_or_else(|| parent_cwd.clone());
            let cwd = match crate::conversation_cwd::validate_conversation_cwd(&cwd) {
                Ok(valid) => valid.into_raw(),
                Err(e) => {
                    let result = ToolResult::error(
                        tool_use_id.clone(),
                        format!("Invalid sub-agent working directory: {e}"),
                    );
                    return Ok(Some(Event::ToolComplete {
                        tool_use_id,
                        result,
                    }));
                }
            };

            // Resolve model: task field > agent default > mode default
            // (REQ-AG-005, REQ-PROJ-008). An explicit model from either the
            // task or the agent definition must exist in the registry.
            let explicit_model = task
                .model
                .clone()
                .or_else(|| agent.and_then(|a| a.model.clone()));
            let resolved_model = if let Some(model) = explicit_model {
                if self.llm_registry.get(&model).is_none() {
                    let result = ToolResult::error(
                        tool_use_id.clone(),
                        format!(
                            "Unknown model '{}'. Available: {:?}",
                            model,
                            self.llm_registry.available_models()
                        ),
                    );
                    return Ok(Some(Event::ToolComplete {
                        tool_use_id,
                        result,
                    }));
                }
                model
            } else {
                match mode {
                    SubAgentMode::Explore => self
                        .llm_registry
                        .cheap_model_id_for_provider(&self.context.model_id),
                    SubAgentMode::Work => self.context.model_id.clone(),
                }
            };

            // Resolve max turns (REQ-PROJ-008)
            let max_turns = task.max_turns.unwrap_or(match mode {
                SubAgentMode::Explore => 20,
                SubAgentMode::Work => 50,
            });

            specs.push(SubAgentSpec {
                agent_id: uuid::Uuid::new_v4().to_string(),
                task: task.task.clone(),
                cwd,
                timeout: DEFAULT_SUBAGENT_TIMEOUT,
                mode,
                model_id: resolved_model,
                max_turns,
                agent_name: agent.map(|a| a.name.clone()),
                persona: agent.map(|a| a.body.clone()),
            });
        }

        // All specs validated; require a spawn channel before sending any.
        let Some(spawn_tx) = &self.spawn_tx else {
            tracing::warn!("No spawn channel configured, cannot spawn sub-agents");
            let result = ToolResult::error(
                tool_use_id.clone(),
                "Sub-agent spawning not configured".to_string(),
            );
            return Ok(Some(Event::ToolComplete {
                tool_use_id,
                result,
            }));
        };

        let mut spawned = Vec::with_capacity(specs.len());
        for spec in specs {
            spawned.push(PendingSubAgent {
                agent_id: spec.agent_id.clone(),
                task: spec.task.clone(),
                mode: spec.mode,
            });
            let request = SubAgentSpawnRequest {
                spec,
                parent_conversation_id: self.context.conversation_id.clone(),
                parent_event_tx: self.event_tx.clone(),
            };
            if let Err(e) = spawn_tx.send(request).await {
                tracing::error!(error = %e, "Failed to send spawn request");
                let result = ToolResult::error(
                    tool_use_id.clone(),
                    format!("Failed to spawn sub-agents: {e}"),
                );
                return Ok(Some(Event::ToolComplete {
                    tool_use_id,
                    result,
                }));
            }
        }

        // Track active Work sub-agents for one-writer constraint (REQ-PROJ-008)
        self.active_work_subagents += work_count_in_batch;

        // Build success result
        let agent_ids: Vec<&str> = spawned.iter().map(|p| p.agent_id.as_str()).collect();
        let output = format!(
            "Spawning {} sub-agent(s): {}",
            spawned.len(),
            agent_ids.join(", ")
        );
        let result = ToolResult::success(tool_use_id.clone(), output);

        // Send SpawnAgentsComplete event (synchronously returned, not async)
        Ok(Some(Event::SpawnAgentsComplete {
            tool_use_id,
            result,
            spawned,
        }))
    }

    /// Execute an effect and optionally return a generated event
    #[allow(clippy::too_many_lines)]
    async fn execute_effect(&mut self, effect: Effect) -> Result<Option<Event>, String> {
        match effect {
            Effect::PersistMessage {
                content,
                display_data,
                usage_data,
                message_id,
                idempotent,
            } => {
                // Idempotent path: skip if already persisted. Prevents double-
                // insert (and seq gap) when a SteerDrainedUserMessages re-fires
                // after crash recovery before ClearSteeringQueueEntries ran.
                // Gated to idempotent=true so non-replayable persists pay no
                // extra query.
                if idempotent && self.storage.message_exists(&message_id).await? {
                    tracing::debug!(
                        message_id = %message_id,
                        "Skipping PersistMessage; message already exists"
                    );
                    return Ok(None);
                }

                let seq = self.broadcast_tx.next_seq();
                let msg = self
                    .storage
                    .add_message_with_seq(
                        &message_id,
                        &self.context.conversation_id,
                        seq,
                        &content,
                        display_data.as_ref(),
                        usage_data.as_ref(),
                    )
                    .await?;

                // Broadcast to clients (display_data already computed at effect creation)
                let _ = self.broadcast_tx.send_message(msg);
                Ok(None)
            }

            Effect::PersistState => self.persist_state_effect(true).await,

            Effect::RequestLlm => self.dispatch_llm_request().await,

            Effect::ExecuteTool { tool } => self.dispatch_tool_execution(tool).await,

            Effect::ScheduleRetry {
                delay,
                attempt,
                reason,
                resets_at,
            } => {
                // REQ-LRV-001: surface the retry context to clients before
                // the backoff window opens. Two sequential `send_seq`
                // calls below (LlmAttempt → eventual StateChange/Token
                // path on the next attempt) keep the StateBar's retry
                // suffix in lockstep with the state machine. Emitted
                // BEFORE the tokio sleep so a client subscribing in the
                // backoff window observes it via the replay ring.
                let backing_off_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::LlmAttempt {
                    sequence_id: seq,
                    attempt,
                    max_attempts: crate::state_machine::transition::MAX_RETRY_ATTEMPTS,
                    reason,
                    backing_off_ms,
                    resets_at,
                });

                // Open a fresh retry generation for this timer. The timer task
                // stamps it onto its fire; the select loop discards a fire whose
                // generation has been superseded (a newer ScheduleRetry here, or
                // leaving the retry-scheduling state). This is the identity guard
                // the pure reducer cannot provide: attempt numbers reset per
                // turn, so the reducer's attempt-equality guard alone cannot
                // reject a stale timer from a cancelled-then-resent turn.
                //
                // Abort any previously-stored timer first (best-effort; normally
                // None). The generation bump already makes the prior timer's fire
                // stale, so the abort is purely to stop the task early.
                if let Some(stale) = self.retry_timer_handle.take() {
                    stale.abort();
                }
                self.retry_generation = self.retry_generation.wrapping_add(1);
                let timer_generation = self.retry_generation;
                let retry_outcome_tx = self.retry_outcome_tx.clone();
                let handle = tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = retry_outcome_tx.send((timer_generation, attempt)).await;
                });
                self.retry_timer_handle = Some(handle.abort_handle());
                Ok(None)
            }

            Effect::NotifyAgentDone => {
                let _ = self
                    .broadcast_tx
                    .send_seq(|seq| SseEvent::AgentDone { sequence_id: seq });
                Ok(None)
            }

            Effect::NotifyStateChange => {
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::StateChange {
                    sequence_id: seq,
                    state: self.state.clone(),
                    presentation_mode: self.state.presentation_mode().to_string(),
                    state_updated_at: self.state_updated_at,
                });
                Ok(None)
            }

            Effect::PersistCheckpoint { data } => self.persist_checkpoint(data).await,

            Effect::BroadcastAssistantMessage { message } => {
                // Broadcast-only: no DB write here. The atomic
                // `PersistCheckpoint` at the end of the tool round performs
                // the durable write (and emits a duplicate `sse_message`
                // that the UI dedups by `message_id`).
                //
                // The eager message is appended to the per-conversation
                // ReplayRing via `send_ephemeral_message` so reconnecting
                // clients during the tool round still see the in-flight
                // assistant content. The eventual persisted Message with
                // the same `message_id` will fire `send_persisted_message`
                // when the checkpoint completes, resetting the ring anchor
                // and discarding this entry; the client dedups by
                // `message_id` via `SseMessageDedupReplay`.
                //
                // `created_at` comes from `AssistantMessage` (captured at
                // LLM-response time) — NOT a fresh `Utc::now()`. The same
                // timestamp is later written to the DB row by
                // `persist_checkpoint`, so a reconnecting client's init
                // payload reads back the same timestamp the UI is already
                // displaying. Without that alignment, the displayed
                // timestamp would jump when init merges the DB row in.
                let seq = self.broadcast_tx.next_seq();
                let agent_content = MessageContent::agent(message.content);
                let db_msg = crate::db::Message {
                    message_id: message.message_id,
                    conversation_id: self.context.conversation_id.clone(),
                    sequence_id: seq,
                    message_type: agent_content.message_type(),
                    content: agent_content,
                    display_data: message.display_data,
                    usage_data: message.usage,
                    created_at: message.created_at,
                };
                let _ = self.broadcast_tx.send_ephemeral_message(db_msg);
                Ok(None)
            }

            Effect::PersistToolResults { results } => {
                for result in results {
                    let content = MessageContent::tool_with_images(
                        &result.tool_use_id,
                        result.output(),
                        result.is_error(),
                        result.images().to_vec(),
                    );
                    let tool_msg_id = uuid::Uuid::new_v4().to_string();
                    let seq = self.broadcast_tx.next_seq();
                    let msg = self
                        .storage
                        .add_message_with_seq(
                            &tool_msg_id,
                            &self.context.conversation_id,
                            seq,
                            &content,
                            None,
                            None,
                        )
                        .await?;

                    // Tool results don't contain bash tool_use blocks, no enrichment needed
                    let _ = self.broadcast_tx.send_message(msg);
                }
                Ok(None)
            }

            Effect::AbortTool { tool_use_id } => {
                // Signal abort to running tool. Unlike `Effect::AbortLlm`, this
                // only cancels the token — the cooperative tool task observes it
                // and produces a real `Aborted` outcome that the state machine
                // consumes (`CancellingTool + ToolAborted -> Idle`). That outcome
                // carries the dispatch generation and is still current, so it
                // must NOT be discarded; the generation is therefore NOT bumped
                // here. The id-reuse race is closed at the two points where the
                // outcome genuinely becomes stale: the next tool dispatch (which
                // bumps the generation) and the forced backstop teardown (which
                // aborts the task, dropping its sender — `handle_cancelling_tool_timeout`).
                tracing::info!(tool_id = %tool_use_id, "Aborting tool execution");
                if let Some(token) = self.tool_cancel_token.take() {
                    token.cancel();
                }
                // The spawned task will send ToolAborted event when it sees cancellation
                Ok(None)
            }

            Effect::AbortLlm => {
                tracing::info!("Aborting LLM request");
                // Bump the generation so the aborted task's forwarded outcome
                // (a synthetic NetworkError from the dropped sender) is stale
                // and gets discarded by the select loop, rather than being
                // applied to a subsequent unrelated `LlmRequesting` turn.
                self.llm_request_generation = self.llm_request_generation.wrapping_add(1);
                if let Some(handle) = self.llm_task_handle.take() {
                    handle.abort();
                }
                Ok(None)
            }

            Effect::CancelSubAgents { ids } => {
                tracing::info!(?ids, "Cancelling sub-agents");

                if let Some(cancel_tx) = &self.cancel_tx {
                    let request = SubAgentCancelRequest {
                        ids,
                        parent_conversation_id: self.context.conversation_id.clone(),
                        parent_event_tx: self.event_tx.clone(),
                    };
                    if let Err(e) = cancel_tx.send(request).await {
                        tracing::error!(error = %e, "Failed to send cancel request");
                    }
                } else {
                    tracing::warn!("No cancel channel configured, cannot cancel sub-agents");
                }
                Ok(None)
            }

            Effect::NotifyParent { outcome } => {
                tracing::info!(?outcome, "Notifying parent of sub-agent completion");

                if let Some(parent_tx) = &self.parent_event_tx {
                    let event = Event::SubAgentResult {
                        agent_id: self.context.conversation_id.clone(),
                        outcome,
                    };
                    if let Err(e) = parent_tx.send(event).await {
                        // Parent may have terminated - that's OK
                        tracing::warn!(error = %e, "Failed to notify parent (may have terminated)");
                    }
                } else {
                    tracing::warn!("No parent channel configured for sub-agent");
                }
                Ok(None)
            }

            Effect::PersistHiddenSystemMarker { marker, message_id } => {
                let seq = self.broadcast_tx.next_seq();
                let content = MessageContent::system(marker);
                let display_data = Some(serde_json::json!({ "hidden": true }));
                let msg = self
                    .storage
                    .add_message_with_seq(
                        &message_id,
                        &self.context.conversation_id,
                        seq,
                        &content,
                        display_data.as_ref(),
                        None,
                    )
                    .await?;

                let _ = self.broadcast_tx.send_message(msg);
                Ok(None)
            }

            Effect::PersistSubAgentResults {
                results,
                spawn_tool_id,
            } => self.persist_sub_agent_results(results, spawn_tool_id).await,

            Effect::RequestContinuation { request } => {
                self.request_continuation(request.rejected_tool_calls);
                Ok(None)
            }

            Effect::NotifyContextExhausted { summary } => {
                // REQ-BED-021 / REQ-BED-031: Notify client of context
                // exhaustion. Worktree is intentionally preserved (no
                // auto-cleanup, no conv_mode demotion) — continuation
                // transfer (REQ-BED-030) or a user-initiated abandon /
                // mark-as-merged is the only path that removes it.
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::StateChange {
                    sequence_id: seq,
                    state: ConvState::ContextExhausted { summary },
                    presentation_mode: "needs_action".to_string(),
                    state_updated_at: self.state_updated_at,
                });
                Ok(None)
            }

            Effect::ApproveTask {
                task_file,
                title,
                priority,
                plan,
            } => {
                self.execute_approve_task(task_file, title, priority, plan)
                    .await?;
                Ok(None)
            }

            Effect::ApproveTaskFreshHandoff {
                task_file,
                title,
                priority,
                plan,
            } => {
                self.execute_approve_task_fresh_handoff(task_file, title, priority, plan)
                    .await
            }

            Effect::ResolveTask {
                system_message,
                repo_root,
            } => {
                self.execute_resolve_task(system_message, repo_root).await?;
                Ok(None)
            }

            Effect::PersistForkProposal {
                proposal_id,
                task_file,
                title,
                priority,
                body,
                checkpoint,
            } => {
                self.execute_persist_fork_proposal(
                    proposal_id,
                    task_file,
                    title,
                    priority,
                    body,
                    checkpoint,
                )
                .await
            }

            Effect::ClearSteeringQueueEntries { message_ids } => {
                if let Err(e) = self
                    .storage
                    .remove_steering_entries(&self.context.conversation_id, &message_ids)
                    .await
                {
                    tracing::warn!(error = %e, "Failed to remove drained steering entries");
                }
                Ok(None)
            }
        }
    }

    /// Dispatch an LLM request: enforce turn/cycle caps, inject grace-turn
    /// messages, build the streaming pipeline, and spawn the LLM task.
    #[allow(clippy::too_many_lines)]
    async fn dispatch_llm_request(&mut self) -> Result<Option<Event>, String> {
        // Parent-conversation tool-use cycle cap (task 24680). Sub-agents
        // have their own lifetime cap below (REQ-PROJ-008); this branch
        // only fires for parent conversations. The counter is reset at
        // the top of `process_event` on every `Event::UserMessage`.
        if !self.context.is_sub_agent && self.parent_tool_cycle_cap > 0 {
            self.parent_tool_cycle_count += 1;
            if self.parent_tool_cycle_count > self.parent_tool_cycle_cap {
                let cap = self.parent_tool_cycle_cap;
                let attempted = self.parent_tool_cycle_count;
                tracing::warn!(
                    conv_id = %self.context.conversation_id,
                    attempted,
                    cap,
                    "parent conversation attempted to exceed tool-use cycle cap; halting"
                );
                self.halt_parent_cycle_cap(cap, attempted).await;
                return Ok(None);
            }
        }

        // Max turns enforcement (REQ-PROJ-008, REQ-BED-026): sub-agents have a
        // finite turn budget. Grace turn mechanism gives the model one extra LLM
        // turn to call submit_result before hard-stopping.
        if self.context.max_turns > 0 {
            self.llm_turn_count += 1;
            if self.llm_turn_count > self.context.max_turns {
                if self.grace_turn_granted {
                    // Second hit: hard stop with partial result extraction
                    // (REQ-BED-026 SubAgentTurnLimitHardStop)
                    tracing::info!(
                        conv_id = %self.context.conversation_id,
                        turns = self.llm_turn_count,
                        max = self.context.max_turns,
                        "Sub-agent grace turn exhausted, extracting partial results"
                    );

                    self.handle_grace_turn_hard_stop().await;
                    return Ok(None);
                }

                // First hit: grant grace turn (REQ-BED-026 SubAgentTurnLimitGraceTurn)
                self.grace_turn_granted = true;
                tracing::info!(
                    conv_id = %self.context.conversation_id,
                    turns = self.llm_turn_count,
                    max = self.context.max_turns,
                    "Sub-agent reached turn limit, granting grace turn"
                );

                // Inject a meta user message prompting submit_result.
                // Uses UserContent::meta() so it appears in the LLM context
                // via the existing User message path (not System, which is
                // UI-only bookkeeping and not sent to the LLM).
                let msg_id = uuid::Uuid::new_v4().to_string();
                let content = MessageContent::User(crate::db::UserContent::meta(
                    "You have reached your turn limit. Please call submit_result now \
                         with whatever findings you have so far. Do not call any other tools.",
                ));
                if let Err(e) = self
                    .storage
                    .add_message(&msg_id, &self.context.conversation_id, &content, None, None)
                    .await
                {
                    tracing::warn!(error = %e, "Failed to persist grace turn message");
                }

                // Allow the normal LLM request to proceed (don't return, don't
                // send UserCancel). The meta message will appear in the next
                // build_llm_messages call as a user-role message.
            }
        }

        // Typed oneshot channel: background task gets Sender<LlmOutcome>,
        // physically cannot send a ToolExecOutcome or other type.
        let (llm_tx, llm_rx) = oneshot::channel::<LlmOutcome>();

        // Open a fresh generation for this dispatch. The forwarder stamps it
        // onto the outcome; the select loop discards an outcome whose
        // generation has been superseded by a later dispatch or an
        // intentional `Effect::AbortLlm`.
        self.llm_request_generation = self.llm_request_generation.wrapping_add(1);
        let dispatch_generation = self.llm_request_generation;
        let llm_outcome_tx = self.llm_outcome_tx.clone();

        let llm_client = self.llm_client.clone();
        let tool_executor = self.tool_executor.clone();
        let clearable_names = self.clearable_names.clone();
        let clear_watermark_cache = self.clear_watermark_cache.clone();
        let storage = self.storage.clone();
        let conv_id = self.context.conversation_id.clone();
        let context_window = self.context.context_window;
        let root_conv_id = self.context.root_conversation_id.clone();
        let model_id = self.context.model_id.clone();
        let working_dir = self.context.working_dir.clone();
        let tasks_dir_name = self.context.tasks_dir_name.clone();
        let is_sub_agent = self.context.is_sub_agent;
        let mode_context = self.context.mode_context.clone();
        let llm_language = self.context.llm_language;
        let persona = self.context.persona.clone();

        // Token streaming channel (REQ-BED-025).
        //
        // Broadcast so the forwarding task can subscribe before the LLM
        // task starts emitting chunks. The forwarder bridges this
        // per-request broadcast to `self.broadcast_tx` (the per-
        // conversation SSE broadcast) as `SseEvent::Token`.
        //
        // Task 24683: the LLM task owns the forwarder's `JoinHandle`
        // and awaits it after the LLM call finishes. That forces a
        // happens-before barrier so every `SseEvent::Token` has been
        // sent to `self.broadcast_tx` before the main executor loop
        // is ever told the call is done (and therefore before it
        // broadcasts `SseEvent::Message`). Without this barrier a
        // trailing Token could land on the SSE channel after its
        // Message, producing a phantom streaming buffer on the
        // client (the "repeated message" bug).
        let (chunk_tx, chunk_rx) = broadcast::channel::<phoenix_llm::TokenChunk>(256);
        let request_id = uuid::Uuid::new_v4().to_string();

        let broadcast_tx_for_tokens = self.broadcast_tx.clone();
        let request_id_for_fwd = request_id.clone();
        let forwarder_handle = tokio::spawn(async move {
            let mut rx = chunk_rx;
            // REQ-WPV-007: emit `SseEvent::LlmFirstByte` exactly once
            // per request, immediately before the first `Token` event
            // for the same `request_id`. The two events get
            // consecutive sequence_ids from the same broadcaster, so
            // the client cannot observe a Token without first
            // observing the marker. If this request completes with
            // zero text chunks (the LLM errored or terminated before
            // emitting any), `LlmFirstByte` is NOT emitted.
            let mut first_text_seen = false;
            loop {
                match rx.recv().await {
                    Ok(phoenix_llm::TokenChunk::Text(text)) => {
                        if !first_text_seen {
                            first_text_seen = true;
                            let request_id_for_first = request_id_for_fwd.clone();
                            let _ =
                                broadcast_tx_for_tokens.send_seq(|seq| SseEvent::LlmFirstByte {
                                    sequence_id: seq,
                                    request_id: request_id_for_first,
                                });
                        }
                        let _ = broadcast_tx_for_tokens.send_seq(|seq| SseEvent::Token {
                            sequence_id: seq,
                            text,
                            request_id: request_id_for_fwd.clone(),
                        });
                    }
                    Ok(phoenix_llm::TokenChunk::RateLimitSnapshot(snapshot)) => {
                        let _ =
                            broadcast_tx_for_tokens.send_seq(|seq| SseEvent::RateLimitSnapshot {
                                sequence_id: seq,
                                snapshot: snapshot.clone(),
                            });
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(n, "Token forwarding lagged — some tokens dropped");
                    }
                }
            }
        });

        let handle = tokio::spawn(async move {
            if is_sub_agent {
                tracing::info!(
                    conv_id = %conv_id,
                    request_id = %request_id,
                    sub_agent = true,
                    "Making LLM request"
                );
            } else {
                tracing::info!(
                    conv_id = %conv_id,
                    request_id = %request_id,
                    "Making LLM request"
                );
            }

            // Build messages from history, applying stale tool-result clearing
            // (specs/stale-tool-results). The whole read/plan/persist/render
            // policy — including its REQ-STR-007 failure handling — lives in
            // assemble_cleared_messages so it is unit-testable apart from dispatch.
            let db_messages = match storage.get_messages(&conv_id).await {
                Ok(m) => m,
                Err(e) => {
                    // Build error → treated as InvalidRequest
                    let _ = llm_tx.send(LlmOutcome::NetworkError { message: e });
                    return;
                }
            };
            let messages = assemble_cleared_messages(
                &storage,
                &conv_id,
                &db_messages,
                &clearable_names,
                context_window,
                &clear_watermark_cache,
            )
            .await;

            // Build system prompt with AGENTS.md content + mode context
            // TODO(task 61006): snapshot system prompt per conversation to stop mid-session cache busts
            let system_prompt = build_system_prompt(
                &working_dir,
                &tasks_dir_name,
                is_sub_agent,
                mode_context.as_ref(),
                llm_language,
                persona.as_deref(),
            );

            // Build request — normalize messages against current tool set
            // to remove tool_use/tool_result blocks for tools no longer
            // available (e.g., propose_task after Explore→Work transition).
            let tools = tool_executor.definitions_for_language(llm_language).await;
            let tool_names: std::collections::HashSet<&str> =
                tools.iter().map(|t| t.name.as_str()).collect();
            let messages = strip_unavailable_tool_blocks(messages, &tool_names);

            let request = LlmRequest {
                system: vec![SystemContent::cached(&system_prompt)],
                messages,
                tools,
                max_tokens: Some(16_384),
                // Every turn in a conversation reuses the same prefix
                // (system prompt + earlier turns), so all turns share one key.
                cache_key: PromptCacheKey::stable(&conv_id),
            };

            // Use streaming — chunk_tx forwards text tokens to SSE clients.
            let llm_outcome = match llm_client.complete_streaming(&request, &chunk_tx).await {
                Ok(response) => {
                    // Extract tool calls from content and convert to typed ToolCall
                    let tool_calls: Vec<ToolCall> = response
                        .tool_uses()
                        .into_iter()
                        .map(|(id, name, input)| {
                            let typed_input = ToolInput::from_name_and_value(name, input.clone());
                            ToolCall::new(id.to_string(), typed_input)
                        })
                        .collect();

                    let usage = &response.usage;
                    tracing::info!(
                        input = usage.input_tokens,
                        output = usage.output_tokens,
                        cache_write = usage.cache_creation_tokens,
                        cache_read = usage.cache_read_tokens,
                        "LLM response token usage"
                    );

                    // Fire-and-forget: persist token usage for this turn.
                    // Errors are logged and do not affect the conversation.
                    let storage_for_usage = storage.clone();
                    let conv_id_for_usage = conv_id.clone();
                    let root_id_for_usage = root_conv_id.clone();
                    let model_for_usage = model_id.clone();
                    let usage_for_insert = usage.clone();
                    tokio::spawn(async move {
                        if let Err(e) = storage_for_usage
                            .insert_turn_usage(
                                &conv_id_for_usage,
                                &root_id_for_usage,
                                &model_for_usage,
                                &usage_for_insert,
                            )
                            .await
                        {
                            tracing::warn!(error = %e, "failed to write turn_usage row");
                        }
                    });

                    LlmOutcome::Response {
                        content: response.content,
                        tool_calls,
                        end_turn: response.end_turn,
                        usage: response.usage,
                        request_id: request_id.clone(),
                    }
                }
                Err(e) => llm_error_to_outcome(e),
            };

            // Task 67004: a terminal UsageLimitReached carries the
            // structured QuotaDetails parsed from the 429 response
            // headers. Replay it through the chunk channel as a
            // `RateLimitSnapshot` so the codex quota store sees the
            // limit-hit state — same channel the 200 path uses to push
            // per-turn snapshots (see `llm/openai.rs`
            // `complete_streaming`). The ErrorBanner reads from the
            // same store to render reset/credits/promo alongside the
            // plan-aware message.
            if let LlmOutcome::UsageLimitReached { ref details, .. } = llm_outcome {
                let _ = chunk_tx.send(phoenix_llm::TokenChunk::RateLimitSnapshot(details.clone()));
            }

            // Happens-before barrier for task 24683: close the chunk
            // broadcast and wait for the forwarder to drain any
            // trailing tokens before the outcome (and therefore the
            // eventual `SseEvent::Message`) is allowed to proceed.
            //
            //   1. Drop `chunk_tx` explicitly. Relying on the end of
            //      the closure isn't enough — we need the forwarder
            //      to see `Err(Closed)` *before* the `.await` below.
            //   2. Await the forwarder's `JoinHandle`. This suspends
            //      this task until every buffered `TokenChunk` has
            //      been broadcast as `SseEvent::Token`.
            //   3. Only then send the outcome that will cause the
            //      main executor loop to broadcast `SseEvent::Message`.
            drop(chunk_tx);
            if let Err(e) = forwarder_handle.await {
                tracing::warn!(error = ?e, "token forwarder task joined with error");
            }

            let _ = llm_tx.send(llm_outcome);
        });
        self.llm_task_handle = Some(handle);

        // Forward the typed outcome, generation-tagged — a dropped sender
        // becomes a typed NetworkError so a panicked/aborted LLM task can never
        // wedge the conversation, while the generation lets the select loop
        // drop a stale outcome from an intentionally-aborted request. See
        // `forward_llm_outcome`.
        tokio::spawn(forward_llm_outcome(
            llm_rx,
            dispatch_generation,
            llm_outcome_tx,
        ));

        Ok(None)
    }

    /// Dispatch tool execution: resolve the tool, build the execution context,
    /// spawn the background task, and wire up the outcome channel.
    #[allow(clippy::too_many_lines)]
    /// REQ-WPV-002: broadcast `tool_starts[tool_use_id] = now_unix_ms` on
    /// the parent assistant message via `MessageUpdated` so the client's
    /// tool widget can render a live elapsed counter sourced from the
    /// server clock. The state machine is guaranteed to be in
    /// `ToolExecuting` when `dispatch_tool_execution` fires (only
    /// `Effect::ExecuteTool { tool }` reaches that method, and that
    /// effect is only emitted on entry to / between tools of
    /// `ToolExecuting`), so the destructure below is structurally safe;
    /// the defensive `else` arm covers a hypothetical future call path.
    ///
    /// Why broadcast-only (no DB write): the assistant message that
    /// owns this `tool_use` is NOT persisted yet during the tool round —
    /// it lives in the state machine via `BroadcastAssistantMessage`
    /// (the eager broadcast, ring-replayable per `sse_wire.allium`'s
    /// `EagerAssistantMessageAppendedToReplayRing`) and the DB write
    /// happens later via `PersistCheckpoint` at the end of the round.
    /// `update_message_display_data` against an unpersisted message
    /// row no-ops with `MessageNotFound`. The client merges the
    /// broadcast `MessageUpdated.display_data` shallowly onto the
    /// in-memory message — that's the entire surface the live elapsed
    /// counter consumes (REQ-WPV-002). Once the tool result lands and
    /// the round checkpoint persists, the message's permanent
    /// `display_data` doesn't need `tool_starts` because the
    /// per-result `duration_ms` (`schema.rs:649`) takes over the
    /// display.
    fn broadcast_tool_start_timestamp(&self, tool_use_id: &str) {
        let assistant_message_id = match &self.state {
            ConvState::ToolExecuting {
                assistant_message, ..
            }
            | ConvState::CancellingTool {
                assistant_message, ..
            } => assistant_message.message_id.clone(),
            _ => {
                tracing::warn!(
                    conv_id = %self.context.conversation_id,
                    tool_use_id = %tool_use_id,
                    state = self.state.variant_name(),
                    "tool_starts broadcast skipped — dispatch_tool_execution called outside ToolExecuting/CancellingTool"
                );
                return;
            }
        };

        // Build the patch: { "tool_starts": { <id>: <unix_ms> } }.
        // The client's MessageUpdated reducer merges this shallowly
        // onto the existing message's display_data, so omitting the
        // existing bash/etc. keys is safe.
        let now_ms = Utc::now().timestamp_millis();
        let mut tool_starts = serde_json::Map::new();
        tool_starts.insert(
            tool_use_id.to_string(),
            serde_json::Value::Number(serde_json::Number::from(now_ms)),
        );
        let mut patch = serde_json::Map::new();
        patch.insert(
            "tool_starts".to_string(),
            serde_json::Value::Object(tool_starts),
        );
        let display_for_broadcast = serde_json::Value::Object(patch);

        let _ = self.broadcast_tx.send_seq(|seq| SseEvent::MessageUpdated {
            sequence_id: seq,
            message_id: assistant_message_id.clone(),
            display_data: Some(display_for_broadcast),
            content: None,
            duration_ms: None,
        });
    }

    async fn dispatch_tool_execution(&mut self, tool: ToolCall) -> Result<Option<Event>, String> {
        // Special handling for spawn_agents tool
        if tool.name() == "spawn_agents" {
            return self.handle_spawn_agents_tool(tool).await;
        }

        // REQ-WPV-002: stamp the per-tool start time into the parent
        // assistant message's `display_data.tool_starts[tool_use_id]` map
        // (unix ms, server-authoritative) and broadcast `MessageUpdated`
        // so the client's tool widget can render a live elapsed counter
        // that survives reconnect / reload / multi-tab. The runtime
        // discovers the parent message_id by destructuring the current
        // ToolExecuting state — the state machine guarantees we're in
        // ToolExecuting when this method is called (the only path here
        // is `Effect::ExecuteTool { tool }` which fires on entry to
        // ToolExecuting). If the destructure fails it's a bug and we
        // silently skip the stamp rather than panic.
        self.broadcast_tool_start_timestamp(&tool.id);

        // Typed oneshot channel: background task gets Sender<ToolExecOutcome>,
        // physically cannot send an LlmOutcome or other type.
        let (tool_tx, tool_rx) = oneshot::channel::<ToolExecOutcome>();

        // Open a fresh generation for this tool dispatch, mirroring the LLM
        // path. The forwarder stamps it onto the outcome; the select loop
        // discards an outcome whose generation has been superseded by a later
        // dispatch or an intentional `Effect::AbortTool` / backstop teardown.
        self.tool_request_generation = self.tool_request_generation.wrapping_add(1);
        let dispatch_generation = self.tool_request_generation;
        let tool_outcome_tx = self.tool_outcome_tx.clone();

        // Create cancellation token for this tool execution
        let cancel_token = CancellationToken::new();
        self.tool_cancel_token = Some(cancel_token.clone());
        let cancel_token_check = cancel_token.clone();

        // Create ToolContext for this invocation. The scope-defining worktree
        // path is the persisted `conv_mode.worktree_path()`, cached on the
        // context at construction (`work_scope_worktree`). It is `Some` for
        // Work/Branch and top-level Explore (which own a worktree) and `None`
        // for Direct and sub-agent Explore (no worktree of their own). Using
        // this — rather than `mode != Direct → working_dir` — keeps
        // `ToolContext.work_scope` in lock-step with the DB-facing scope
        // derivations: a sub-agent Explore resolves to
        // `WorkScope::Conversation(id)` on both sides instead of diverging to
        // `WorkScope::Worktree(cwd)` on the tool side only.
        let scope_worktree = self.context.work_scope_worktree.clone();
        let tool_ctx = ToolContext::new(
            cancel_token,
            self.context.conversation_id.clone(),
            self.context.working_dir.clone(),
            self.browser_sessions.clone(),
            self.bash_handles.clone(),
            self.llm_registry.clone(),
            self.terminals.clone(),
            self.tmux_registry.clone(),
            scope_worktree,
        );

        let conv_id = self.context.conversation_id.clone();
        let storage = self.storage.clone();
        let tool_executor = self.tool_executor.clone();
        let tool_use_id = tool.id.clone();
        // Retained for the outcome forwarder so a dropped sender (panicked or
        // aborted tool task) still produces a typed outcome for this tool_use.
        let forwarder_tool_use_id = tool.id.clone();
        let tool_name = tool.name().to_string();
        let tool_input = tool.input.to_value();

        // Permission seam (specs/permissions REQ-PERM-001): every tool call is
        // gate-cleared before the tool task is spawned, upstream of the
        // registry-vs-MCP split inside `ToolExecutor::execute` so MCP tools are
        // covered too. On clear the minted `CheckedToolCall` is the only value
        // `execute` accepts; on deny no task spawns and the denial returns via
        // the existing outcome channel (REQ-PERM-005). `spawn_agents` is handled
        // above and bypasses Layer 0 as intent-relative (REQ-PERM-007).
        let checked =
            match crate::runtime::deny_gate::DenyGate::check(tool_name.clone(), tool_input) {
                Ok(checked) => checked,
                Err(denial) => {
                    tracing::info!(conv_id = %conv_id, tool = %tool_name, id = %tool_use_id,
                    error = %denial.error, "Tool call denied by permission gate");
                    forward_denial_outcome(
                        forwarder_tool_use_id,
                        denial,
                        dispatch_generation,
                        tool_outcome_tx,
                    );
                    return Ok(None);
                }
            };

        let tool_task = tokio::spawn(async move {
            let tool_outcome = execute_tool_to_outcome(
                storage,
                tool_executor,
                checked,
                tool_ctx,
                &cancel_token_check,
                conv_id,
                tool_name,
                tool_use_id,
            )
            .await;
            // Send typed outcome through oneshot channel
            let _ = tool_tx.send(tool_outcome);
        });
        // Retain the handle so the cancellation backstop (REQ-BED-005a) can
        // abort an uncooperative tool task that never observes its token.
        // Replacing any prior handle is correct: only one tool executes at a
        // time, and a prior completed task's handle is inert.
        self.tool_task_handle = Some(tool_task);

        // Forward the typed outcome — a dropped sender becomes a typed Failed
        // outcome so a panicked/aborted tool task can never wedge the
        // conversation. See `forward_tool_outcome`.
        //
        // Generation-tagged, mirroring the LLM/retry paths: the forwarder
        // stamps `dispatch_generation`, and the select loop discards an outcome
        // whose generation has been superseded (a later dispatch or an
        // intentional abort/backstop teardown). The state machine's id/state
        // gate remains as defense in depth, but the generation guard is what
        // closes the same-id-reuse race — a stale outcome for a `tool_use_id`
        // that the next turn happens to reuse is dropped before it can reach
        // the reducer. See specs/bedrock REQ-BED-005a.
        tokio::spawn(forward_tool_outcome(
            tool_rx,
            forwarder_tool_use_id,
            dispatch_generation,
            tool_outcome_tx,
        ));

        Ok(None)
    }

    /// Persist a checkpoint (assistant message + tool results) atomically.
    async fn persist_checkpoint(&mut self, data: CheckpointData) -> Result<Option<Event>, String> {
        match data {
            CheckpointData::ToolRound {
                assistant_message,
                tool_results,
            } => {
                let conv_id = self.context.conversation_id.clone();

                // Build the assistant message row.
                //
                // `created_at` carries the exact value the eager broadcast
                // (`Effect::BroadcastAssistantMessage`) already delivered for
                // this `message_id`, so a reconnect that lands during persistence
                // never sees a shifted timestamp on the message the UI displays.
                let agent_content = MessageContent::agent(assistant_message.content);
                let agent_seq = self.broadcast_tx.next_seq();
                let agent_msg = crate::db::Message {
                    message_id: assistant_message.message_id.clone(),
                    conversation_id: conv_id.clone(),
                    sequence_id: agent_seq,
                    message_type: agent_content.message_type(),
                    content: agent_content,
                    display_data: assistant_message.display_data.clone(),
                    usage_data: assistant_message.usage.clone(),
                    created_at: assistant_message.created_at,
                };

                // Build all tool-result rows.
                let mut tool_msgs: Vec<crate::db::Message> = Vec::with_capacity(tool_results.len());
                for result in &tool_results {
                    let tool_content = MessageContent::tool_with_images(
                        &result.tool_use_id,
                        result.output(),
                        result.is_error(),
                        result.images().to_vec(),
                    );
                    let merged_display =
                        merge_duration_into_display_data(result.display_data(), result.duration_ms);
                    let tool_seq = self.broadcast_tx.next_seq();
                    tool_msgs.push(crate::db::Message {
                        message_id: tool_result_message_id(&result.tool_use_id),
                        conversation_id: conv_id.clone(),
                        sequence_id: tool_seq,
                        message_type: tool_content.message_type(),
                        content: tool_content,
                        display_data: merged_display,
                        usage_data: None,
                        created_at: Utc::now(),
                    });
                }

                // Persist the assistant message and every tool result in one
                // transaction: either the full round is durable or none of it
                // is. A partial write would leave an unpaired `tool_use` that
                // 400s every later LLM request (REQ-BED-007, FM-2 Prevention).
                self.storage
                    .persist_tool_round(&conv_id, &agent_msg, &tool_msgs)
                    .await?;

                // Broadcast the now-durable rows so connected clients render
                // the assistant message and each tool result.
                let _ = self.broadcast_tx.send_message(agent_msg);
                for (msg, result) in tool_msgs.into_iter().zip(tool_results.iter()) {
                    let _ = self.broadcast_tx.send_message(msg.clone());
                    // Emit a typed `MessageUpdated` so the live-connection reducer
                    // can populate `display_data.duration_ms` without parsing the
                    // opaque `display_data` blob. Reconnect paths read from the
                    // DB where `duration_ms` is already baked into `display_data`
                    // by `merge_duration_into_display_data` above.
                    if result.duration_ms.is_some() {
                        let _ = self.broadcast_tx.send_seq(|seq| SseEvent::MessageUpdated {
                            sequence_id: seq,
                            message_id: msg.message_id.clone(),
                            display_data: None,
                            content: None,
                            duration_ms: result.duration_ms,
                        });
                    }
                }
            }
        }
        Ok(None)
    }

    /// Persist a decoupled fork proposal atomically with the originating turn's
    /// tool round (REQ-PROJ-033). The assistant message, synthetic success ack,
    /// and the `fork_proposals` row commit in one transaction; the ack and the
    /// row that the review surface reads are never durable independently.
    async fn execute_persist_fork_proposal(
        &mut self,
        proposal_id: String,
        task_file: String,
        title: String,
        priority: phoenix_core::task_source::Priority,
        body: String,
        checkpoint: CheckpointData,
    ) -> Result<Option<Event>, String> {
        let CheckpointData::ToolRound {
            assistant_message,
            tool_results,
        } = checkpoint;

        // Repo-relative normalization (REQ-PROJ-033): `task_file` is relative to
        // the conversation's working_dir. The stored path is relative to the
        // repository root so the spawn/review surface resolves it regardless of
        // which worktree or subdir the origin ran in.
        let normalized_task_file =
            normalize_task_file_repo_relative(&self.context.working_dir, &task_file, &proposal_id);

        let conv_id = self.context.conversation_id.clone();

        // Build the assistant message row with a seq strictly greater than any
        // ephemeral event broadcast earlier (mirrors `persist_checkpoint`).
        let agent_content = MessageContent::agent(assistant_message.content);
        let agent_seq = self.broadcast_tx.next_seq();
        let agent_msg = crate::db::Message {
            message_id: assistant_message.message_id.clone(),
            conversation_id: conv_id.clone(),
            sequence_id: agent_seq,
            message_type: agent_content.message_type(),
            content: agent_content,
            display_data: assistant_message.display_data.clone(),
            usage_data: assistant_message.usage.clone(),
            created_at: assistant_message.created_at,
        };

        // Build the synthetic tool-result rows. The success ack carries the
        // `fork_proposal_id` in its display_data (UI-only) per the interception
        // contract; `merge_duration_into_display_data` preserves it.
        let mut tool_msgs: Vec<crate::db::Message> = Vec::with_capacity(tool_results.len());
        for result in &tool_results {
            let tool_content = MessageContent::tool_with_images(
                &result.tool_use_id,
                result.output(),
                result.is_error(),
                result.images().to_vec(),
            );
            let merged_display =
                merge_duration_into_display_data(result.display_data(), result.duration_ms);
            let tool_seq = self.broadcast_tx.next_seq();
            tool_msgs.push(crate::db::Message {
                message_id: tool_result_message_id(&result.tool_use_id),
                conversation_id: conv_id.clone(),
                sequence_id: tool_seq,
                message_type: tool_content.message_type(),
                content: tool_content,
                display_data: merged_display,
                usage_data: None,
                created_at: Utc::now(),
            });
        }

        let proposal = crate::db::ForkProposal {
            id: proposal_id,
            origin_conversation_id: conv_id.clone(),
            task_file: normalized_task_file,
            title,
            priority: priority.as_str().to_string(),
            body,
            status: crate::db::ForkProposalStatus::Pending,
            fork_conversation_id: None,
            refinement_conversation_id: None,
            created_at: Utc::now(),
            resolved_at: None,
        };

        self.storage
            .persist_fork_proposal_with_tool_round(&conv_id, &agent_msg, &tool_msgs, &proposal)
            .await?;

        // Broadcast the now-durable rows so connected clients render the
        // assistant message and the success ack (matches `persist_checkpoint`).
        let _ = self.broadcast_tx.send_message(agent_msg);
        for (msg, result) in tool_msgs.into_iter().zip(tool_results.iter()) {
            let _ = self.broadcast_tx.send_message(msg.clone());
            if result.duration_ms.is_some() {
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::MessageUpdated {
                    sequence_id: seq,
                    message_id: msg.message_id.clone(),
                    display_data: None,
                    content: None,
                    duration_ms: result.duration_ms,
                });
            }
        }

        Ok(None)
    }

    /// Persist aggregated sub-agent results. With a `spawn_tool_id`, update the
    /// originating `spawn_agents` tool message's content and `display_data`.
    /// Without one, persist a standalone meta-user summary — never a fabricated
    /// `tool_result`, which would be an orphan with no matching `tool_use`.
    async fn persist_sub_agent_results(
        &mut self,
        results: Vec<SubAgentResult>,
        spawn_tool_id: Option<String>,
    ) -> Result<Option<Event>, String> {
        // Build the display_data for subagent results
        let display_data = serde_json::json!({
            "type": "subagent_summary",
            "results": results
        });

        // Human-readable summary of sub-agent outcomes for the LLM. Both branches
        // feed the model the same text; only the carrier differs (tool_result vs
        // assistant text).
        let llm_content = render_sub_agent_summary(&results);

        // If we have a spawn_tool_id, update its message's content (for LLM history)
        // and display_data (for UI).
        if let Some(tool_id) = spawn_tool_id {
            let message_id = tool_result_message_id(&tool_id);

            // This summary replaces the initial "Spawning N sub-agents..."
            // acknowledgement so build_llm_messages_static feeds the actual
            // results to the model.

            // Both writes must succeed before broadcasting. Otherwise the client
            // would see state the DB can't corroborate on reconnect (full resync
            // from DB would revert the UI to stale values).
            if let Err(e) = self
                .storage
                .update_tool_message_content(&message_id, &llm_content)
                .await
            {
                tracing::warn!(
                    error = %e,
                    message_id = %message_id,
                    "Failed to update spawn_agents message content with sub-agent results"
                );
                return Ok(None);
            }

            if let Err(e) = self
                .storage
                .update_message_display_data(&message_id, &display_data)
                .await
            {
                tracing::warn!(
                    error = %e,
                    message_id = %message_id,
                    "Failed to update spawn_agents message display_data"
                );
                return Ok(None);
            }

            let updated_content = crate::db::MessageContent::tool(&tool_id, &llm_content, false);
            let _ = self.broadcast_tx.send_seq(|seq| SseEvent::MessageUpdated {
                sequence_id: seq,
                message_id: message_id.clone(),
                display_data: Some(display_data.clone()),
                content: Some(updated_content),
                duration_ms: None,
            });
        } else {
            // No spawn_tool_id: spawn_agents wasn't the last tool in the batch, so
            // there is no assistant tool_use to pair a tool_result against. Persist
            // the summary as a META USER observation: it renders as user role
            // (render_messages), so the model RESPONDS to the results instead of
            // continuing them (an assistant-role message would leave the turn on
            // the assistant side, to be extended rather than answered). It carries
            // no tool id, so it can never be an orphan tool_result, and `is_meta`
            // keeps the UI distinguishing it from real user input.
            let content =
                crate::db::MessageContent::User(crate::db::UserContent::meta(&llm_content));
            let msg_id = uuid::Uuid::new_v4().to_string();
            let seq = self.broadcast_tx.next_seq();
            let message = self
                .storage
                .add_message_with_seq(
                    &msg_id,
                    &self.context.conversation_id,
                    seq,
                    &content,
                    Some(&display_data),
                    None,
                )
                .await?;

            // Broadcast the new message (meta user observation, no bash enrichment needed)
            let _ = self.broadcast_tx.send_message(message);
        }

        Ok(None)
    }

    /// Build LLM messages from conversation history (instance method)
    #[allow(dead_code)] // May be useful for non-spawned code paths
    async fn build_llm_messages(&self) -> Result<Vec<LlmMessage>, String> {
        Self::build_llm_messages_static(&self.storage, &self.context.conversation_id).await
    }

    /// Build LLM messages from conversation history (static, for spawned tasks)
    async fn build_llm_messages_static(
        storage: &S,
        conv_id: &str,
    ) -> Result<Vec<LlmMessage>, String> {
        let db_messages = storage.get_messages(conv_id).await?;
        Ok(render_messages(
            &db_messages,
            &std::collections::HashSet::new(),
        ))
    }

    /// Request continuation summary from LLM (REQ-BED-020)
    #[allow(clippy::needless_pass_by_value)] // Consistent with Effect signature
    fn request_continuation(&mut self, rejected_tool_calls: Vec<ToolCall>) {
        let llm_client = Arc::clone(&self.llm_client);
        let storage = self.storage.clone();
        let event_tx = self.event_tx.clone();
        let conv_id = self.context.conversation_id.clone();
        let context_window = self.context.context_window;

        // Build continuation prompt
        let continuation_prompt = build_continuation_prompt(&rejected_tool_calls);

        let handle = tokio::spawn(async move {
            // Build messages from history and add continuation request
            let messages = match Self::build_llm_messages_static(&storage, &conv_id).await {
                Ok(m) => m,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to build messages for continuation");
                    let _ = event_tx.send(Event::ContinuationFailed { error: e }).await;
                    return;
                }
            };

            // The continuation request declares no tools, so any tool_use /
            // tool_result / server-handled block left in history would make the
            // API 400 ("tool reference not found"). Flatten them to text rather
            // than deleting them — the diffs applied, commands run, and output
            // observed are exactly what the summary should draw on. Each block's
            // text is capped so a single huge result can't dominate. Replayed
            // screenshots are bounded to the most recent few so they cannot evict
            // older textual context under the budget cap below.
            let messages = cap_replayed_images(
                flatten_tool_blocks(messages),
                CONTINUATION_MAX_REPLAYED_IMAGES,
            );

            // Proactive overflow guard: continuation fires near the top of the
            // window, so the flattened history can still exceed it. Keep the
            // most-recent messages within a token budget, dropping oldest first,
            // so the request can't 400 with ContextWindowExceeded and loop
            // deterministically to the fallback summary. The budget reserves the
            // model's reply plus the *actual* size of the continuation prompt
            // and system text — both grow (the prompt with rejected-call args)
            // and must not be allowed to push the request over the window after
            // history has filled the budget.
            let fixed_tokens = estimate_text_tokens(&continuation_prompt)
                + estimate_text_tokens(CONTINUATION_SYSTEM_PROMPT)
                + CONTINUATION_OUTPUT_RESERVE_TOKENS
                + CONTINUATION_SAFETY_MARGIN_TOKENS;
            let input_budget = context_window.saturating_sub(fixed_tokens);
            let (messages, dropped) = cap_messages_to_token_budget(messages, input_budget);
            // The budget cap drops oldest-first, which can drop the original
            // leading user task and expose an assistant message at the front.
            // The Anthropic Messages API rejects a request whose first message
            // is not user-role, so trim leading non-user messages — re-creating
            // the exact ContextWindowExceeded→fallback loop this guard prevents
            // otherwise. Flattened history carries no tool_use/tool_result
            // pairing, so dropping leading assistant narration is safe.
            let (mut messages, trimmed) = drop_leading_non_user(messages);
            if dropped > 0 || trimmed > 0 {
                tracing::debug!(
                    dropped,
                    trimmed_for_user_first = trimmed,
                    context_window,
                    input_budget,
                    "continuation: trimmed history to fit budget and start user-first"
                );
            }

            // Add the continuation request as a user message
            messages.push(LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::text(&continuation_prompt)],
            });

            // Build a tool-less request
            let request = LlmRequest {
                messages,
                system: vec![SystemContent::new(CONTINUATION_SYSTEM_PROMPT)],
                tools: vec![], // No tools for continuation
                // Handoff quality favors completeness; cap high enough that a
                // thorough summary is not truncated mid-thought.
                max_tokens: Some(4096),
                // Same conversation as the main loop — different system
                // prompt won't share a prefix in practice, but using the
                // conv id keeps the cache cohort coherent.
                cache_key: PromptCacheKey::stable(&conv_id),
            };

            match llm_client.complete(&request).await {
                Ok(response) => {
                    // Extract the text content as summary
                    let summary = response
                        .content
                        .iter()
                        .filter_map(|block| {
                            if let ContentBlock::Text { text } = block {
                                Some(text.clone())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    if summary.trim().is_empty() {
                        // An empty summary is indistinguishable to the user from
                        // "no summary" but silently seeds a blank continuation.
                        // Route it through the same fallback path as a failure
                        // so the terminal state carries an explanatory message.
                        tracing::warn!(
                            conv_id = %conv_id,
                            "continuation: model returned an empty summary; using fallback"
                        );
                        let _ = event_tx
                            .send(Event::ContinuationFailed {
                                error: "the model returned an empty summary".to_string(),
                            })
                            .await;
                        return;
                    }

                    let _ = event_tx.send(Event::ContinuationResponse { summary }).await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "Continuation LLM request failed");
                    // Send LlmError so the state machine's AwaitingContinuation retry logic fires.
                    // The attempt field is ignored by that arm (tracked in state), so 0 is fine.
                    let _ = event_tx
                        .send(Event::LlmError {
                            message: e.message.clone(),
                            error_kind: llm_error_to_db_error(e.kind),
                            attempt: 0,
                            recovery_in_progress: e.recovery_in_progress,
                            resets_at: e.quota.as_ref().and_then(|q| q.resets_at),
                        })
                        .await;
                }
            }
        });
        self.llm_task_handle = Some(handle);
    }

    /// Handle task resolution: finalize conversation state/mode/cwd, inject system message,
    /// and broadcast SSE events. Called after git operations have already completed.
    async fn execute_resolve_task(
        &mut self,
        system_message: String,
        repo_root: String,
    ) -> Result<(), String> {
        let conv_id = &self.context.conversation_id;

        // Update state. Mode is preserved (Branch stays Branch, Work stays Work).
        // Stamp the entry time first and thread it into the DB write so the
        // persisted row and the Terminal SseEvent::StateChange below carry the
        // identical value (REQ-WPV-001). `self.state` itself is not mutated
        // because the runtime is about to exit after this function returns.
        self.state_updated_at = Utc::now();
        self.storage
            .update_state(conv_id, &ConvState::Terminal, self.state_updated_at)
            .await?;
        // Legitimate cwd mutation (task 13012, teardown fallback): the
        // worktree is gone by this point, but API handlers (search_files,
        // list_skills, list_tasks, get_system_prompt) read conv.cwd for
        // terminal conversations without a state guard. Resetting to
        // repo_root gives them a valid directory rather than a deleted
        // worktree path.
        let repo_root_cwd = crate::conversation_cwd::validate_conversation_cwd(&repo_root)
            .map_err(|e| e.to_string())?;
        self.storage
            .update_conversation_cwd_recovery_only(conv_id, repo_root_cwd.raw())
            .await?;

        // Inject system message
        let msg_id = uuid::Uuid::new_v4().to_string();
        let seq = self.broadcast_tx.next_seq();
        let msg = self
            .storage
            .add_message_with_seq(
                &msg_id,
                conv_id,
                seq,
                &MessageContent::system(&system_message),
                None,
                None,
            )
            .await?;

        // Broadcast SSE events
        let _ = self.broadcast_tx.send_message(msg);
        let _ = self.broadcast_tx.send_seq(|seq| SseEvent::StateChange {
            sequence_id: seq,
            state: ConvState::Terminal,
            presentation_mode: ConvState::Terminal.presentation_mode().to_string(),
            state_updated_at: self.state_updated_at,
        });
        let _ = self
            .broadcast_tx
            .send_seq(|seq| SseEvent::ConversationUpdate {
                sequence_id: seq,
                update: crate::runtime::ConversationMetadataUpdate {
                    cwd: Some(repo_root),
                    branch_name: None,
                    worktree_path: None,
                    conv_mode_label: None,
                    base_branch: None,
                    task_title: None,
                },
            });

        Ok(())
    }

    /// REQ-BED-028: Execute git operations for task approval.
    ///
    /// Sequence: parse on-disk task file -> create worktree (or promote early one) ->
    /// rename status to in-progress if needed -> git commit -> update `conv_mode`.
    ///
    /// On failure: revert in-memory state to `AwaitingTaskApproval` so the user can retry.
    /// Collision check on retry handles partial state.
    #[allow(clippy::too_many_lines)] // NonEmptyString construction adds wrapping lines
    async fn execute_approve_task(
        &mut self,
        task_file: String,
        title: String,
        priority: crate::task_source::Priority,
        plan: String,
    ) -> Result<(), String> {
        let cwd = self.context.working_dir.clone();
        // The spec invariant WorktreePathDerivedFromConversation requires
        // the worktree path to be rooted at the repo root, not at cwd.
        // For Managed conversations cwd IS the Explore worktree; for legacy
        // pre-REQ-PROJ-028 Managed conversations cwd IS already the repo root.
        let repo_root =
            crate::git_ops::repo_root_from_phoenix_worktree(&cwd).unwrap_or_else(|| cwd.clone());
        let conv_id = self.context.conversation_id.clone();
        let desired_base_branch = self.context.desired_base_branch.clone();
        let tasks_dir_name = self.context.tasks_dir_name.clone();
        let storage = self.storage.clone();

        // Clone for state revert on failure (originals moved into spawn_blocking)
        let task_file_backup = task_file.clone();
        let title_backup = title.clone();
        let priority_backup = priority;
        let plan_backup = plan.clone();

        // Run blocking git/fs operations on a blocking thread
        let result = tokio::task::spawn_blocking(move || {
            execute_approve_task_blocking(
                &cwd,
                &repo_root,
                &conv_id,
                &tasks_dir_name,
                &task_file,
                &title,
                desired_base_branch.as_deref(),
            )
        })
        .await
        .map_err(|e| format!("Task approval join error: {e}"))?;

        match result {
            Ok(approval_result) => {
                // Update conversation mode to Work (includes worktree_path, base_branch, task_number)
                let work_mode = crate::db::ConvMode::Work {
                    branch_name: crate::db::NonEmptyString::new(
                        approval_result.branch_name.clone(),
                    )
                    .expect("branch_name from task approval must be non-empty"),
                    worktree_path: crate::db::NonEmptyString::new(
                        approval_result.worktree_path.clone(),
                    )
                    .expect("worktree_path from task approval must be non-empty"),
                    base_branch: crate::db::NonEmptyString::new(
                        approval_result.base_branch.clone(),
                    )
                    .expect("base_branch from task approval must be non-empty"),
                    task_id: crate::db::NonEmptyString::new(approval_result.task_id.clone())
                        .expect("task_id from task approval must be non-empty"),
                    task_title: crate::db::NonEmptyString::new(approval_result.task_title.clone())
                        .expect("task_title from task approval must be non-empty"),
                };
                storage
                    .update_conversation_mode(&self.context.conversation_id, &work_mode)
                    .await?;

                // Legitimate cwd mutation (task 13012, in-place promotion).
                // For Managed conversations (REQ-PROJ-028): the early Explore
                // worktree is promoted in place (branch rename, same path), so
                // this write is a no-op — worktree_path == conv.cwd already.
                // For legacy Managed conversations whose cwd was the repo root,
                // this is load-bearing: it moves cwd to the new worktree path.
                storage
                    .update_conversation_cwd_recovery_only(
                        &self.context.conversation_id,
                        crate::conversation_cwd::validate_conversation_cwd(
                            &approval_result.worktree_path,
                        )
                        .map_err(|e| e.to_string())?
                        .raw(),
                    )
                    .await?;
                self.context.working_dir = std::path::PathBuf::from(&approval_result.worktree_path);

                // Refresh in-memory mode_context so downstream checks
                // (e.g. spawn_agents Work-parent guard) observe Work mode
                // for the rest of this runtime's lifetime. Without this,
                // mode_context stays the Explore value set at runtime start.
                self.context.mode_context = Some(ModeContext::Work {
                    branch_name: approval_result.branch_name.clone(),
                    base_branch: approval_result.base_branch.clone(),
                    worktree_path: approval_result.worktree_path.clone(),
                });

                // Refresh the cached scope-defining worktree so in-runtime tool
                // calls (bash/tmux/browser) key resources under the same
                // `WorkScope` the DB-facing inventory/cleanup resolve. Approval
                // promotes Explore (no worktree -> `WorkScope::Conversation`) to
                // Work (owns a worktree -> `WorkScope::Worktree`); leaving the
                // cached value stale would split the two sides until restart.
                // The post-approval `conv_mode` is Work, whose
                // `worktree_path()` is the path just created, mirroring how
                // construction seeds this from `conv_mode.worktree_path()`.
                //
                // The scope flips here from `old_scope` (pre-approval) to
                // `new_scope` (post-approval). Resources opened pre-approval
                // (bash/browser/tmux) are keyed under `old_scope`; migrate them
                // to `new_scope` below so the inventory and idle/cleanup paths
                // resolve them under the same scope the cache now uses.
                let old_scope = phoenix_core::work_scope::WorkScope::resolve(
                    self.context.conversation_id.clone(),
                    self.context.work_scope_worktree.as_deref(),
                );
                self.context.work_scope_worktree =
                    Some(std::path::PathBuf::from(&approval_result.worktree_path));
                let new_scope = phoenix_core::work_scope::WorkScope::resolve(
                    self.context.conversation_id.clone(),
                    self.context.work_scope_worktree.as_deref(),
                );

                // Migrate WorkScope-keyed resources opened before approval from
                // the conversation scope to the worktree scope. Each rekey moves
                // the in-memory lookup key only — the underlying process /
                // session / server is untouched. A no-op when nothing was opened
                // pre-approval (the common case) or when the scope did not flip
                // (a top-level Explore that already owned a worktree).
                let bash_moved = self.bash_handles.rekey_scope(&old_scope, &new_scope).await;
                let browser_moved = self
                    .browser_sessions
                    .rekey_scope(&old_scope, &new_scope)
                    .await;
                let tmux_moved = self.tmux_registry.rekey_scope(&old_scope, &new_scope).await;

                // If anything migrated, nudge the work-scope bridge to
                // re-broadcast `new_scope`'s inventory so the panel reflects the
                // moved resources without waiting for its next poll. The bridge
                // assembles the full (bash + tmux + browser) inventory from the
                // affected scope, so a single emit on any registry sink covers
                // all three kinds; bash is used as the carrier.
                if bash_moved || browser_moved || tmux_moved {
                    self.bash_handles.emit_lifecycle(&new_scope);
                }

                // Upgrade tool registry from Explore to Work mode so the agent
                // gets bash, patch, etc. for the rest of this conversation.
                self.tool_executor.upgrade_to_work_mode();
                // The tool set changed, so refresh the cached clearable-tool set.
                self.clearable_names = Arc::new(self.tool_executor.clearable_tool_names());

                tracing::info!(
                    task_id = %approval_result.task_id,
                    branch = %approval_result.branch_name,
                    worktree = %approval_result.worktree_path,
                    first_task = approval_result.first_task,
                    "Task approved — worktree created"
                );

                // Persist as a user message so the LLM sees the approval + plan context.
                // The propose_task tool_use/result get stripped from history (tool not in
                // Work registry), so this message carries the plan forward. Must be the
                // last message before the next LLM call to avoid ending on an assistant
                // message (Anthropic rejects trailing assistant as "prefill").
                let branch_msg = format!(
                    "Task approved. You are on branch {} in {}.\n\n\
                     ## Approved plan: {}\n\n\
                     Priority: {}\n\n\
                     {}",
                    approval_result.branch_name,
                    approval_result.worktree_path,
                    title_backup,
                    priority_backup,
                    plan_backup,
                );
                let msg_id = uuid::Uuid::new_v4().to_string();
                let content = MessageContent::User(crate::db::UserContent::meta(&branch_msg));
                let seq = self.broadcast_tx.next_seq();
                let msg = self
                    .storage
                    .add_message_with_seq(
                        &msg_id,
                        &self.context.conversation_id,
                        seq,
                        &content,
                        None,
                        None,
                    )
                    .await?;
                let _ = self.broadcast_tx.send_message(msg);

                // Push updated conversation metadata to the client so it
                // reflects the new cwd, branch, worktree_path, and mode label
                // without requiring a reconnect.
                let _ = self
                    .broadcast_tx
                    .send_seq(|seq| SseEvent::ConversationUpdate {
                        sequence_id: seq,
                        update: crate::runtime::ConversationMetadataUpdate {
                            cwd: Some(approval_result.worktree_path.clone()),
                            branch_name: Some(approval_result.branch_name.clone()),
                            worktree_path: Some(approval_result.worktree_path.clone()),
                            conv_mode_label: Some("Work".to_string()),
                            base_branch: Some(approval_result.base_branch.clone()),
                            task_title: Some(approval_result.task_title.clone()),
                        },
                    });

                Ok(())
            }
            Err(e) => {
                tracing::error!(error = %e, "Task approval git operations failed");

                // Revert in-memory state to AwaitingTaskApproval so the user can retry.
                // The DB still has AwaitingTaskApproval (PersistState hasn't run for the
                // new Idle state yet), so this keeps memory and DB consistent.
                self.state = ConvState::AwaitingTaskApproval {
                    task_file: task_file_backup,
                    title: title_backup,
                    priority: priority_backup,
                    plan: plan_backup,
                };
                self.state_updated_at = Utc::now();
                // Publish the revert so live-state observers (effective_conversation_state)
                // see AwaitingTaskApproval, not the LlmRequesting this approval briefly
                // advanced to before the failure.
                if let Some(tx) = &self.state_watcher {
                    let _ = tx.send(self.state.clone());
                }

                // Broadcast an error so the UI knows, but don't propagate — the
                // conversation stays in AwaitingTaskApproval for retry.
                // Task 24682: use the typed UserFacingError. `e` is the
                // approval-pipeline error (Display-formatted, no Debug leak)
                // so it's safe to inline as the human detail.
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::Error {
                    sequence_id: seq,
                    error: crate::runtime::user_facing_error::UserFacingError::retryable(
                        "Task approval failed",
                        format!(
                            "Phoenix could not finalise the task: {e}. The conversation \
                             stays in approval state — try approving again or abandon."
                        ),
                    ),
                });

                Ok(())
            }
        }
    }
    async fn execute_approve_task_fresh_handoff(
        &mut self,
        task_file: String,
        title: String,
        priority: crate::task_source::Priority,
        plan: String,
    ) -> Result<Option<Event>, String> {
        let cwd = self.context.working_dir.clone();
        let repo_root =
            crate::git_ops::repo_root_from_phoenix_worktree(&cwd).unwrap_or_else(|| cwd.clone());
        let conv_id = self.context.conversation_id.clone();
        let desired_base_branch = self.context.desired_base_branch.clone();
        let tasks_dir_name = self.context.tasks_dir_name.clone();

        let task_file_backup = task_file.clone();
        let title_backup = title.clone();
        let priority_backup = priority;
        let plan_backup = plan.clone();

        let result = tokio::task::spawn_blocking(move || {
            execute_approve_task_blocking(
                &cwd,
                &repo_root,
                &conv_id,
                &tasks_dir_name,
                &task_file,
                &title,
                desired_base_branch.as_deref(),
            )
        })
        .await
        .map_err(|e| format!("Task approval join error: {e}"))?;

        let approval_result = match result {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(error = %e, "Fresh task approval git operations failed");
                self.state = ConvState::AwaitingTaskApproval {
                    task_file: task_file_backup,
                    title: title_backup,
                    priority: priority_backup,
                    plan: plan_backup,
                };
                self.state_updated_at = Utc::now();
                // Publish the revert so live-state observers stay consistent.
                if let Some(tx) = &self.state_watcher {
                    let _ = tx.send(self.state.clone());
                }
                let _ = self.broadcast_tx.send_seq(|seq| SseEvent::Error {
                    sequence_id: seq,
                    error: crate::runtime::user_facing_error::UserFacingError::retryable(
                        "Task approval failed",
                        format!(
                            "Phoenix could not finalise the task: {e}. The conversation \
                             stays in approval state — try approving again or abandon."
                        ),
                    ),
                });
                return Ok(None);
            }
        };

        let Some(handoff_tx) = &self.handoff_tx else {
            return Err(
                "fresh task approval unavailable: runtime handoff channel missing".to_string(),
            );
        };
        let (response_tx, response_rx) = oneshot::channel();
        let request = TaskApprovalHandoffRequest {
            parent_conversation_id: self.context.conversation_id.clone(),
            approval: TaskApprovalHandoffData {
                task_id: approval_result.task_id,
                task_title: approval_result.task_title,
                branch_name: approval_result.branch_name,
                worktree_path: approval_result.worktree_path,
                base_branch: approval_result.base_branch,
                title: title_backup,
                priority: priority_backup,
                plan: plan_backup,
                task_file: approval_result.task_file,
            },
            response_tx,
        };
        handoff_tx
            .send(request)
            .await
            .map_err(|e| format!("failed to request fresh task handoff: {e}"))?;
        let response = response_rx
            .await
            .map_err(|e| format!("fresh task handoff response dropped: {e}"))??;
        Ok(Some(Event::TaskHandoffComplete {
            successor_conv_id: response.successor_conv_id,
        }))
    }
}

/// Result of a successful task approval
struct TaskApprovalResult {
    task_id: String,
    task_title: String,
    branch_name: String,
    first_task: bool,
    task_file: String,
    /// Absolute path to the git worktree created for this conversation
    worktree_path: String,
    /// The branch that was checked out when the task was approved (merge target)
    base_branch: String,
}

/// Per-block character cap when flattening tool blocks for the continuation
/// request. Bounds a single huge tool result (a large file read, verbose
/// command output) so it cannot dominate the summary input. Overall request
/// size is bounded separately by [`cap_messages_to_token_budget`].
const CONTINUATION_BLOCK_CHAR_CAP: usize = 2000;

/// Reserve (tokens) for the continuation reply — kept in lockstep with the
/// `max_tokens` on the continuation request.
const CONTINUATION_OUTPUT_RESERVE_TOKENS: usize = 4096;

/// Safety margin (tokens) against the chars/token estimate being optimistic.
/// The prompt and system text are budgeted from their actual size separately,
/// so this only covers estimate error, not their bulk.
const CONTINUATION_SAFETY_MARGIN_TOKENS: usize = 512;

/// Most-recent image blocks replayed into a continuation/summarization request.
/// Older screenshots are dropped to their text so an image-heavy session cannot
/// fill the request with stale screenshots and evict the older textual context a
/// summary needs (the continuation analogue of the recency floor).
const CONTINUATION_MAX_REPLAYED_IMAGES: usize = 6;

/// Rough chars-per-token ratio for the proactive budget estimate. Deliberately
/// conservative (English prose is ~4); paired with the per-message overhead and
/// ceiling rounding below it errs toward dropping a message rather than
/// overflowing the window.
const CHARS_PER_TOKEN_ESTIMATE: usize = 4;

/// Per-message token overhead for role framing and wire envelope. Charged on
/// top of content so a long run of tiny messages (e.g. repeated one-word turns)
/// cannot slip under the budget at an estimated zero cost each.
const MESSAGE_OVERHEAD_TOKENS: usize = 4;

/// System prompt for the continuation request. A module const so the budget
/// estimate and the request that is actually sent draw from the same text and
/// cannot drift.
const CONTINUATION_SYSTEM_PROMPT: &str = "You are an agent writing a handoff note for the next \
    agent, who will continue this work in the same working directory with no memory of this \
    session, with the same tools available to you now. Be precise and concrete: real file paths, \
    real commands, and an honest split between what you verified and what you only assumed.";

/// Estimate the token cost of a string for the proactive budget. Ceiling
/// division so a sub-`CHARS_PER_TOKEN_ESTIMATE` string still costs at least one
/// token rather than rounding to zero.
fn estimate_text_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN_ESTIMATE)
}

/// Flatten tool-related blocks into text for the tool-less continuation request.
///
/// The continuation request declares no tools, so any `tool_use`, `tool_result`,
/// `server_tool_use`, `tool_search_tool_result`, or MCP block left in history
/// would make the API 400 with "Tool reference X not found in available tools".
/// Rather than delete those blocks — discarding the work record the summary
/// should draw on — each is rendered to text via the exhaustive
/// [`ContentBlock::render_text`] and capped at [`CONTINUATION_BLOCK_CHAR_CAP`].
/// `Text` and `Image` blocks pass through untouched; a tool result's images are
/// preserved as image blocks so screenshots remain available to the summary.
fn flatten_tool_blocks(messages: Vec<LlmMessage>) -> Vec<LlmMessage> {
    use phoenix_llm::ContentBlock;

    messages
        .into_iter()
        .map(|msg| {
            let mut flattened: Vec<ContentBlock> = Vec::with_capacity(msg.content.len());
            for block in msg.content {
                match block {
                    ContentBlock::Text { .. } | ContentBlock::Image { .. } => {
                        flattened.push(block);
                    }
                    ContentBlock::ToolResult {
                        content,
                        images,
                        is_error,
                        ..
                    } => {
                        let marker = if is_error {
                            "[tool result (error)]: "
                        } else {
                            "[tool result]: "
                        };
                        flattened.push(ContentBlock::text(format!(
                            "{marker}{}",
                            cap_block_text(content)
                        )));
                        for source in images {
                            flattened.push(ContentBlock::Image { source });
                        }
                    }
                    other => {
                        flattened.push(ContentBlock::text(cap_block_text(other.render_text())));
                    }
                }
            }
            LlmMessage {
                role: msg.role,
                content: flattened,
            }
        })
        .filter(|msg| !msg.content.is_empty())
        .collect()
}

/// Keep only the most recent `max_images` image blocks across the flattened
/// continuation history, dropping older ones (their tool-result text survives).
///
/// Without this bound, an image-heavy session replays every historical screenshot
/// into the tool-less summarization request; `cap_messages_to_token_budget` then
/// evicts the *oldest whole messages* first, so the budget fills with recent
/// images and the older textual context the summary should draw on is dropped.
/// Capping images first preserves that text (specs/stale-tool-results).
fn cap_replayed_images(mut messages: Vec<LlmMessage>, max_images: usize) -> Vec<LlmMessage> {
    let mut kept = 0usize;
    // Walk newest message first so the retained images are the most recent.
    for msg in messages.iter_mut().rev() {
        msg.content.retain(|block| {
            if matches!(block, ContentBlock::Image { .. }) {
                let keep = kept < max_images;
                kept += usize::from(keep);
                keep
            } else {
                true
            }
        });
    }
    messages.retain(|msg| !msg.content.is_empty());
    messages
}

/// Truncate flattened-block text to [`CONTINUATION_BLOCK_CHAR_CAP`] chars,
/// counting by chars so the result is always valid UTF-8.
fn cap_block_text(text: String) -> String {
    if text.chars().count() <= CONTINUATION_BLOCK_CHAR_CAP {
        return text;
    }
    let truncated: String = text.chars().take(CONTINUATION_BLOCK_CHAR_CAP).collect();
    format!("{truncated}…[truncated]")
}

/// Estimate the token cost of a single message for the proactive budget.
fn estimate_message_tokens(msg: &LlmMessage) -> usize {
    use phoenix_llm::ContentBlock;
    // A flattened-and-replayed image still costs a fixed block of tokens on the
    // wire; approximate rather than under-count it to zero.
    let content: usize = msg
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Image { .. } => IMAGE_TOKEN_ESTIMATE,
            other => estimate_text_tokens(&other.render_text()),
        })
        .sum();
    // Per-message overhead bounds the count of tiny messages so a long run of
    // them cannot be retained wholesale under a small budget.
    content + MESSAGE_OVERHEAD_TOKENS
}

/// Keep the most-recent messages whose combined estimated token cost fits
/// `budget_tokens`, dropping the oldest first to preserve a contiguous recent
/// window. The single most-recent message is always kept even if it alone
/// exceeds the budget (sending an over-budget request is strictly better than
/// sending none). Returns the kept messages and the number dropped.
fn cap_messages_to_token_budget(
    messages: Vec<LlmMessage>,
    budget_tokens: usize,
) -> (Vec<LlmMessage>, usize) {
    let total = messages.len();
    if total == 0 {
        return (messages, 0);
    }
    let mut running = 0usize;
    let mut keep_from = 0usize;
    for (i, msg) in messages.iter().enumerate().rev() {
        let est = estimate_message_tokens(msg);
        // Always keep the newest message; for older ones, stop once adding
        // would exceed the budget so the kept window stays contiguous.
        if i != total - 1 && running + est > budget_tokens {
            keep_from = i + 1;
            break;
        }
        running += est;
    }
    let dropped = keep_from;
    let kept = messages.into_iter().skip(keep_from).collect();
    (kept, dropped)
}

/// Drop leading non-user messages so the list starts with a user message (or is
/// empty). The Anthropic Messages API requires the first message to be
/// user-role; the budget cap drops oldest-first and can expose a leading
/// assistant message. Returns the trimmed list and how many were dropped.
fn drop_leading_non_user(messages: Vec<LlmMessage>) -> (Vec<LlmMessage>, usize) {
    let trimmed = messages
        .iter()
        .take_while(|m| m.role != MessageRole::User)
        .count();
    let kept = messages.into_iter().skip(trimmed).collect();
    (kept, trimmed)
}

/// Merge a `duration_ms` value into an existing `display_data` JSON blob.
///
/// If `duration_ms` is `None`, returns a clone of the existing data unchanged.
/// If `display_data` is `None`, returns `{ "duration_ms": ms }` when a
/// duration is present. If both are `Some`, inserts `duration_ms` into the
/// existing object without overwriting any tool-specific fields.
fn merge_duration_into_display_data(
    existing: Option<&serde_json::Value>,
    duration_ms: Option<u64>,
) -> Option<serde_json::Value> {
    match (existing, duration_ms) {
        (None, None) => None,
        (Some(v), None) => Some(v.clone()),
        (None, Some(ms)) => Some(serde_json::json!({ "duration_ms": ms })),
        (Some(v), Some(ms)) => {
            let mut merged = v.clone();
            if let Some(obj) = merged.as_object_mut() {
                obj.insert(
                    "duration_ms".to_string(),
                    serde_json::Value::Number(ms.into()),
                );
            }
            Some(merged)
        }
    }
}

/// Normalize a working-dir-relative `task_file` to repository-relative
/// (REQ-PROJ-033). For a Work/Branch origin whose cwd is the worktree top this
/// is `task_file` unchanged; for a Direct origin started in a repo subdir it
/// gains the subdir offset.
///
/// Falls back to the raw `task_file` (logged at warn) if the repo root cannot
/// be detected or the joined path does not sit under it — losing the proposal
/// is worse than storing a working-dir-relative path.
fn normalize_task_file_repo_relative(
    working_dir: &std::path::Path,
    task_file: &str,
    proposal_id: &str,
) -> String {
    let Some(repo_root) = phoenix_core::git::detect_git_repo_root(working_dir) else {
        tracing::warn!(
            proposal_id = %proposal_id,
            working_dir = %working_dir.display(),
            "fork proposal: no git repo root for working_dir; storing raw task_file"
        );
        return task_file.to_string();
    };
    let repo_root = std::path::Path::new(&repo_root);

    // Compute the subdir offset from `working_dir`, not from
    // `working_dir.join(task_file)`: the task file may not exist on disk yet, and
    // `Path::canonicalize` fails on a non-existent path — falling back to the raw
    // (still-symlinked) path and defeating the strip on platforms where tmp roots
    // are symlinks (macOS /var -> /private/var). `working_dir` always exists, so
    // canonicalizing it and the repo root resolves symlinks on both sides.
    let canon_wd = working_dir
        .canonicalize()
        .unwrap_or_else(|_| working_dir.to_path_buf());
    let canon_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());

    if let Ok(offset) = canon_wd.strip_prefix(&canon_root) {
        offset.join(task_file).to_string_lossy().into_owned()
    } else {
        tracing::warn!(
            proposal_id = %proposal_id,
            task_file = %task_file,
            repo_root = %canon_root.display(),
            "fork proposal: working_dir not under repo root; storing raw task_file"
        );
        task_file.to_string()
    }
}

/// Remove `tool_use` and `tool_result` blocks that reference tools not in the current set.
///
/// Handles mode transitions (e.g., Explore -> Work) where the tool set changes
/// but the conversation history contains `tool_use` blocks for the old set.
/// Anthropic's API rejects requests where `tool_use` blocks reference unavailable tools.
///
/// The DB history is not modified -- this operates on the in-memory message Vec only.
fn strip_unavailable_tool_blocks(
    messages: Vec<LlmMessage>,
    available_tools: &std::collections::HashSet<&str>,
) -> Vec<LlmMessage> {
    use phoenix_llm::ContentBlock;

    // First pass: collect IDs of tool_use blocks we're going to strip
    let mut stripped_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for msg in &messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                if !available_tools.contains(name.as_str()) {
                    stripped_ids.insert(id.clone());
                }
            }
        }
    }

    if stripped_ids.is_empty() {
        return messages;
    }

    tracing::debug!(
        count = stripped_ids.len(),
        "Stripping tool_use/tool_result blocks for unavailable tools"
    );

    // Second pass: filter out stripped tool_use/tool_result blocks.
    // For ToolSearchToolResult, remove individual bad references but keep the block
    // (it's paired with a ServerToolUse that we must not orphan).
    messages
        .into_iter()
        .map(|msg| {
            let filtered: Vec<ContentBlock> = msg
                .content
                .into_iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { ref id, .. } => {
                        if stripped_ids.contains(id) {
                            None
                        } else {
                            Some(block)
                        }
                    }
                    ContentBlock::ToolResult {
                        ref tool_use_id, ..
                    } => {
                        if stripped_ids.contains(tool_use_id) {
                            None
                        } else {
                            Some(block)
                        }
                    }
                    // Filter individual unavailable references but keep the block
                    ContentBlock::ToolSearchToolResult {
                        tool_use_id,
                        mut content,
                    } => {
                        content
                            .tool_references
                            .retain(|r| available_tools.contains(r.tool_name.as_str()));
                        Some(ContentBlock::ToolSearchToolResult {
                            tool_use_id,
                            content,
                        })
                    }
                    // ServerToolUse blocks are server-side — never strip
                    _ => Some(block),
                })
                .collect();
            LlmMessage {
                role: msg.role,
                content: filtered,
            }
        })
        // Drop messages that became empty after filtering
        .filter(|msg| !msg.content.is_empty())
        .collect()
}

#[cfg(test)]
mod strip_tool_blocks_tests {
    use super::*;
    use phoenix_llm::{
        ContentBlock, ImageSource, LlmMessage, MessageRole, ToolReference, ToolSearchResultContent,
    };

    fn user_text(s: &str) -> LlmMessage {
        LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::text(s)],
        }
    }

    fn assistant(blocks: Vec<ContentBlock>) -> LlmMessage {
        LlmMessage {
            role: MessageRole::Assistant,
            content: blocks,
        }
    }

    fn user(blocks: Vec<ContentBlock>) -> LlmMessage {
        LlmMessage {
            role: MessageRole::User,
            content: blocks,
        }
    }

    fn tool_use(id: &str, name: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input: serde_json::json!({}),
        }
    }

    fn tool_result(id: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: "ok".into(),
            images: vec![],
            is_error: false,
        }
    }

    fn server_tool_use(id: &str, name: &str) -> ContentBlock {
        ContentBlock::ServerToolUse {
            id: id.into(),
            name: name.into(),
            input: serde_json::json!({}),
        }
    }

    fn tool_search_result(tool_use_id: &str, refs: &[&str]) -> ContentBlock {
        ContentBlock::ToolSearchToolResult {
            tool_use_id: tool_use_id.into(),
            content: ToolSearchResultContent {
                r#type: "tool_search_tool_search_result".into(),
                tool_references: refs
                    .iter()
                    .map(|n| ToolReference {
                        r#type: "tool_reference".into(),
                        tool_name: (*n).into(),
                    })
                    .collect(),
                error_code: None,
            },
        }
    }

    // ----- flatten_tool_blocks -----

    /// No block survives that the tool-less continuation request could not
    /// declare — i.e. nothing but `Text`/`Image` remains. This is the
    /// invariant that keeps the API from 400ing on an undeclared tool ref.
    fn assert_no_tool_blocks(out: &[LlmMessage]) {
        for msg in out {
            for block in &msg.content {
                assert!(
                    matches!(
                        block,
                        ContentBlock::Text { .. } | ContentBlock::Image { .. }
                    ),
                    "flatten left a non-text/image block: {block:?}"
                );
            }
        }
    }

    #[test]
    fn flatten_keeps_text_only_history_unchanged() {
        let msgs = vec![
            user_text("hello"),
            assistant(vec![ContentBlock::text("hi")]),
        ];
        let out = flatten_tool_blocks(msgs.clone());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content.len(), 1);
        assert_eq!(out[1].content.len(), 1);
    }

    #[test]
    fn flatten_keeps_image_blocks() {
        // Continuation summary should retain images (e.g. screenshots from
        // the user's earlier turns) so the model has the visual context.
        let msgs = vec![user(vec![
            ContentBlock::text("look"),
            ContentBlock::Image {
                source: ImageSource::Base64 {
                    media_type: "image/png".into(),
                    data: "AAA".into(),
                },
            },
        ])];
        let out = flatten_tool_blocks(msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content.len(), 2);
    }

    #[test]
    fn cap_replayed_images_keeps_only_most_recent() {
        let img = || ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "x".into(),
            },
        };
        // Oldest → newest, each message carrying text plus one image.
        let msgs = vec![
            user(vec![ContentBlock::text("old"), img()]),
            user(vec![ContentBlock::text("mid"), img()]),
            user(vec![ContentBlock::text("new"), img()]),
        ];
        let out = cap_replayed_images(msgs, 1);

        let images: usize = out
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::Image { .. }))
            .count();
        assert_eq!(images, 1, "only the most-recent image survives the cap");
        assert_eq!(out.len(), 3, "no message dropped — each keeps its text");
        assert!(
            matches!(out[2].content.last(), Some(ContentBlock::Image { .. })),
            "the kept image is in the newest message",
        );
        assert!(
            out[0]
                .content
                .iter()
                .all(|b| matches!(b, ContentBlock::Text { .. })),
            "older messages keep their text but lose their image",
        );
    }

    #[test]
    fn flatten_renders_tool_use_and_result_as_text() {
        let msgs = vec![
            assistant(vec![
                ContentBlock::text("calling bash"),
                tool_use("t1", "bash"),
            ]),
            user(vec![tool_result("t1")]),
            assistant(vec![ContentBlock::text("done")]),
        ];
        let out = flatten_tool_blocks(msgs);
        // Every message survives now — the work record is preserved as text
        // rather than deleted.
        assert_eq!(out.len(), 3);
        assert_no_tool_blocks(&out);
        // tool_use is flattened alongside the narration it accompanied.
        assert_eq!(out[0].content.len(), 2);
        assert!(
            matches!(&out[0].content[0], ContentBlock::Text { text } if text == "calling bash")
        );
        assert!(matches!(&out[0].content[1], ContentBlock::Text { text } if text.contains("bash")));
        // tool_result becomes a marked text block carrying its content.
        assert!(matches!(&out[1].content[0], ContentBlock::Text { text }
            if text.contains("tool result") && text.contains("ok")));
    }

    #[test]
    fn flatten_renders_server_blocks_as_text() {
        // Reproduces the production failure shape: tool_search-discovered MCP
        // tool referenced in history. With tools=[] the request would 400 on
        // these blocks; flattening renders them to text so it can't.
        let msgs = vec![
            assistant(vec![
                server_tool_use("srv1", "tool_search_tool_regex"),
                tool_search_result("srv1", &["datadog-mcp-prod__aggregate_events"]),
                tool_use("call1", "datadog-mcp-prod__aggregate_events"),
            ]),
            user(vec![tool_result("call1")]),
            assistant(vec![ContentBlock::text("summary so far")]),
        ];
        let out = flatten_tool_blocks(msgs);
        assert_eq!(out.len(), 3);
        assert_no_tool_blocks(&out);
        assert_eq!(
            out[0].content.len(),
            3,
            "three server/tool blocks flattened"
        );
    }

    #[test]
    fn flatten_preserves_tool_only_messages_as_text() {
        // A message whose only blocks were tool blocks is no longer dropped —
        // its work record is rendered to text.
        let msgs = vec![
            assistant(vec![tool_use("t1", "bash")]),
            user(vec![tool_result("t1")]),
        ];
        let out = flatten_tool_blocks(msgs);
        assert_eq!(out.len(), 2);
        assert_no_tool_blocks(&out);
    }

    #[test]
    fn cap_block_text_truncates_by_chars() {
        let capped = cap_block_text("é".repeat(CONTINUATION_BLOCK_CHAR_CAP + 100));
        assert!(capped.ends_with("…[truncated]"));
        assert!(capped.chars().count() <= CONTINUATION_BLOCK_CHAR_CAP + 12);
    }

    #[test]
    fn budget_cap_keeps_recent_drops_oldest() {
        // Each message ~5 chars of text => ~1 token. Budget of 2 tokens keeps
        // roughly the two newest; oldest are dropped, newest always survives.
        let msgs = vec![
            user_text("aaaaaaaa"),
            assistant(vec![ContentBlock::text("bbbbbbbb")]),
            user_text("cccccccc"),
        ];
        let (kept, dropped) = cap_messages_to_token_budget(msgs, 2);
        assert!(dropped >= 1, "oldest should be dropped");
        assert_eq!(kept.len() + dropped, 3);
        // The newest message is always retained.
        assert!(matches!(&kept.last().unwrap().content[0],
            ContentBlock::Text { text } if text == "cccccccc"));
    }

    #[test]
    fn budget_cap_keeps_single_oversized_newest_message() {
        let msgs = vec![user_text(&"x".repeat(10_000))];
        let (kept, dropped) = cap_messages_to_token_budget(msgs, 1);
        assert_eq!(kept.len(), 1, "the sole newest message is always kept");
        assert_eq!(dropped, 0);
    }

    #[test]
    fn drop_leading_non_user_trims_assistant_prefix() {
        // The budget cap can drop the original leading user task, leaving an
        // assistant message first — which the Anthropic API rejects.
        let msgs = vec![
            assistant(vec![ContentBlock::text("orphaned narration")]),
            assistant(vec![ContentBlock::text("more narration")]),
            user_text("tool result stand-in"),
            assistant(vec![ContentBlock::text("latest")]),
        ];
        let (kept, trimmed) = drop_leading_non_user(msgs);
        assert_eq!(trimmed, 2);
        assert_eq!(kept.first().unwrap().role, MessageRole::User);
    }

    #[test]
    fn drop_leading_non_user_noop_when_already_user_first() {
        let msgs = vec![user_text("hi"), assistant(vec![ContentBlock::text("yo")])];
        let (kept, trimmed) = drop_leading_non_user(msgs);
        assert_eq!(trimmed, 0);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn drop_leading_non_user_empties_all_assistant_history() {
        // Degenerate case: nothing but assistant messages. Trimming to empty is
        // correct — request_continuation appends the user prompt afterward, so
        // the sent request still starts user-first.
        let msgs = vec![
            assistant(vec![ContentBlock::text("a")]),
            assistant(vec![ContentBlock::text("b")]),
        ];
        let (kept, trimmed) = drop_leading_non_user(msgs);
        assert_eq!(trimmed, 2);
        assert!(kept.is_empty());
    }

    #[test]
    fn budget_cap_charges_overhead_for_tiny_messages() {
        // A run of sub-CHARS_PER_TOKEN messages must still be bounded: the
        // per-message overhead (and ceiling rounding) keeps them from being
        // retained wholesale at an estimated zero cost each.
        let msgs: Vec<LlmMessage> = (0..100).map(|_| user_text("x")).collect();
        let (kept, dropped) = cap_messages_to_token_budget(msgs, 20);
        assert!(
            dropped > 0,
            "tiny messages must still be charged and dropped"
        );
        assert!(kept.len() < 100, "not every tiny message should fit");
    }

    #[test]
    fn estimate_text_tokens_rounds_up() {
        assert_eq!(estimate_text_tokens(""), 0);
        assert_eq!(
            estimate_text_tokens("x"),
            1,
            "sub-ratio text costs >= 1 token"
        );
        assert_eq!(estimate_text_tokens(&"x".repeat(5)), 2);
    }

    // ----- strip_unavailable_tool_blocks -----

    #[test]
    fn strip_unavailable_noop_when_all_tools_available() {
        let available: std::collections::HashSet<&str> = ["bash", "patch"].into_iter().collect();
        let msgs = vec![
            assistant(vec![ContentBlock::text("x"), tool_use("t1", "bash")]),
            user(vec![tool_result("t1")]),
        ];
        let out = strip_unavailable_tool_blocks(msgs.clone(), &available);
        assert_eq!(out.len(), msgs.len());
        assert_eq!(out[0].content.len(), 2);
        assert_eq!(out[1].content.len(), 1);
    }

    #[test]
    fn strip_unavailable_removes_tool_use_and_paired_result() {
        let available: std::collections::HashSet<&str> = ["bash"].into_iter().collect();
        let msgs = vec![
            assistant(vec![
                ContentBlock::text("mixed"),
                tool_use("keep", "bash"),
                tool_use("drop", "propose_task"),
            ]),
            user(vec![tool_result("keep"), tool_result("drop")]),
        ];
        let out = strip_unavailable_tool_blocks(msgs, &available);
        // Assistant: text + the bash tool_use survive; propose_task tool_use is gone
        assert_eq!(out[0].content.len(), 2);
        assert!(out[0]
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == "keep")));
        // User message: only the tool_result for the surviving tool_use remains
        assert_eq!(out[1].content.len(), 1);
        assert!(
            matches!(&out[1].content[0], ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "keep")
        );
    }

    #[test]
    fn strip_unavailable_filters_tool_search_references_in_place() {
        // Mode transition mid-conversation: some referenced tools are gone.
        // The ToolSearchToolResult block must survive (paired with its
        // ServerToolUse) but its tool_references list is filtered.
        let available: std::collections::HashSet<&str> = ["bash"].into_iter().collect();
        let msgs = vec![assistant(vec![
            tool_use("ghost", "removed_tool"), // forces stripped_ids non-empty path
            server_tool_use("srv1", "tool_search_tool_regex"),
            tool_search_result("srv1", &["bash", "removed_tool", "other"]),
        ])];
        let out = strip_unavailable_tool_blocks(msgs, &available);
        assert_eq!(out.len(), 1);

        let ts_block = out[0]
            .content
            .iter()
            .find_map(|b| {
                if let ContentBlock::ToolSearchToolResult { content, .. } = b {
                    Some(content)
                } else {
                    None
                }
            })
            .expect("tool_search block should survive");
        assert_eq!(
            ts_block.tool_references.len(),
            1,
            "only references to available tools should remain"
        );
        assert_eq!(ts_block.tool_references[0].tool_name, "bash");

        // ServerToolUse must NOT be stripped — it pairs with the tool_search
        // result and orphaning it would 400 the request.
        assert!(out[0]
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ServerToolUse { id, .. } if id == "srv1")));
    }

    #[test]
    fn strip_unavailable_drops_messages_that_become_empty() {
        let available: std::collections::HashSet<&str> = ["bash"].into_iter().collect();
        let msgs = vec![
            assistant(vec![tool_use("t1", "removed_tool")]),
            user(vec![tool_result("t1")]),
            assistant(vec![ContentBlock::text("survives")]),
        ];
        let out = strip_unavailable_tool_blocks(msgs, &available);
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0].content[0], ContentBlock::Text { text } if text == "survives"));
    }
}

// Re-export git helpers so existing `crate::runtime::executor::{run_git, ensure_gitignore_has_phoenix}`
// imports continue to resolve. Canonical definitions live in `crate::git_ops`.
pub(crate) use crate::git_ops::{ensure_gitignore_has_phoenix, run_git};

/// Rename a task file to `in-progress` status if it isn't already.
///
/// Returns the final filename (unchanged if no rename was needed). The file
/// is renamed in place via `taskmd_core::tasks::update_task`, which is a
/// single `std::fs::rename` on the filename — the body is untouched.
pub(crate) fn promote_task_status_to_in_progress(
    tasks_dir: &std::path::Path,
    task_id: &str,
    current_status: taskmd_core::constants::Status,
    original_filename: &str,
) -> Result<String, String> {
    use taskmd_core::constants::Status;
    use taskmd_core::tasks::{update_task, TaskUpdate};

    if current_status == Status::InProgress {
        return Ok(original_filename.to_string());
    }
    let result = update_task(
        tasks_dir,
        task_id,
        TaskUpdate {
            status: Some(Status::InProgress),
            ..Default::default()
        },
    )
    .map_err(|e| format!("Failed to rename task file to in-progress status: {e}"))?;
    Ok(result.new_filename)
}

/// Global mutex serializing the scan-tasks + write + commit sequence.
/// Task approval is rare; a single mutex is sufficient.
pub(crate) static TASK_APPROVAL_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Resolve the base branch to record on the Work-mode conversation a task
/// approval produces, and single-branch-fetch it (REQ-PROJ-022, best-effort).
///
/// Normally the conversation recorded the base at creation time
/// (`desired_base_branch`). If not (e.g. an older `mode=auto` Managed
/// conversation), fall back to the *main checkout's* HEAD via `repo_root` —
/// **not** `cwd`'s HEAD, which is the early Explore worktree's `task-pending-…`
/// temp branch.
fn resolve_approval_base_branch(
    cwd: &std::path::Path,
    repo_root: &std::path::Path,
    desired_base_branch: Option<&str>,
) -> Result<String, String> {
    let base_branch = if let Some(b) = desired_base_branch {
        b.to_string()
    } else {
        let b = run_git(repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])?
            .trim()
            .to_string();
        if b.is_empty() || b == "HEAD" {
            return Err(
                "Cannot determine the base branch for this approval (the conversation didn't \
                 record one and the repository is on a detached HEAD). Re-create the \
                 conversation with an explicit base branch."
                    .to_string(),
            );
        }
        b
    };
    crate::git_ops::materialize_branch(cwd, &base_branch).map_err(|e| e.to_string())?;
    Ok(base_branch)
}

/// Locate the early Explore worktree at `{repo_root}/.phoenix/worktrees/{conv_id}`
/// and rename its `task-pending-…` temp branch to `task_branch` in place
/// (REQ-PROJ-028). Returns the worktree path.
///
/// `propose_task` is Managed-only and a Managed conversation gets this worktree
/// on its first message, so it always exists by approval time; if it somehow
/// doesn't, this errors with a clear "reject and re-propose" message rather than
/// nesting a new worktree.
fn open_early_worktree_and_rename_branch(
    repo_root: &std::path::Path,
    conv_id: &str,
    task_branch: &str,
) -> Result<std::path::PathBuf, String> {
    let worktree_path = repo_root.join(".phoenix/worktrees").join(conv_id);
    let exists = worktree_path.is_dir()
        && run_git(&worktree_path, &["rev-parse", "--is-inside-work-tree"]).is_ok();
    if !exists {
        return Err(format!(
            "No Explore worktree found at {} for this conversation. \
             Reject the plan and ask the agent to propose again.",
            worktree_path.display()
        ));
    }
    let temp_branch = run_git(&worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map_err(|e| format!("Failed to determine current branch in worktree: {e}"))?
        .trim()
        .to_string();
    tracing::info!(temp_branch = %temp_branch, task_branch, "REQ-PROJ-028: renaming temp branch");
    run_git(&worktree_path, &["branch", "-m", &temp_branch, task_branch])
        .map_err(|e| format!("Failed to rename branch '{temp_branch}' to '{task_branch}': {e}"))?;
    Ok(worktree_path)
}

/// Blocking implementation of taskmd task approval (REQ-PROJ-028).
/// Runs on a blocking thread via `spawn_blocking`.
///
/// `propose_task` is a Managed-only tool and a Managed conversation gets its
/// Explore worktree on its first message, so by the time approval runs `cwd`
/// IS that worktree, sitting on a `task-pending-…` temp branch with the task
/// file already written under `{tasks_dir}/`. Approval renames the temp branch
/// to `task-{id}-{slug}`, promotes the file's status segment to `in-progress`
/// if needed, and commits it on the task branch. There is no "task file lives
/// in the repo-root checkout" fallback — the early worktree always exists; if
/// it somehow doesn't, approval fails with a clear retry message rather than
/// nesting a new worktree.
///
/// A plain-markdown task file (a `.md` name that isn't a taskmd filename) is
/// handled by [`execute_approve_plain_markdown_blocking`] — dispatched below.
///
/// `cwd` is the Explore worktree; `repo_root` is the git repository root, used
/// for the canonical worktree path `{repo_root}/.phoenix/worktrees/{conv_id}`.
fn execute_approve_task_blocking(
    cwd: &std::path::Path,
    repo_root: &std::path::Path,
    conv_id: &str,
    tasks_dir_name: &str,
    task_file: &str,
    title: &str,
    desired_base_branch: Option<&str>,
) -> Result<TaskApprovalResult, String> {
    // task 13009: a plain-markdown task file (a `.md` file whose name doesn't
    // match the taskmd pattern) takes a separate, simpler approval path — no
    // taskmd id/status/slug, no status-rename, branch uniquified by conversation
    // id. The empty-string legacy shim and all taskmd handling fall through to
    // the code below (a non-`.md`, non-taskmd path produces the taskmd-pattern
    // error there).
    if let Some(filename) = std::path::Path::new(task_file)
        .file_name()
        .and_then(|f| f.to_str())
    {
        if let Some(crate::task_source::TaskSource::PlainMarkdown { stem }) =
            crate::task_source::TaskSource::detect(filename)
        {
            return execute_approve_plain_markdown_blocking(
                cwd,
                repo_root,
                conv_id,
                task_file,
                &stem,
                title,
                desired_base_branch,
            );
        }
    }

    // Serialize approvals so concurrent attempts can't race on the same
    // branch/worktree name.
    let _guard = TASK_APPROVAL_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if task_file.is_empty() {
        // Backward-compat shim: AwaitingTaskApproval rows persisted before
        // task_file existed deserialise with an empty string.
        return Err(format!(
            "This approval predates the file-based propose_task flow. \
             Reject the plan and ask the agent to propose again — it will \
             draft a task file under {tasks_dir_name}/ this time."
        ));
    }

    let base_branch = resolve_approval_base_branch(cwd, repo_root, desired_base_branch)?;

    // Parse the on-disk task filename — in taskmd 1.0 the filename is the sole
    // source of id/priority/status/slug; Phoenix allocates no ID.
    let rel_path = std::path::Path::new(task_file);
    let original_filename = rel_path
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| format!("task_file has no filename component: '{task_file}'"))?
        .to_string();
    let parsed = taskmd_core::filename::parse_filename(&original_filename).ok_or_else(|| {
        format!(
            "task_file '{original_filename}' does not match the taskmd filename pattern \
             (NNNNN-pX-status--slug.md)"
        )
    })?;
    let task_id = parsed.id.clone();
    let slug = parsed.slug.clone();
    let branch_name = format!("task-{task_id}-{slug}");

    let cwd_filepath = cwd.join(rel_path);
    if !cwd_filepath.exists() {
        return Err(format!(
            "Task file '{task_file}' does not exist under {}. \
             The file must be on disk before approval.",
            cwd.display()
        ));
    }

    // REQ-PROJ-028: `cwd` IS the early Explore worktree, on a `task-pending-…`
    // temp branch, with the task file already under `{tasks_dir}/`. Rename the
    // temp branch, promote the file's status to `in-progress` if needed, then
    // commit it on the task branch.
    let worktree_path = open_early_worktree_and_rename_branch(repo_root, conv_id, &branch_name)?;
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    let final_filename = promote_task_status_to_in_progress(
        &worktree_path.join(tasks_dir_name),
        &task_id,
        parsed.status,
        &original_filename,
    )?;

    ensure_gitignore_has_phoenix(&worktree_path)?;
    // If `update_task` renamed the file, the old name is now a deletion — stage
    // it so the commit captures the rename rather than a duplicate task ID.
    // `git add` on an untracked-and-missing path errors harmlessly.
    if final_filename != original_filename {
        let _ = run_git(
            &worktree_path,
            &[
                "add",
                "--",
                &format!("{tasks_dir_name}/{original_filename}"),
            ],
        );
    }
    run_git(
        &worktree_path,
        &["add", "--", &format!("{tasks_dir_name}/{final_filename}")],
    )?;
    let commit_msg = format!("task {task_id}: {title}");
    // Nothing staged — e.g. reusing an existing already-`in-progress` task file
    // that wasn't modified: it's already on the branch, skip the commit.
    if run_git(&worktree_path, &["diff", "--cached", "--quiet"]).is_err() {
        if let Err(e) = run_git(&worktree_path, &["commit", "-m", &commit_msg]) {
            return Err(format!("Failed to commit task file in worktree: {e}"));
        }
        tracing::info!(branch = %branch_name, commit_msg = %commit_msg, "Task file committed on task branch");
    } else {
        tracing::info!(branch = %branch_name, "Task file already on the branch unchanged — no commit needed");
    }

    Ok(TaskApprovalResult {
        task_id,
        task_title: title.to_string(),
        branch_name,
        first_task: false,
        task_file: format!("{tasks_dir_name}/{final_filename}"),
        worktree_path: worktree_path_str,
        base_branch,
    })
}

/// Blocking git operations for approving a *plain-markdown* task file (task
/// 13009) — a sibling of [`execute_approve_task_blocking`] for the case where
/// the task file's name is not a taskmd 1.0 filename.
///
/// Differences from the taskmd path:
/// - branch name is `task-{sanitized-stem}-{conv-id-prefix}` — the conv-id
///   prefix is the uniquifier so two conversations proposing files with the
///   same stem don't collide (the approval mutex only serializes);
/// - no `...-ready--` → `...-in-progress--` rename, no `format_filename` call —
///   a plain brief has no status segment;
/// - the file is committed at its own path (e.g. `docs/plan.md`), not under the
///   project's tasks dir.
///
/// Only the REQ-PROJ-028 early-worktree case is handled. `propose_task` is a
/// Managed-only tool, and a Managed conversation gets its Explore worktree on
/// its first message (`ManagedWorktreeOnFirstMessage`), so by the time approval
/// runs the worktree always exists — there is no "task file lives in the
/// repo-root checkout" legacy fallback here (that scenario never existed for
/// plain-markdown task files: they're new in task 13009). If the worktree is
/// somehow missing, the approval fails with a clear "reject and re-propose"
/// message rather than silently recreating one.
///
/// `stem` is the task file's filename with the `.md` extension stripped (the
/// caller has already classified the filename as plain-markdown). `cwd` /
/// `repo_root` have the same meaning as in [`execute_approve_task_blocking`].
fn execute_approve_plain_markdown_blocking(
    cwd: &std::path::Path,
    repo_root: &std::path::Path,
    conv_id: &str,
    task_file: &str,
    stem: &str,
    title: &str,
    desired_base_branch: Option<&str>,
) -> Result<TaskApprovalResult, String> {
    let _guard = TASK_APPROVAL_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // base_branch is recorded on the resulting Work-mode conversation; for the
    // early-worktree case it is otherwise unused here (the worktree was already
    // created from it).
    let base_branch = resolve_approval_base_branch(cwd, repo_root, desired_base_branch)?;

    let cwd_filepath = cwd.join(task_file);
    if !cwd_filepath.exists() {
        return Err(format!(
            "Task file '{task_file}' does not exist under {}. \
             The file must be on disk before approval.",
            cwd.display()
        ));
    }

    let (branch_name, task_id) = crate::task_source::TaskSource::PlainMarkdown {
        stem: stem.to_string(),
    }
    .branch_and_id(conv_id);
    let commit_msg = format!("task {task_id}: {title}");

    // REQ-PROJ-028: `cwd` IS the early Explore worktree, on a temp branch, with
    // the task file already in place. Rename the temp branch, then commit the
    // file at its own path on it.
    let worktree_path = open_early_worktree_and_rename_branch(repo_root, conv_id, &branch_name)?;
    let worktree_path_str = worktree_path.to_string_lossy().to_string();
    ensure_gitignore_has_phoenix(&worktree_path)?;
    run_git(&worktree_path, &["add", "--", task_file])?;
    // If the agent pointed at an existing file that was already on the branch
    // (inherited from base_branch) and didn't modify it — common when
    // `propose_task` targets something like `docs/plan.md` that Explore mode
    // can't edit — there is nothing staged. The file is already on the branch;
    // skip the commit rather than failing with "nothing to commit". `git diff
    // --cached --quiet` exits 0 when the index matches HEAD.
    if run_git(&worktree_path, &["diff", "--cached", "--quiet"]).is_err() {
        if let Err(e) = run_git(&worktree_path, &["commit", "-m", &commit_msg]) {
            return Err(format!("Failed to commit task file in worktree: {e}"));
        }
        tracing::info!(branch = %branch_name, worktree = %worktree_path_str, "Plain-markdown task approved — temp branch renamed, task file committed");
    } else {
        tracing::info!(branch = %branch_name, worktree = %worktree_path_str, "Plain-markdown task approved — temp branch renamed; task file already on the branch unchanged, no commit needed");
    }

    Ok(TaskApprovalResult {
        task_id,
        task_title: title.to_string(),
        branch_name,
        first_task: false,
        task_file: task_file.to_string(),
        worktree_path: worktree_path_str,
        base_branch,
    })
}

/// Build the continuation prompt (REQ-BED-020).
///
/// The summary seeds a fresh agent that restarts with no memory of this session
/// in the same working directory. That successor inherits this conversation's
/// mode and tool set (continuation copies `conv_mode` verbatim), so the prompt
/// promises "the same tools you have now" rather than full access — an Explore
/// continuation stays in Explore mode and cannot run write/edit tools. It asks
/// for an operational handoff (exact paths, repo state, verified-vs-assumed)
/// rather than a human-facing recap.
fn build_continuation_prompt(rejected_tool_calls: &[ToolCall]) -> String {
    let mut prompt = String::from(
        "This conversation has reached its context limit. Write a handoff that lets a fresh \
        agent — no memory of this session, but the same tools you have now, in the same working \
        directory — pick up exactly where you left off.\n\n\
        Cover:\n\
        1. The goal: what you were asked to do, and the current status toward it.\n\
        2. Work done: concrete changes made, with exact file paths; commands run and their \
        outcome.\n\
        3. Verified vs. assumed: what you confirmed works (tests run, output observed) versus \
        what you believe but have not checked.\n\
        4. Repo state: branch, what is committed vs. uncommitted, anything pushed.\n\
        5. Next steps: the specific actions to take next, the first concrete one first.\n\
        6. Pitfalls: dead ends, gotchas, or context the next agent would otherwise have to \
        rediscover the hard way.\n\n\
        Favor completeness over brevity — a dropped file path or an unstated caveat costs the \
        next agent far more than length does. Write directly to that agent.",
    );

    if !rejected_tool_calls.is_empty() {
        use std::fmt::Write;
        prompt.push_str(
            "\n\nThese tool calls were pending when the limit hit and did not run. Fold the \
            intended actions into the next steps:\n",
        );
        for tool in rejected_tool_calls {
            let _ = writeln!(prompt, "- {}", render_rejected_tool_call(tool));
        }
    }

    prompt
}

/// Render a pending (rejected) tool call as `name: {compact-json-args}`, with
/// the arguments truncated on a char boundary so one oversized payload cannot
/// bloat the prompt. The next agent needs to know *what* was about to run
/// (which file the patch targeted, which command was queued), not just the
/// tool type. The internal `_tool` discriminant is stripped since the name is
/// already printed.
fn render_rejected_tool_call(tool: &ToolCall) -> String {
    const MAX_ARGS_CHARS: usize = 500;
    let name = tool.name();
    let args = match serde_json::to_value(&tool.input) {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                obj.remove("_tool");
            }
            serde_json::to_string(&v).unwrap_or_default()
        }
        Err(_) => String::new(),
    };
    if args.is_empty() || args == "{}" {
        return name.to_string();
    }
    // Truncate by char count rather than byte slice: respects UTF-8 boundaries
    // by construction and satisfies clippy::string_slice.
    if args.chars().count() > MAX_ARGS_CHARS {
        let truncated: String = args.chars().take(MAX_ARGS_CHARS).collect();
        format!("{name}: {truncated}… (truncated)")
    } else {
        format!("{name}: {args}")
    }
}

fn llm_error_to_db_error(kind: phoenix_llm::LlmErrorKind) -> crate::db::ErrorKind {
    // Explicit match arms — no catch-all. The compiler enforces exhaustiveness.
    match kind {
        phoenix_llm::LlmErrorKind::Auth => crate::db::ErrorKind::Auth,
        phoenix_llm::LlmErrorKind::RateLimit => crate::db::ErrorKind::RateLimit,
        phoenix_llm::LlmErrorKind::UsageLimitReached => crate::db::ErrorKind::UsageLimitReached,
        phoenix_llm::LlmErrorKind::Network => crate::db::ErrorKind::Network,
        phoenix_llm::LlmErrorKind::InvalidRequest => crate::db::ErrorKind::InvalidRequest,
        phoenix_llm::LlmErrorKind::InvalidResponse => crate::db::ErrorKind::InvalidResponse,
        phoenix_llm::LlmErrorKind::ServerError => crate::db::ErrorKind::ServerError,
        phoenix_llm::LlmErrorKind::ServerOverloaded => crate::db::ErrorKind::ServerOverloaded,
        phoenix_llm::LlmErrorKind::ContentFilter => crate::db::ErrorKind::ContentFilter,
        phoenix_llm::LlmErrorKind::ContextWindowExceeded => crate::db::ErrorKind::ContextExhausted,
    }
}

/// Convert an LLM error into a typed `LlmOutcome`.
/// Explicit match arms — the compiler enforces exhaustiveness.
fn llm_error_to_outcome(error: phoenix_llm::LlmError) -> LlmOutcome {
    use phoenix_llm::LlmErrorKind;
    match error.kind {
        LlmErrorKind::RateLimit => LlmOutcome::RateLimited {
            retry_after: None,
            // Thread `resets_at` from the upstream `QuotaDetails` when the
            // 429 response included one. Surfaces on `SseEvent::LlmAttempt`
            // so the retry suffix can show "(retry K/N after rate limit,
            // resets at HH:MM)" — specs/llm-retry-visibility/.
            resets_at: error.quota.as_ref().and_then(|q| q.resets_at),
        },
        LlmErrorKind::UsageLimitReached => {
            // The codex parser always attaches a QuotaDetails to UsageLimitReached
            // errors; fall back to an empty payload only as a defensive measure
            // in case a future caller forgets to populate it.
            let details = error.quota.map_or(
                phoenix_llm::QuotaDetails {
                    plan_type: None,
                    resets_at: None,
                    limit_id: None,
                    limit_name: None,
                    primary: None,
                    secondary: None,
                    credits: None,
                    promo_message: None,
                },
                |boxed| *boxed,
            );
            LlmOutcome::UsageLimitReached {
                details,
                message: error.message,
            }
        }
        LlmErrorKind::ServerError => LlmOutcome::ServerError {
            status: 500,
            body: error.message,
        },
        LlmErrorKind::InvalidResponse => LlmOutcome::InvalidResponse {
            message: error.message,
        },
        LlmErrorKind::ServerOverloaded => LlmOutcome::ServerOverloaded {
            message: error.message,
        },
        LlmErrorKind::Network => LlmOutcome::NetworkError {
            message: error.message,
        },
        LlmErrorKind::ContextWindowExceeded => LlmOutcome::TokenBudgetExceeded,
        LlmErrorKind::Auth => LlmOutcome::AuthError {
            message: error.message,
            recovery_in_progress: error.recovery_in_progress,
        },
        LlmErrorKind::InvalidRequest | LlmErrorKind::ContentFilter => LlmOutcome::RequestRejected {
            message: error.message,
        },
    }
}

#[cfg(test)]
mod continuation_prompt_tests {
    use super::*;
    use crate::state_machine::state::ToolInput;
    use crate::tools::BashToolInput;

    #[test]
    fn rejected_tool_call_includes_arguments_without_tool_discriminant() {
        let call = ToolCall::new("t1", ToolInput::Bash(BashToolInput::run("cargo test")));
        let rendered = render_rejected_tool_call(&call);
        assert!(rendered.starts_with("bash: "), "got: {rendered}");
        assert!(rendered.contains("cargo test"), "args missing: {rendered}");
        assert!(
            !rendered.contains("_tool"),
            "discriminant should be stripped: {rendered}"
        );
    }

    #[test]
    fn rejected_tool_call_truncates_oversized_arguments_on_char_boundary() {
        let big = "é".repeat(2000); // multi-byte chars stress the boundary walk
        let call = ToolCall::new("t1", ToolInput::Bash(BashToolInput::run(big)));
        let rendered = render_rejected_tool_call(&call);
        assert!(rendered.ends_with("… (truncated)"), "got tail: {rendered}");
        // Truncated near the char cap, not the full 2000-char payload, and
        // still valid UTF-8 (constructing it would have panicked otherwise).
        assert!(
            rendered.chars().count() < 600,
            "not truncated: {} chars",
            rendered.chars().count()
        );
    }

    #[test]
    fn continuation_prompt_lists_rejected_calls_and_drops_brevity_framing() {
        let calls = vec![ToolCall::new(
            "t1",
            ToolInput::Bash(BashToolInput::run("git push")),
        )];
        let prompt = build_continuation_prompt(&calls);
        assert!(prompt.contains("handoff"), "should frame as handoff");
        assert!(prompt.contains("git push"), "rejected args should appear");
        assert!(
            !prompt.to_lowercase().contains("brief") && !prompt.to_lowercase().contains("concise"),
            "brevity framing should be gone: {prompt}"
        );
    }
}

#[cfg(test)]
mod error_mapping_tests {
    use super::*;
    use phoenix_llm::LlmErrorKind;

    #[test]
    fn test_llm_error_to_db_error_mapping() {
        // Test all mappings are explicit and correct
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::Auth),
            crate::db::ErrorKind::Auth
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::RateLimit),
            crate::db::ErrorKind::RateLimit
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::Network),
            crate::db::ErrorKind::Network
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::InvalidRequest),
            crate::db::ErrorKind::InvalidRequest
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::ServerError),
            crate::db::ErrorKind::ServerError,
            "ServerError must map to ServerError"
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::InvalidResponse),
            crate::db::ErrorKind::InvalidResponse
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::ContentFilter),
            crate::db::ErrorKind::ContentFilter
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::ContextWindowExceeded),
            crate::db::ErrorKind::ContextExhausted
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::UsageLimitReached),
            crate::db::ErrorKind::UsageLimitReached
        );
        assert_eq!(
            llm_error_to_db_error(LlmErrorKind::ServerOverloaded),
            crate::db::ErrorKind::ServerOverloaded
        );
    }

    #[test]
    fn test_usage_limit_reached_is_terminal_after_mapping() {
        let db_kind = llm_error_to_db_error(LlmErrorKind::UsageLimitReached);
        assert!(
            !db_kind.is_auto_retryable(),
            "UsageLimitReached must NOT be retryable after mapping"
        );
        let db_kind = llm_error_to_db_error(LlmErrorKind::ServerOverloaded);
        assert!(
            !db_kind.is_auto_retryable(),
            "ServerOverloaded must NOT be retryable after mapping"
        );
    }

    #[test]
    fn test_server_error_is_retryable_after_mapping() {
        // This is the critical test - ServerError from LLM must be retryable
        let llm_error = LlmErrorKind::ServerError;
        let db_error = llm_error_to_db_error(llm_error);
        assert!(
            db_error.is_auto_retryable(),
            "ServerError must be retryable after mapping to db::ErrorKind"
        );
    }

    #[test]
    fn test_invalid_response_is_retryable_and_resumable_after_mapping() {
        // A malformed *response* (parse failure, unexpected block shape) is a
        // server/transport fault, not a bad request: it must stay both
        // auto-retryable and user-resumable so a transient blip does not
        // dead-end the conversation and discard already-billed output.
        let db_error = llm_error_to_db_error(LlmErrorKind::InvalidResponse);
        assert!(
            db_error.is_auto_retryable(),
            "InvalidResponse must be auto-retryable after mapping to db::ErrorKind"
        );
        assert!(
            db_error.is_user_resumable(),
            "InvalidResponse must be user-resumable after mapping to db::ErrorKind"
        );
    }

    #[test]
    fn test_invalid_response_outcome_is_not_request_rejected() {
        // RequestRejected is terminal/non-resumable; InvalidResponse must take
        // the retryable outcome path instead.
        let outcome =
            llm_error_to_outcome(phoenix_llm::LlmError::invalid_response("garbled SSE event"));
        assert!(
            matches!(outcome, LlmOutcome::InvalidResponse { .. }),
            "invalid_response must map to LlmOutcome::InvalidResponse, got {outcome:?}"
        );
    }
}

/// Shared git fixture helpers for the executor's worktree-aware test
/// modules. Lives next to those modules (rather than in
/// `runtime::testing`) because every consumer is in this file and the
/// helpers are deliberately tied to the on-disk layout
/// `create_managed_explore_worktree_blocking` produces.
#[cfg(test)]
mod test_git_helpers {
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    pub fn init_repo() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                // Don't depend on the host's commit-signing setup in tests.
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "init",
                "-q",
            ][..],
        ] {
            let s = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap();
            assert!(s.success(), "git {args:?} failed");
        }
        (tmp, root)
    }

    /// Add a worktree at `{repo}/.phoenix/worktrees/{id}` on a fresh
    /// `branch`. Used by tests that need a Work-mode worktree directly,
    /// skipping the Explore->Work promotion path.
    pub fn add_worktree(repo: &Path, id: &str, branch: &str) -> String {
        let wt = repo.join(".phoenix").join("worktrees").join(id);
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        let wt_s = wt.to_string_lossy().to_string();
        let s = std::process::Command::new("git")
            .args(["worktree", "add", "-b", branch, &wt_s])
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(s.success(), "git worktree add failed");
        wt_s
    }

    /// Add an Explore worktree at the canonical Phoenix path
    /// `{repo}/.phoenix/worktrees/{conv_id}` on a temp branch, exactly
    /// as `create_managed_explore_worktree_blocking` does in production.
    /// Used by tests that exercise the Explore->Work promotion path.
    pub fn add_explore_worktree(repo: &Path, conv_id: &str, base_branch: &str) -> PathBuf {
        let id_prefix: String = conv_id.chars().take(8).collect();
        let temp_branch = format!("task-pending-{id_prefix}");
        let wt = repo.join(".phoenix").join("worktrees").join(conv_id);
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        let s = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &temp_branch,
                wt.to_str().unwrap(),
                base_branch,
            ])
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(s.success(), "git worktree add failed");
        wt
    }

    pub fn branch_exists(repo: &Path, branch: &str) -> bool {
        let o = std::process::Command::new("git")
            .args(["branch", "--list", branch])
            .current_dir(repo)
            .output()
            .unwrap();
        !String::from_utf8_lossy(&o.stdout).trim().is_empty()
    }

    pub fn worktree_list(repo: &Path) -> String {
        String::from_utf8_lossy(
            &std::process::Command::new("git")
                .args(["worktree", "list", "--porcelain"])
                .current_dir(repo)
                .output()
                .unwrap()
                .stdout,
        )
        .to_string()
    }
}

/// Task 24696 Phase 3: verify the `Effect::NotifyContextExhausted` handler
/// preserves the worktree and does NOT demote `conv_mode`. The old
/// `cleanup_context_exhausted_worktree` path is gone — worktree handoff to a
/// continuation (REQ-BED-030) or a user-initiated abandon / mark-as-merged
/// are now the only ways a context-exhausted worktree is removed.
///
/// The effect is dispatched through the real `execute_effect` match arm
/// (not a private helper) so the handler's full transition-time behaviour
/// is exercised end-to-end.
#[cfg(test)]
mod context_exhausted_preserves_worktree_tests {
    use super::test_git_helpers::{add_worktree, branch_exists, init_repo};
    use super::*;
    use crate::db::{ConvMode, NonEmptyString};
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::state_machine::{ConvContext, Effect};
    use crate::tools::BrowserSessionManager;
    use phoenix_llm::ModelRegistry;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{broadcast, mpsc};

    #[allow(clippy::type_complexity)]
    fn build_runtime(
        storage: Arc<InMemoryStorage>,
        conv_id: &str,
        working_dir: PathBuf,
    ) -> (
        ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>,
        broadcast::Receiver<SseEvent>,
    ) {
        build_runtime_with_state(storage, conv_id, working_dir, ConvState::Idle)
    }

    #[allow(clippy::type_complexity)]
    fn build_runtime_with_state(
        storage: Arc<InMemoryStorage>,
        conv_id: &str,
        working_dir: PathBuf,
        initial_state: ConvState,
    ) -> (
        ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>,
        broadcast::Receiver<SseEvent>,
    ) {
        let context = ConvContext::new(conv_id, working_dir, "test-model", 200_000);
        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);
        let broadcast_rx = broadcaster.subscribe();

        let rt = ConversationRuntime::new(
            context,
            initial_state,
            storage,
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        );
        (rt, broadcast_rx)
    }

    /// Drain the broadcaster for up to `timeout` and return true if a
    /// `StateChange { state: ContextExhausted, .. }` is observed.
    async fn wait_for_context_exhausted_broadcast(
        rx: &mut broadcast::Receiver<SseEvent>,
        timeout: Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if let Ok(Ok(SseEvent::StateChange {
                state: ConvState::ContextExhausted { .. },
                ..
            })) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
            {
                return true;
            }
        }
        false
    }

    /// Task 24696 regression: Work-mode conversation reaching
    /// `ContextExhausted` MUST keep its worktree on disk, keep its
    /// `ConvMode::Work` in storage, and broadcast the state change.
    #[tokio::test]
    async fn work_mode_context_exhausted_preserves_worktree_and_mode() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "ctx-work-1";
        let branch = "task-42-fix-bug";
        let wt_path = add_worktree(&repo_root, conv_id, branch);

        let storage = Arc::new(InMemoryStorage::new());
        let original_mode = ConvMode::Work {
            branch_name: NonEmptyString::new(branch).unwrap(),
            worktree_path: NonEmptyString::new(&wt_path).unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("YF042").unwrap(),
            task_title: NonEmptyString::new("Fix bug").unwrap(),
        };
        storage.set_mode(conv_id, original_mode.clone());

        let (mut rt, mut rx) = build_runtime(storage.clone(), conv_id, PathBuf::from(&wt_path));

        // Drive the effect through the real dispatcher so the whole
        // `Effect::NotifyContextExhausted` arm fires, not a private helper.
        let gen = rt
            .execute_effect(Effect::NotifyContextExhausted {
                summary: "out of context".to_string(),
            })
            .await
            .expect("effect dispatch should not error");
        assert!(gen.is_none(), "notify effect generates no chained event");

        // Worktree directory still exists on disk.
        assert!(
            Path::new(&wt_path).exists(),
            "REQ-BED-031: worktree must be preserved on context exhaustion"
        );
        // Branch still exists.
        assert!(branch_exists(&repo_root, branch));
        // conv_mode in storage is untouched (still Work with same fields).
        assert_eq!(
            storage.get_mode(conv_id),
            Some(original_mode),
            "conv_mode must NOT be demoted to Explore anymore"
        );
        // The ContextExhausted StateChange SSE broadcast still fires.
        assert!(
            wait_for_context_exhausted_broadcast(&mut rx, Duration::from_secs(1)).await,
            "StateChange::ContextExhausted must still be broadcast"
        );
    }

    /// Branch-mode twin: same preservation semantics.
    #[tokio::test]
    async fn branch_mode_context_exhausted_preserves_worktree_and_mode() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "ctx-branch-1";
        let branch = "feature/pr-99";
        let wt_path = add_worktree(&repo_root, conv_id, branch);

        let storage = Arc::new(InMemoryStorage::new());
        let original_mode = ConvMode::Branch {
            branch_name: NonEmptyString::new(branch).unwrap(),
            worktree_path: NonEmptyString::new(&wt_path).unwrap(),
            base_branch: NonEmptyString::new(branch).unwrap(),
        };
        storage.set_mode(conv_id, original_mode.clone());

        let (mut rt, mut rx) = build_runtime(storage.clone(), conv_id, PathBuf::from(&wt_path));

        rt.execute_effect(Effect::NotifyContextExhausted {
            summary: "exhausted".to_string(),
        })
        .await
        .expect("effect dispatch should not error");

        assert!(
            Path::new(&wt_path).exists(),
            "Branch-mode worktree must survive context exhaustion"
        );
        assert!(branch_exists(&repo_root, branch));
        assert_eq!(
            storage.get_mode(conv_id),
            Some(original_mode),
            "Branch mode must NOT demote to Direct"
        );
        assert!(wait_for_context_exhausted_broadcast(&mut rx, Duration::from_secs(1)).await);
    }

    /// REQ-BED-031 regression: the executor's terminal-exit lifecycle hook
    /// (`emit_terminal_lifecycle_event` → `cleanup_worktree_if_present`) must
    /// NOT remove the worktree when the terminal state is `ContextExhausted`.
    ///
    /// The `Effect::NotifyContextExhausted` tests above only exercise the
    /// effect dispatcher; they do not drive the executor-loop exit path
    /// where `cleanup_worktree_if_present` is called. The original Phase 3
    /// commit (e82c1db) removed `cleanup_context_exhausted_worktree` but
    /// missed this sibling cleanup added in 4a94509 for Explore-mode leaks.
    /// As a result, every Work/Branch conversation that hit
    /// `ContextExhausted` had its worktree force-removed on executor exit,
    /// breaking the continuation handoff because the child inherits the
    /// parent's now-missing `worktree_path`.
    #[tokio::test]
    async fn terminal_exit_preserves_worktree_when_context_exhausted() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "term-exit-ctx-1";
        let branch = "task-99-preserve-me";
        let wt_path = add_worktree(&repo_root, conv_id, branch);

        let storage = Arc::new(InMemoryStorage::new());
        let original_mode = ConvMode::Work {
            branch_name: NonEmptyString::new(branch).unwrap(),
            worktree_path: NonEmptyString::new(&wt_path).unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("YF099").unwrap(),
            task_title: NonEmptyString::new("Preserve me").unwrap(),
        };
        storage.set_mode(conv_id, original_mode);

        let (rt, _rx) = build_runtime_with_state(
            storage,
            conv_id,
            PathBuf::from(&wt_path),
            ConvState::ContextExhausted {
                summary: "ran out of context".to_string(),
            },
        );

        // Fire the exact lifecycle hook the executor's `run()` loop calls
        // when it observes a terminal state.
        rt.emit_terminal_lifecycle_event().await;

        assert!(
            Path::new(&wt_path).exists(),
            "REQ-BED-031: terminal-exit cleanup must NOT remove a \
             ContextExhausted worktree — continuation transfer depends on it"
        );
        assert!(
            branch_exists(&repo_root, branch),
            "branch must also survive — worktree remove --force would have nuked it"
        );
    }

    /// Negative control: the original Explore-mode-leak intent of
    /// `cleanup_worktree_if_present` (commit 4a94509) is preserved.
    /// A non-context-exhausted terminal exit (here `ConvState::Terminal`,
    /// the post-abandon / post-mark-merged sink) still cleans up any
    /// stray worktree at `.phoenix/worktrees/{conv_id}`.
    #[tokio::test]
    async fn terminal_exit_still_cleans_up_non_context_exhausted_terminal() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "term-exit-stray-1";
        let branch = "stray-explore-branch";
        let wt_path = add_worktree(&repo_root, conv_id, branch);

        let storage = Arc::new(InMemoryStorage::new());
        let (rt, _rx) = build_runtime_with_state(
            storage,
            conv_id,
            PathBuf::from(&wt_path),
            ConvState::Terminal,
        );

        rt.emit_terminal_lifecycle_event().await;

        assert!(
            !Path::new(&wt_path).exists(),
            "Non-ContextExhausted terminal exit must still reap stray worktrees \
             (the original 4a94509 intent for Explore-mode leaks)"
        );
    }
}

// ============================================================
// CWD immutability: task 02702
// ============================================================
//
// Verifies that for Managed conversations the Explore worktree is promoted
// in place at approval time (branch renamed, same path returned) so
// conv.cwd never changes across the Explore→Work transition.

#[cfg(test)]
mod cwd_immutability_tests {
    use super::test_git_helpers::{add_explore_worktree, branch_exists, init_repo, worktree_list};
    use super::*;

    /// Core task-02702 regression:
    ///
    /// For a Managed conversation, `execute_approve_task_blocking` must
    /// detect the early Explore worktree and promote it in place (branch
    /// rename only). The returned `worktree_path` must equal the original
    /// Explore worktree path, so `conv.cwd` is unchanged at approval time.
    #[test]
    fn approve_task_returns_same_path_as_explore_worktree() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "test-conv-immutable-cwd";
        let base_branch = "main";

        // Simulate REQ-PROJ-028: create the early Explore worktree
        let explore_wt = add_explore_worktree(&repo_root, conv_id, base_branch);
        let explore_wt_str = explore_wt.to_string_lossy().to_string();

        // Stage a taskmd-1.0 task file in the worktree (the agent would
        // have created this via the patch tool before calling propose_task).
        let tasks_dir = explore_wt.join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        let task_filename = "12345-p2-ready--fix-the-login-bug.md";
        std::fs::write(
            tasks_dir.join(task_filename),
            "# Fix the login bug\n\n1. Investigate\n2. Fix\n3. Test\n",
        )
        .unwrap();

        let result = execute_approve_task_blocking(
            &explore_wt,
            &repo_root,
            conv_id,
            "tasks",
            &format!("tasks/{task_filename}"),
            "Fix the login bug",
            Some(base_branch),
        )
        .expect("approve_task_blocking failed");

        // The path must not change: Explore worktree promoted in place.
        assert_eq!(
            result.worktree_path, explore_wt_str,
            "worktree_path must equal original Explore worktree path; \
             a different value means a nested worktree was created"
        );

        // The temp branch must be gone, renamed to the task branch.
        let id_prefix: String = conv_id.chars().take(8).collect();
        let temp_branch = format!("task-pending-{id_prefix}");
        assert!(
            !branch_exists(&repo_root, &temp_branch),
            "temp branch {temp_branch} should have been renamed, not left behind"
        );
        assert!(
            branch_exists(&repo_root, &result.branch_name),
            "task branch {} must exist after approval",
            result.branch_name
        );

        // Only two worktree entries: the main checkout and the one Explore/Work
        // worktree. A nested worktree would show a third entry.
        let wt_list = worktree_list(&repo_root);
        let entry_count = wt_list
            .split("\n\n")
            .filter(|s| !s.trim().is_empty())
            .count();
        assert_eq!(
            entry_count, 2,
            "expected exactly 2 worktree entries (main + promoted worktree), got {entry_count}:\n{wt_list}"
        );

        // No nested .phoenix directory inside the worktree (the pre-fix symptom).
        assert!(
            !explore_wt.join(".phoenix").exists(),
            ".phoenix must not exist inside the worktree; \
             its presence means a nested worktree was created"
        );
    }
}

// ============================================================
// Plain-markdown task files (task 13009)
// ============================================================

#[cfg(test)]
mod plain_markdown_approval_tests {
    use super::test_git_helpers::{add_explore_worktree, branch_exists, init_repo};
    use super::*;

    fn git_show_head(cwd: &std::path::Path) -> String {
        String::from_utf8_lossy(
            &std::process::Command::new("git")
                .args(["log", "-1", "--name-only", "--pretty=%s"])
                .current_dir(cwd)
                .output()
                .unwrap()
                .stdout,
        )
        .to_string()
    }

    /// A Managed conversation can approve a plain-markdown task file (one whose
    /// name is not a taskmd filename). The temp branch is renamed to
    /// `task-{stem}-{conv-id-prefix}`, the file is committed at its own path,
    /// and no `...-ready--` -> `...-in-progress--` rename happens.
    #[test]
    fn approve_plain_markdown_task_file_in_early_worktree() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "plainconv-abcdef";
        let base_branch = "main";

        let explore_wt = add_explore_worktree(&repo_root, conv_id, base_branch);
        // The Explore-mode patch tool is restricted to `tasks/`, so a plain
        // task brief lands there too.
        let tasks_dir = explore_wt.join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        std::fs::write(tasks_dir.join("my-plan.md"), "# My plan\n\nDo the thing.\n").unwrap();

        let result = execute_approve_task_blocking(
            &explore_wt,
            &repo_root,
            conv_id,
            "tasks",
            "tasks/my-plan.md",
            "My plan",
            Some(base_branch),
        )
        .expect("approve_task_blocking failed for plain markdown");

        let conv_prefix: String = conv_id.chars().take(8).collect();
        assert_eq!(result.branch_name, format!("task-my-plan-{conv_prefix}"));
        assert_eq!(result.task_id, "my-plan");
        assert_eq!(result.task_title, "My plan");
        assert_eq!(result.worktree_path, explore_wt.to_string_lossy());
        assert!(branch_exists(&repo_root, &result.branch_name));
        assert!(
            !branch_exists(&repo_root, &format!("task-pending-{conv_prefix}")),
            "temp branch should have been renamed"
        );
        // No status-rename: the file keeps its original name.
        assert!(explore_wt.join("tasks/my-plan.md").exists());
        let head = git_show_head(&explore_wt);
        assert!(
            head.contains("task my-plan: My plan"),
            "commit subject: {head}"
        );
        assert!(head.contains("tasks/my-plan.md"), "committed files: {head}");
    }

    /// `propose_task` may point at any markdown file, including one outside the
    /// tasks dir — it is committed at its own path.
    #[test]
    fn approve_plain_markdown_outside_tasks_dir() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "docsconv-99887766";
        let explore_wt = add_explore_worktree(&repo_root, conv_id, "main");
        std::fs::create_dir_all(explore_wt.join("docs")).unwrap();
        std::fs::write(explore_wt.join("docs/plan.md"), "# Doc plan\n").unwrap();

        let result = execute_approve_task_blocking(
            &explore_wt,
            &repo_root,
            conv_id,
            "tasks",
            "docs/plan.md",
            "Doc plan",
            Some("main"),
        )
        .expect("approve failed for docs/plan.md");

        let conv_prefix: String = conv_id.chars().take(8).collect();
        assert_eq!(result.branch_name, format!("task-plan-{conv_prefix}"));
        let head = git_show_head(&explore_wt);
        assert!(head.contains("docs/plan.md"), "committed files: {head}");
    }

    /// Two conversations proposing files with the same stem must get distinct
    /// branch names — the conversation-id suffix is the uniquifier.
    #[test]
    fn plain_markdown_branches_distinct_across_conversations() {
        let (_tmp, repo_root) = init_repo();
        for conv_id in ["aaaaaaaa-conv-1", "bbbbbbbb-conv-2"] {
            let explore_wt = add_explore_worktree(&repo_root, conv_id, "main");
            std::fs::create_dir_all(explore_wt.join("tasks")).unwrap();
            std::fs::write(explore_wt.join("tasks/feature.md"), "# Feature\n").unwrap();
            let r = execute_approve_task_blocking(
                &explore_wt,
                &repo_root,
                conv_id,
                "tasks",
                "tasks/feature.md",
                "Feature",
                Some("main"),
            )
            .expect("approve failed");
            let prefix: String = conv_id.chars().take(8).collect();
            assert_eq!(r.branch_name, format!("task-feature-{prefix}"));
        }
        assert!(branch_exists(&repo_root, "task-feature-aaaaaaaa"));
        assert!(branch_exists(&repo_root, "task-feature-bbbbbbbb"));
    }

    /// `propose_task` may point at a file that already exists on the base branch
    /// and that the agent didn't (couldn't) modify — approval still succeeds, it
    /// just doesn't create an empty commit; the task branch == the base branch.
    #[test]
    fn approve_plain_markdown_unchanged_existing_file_skips_commit() {
        let (_tmp, repo_root) = init_repo();
        // Put docs/plan.md (and a .gitignore that already lists .phoenix/, so
        // ensure_gitignore_has_phoenix is a no-op in the worktree) on `main`.
        std::fs::create_dir_all(repo_root.join("docs")).unwrap();
        std::fs::write(repo_root.join("docs/plan.md"), "# Existing plan\n").unwrap();
        std::fs::write(repo_root.join(".gitignore"), ".phoenix/\n").unwrap();
        for args in [
            &["add", "docs/plan.md", ".gitignore"][..],
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "add plan",
                "-q",
            ][..],
        ] {
            let s = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo_root)
                .status()
                .unwrap();
            assert!(s.success(), "git {args:?} failed");
        }
        let conv_id = "existconv-12345678";
        let explore_wt = add_explore_worktree(&repo_root, conv_id, "main");
        assert!(explore_wt.join("docs/plan.md").exists());

        let result = execute_approve_task_blocking(
            &explore_wt,
            &repo_root,
            conv_id,
            "tasks",
            "docs/plan.md",
            "Existing plan",
            Some("main"),
        )
        .expect("approve should succeed even with nothing to commit");

        let conv_prefix: String = conv_id.chars().take(8).collect();
        assert_eq!(result.branch_name, format!("task-plan-{conv_prefix}"));
        let rev = |r: &str| {
            String::from_utf8_lossy(
                &std::process::Command::new("git")
                    .args(["rev-parse", r])
                    .current_dir(&repo_root)
                    .output()
                    .unwrap()
                    .stdout,
            )
            .trim()
            .to_string()
        };
        assert_eq!(
            rev(&result.branch_name),
            rev("main"),
            "no empty commit should have been created"
        );
    }
}

// ============================================================
// Explore prompt cache shape across tool loops
// ============================================================

#[cfg(test)]
mod explore_prompt_cache_shape_tests {
    use super::*;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient};
    use crate::runtime::traits::ToolExecutor;
    use crate::state_machine::{ConvContext, Event};
    use crate::system_prompt::{snapshot_next_taskmd_id_hint, ModeContext};
    use crate::tools::{BrowserSessionManager, ToolContext, ToolOutput};
    use async_trait::async_trait;
    use phoenix_llm::{ContentBlock, LlmResponse, ModelRegistry, ToolDefinition, Usage};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    struct TaskCreatingPatchExecutor {
        task_path: PathBuf,
    }

    #[async_trait]
    impl ToolExecutor for TaskCreatingPatchExecutor {
        async fn execute(
            &self,
            call: crate::runtime::deny_gate::CheckedToolCall,
            _ctx: ToolContext,
        ) -> Option<ToolOutput> {
            let (name, _input) = call.into_parts();
            if name != "patch" {
                return None;
            }
            std::fs::write(&self.task_path, "# Draft\n").unwrap();
            Some(ToolOutput::success("created task draft"))
        }

        async fn definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "patch".to_string(),
                description: "Mock patch".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                defer_loading: false,
            }]
        }
    }

    #[tokio::test]
    async fn explore_tool_loop_keeps_system_prompt_and_cache_key_stable_after_task_file_write() {
        let temp = TempDir::new().unwrap();
        let tasks_dir = temp.path().join("tasks");
        std::fs::create_dir(&tasks_dir).unwrap();
        std::fs::write(
            tasks_dir.join(taskmd_core::constants::TEMPLATE_FILENAME),
            "# Task Title\n",
        )
        .unwrap();
        let hinted_id = snapshot_next_taskmd_id_hint(temp.path(), "tasks")
            .expect("taskmd marker should produce hint")
            .to_string();

        let conv_id = "explore-cache-shape";
        let mut context =
            ConvContext::new(conv_id, temp.path().to_path_buf(), "test-model", 200_000);
        context.mode_context = Some(ModeContext::Explore {
            next_taskmd_id_hint: Some(hinted_id.clone()),
        });
        context.tasks_dir_name = "tasks".to_string();

        let storage = Arc::new(InMemoryStorage::new());
        let llm = Arc::new(MockLlmClient::new("test-model"));
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-patch-1".to_string(),
                name: "patch".to_string(),
                input: serde_json::json!({
                    "path": format!("tasks/{hinted_id}-p2-ready--draft.md"),
                    "patches": [{"operation": "overwrite", "newText": "# Draft\\n"}]
                }),
            }],
            end_turn: false,
            usage: Usage::default(),
        });
        llm.queue_response(LlmResponse {
            content: vec![ContentBlock::text("ready to propose")],
            end_turn: true,
            usage: Usage::default(),
        });

        let (event_tx, runtime_event_rx) = mpsc::channel(32);
        let broadcaster = SseBroadcaster::new(128, 0);
        let runtime = ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage,
            llm.clone(),
            Arc::new(TaskCreatingPatchExecutor {
                task_path: tasks_dir.join(format!("{hinted_id}-p2-ready--draft.md")),
            }),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            runtime_event_rx,
            event_tx.clone(),
            broadcaster,
        );
        let runtime_handle = tokio::spawn(async move { runtime.run().await });

        event_tx
            .send(Event::UserMessage {
                text: "draft a task".to_string(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: "user-msg-1".to_string(),
                user_agent: None,
                skill_invocation: None,
            })
            .await
            .unwrap();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if llm.recorded_requests().len() >= 2 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for two LLM requests"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let requests = llm.recorded_requests();
        assert!(tasks_dir
            .join(format!("{hinted_id}-p2-ready--draft.md"))
            .is_file());
        assert_eq!(requests[0].system.len(), requests[1].system.len());
        assert_eq!(requests[0].system[0].text, requests[1].system[0].text);
        assert_eq!(
            requests[0].cache_key.as_str(),
            requests[1].cache_key.as_str()
        );
        assert_eq!(requests[0].cache_key.as_str(), conv_id);

        runtime_handle.abort();
    }
}

// ============================================================
// Mode-context refresh on Explore -> Work promotion
// ============================================================
//
// Regression test for task 03002: after a Managed conversation
// approves its task and is promoted to Work mode, the in-memory
// `context.mode_context` must reflect Work (not the stale Explore
// value from runtime startup). The `spawn_agents` Work-parent guard
// reads this field; a stale Explore value rejects legitimate
// `mode: "work"` sub-agent requests from a Work-mode parent.

#[cfg(test)]
mod approve_task_refreshes_mode_context_tests {
    use super::test_git_helpers::{add_explore_worktree, init_repo};
    use super::*;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::state_machine::{ConvContext, ConvState, Effect};
    use crate::system_prompt::ModeContext;
    use crate::tools::BrowserSessionManager;
    use phoenix_llm::ModelRegistry;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    // End-to-end approval flow: repo setup, task creation, approval, and
    // post-conditions read clearer as one linear scenario than split apart.
    #[allow(clippy::too_many_lines)]
    async fn approve_task_sets_mode_context_to_work() {
        let (_tmp, repo_root) = init_repo();
        let conv_id = "mode-ctx-refresh-1";
        let base_branch = "main";

        let explore_wt = add_explore_worktree(&repo_root, conv_id, base_branch);
        let tasks_dir = explore_wt.join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        let task_filename = "12345-p2-ready--spawn-work-subagents.md";
        std::fs::write(
            tasks_dir.join(task_filename),
            "# Spawn work subagents\n\n1. Plan\n2. Spawn\n",
        )
        .unwrap();

        let storage = Arc::new(InMemoryStorage::new());
        let mut context = ConvContext::new(conv_id, explore_wt.clone(), "test-model", 200_000);
        context.mode_context = Some(ModeContext::Explore {
            next_taskmd_id_hint: None,
        });
        context.desired_base_branch = Some(base_branch.to_string());
        // Pre-approval Explore owns no scope-defining worktree (the
        // sub-agent-Explore shape that keys tool resources under
        // `WorkScope::Conversation`). Approval must refresh this cache.
        context.work_scope_worktree = None;

        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);

        // Seed a bash handle under the pre-approval conversation scope —
        // the sub-agent-Explore shape keys resources under
        // `WorkScope::Conversation(conv_id)`. After approval it must be
        // reachable under the worktree scope (and gone from the old scope).
        let bash_handles = Arc::new(crate::tools::BashHandleRegistry::new());
        let old_scope = crate::work_scope::WorkScope::Conversation(conv_id.to_string());
        {
            use phoenix_tools::bash::handle::{Handle, HandleId};
            use phoenix_tools::bash::ring::RING_BUFFER_BYTES;
            let table = bash_handles.get_or_create(&old_scope).await;
            table.write().await.insert(Handle::new_live(
                old_scope.clone(),
                HandleId::new("b-1"),
                "echo pre-approval".to_string(),
                None,
                12345,
                12345,
                RING_BUFFER_BYTES,
            ));
        }

        let mut rt = ConversationRuntime::new(
            context,
            ConvState::AwaitingTaskApproval {
                task_file: format!("tasks/{task_filename}"),
                title: "Spawn work subagents".to_string(),
                priority: crate::task_source::Priority::P2,
                plan: "Plan and spawn".to_string(),
            },
            storage,
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            bash_handles.clone(),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        );

        rt.execute_effect(Effect::ApproveTask {
            task_file: format!("tasks/{task_filename}"),
            title: "Spawn work subagents".to_string(),
            priority: crate::task_source::Priority::P2,
            plan: "Plan and spawn".to_string(),
        })
        .await
        .expect("approve task effect failed");

        match rt.context.mode_context.as_ref() {
            Some(ModeContext::Work {
                branch_name,
                base_branch: bb,
                worktree_path,
            }) => {
                assert!(
                    !branch_name.is_empty(),
                    "branch_name must be populated post-approval"
                );
                assert_eq!(bb, base_branch, "base_branch must round-trip");
                assert_eq!(
                    worktree_path,
                    &explore_wt.to_string_lossy().to_string(),
                    "worktree_path must equal the in-place-promoted Explore worktree"
                );
            }
            other => panic!(
                "mode_context must be refreshed to Work after approval; got {other:?}. \
                 Stale Explore here is the task-03002 bug: spawn_agents rejects \
                 mode: \"work\" sub-agents because parent_allows_work is false."
            ),
        }

        // The cached scope-defining worktree must follow the promotion: a
        // stale `None` would key in-runtime tool resources under
        // `WorkScope::Conversation` while DB-facing cleanup resolves
        // `WorkScope::Worktree`, splitting the panel/cleanup until restart.
        assert_eq!(
            rt.context.work_scope_worktree.as_deref(),
            Some(explore_wt.as_path()),
            "work_scope_worktree must be refreshed to the post-approval Work worktree"
        );

        // The pre-approval bash handle, opened under the conversation scope,
        // must follow the scope flip: reachable under the new worktree scope
        // and gone from the old conversation scope. Without the rekey it would
        // be orphaned — invisible to the inventory and reapable by the idle
        // reaper as an abandoned conversation scope.
        let new_scope =
            crate::work_scope::WorkScope::Worktree(explore_wt.to_string_lossy().into_owned());
        assert!(
            bash_handles.get_existing(&old_scope).await.is_none(),
            "old conversation scope must be empty after approval rekey"
        );
        let migrated = bash_handles
            .get_existing(&new_scope)
            .await
            .expect("bash handle table must be reachable under the new worktree scope");
        assert!(
            migrated
                .read()
                .await
                .get(&phoenix_tools::bash::handle::HandleId::new("b-1"))
                .is_some(),
            "the pre-approval handle must be present under the worktree scope"
        );
    }
}

// ============================================================
// Steering queue multi-drain detectors (Phase 2)
// ============================================================
//
// These tests exercise the executor-level drain logic in
// `apply_transition_result` for `SteerDrainedUserMessages`. They drive
// synthetic `TransitionResult`s so the detectors are isolated from the
// transition machinery (which is tested separately in
// state_machine/transition.rs).

#[cfg(test)]
mod steer_drain_detector_tests {
    use super::*;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::state_machine::event::SteerEntry;
    use crate::state_machine::state::{
        AssistantMessage, PendingSubAgent, SubAgentMode, ToolCall, ToolInput,
    };
    use crate::state_machine::transition::TransitionResult;
    use crate::state_machine::ConvContext;
    use crate::tools::BrowserSessionManager;
    use phoenix_llm::ModelRegistry;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn mk_entry(id: &str, text: &str) -> SteerEntry {
        SteerEntry {
            text: text.to_string(),
            llm_text: None,
            images: vec![],
            files: vec![],
            message_id: id.to_string(),
            user_agent: None,
            skill_invocation: None,
        }
    }

    fn mk_tool_executing() -> ConvState {
        ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "tool-1",
                ToolInput::Bash(crate::tools::BashToolInput::run("echo hi")),
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::default(),
        }
    }

    fn mk_awaiting_sub_agents() -> ConvState {
        ConvState::AwaitingSubAgents {
            pending: vec![PendingSubAgent {
                agent_id: "sub-1".to_string(),
                task: "do thing".to_string(),
                mode: SubAgentMode::Work,
            }],
            completed_results: vec![],
            spawn_tool_id: None,
        }
    }

    #[allow(clippy::type_complexity)]
    fn build_runtime_with_state_and_queue(
        conv_id: &str,
        initial_state: ConvState,
        queue: Vec<SteerEntry>,
    ) -> (
        ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>,
        Arc<InMemoryStorage>,
    ) {
        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new(conv_id, PathBuf::from("/tmp"), "test-model", 200_000);
        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);

        let rt = ConversationRuntime::new(
            context,
            initial_state,
            storage.clone(),
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        )
        .with_steering_queue(queue);
        (rt, storage)
    }

    /// Filter generated events down to `SteerDrainedUserMessages` payloads.
    fn extract_steer_drain_entries(events: &[Event]) -> Vec<Vec<SteerEntry>> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::SteerDrainedUserMessages { entries } => Some(entries.clone()),
                _ => None,
            })
            .collect()
    }

    /// Drain-all on entering `Idle`: from `LlmRequesting` → `Idle` with 3 queued
    /// entries, the executor must emit one `SteerDrainedUserMessages` carrying
    /// all 3 entries and clear in-memory `self.steering_queue`. DB queue clear
    /// is the `Effect::ClearSteeringQueueEntries` arm's job, covered by
    /// `clear_steering_queue_entries_preserves_concurrent_enqueue`.
    /// Drain-all on entering Idle is processed INLINE: persists land before
    /// `apply_transition_result` returns. Assertion is via storage rather than
    /// `generated_events` because the drain event no longer surfaces externally.
    #[tokio::test]
    async fn drain_all_on_entering_idle() {
        let queue = vec![
            mk_entry("s1", "first"),
            mk_entry("s2", "second"),
            mk_entry("s3", "third"),
        ];
        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-drain-idle",
            ConvState::LlmRequesting { attempt: 1 },
            queue,
        );

        let result = TransitionResult::new(ConvState::Idle);
        rt.apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        // Persists ran inline → messages now in storage in FIFO order.
        let msgs = storage.get_all_messages("conv-drain-idle");
        let persisted_ids: Vec<&str> = msgs.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(persisted_ids, vec!["s1", "s2", "s3"]);

        assert!(
            rt.steering_queue.is_empty(),
            "in-memory queue must be empty"
        );
        // Drain transition lands in LlmRequesting (Idle → LlmRequesting via the
        // SteerDrainedUserMessages arm).
        assert!(matches!(rt.state, ConvState::LlmRequesting { .. }));
    }

    /// Task 60004: entering Idle with a non-empty steering queue must NOT
    /// broadcast an intermediate `StateChange { Idle }` before the drain's
    /// `StateChange { LlmRequesting }`. The Idle state is still persisted to
    /// the DB; only its SSE broadcast is suppressed.
    #[tokio::test]
    async fn entering_idle_with_queued_steer_suppresses_idle_state_change() {
        let queue = vec![mk_entry("s1", "first")];
        let (mut rt, _storage) = build_runtime_with_state_and_queue(
            "conv-no-flicker",
            ConvState::LlmRequesting { attempt: 1 },
            queue,
        );

        let mut rx = rt.broadcast_tx.subscribe();

        // Original transition LlmRequesting -> Idle carries a PersistState
        // effect (the usual turn-end shape).
        let result = TransitionResult::new(ConvState::Idle)
            .with_effect(crate::state_machine::effect::Effect::PersistState);
        rt.apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        let mut saw_idle_state_change = false;
        let mut saw_llm_requesting_state_change = false;
        while let Ok(ev) = rx.try_recv() {
            if let SseEvent::StateChange { state, .. } = ev {
                match state {
                    ConvState::Idle => saw_idle_state_change = true,
                    ConvState::LlmRequesting { .. } => {
                        saw_llm_requesting_state_change = true;
                    }
                    _ => {}
                }
            }
        }

        assert!(
            !saw_idle_state_change,
            "intermediate Idle StateChange must be suppressed during inline drain"
        );
        assert!(
            saw_llm_requesting_state_change,
            "drain must still broadcast the authoritative LlmRequesting StateChange"
        );
        assert!(matches!(rt.state, ConvState::LlmRequesting { .. }));
    }

    /// Mid-turn drain from `ToolExecuting` → `LlmRequesting`: persists run
    /// inline before the (deferred) `RequestLlm`, so the spawned LLM task reads
    /// a DB that already has the steered messages.
    #[tokio::test]
    async fn drain_all_mid_turn_from_tool_executing() {
        let queue = vec![mk_entry("s1", "one"), mk_entry("s2", "two")];
        let (mut rt, storage) =
            build_runtime_with_state_and_queue("conv-drain-tool", mk_tool_executing(), queue);

        let result = TransitionResult::new(ConvState::LlmRequesting { attempt: 1 });
        rt.apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        let msgs = storage.get_all_messages("conv-drain-tool");
        let persisted_ids: Vec<&str> = msgs.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(persisted_ids, vec!["s1", "s2"]);
        assert!(rt.steering_queue.is_empty());
        assert!(matches!(rt.state, ConvState::LlmRequesting { .. }));
    }

    /// Mid-turn drain with `RequestLlm` in the original effects list: the
    /// executor must defer `RequestLlm` so persists land before the LLM task
    /// reads the DB. This is a smoke test that the deferred-RequestLlm path
    /// runs without panic, persists land, and final state is `LlmRequesting`.
    /// Stronger ordering is enforced by `apply_transition_result`'s sequential
    /// awaits (persists before deferred `RequestLlm` spawn).
    #[tokio::test]
    async fn mid_turn_drain_defers_request_llm_until_after_persists() {
        let queue = vec![mk_entry("s1", "first"), mk_entry("s2", "second")];
        let (mut rt, storage) =
            build_runtime_with_state_and_queue("conv-defer-req", mk_tool_executing(), queue);

        // Queue an LLM response so dispatch_llm_request can complete cleanly.
        // We don't assert on the response — just that the pipeline doesn't panic
        // and the persists landed before RequestLlm was dispatched.
        rt.llm_client.queue_response(phoenix_llm::LlmResponse {
            content: vec![],
            end_turn: true,
            usage: phoenix_llm::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        });

        let result = TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
            .with_effect(Effect::RequestLlm);
        rt.apply_transition_result(result)
            .await
            .expect("apply_transition_result with deferred RequestLlm must succeed");

        // Persists landed before the LLM task spawn (sequential await ordering).
        let msgs = storage.get_all_messages("conv-defer-req");
        let persisted_ids: Vec<&str> = msgs.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(persisted_ids, vec!["s1", "s2"]);
        assert!(matches!(rt.state, ConvState::LlmRequesting { .. }));
    }

    /// Mid-turn drain from `AwaitingSubAgents` → `LlmRequesting`.
    #[tokio::test]
    async fn drain_all_mid_turn_from_awaiting_subagents() {
        let queue = vec![mk_entry("s1", "alpha")];
        let (mut rt, storage) =
            build_runtime_with_state_and_queue("conv-drain-sub", mk_awaiting_sub_agents(), queue);

        let result = TransitionResult::new(ConvState::LlmRequesting { attempt: 1 });
        rt.apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        let msgs = storage.get_all_messages("conv-drain-sub");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].message_id, "s1");
        assert!(rt.steering_queue.is_empty());
    }

    /// Fix B/C: `persist_sub_agent_results(_, None)` must persist a NON-tool
    /// message. A fabricated `tool_result` (random id, no matching `tool_use`) is an
    /// orphan the provider rejects; the `None` branch must instead emit an
    /// assistant text summary that renders into LLM history. This pins that the
    /// orphan branch is gone.
    #[tokio::test]
    async fn persist_sub_agent_results_none_emits_non_tool_message() {
        use crate::db::MessageContent;

        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-persist-none",
            ConvState::LlmRequesting { attempt: 1 },
            vec![],
        );

        let results = vec![SubAgentResult {
            agent_id: "a".to_string(),
            task: "investigate".to_string(),
            outcome: SubAgentOutcome::TimedOut,
        }];
        rt.persist_sub_agent_results(results, None)
            .await
            .expect("persist must succeed");

        let msgs = storage.get_all_messages("conv-persist-none");
        assert_eq!(msgs.len(), 1, "exactly one summary message persisted");
        let MessageContent::User(user_content) = &msgs[0].content else {
            panic!(
                "expected a non-tool meta-User message, got {:?} (a tool_result here would be an orphan)",
                msgs[0].content.message_type()
            );
        };
        let rendered = render_messages(&msgs, &std::collections::HashSet::new());
        assert_eq!(rendered.len(), 1, "the summary must reach LLM history");
        assert_eq!(
            rendered[0].role,
            MessageRole::User,
            "delivered as a user observation so the model RESPONDS to the results rather than continuing them"
        );
        let text = format!("{user_content:?}");
        assert!(
            text.contains("Timed out") && text.contains("investigate"),
            "the summary must carry the per-result outcome, got: {text}"
        );
    }

    /// Regression (task 61005): a `SubAgentResult` buffered from an earlier
    /// batch in the same awaiting-round must survive a subsequent
    /// `spawn_agents` dispatch and be delivered when the parent enters
    /// `AwaitingSubAgents`. The bug: `handle_spawn_agents_tool` reassigned
    /// `sub_agent_result_buffer = Vec::with_capacity(..)` on every dispatch,
    /// destroying any result buffered while the parent was still running a
    /// prior tool (e.g. tool sequence `[spawn_agents A, bash, spawn_agents B]`,
    /// where A's result arrives during `bash`). The discarded result left its
    /// agent stuck `pending`, stalling the conversation for the full sub-agent
    /// timeout (~20 min) before a synthetic `TimedOut`. The fix uses `reserve`,
    /// which hints capacity without clearing buffered contents.
    #[tokio::test]
    async fn buffered_result_survives_second_spawn_dispatch() {
        // Parent is mid-round running a non-spawn tool, so it cannot yet handle
        // SubAgentResult events — they get buffered.
        let (mut rt, _storage) =
            build_runtime_with_state_and_queue("conv-buf-survive", mk_tool_executing(), vec![]);
        assert!(!rt.can_handle_sub_agent_result());

        // Batch A's agent ("sub-1") completes while the parent is still running
        // `bash`. The result is buffered, not processed.
        rt.process_event(Event::SubAgentResult {
            agent_id: "sub-1".to_string(),
            outcome: SubAgentOutcome::Success {
                result: "A's real work".to_string(),
            },
        })
        .await
        .expect("buffering a SubAgentResult must not error");
        assert_eq!(
            rt.sub_agent_result_buffer.len(),
            1,
            "earlier-batch result should be buffered while parent is not awaiting"
        );

        // A second `spawn_agents B` dispatch hints capacity for its own batch.
        // This is exactly the operation that previously reassigned (cleared)
        // the buffer; with the fix it must preserve the buffered batch-A result.
        rt.sub_agent_result_buffer.reserve(3);
        assert_eq!(
            rt.sub_agent_result_buffer.len(),
            1,
            "spawn dispatch must not discard the earlier-batch buffered result"
        );

        // Parent now enters AwaitingSubAgents (still pending on sub-1). The drain
        // must deliver batch-A's result as a generated event.
        let generated = rt
            .apply_transition_result(TransitionResult::new(mk_awaiting_sub_agents()))
            .await
            .expect("entering AwaitingSubAgents must succeed");

        let drained: Vec<&Event> = generated
            .iter()
            .filter(|e| matches!(e, Event::SubAgentResult { .. }))
            .collect();
        assert_eq!(
            drained.len(),
            1,
            "the buffered batch-A result must be drained on entering AwaitingSubAgents"
        );
        match drained[0] {
            Event::SubAgentResult {
                agent_id,
                outcome: SubAgentOutcome::Success { result },
            } => {
                assert_eq!(agent_id, "sub-1");
                assert_eq!(result, "A's real work");
            }
            other => panic!("unexpected drained event: {other:?}"),
        }

        // No cross-round leak: the drain (`mem::take`) empties the buffer, so the
        // next genuine awaiting-round starts clean.
        assert!(
            rt.sub_agent_result_buffer.is_empty(),
            "buffer must be empty after draining — no leak across rounds"
        );
    }

    /// Build an `AwaitingSubAgents` state whose single pending agent has the
    /// given `agent_id` (for cross-round drain tests).
    fn mk_awaiting_with_pending(agent_id: &str) -> ConvState {
        ConvState::AwaitingSubAgents {
            pending: vec![PendingSubAgent {
                agent_id: agent_id.to_string(),
                task: "do thing".to_string(),
                mode: SubAgentMode::Work,
            }],
            completed_results: vec![],
            spawn_tool_id: None,
        }
    }

    /// Finding #3: a late `SubAgentResult` from a COMPLETED round (a timed-out
    /// agent whose real result lands after the parent left `AwaitingSubAgents`)
    /// must NOT be drained into the NEXT round. The buffer-drain keeps only
    /// results whose agent is pending in the entering round and discards the
    /// rest, so the stale result can neither misapply to the wrong round nor
    /// reject the round-entry transition.
    #[tokio::test]
    async fn late_result_from_completed_round_is_discarded_not_applied_to_next_round() {
        // Parent is mid-round running a non-spawn tool, so it cannot handle
        // SubAgentResult events — they get buffered.
        let (mut rt, _storage) =
            build_runtime_with_state_and_queue("conv-late-discard", mk_tool_executing(), vec![]);
        assert!(!rt.can_handle_sub_agent_result());

        // Round R's agent "old-agent" timed out earlier; its real result arrives
        // now, after round R settled. It is buffered (parent not awaiting).
        rt.process_event(Event::SubAgentResult {
            agent_id: "old-agent".to_string(),
            outcome: SubAgentOutcome::Success {
                result: "late work from a dead round".to_string(),
            },
        })
        .await
        .expect("buffering a late SubAgentResult must not error");
        assert_eq!(rt.sub_agent_result_buffer.len(), 1);

        // The parent now opens round R+1 awaiting a DIFFERENT agent
        // ("new-agent"). The buffered late result belongs to "old-agent", which
        // is not pending in this round → it must be discarded, not drained.
        let generated = rt
            .apply_transition_result(TransitionResult::new(mk_awaiting_with_pending("new-agent")))
            .await
            .expect("entering AwaitingSubAgents must succeed");

        let drained: Vec<&Event> = generated
            .iter()
            .filter(|e| matches!(e, Event::SubAgentResult { .. }))
            .collect();
        assert!(
            drained.is_empty(),
            "a completed-round result must not be drained into the next round, got {drained:?}"
        );
        assert!(
            rt.sub_agent_result_buffer.is_empty(),
            "the buffer must be empty after the drain discards the stale result"
        );
        // The new round is intact and still awaiting its own agent.
        assert!(
            matches!(rt.state, ConvState::AwaitingSubAgents { .. }),
            "the next round's entry transition must not be rejected, got {:?}",
            rt.state.variant_name()
        );
    }

    /// Finding #3 (no-regress of H1): a genuine EARLY result for the CURRENT
    /// round still drains even when a stale completed-round result is buffered
    /// alongside it. The current-round result survives; the stale one is dropped.
    #[tokio::test]
    async fn early_current_round_result_drains_while_stale_is_discarded() {
        let (mut rt, _storage) =
            build_runtime_with_state_and_queue("conv-mixed-buf", mk_tool_executing(), vec![]);

        // A stale completed-round result for "old-agent".
        rt.process_event(Event::SubAgentResult {
            agent_id: "old-agent".to_string(),
            outcome: SubAgentOutcome::Success {
                result: "stale".to_string(),
            },
        })
        .await
        .expect("buffering stale result must not error");

        // An EARLY result for THIS round's agent "cur-agent", buffered while the
        // parent was still running a prior tool.
        rt.process_event(Event::SubAgentResult {
            agent_id: "cur-agent".to_string(),
            outcome: SubAgentOutcome::Success {
                result: "current-round work".to_string(),
            },
        })
        .await
        .expect("buffering current-round result must not error");
        assert_eq!(rt.sub_agent_result_buffer.len(), 2);

        // Enter the round awaiting "cur-agent". Only the current-round result
        // drains; the stale one is discarded.
        let generated = rt
            .apply_transition_result(TransitionResult::new(mk_awaiting_with_pending("cur-agent")))
            .await
            .expect("entering AwaitingSubAgents must succeed");

        let drained: Vec<&Event> = generated
            .iter()
            .filter(|e| matches!(e, Event::SubAgentResult { .. }))
            .collect();
        assert_eq!(
            drained.len(),
            1,
            "exactly the current-round result must drain, got {drained:?}"
        );
        match drained[0] {
            Event::SubAgentResult {
                agent_id,
                outcome: SubAgentOutcome::Success { result },
            } => {
                assert_eq!(agent_id, "cur-agent");
                assert_eq!(result, "current-round work");
            }
            other => panic!("unexpected drained event: {other:?}"),
        }
        assert!(
            rt.sub_agent_result_buffer.is_empty(),
            "buffer empty after drain — stale entry discarded, current entry drained"
        );
    }

    /// Build a `CancellingSubAgents` state pending on the given agent ids (all
    /// Work mode) with the given cause.
    fn mk_cancelling_sub_agents(
        agent_ids: &[&str],
        cause: crate::state_machine::event::CancelCause,
    ) -> ConvState {
        ConvState::CancellingSubAgents {
            pending: agent_ids
                .iter()
                .map(|id| PendingSubAgent {
                    agent_id: (*id).to_string(),
                    task: format!("task {id}"),
                    mode: SubAgentMode::Work,
                })
                .collect(),
            completed_results: vec![],
            cause,
            // Default to a real spawn id so the last-one drain still persists
            // results (exercising the common AwaitingSubAgents-origin path).
            spawn_tool_id: Some("spawn-1".to_string()),
        }
    }

    /// Test 4 (part A): the one-writer reservation is released ONLY when a
    /// `SubAgentResult` for the in-flight Work agent is actually processed — not
    /// merely because the parent is in `CancellingSubAgents`. Seed the counter at
    /// 1 (a Work agent is in flight), process its result, confirm the counter
    /// drops to 0.
    #[tokio::test]
    async fn one_writer_released_on_confirmed_stop() {
        let (mut rt, _storage) = build_runtime_with_state_and_queue(
            "conv-onewriter-release",
            mk_cancelling_sub_agents(
                &["w1"],
                crate::state_machine::event::CancelCause::UserRequested,
            ),
            vec![],
        );
        rt.active_work_subagents = 1;

        // Before the result drains, the reservation is still held.
        assert_eq!(
            rt.active_work_subagents, 1,
            "reservation held until the Work agent's result is processed"
        );

        rt.process_event(Event::SubAgentResult {
            agent_id: "w1".to_string(),
            outcome: SubAgentOutcome::Failure {
                error: "cancelled".to_string(),
                error_kind: crate::db::ErrorKind::Cancelled,
            },
        })
        .await
        .expect("processing the Work agent's result must succeed");

        assert_eq!(
            rt.active_work_subagents, 0,
            "reservation released exactly once when the Work result drained"
        );
        assert!(
            matches!(rt.state, ConvState::Idle),
            "the last drained result resolves CancellingSubAgents -> Idle, got {}",
            rt.state.variant_name()
        );
    }

    /// Test 4 (part B): after the 6s last-resort presumes a silent Work agent
    /// dead and injects a synthetic result, the one-writer counter returns to 0
    /// — no leak. Drives the backstop directly (no real 6s wait).
    #[tokio::test]
    async fn one_writer_released_by_last_resort_backstop() {
        let (mut rt, _storage) = build_runtime_with_state_and_queue(
            "conv-onewriter-backstop",
            mk_cancelling_sub_agents(&["w1"], crate::state_machine::event::CancelCause::Timeout),
            vec![],
        );
        rt.active_work_subagents = 1;

        // Fire the last-resort backstop directly (the deadline arm would call
        // this after 6s).
        rt.handle_cancelling_sub_agents_timeout().await;

        assert_eq!(
            rt.active_work_subagents, 0,
            "presumed-dead teardown must release the one-writer reservation — no leak"
        );
        assert!(
            matches!(rt.state, ConvState::LlmRequesting { .. }),
            "a Timeout teardown resumes the parent (LlmRequesting), got {}",
            rt.state.variant_name()
        );
    }

    /// Test 5 (mixed drain): two pending Work agents — one reports a real result,
    /// the other is presumed dead by the last-resort backstop. Exactly one
    /// decrement each (no double-release, no leak); the parent reaches Idle.
    #[tokio::test]
    async fn mixed_drain_real_result_then_backstop_no_double_release() {
        let (mut rt, _storage) = build_runtime_with_state_and_queue(
            "conv-mixed-drain",
            mk_cancelling_sub_agents(
                &["real", "silent"],
                crate::state_machine::event::CancelCause::Timeout,
            ),
            vec![],
        );
        rt.active_work_subagents = 2;

        // "real" reports a genuine Success — fidelity preserved, counter -> 1.
        rt.process_event(Event::SubAgentResult {
            agent_id: "real".to_string(),
            outcome: SubAgentOutcome::Success {
                result: "did real work".to_string(),
            },
        })
        .await
        .expect("processing the real result must succeed");

        assert_eq!(
            rt.active_work_subagents, 1,
            "exactly one decrement for the real result"
        );
        assert!(
            matches!(rt.state, ConvState::CancellingSubAgents { .. }),
            "still draining the silent agent, got {}",
            rt.state.variant_name()
        );

        // "silent" never reports; the backstop presumes it dead and drains it.
        rt.handle_cancelling_sub_agents_timeout().await;

        assert_eq!(
            rt.active_work_subagents, 0,
            "exactly one decrement for the presumed-dead agent — no double-release, no leak"
        );
        assert!(
            matches!(rt.state, ConvState::LlmRequesting { .. }),
            "Timeout teardown resumes the parent after both agents drain, got {}",
            rt.state.variant_name()
        );
    }

    /// Double-release / underflow probe: a real result drains a Work agent
    /// (counter -> 0, agent removed from pending), then a LATE synthetic result
    /// for the SAME agent arrives. The pending-membership guard means the second
    /// is a harmless rejected transition that does NOT decrement again — the
    /// `saturating_sub` floor is never even reached because the guard fires first.
    #[tokio::test]
    async fn late_duplicate_result_for_same_agent_does_not_double_release() {
        let (mut rt, _storage) = build_runtime_with_state_and_queue(
            "conv-dup-nounder",
            mk_cancelling_sub_agents(
                &["w1"],
                crate::state_machine::event::CancelCause::UserRequested,
            ),
            vec![],
        );
        rt.active_work_subagents = 1;

        rt.process_event(Event::SubAgentResult {
            agent_id: "w1".to_string(),
            outcome: SubAgentOutcome::Failure {
                error: "cancelled".to_string(),
                error_kind: crate::db::ErrorKind::Cancelled,
            },
        })
        .await
        .expect("first result must succeed");
        assert_eq!(rt.active_work_subagents, 0);
        assert!(matches!(rt.state, ConvState::Idle));

        // A late duplicate for the same agent. The parent is now Idle, so this is
        // buffered (Idle can't handle SubAgentResult) rather than re-decrementing.
        rt.process_event(Event::SubAgentResult {
            agent_id: "w1".to_string(),
            outcome: SubAgentOutcome::Failure {
                error: "late dup".to_string(),
                error_kind: crate::db::ErrorKind::Cancelled,
            },
        })
        .await
        .expect("a late duplicate must not error");

        assert_eq!(
            rt.active_work_subagents, 0,
            "a late duplicate for an already-drained agent must not decrement again"
        );
    }

    /// Entering `Idle` with an empty queue produces no drain event.
    #[tokio::test]
    async fn no_drain_when_queue_empty_idle() {
        let (mut rt, _storage) = build_runtime_with_state_and_queue(
            "conv-empty-idle",
            ConvState::LlmRequesting { attempt: 1 },
            vec![],
        );

        let result = TransitionResult::new(ConvState::Idle);
        let generated = rt
            .apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        assert!(
            extract_steer_drain_entries(&generated).is_empty(),
            "no drain event should be emitted when queue is empty"
        );
    }

    /// Entering `LlmRequesting` from a tool round with an empty queue produces
    /// no drain event.
    #[tokio::test]
    async fn no_drain_when_queue_empty_mid_turn() {
        let (mut rt, _storage) =
            build_runtime_with_state_and_queue("conv-empty-mid", mk_tool_executing(), vec![]);

        let result = TransitionResult::new(ConvState::LlmRequesting { attempt: 1 });
        let generated = rt
            .apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        assert!(
            extract_steer_drain_entries(&generated).is_empty(),
            "no drain when queue is empty"
        );
    }

    /// `LlmRequesting` → `ToolExecuting` (LLM responded with tools) must NOT
    /// trigger a drain — this is mid-conversation but not a hook point. Queue
    /// must be preserved untouched.
    #[tokio::test]
    async fn no_drain_on_intermediate_states() {
        let queue = vec![mk_entry("keep-me", "still here")];
        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-intermediate",
            ConvState::LlmRequesting { attempt: 1 },
            queue,
        );

        let result = TransitionResult::new(mk_tool_executing());
        let generated = rt
            .apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        assert!(
            extract_steer_drain_entries(&generated).is_empty(),
            "transition without an entry hook must not drain"
        );
        assert_eq!(
            rt.steering_queue.len(),
            1,
            "queue must be preserved when no drain hook fires"
        );
        assert_eq!(rt.steering_queue[0].message_id, "keep-me");
        // No persist call happened on this path, so storage's queue is still empty
        // (it was never written to). The point of the assertion above is that the
        // in-memory queue is the live source — it was not modified.
        let _ = storage; // touch to silence unused warning
    }

    /// `Effect::ClearSteeringQueueEntries` removes ONLY the matching `message_ids`
    /// from storage; concurrently-enqueued entries are preserved. Models the
    /// enqueue-during-drain race.
    #[tokio::test]
    async fn clear_steering_queue_entries_preserves_concurrent_enqueue() {
        use crate::runtime::traits::StateStore;
        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-clear-effect",
            ConvState::LlmRequesting { attempt: 1 },
            vec![],
        );
        // Pre-seed storage as if drain took [p1, p2] from in-memory, then a
        // concurrent enqueue persisted [p1, p2, c1] (c1 added by enqueue).
        storage
            .update_steering_queue(
                "conv-clear-effect",
                &[
                    mk_entry("p1", "pending-1"),
                    mk_entry("p2", "pending-2"),
                    mk_entry("c1", "concurrent"),
                ],
            )
            .await
            .expect("seed steering queue");

        // Drain only removes p1 and p2.
        rt.execute_effect(Effect::ClearSteeringQueueEntries {
            message_ids: vec!["p1".to_string(), "p2".to_string()],
        })
        .await
        .expect("ClearSteeringQueueEntries effect must succeed");

        let remaining = storage.get_steering_queue("conv-clear-effect");
        assert_eq!(remaining.len(), 1, "concurrent enqueue must survive drain");
        assert_eq!(remaining[0].message_id, "c1");
    }

    /// `PersistMessage` is idempotent on duplicate `message_id`. Models the
    /// crash-recovery re-drain path: a `SteerDrainedUserMessages` event re-fires
    /// after a partial drain, but messages already persisted are skipped without
    /// allocating a new `sequence_id` or producing a duplicate row.
    #[tokio::test]
    async fn persist_message_is_idempotent_on_duplicate_id() {
        use crate::db::{MessageContent, UserContent};
        use crate::runtime::traits::MessageStore;
        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-idem",
            ConvState::LlmRequesting { attempt: 1 },
            vec![],
        );
        let dup_id = "dup-msg-1".to_string();
        // Seed: message already exists in storage (simulates partial drain that
        // persisted this entry before a crash).
        storage
            .add_message(
                &dup_id,
                "conv-idem",
                &MessageContent::User(UserContent::new("already-there")),
                None,
                None,
            )
            .await
            .expect("seed message");
        let initial_count = storage.get_all_messages("conv-idem").len();
        assert_eq!(initial_count, 1);

        // Run idempotent PersistMessage with the same message_id — must be a no-op.
        rt.execute_effect(Effect::PersistMessage {
            content: MessageContent::User(UserContent::new("duplicate-attempt")),
            display_data: None,
            usage_data: None,
            message_id: dup_id.clone(),
            idempotent: true,
        })
        .await
        .expect("PersistMessage on duplicate must succeed");

        let after = storage.get_all_messages("conv-idem");
        assert_eq!(
            after.len(),
            1,
            "no duplicate row should be inserted for an existing message_id"
        );
        assert_eq!(after[0].message_id, dup_id);
    }

    /// Sub-agent runtimes never drain a steering queue. Steering is a parent-
    /// only feature; even if a sub-agent's in-memory queue is non-empty (e.g.,
    /// corrupted state on resume), the drain detector must skip it.
    #[tokio::test]
    async fn no_drain_on_sub_agent_runtime() {
        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::sub_agent(
            "conv-sub-drain",
            PathBuf::from("/tmp"),
            "test-model",
            200_000,
            "parent-conv",
        );
        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);
        let mut rt = ConversationRuntime::new(
            context,
            ConvState::LlmRequesting { attempt: 1 },
            storage,
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        )
        .with_steering_queue(vec![mk_entry("ignored", "should not drain")]);

        let result = TransitionResult::new(ConvState::Idle);
        let generated = rt
            .apply_transition_result(result)
            .await
            .expect("apply_transition_result must succeed");

        assert!(
            extract_steer_drain_entries(&generated).is_empty(),
            "sub-agent runtimes must NOT drain queued steers"
        );
        assert_eq!(
            rt.steering_queue.len(),
            1,
            "sub-agent queue must remain untouched"
        );
    }

    /// Non-idempotent `PersistMessage` does NOT do a `message_exists` precheck.
    /// This is the hot-path guarantee: agent/tool/checkpoint persistence pays
    /// no extra DB query.
    #[tokio::test]
    async fn persist_message_non_idempotent_skips_existence_check() {
        use crate::db::{MessageContent, UserContent};
        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-non-idem",
            ConvState::LlmRequesting { attempt: 1 },
            vec![],
        );

        // Fresh message_id; non-idempotent path should persist normally.
        rt.execute_effect(Effect::PersistMessage {
            content: MessageContent::User(UserContent::new("fresh-message")),
            display_data: None,
            usage_data: None,
            message_id: "new-1".to_string(),
            idempotent: false,
        })
        .await
        .expect("non-idempotent PersistMessage must succeed");

        assert_eq!(storage.get_all_messages("conv-non-idem").len(), 1);
    }

    /// Regression: a tool result carrying typed images must survive the
    /// normal `PersistCheckpoint` round into `ToolContent.images`. Before
    /// this was threaded, `MessageContent::tool(...)` dropped the field, so
    /// `read_image` (which now relies solely on the typed channel) lost its
    /// image bytes for both the UI and the next LLM turn.
    #[tokio::test]
    async fn persist_checkpoint_preserves_tool_result_images() {
        use crate::db::{MessageContent, ToolContentImage, ToolOutcome, ToolResult};
        use crate::state_machine::{AssistantMessage, CheckpointData};
        use phoenix_llm::ContentBlock;

        let (mut rt, storage) = build_runtime_with_state_and_queue(
            "conv-img",
            ConvState::LlmRequesting { attempt: 1 },
            vec![],
        );

        let assistant = AssistantMessage::new(
            uuid::Uuid::new_v4().to_string(),
            vec![ContentBlock::ToolUse {
                id: "tool-img-1".to_string(),
                name: "read_image".to_string(),
                input: serde_json::json!({"path": "x.png"}),
            }],
            None,
            None,
        );
        let result = ToolResult {
            tool_use_id: "tool-img-1".to_string(),
            outcome: ToolOutcome::Success {
                output: "Image loaded: x.png (3 bytes)".to_string(),
                display_data: None,
                images: vec![ToolContentImage {
                    media_type: "image/png".to_string(),
                    data: "QUJD".to_string(),
                }],
            },
            duration_ms: None,
        };
        let data = CheckpointData::tool_round(assistant, vec![result]).expect("tool_round");

        rt.execute_effect(Effect::PersistCheckpoint { data })
            .await
            .expect("PersistCheckpoint must succeed");

        let msgs = storage.get_all_messages("conv-img");
        let tool_msg = msgs
            .iter()
            .find_map(|m| match &m.content {
                MessageContent::Tool(tc) if tc.tool_use_id == "tool-img-1" => Some(tc),
                _ => None,
            })
            .expect("persisted tool result message");
        assert_eq!(
            tool_msg.images,
            vec![ToolContentImage {
                media_type: "image/png".to_string(),
                data: "QUJD".to_string(),
            }],
            "typed images must not be dropped at checkpoint persistence"
        );
    }
}

// ============================================================
// Work-sub-agent cwd-scoping guard (REQ-PROJ-008)
// ============================================================
//
// Distilled from specs/subagents/subagents.allium
// (SpawnRejectedWorkCwdOutsideWorktree). A Work sub-agent's overridden
// `cwd` must stay inside the parent's worktree -- without the guard, the
// sub-agent's runtime would see a different working_dir than the parent
// and writes could escape projects.allium's WriteBlockedOutsideWorktree.

#[cfg(test)]
mod work_subagent_cwd_guard_tests {
    use super::*;
    use crate::db::ToolOutcome;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::state_machine::state::{SpawnAgentsInput, SubAgentMode, SubAgentTask, ToolInput};
    use crate::state_machine::ConvContext;
    use crate::system_prompt::ModeContext;
    use crate::tools::BrowserSessionManager;
    use phoenix_llm::ModelRegistry;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    fn runtime_in_mode(
        working_dir: &std::path::Path,
        mode_context: ModeContext,
    ) -> ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>> {
        let storage = Arc::new(InMemoryStorage::new());
        let mut context = ConvContext::new(
            "cwd-guard-conv",
            working_dir.to_path_buf(),
            "test-model",
            200_000,
        );
        context.mode_context = Some(mode_context);
        context.mode = crate::state_machine::state::ModeKind::Managed;

        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);

        ConversationRuntime::new(
            context,
            ConvState::Idle,
            storage,
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        )
    }

    fn runtime_in_work_mode(
        worktree_path: &std::path::Path,
    ) -> ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>> {
        runtime_in_mode(
            worktree_path,
            ModeContext::Work {
                branch_name: "task-99999-x".to_string(),
                base_branch: "main".to_string(),
                worktree_path: worktree_path.to_string_lossy().to_string(),
            },
        )
    }

    fn runtime_in_direct_mode(
        working_dir: &std::path::Path,
    ) -> ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>> {
        runtime_in_mode(working_dir, ModeContext::Direct)
    }

    fn spawn_tool(input: SpawnAgentsInput) -> ToolCall {
        ToolCall::new("tool-spawn-1", ToolInput::SpawnAgents(input))
    }

    fn tool_result_text(result: &ToolResult) -> String {
        match &result.outcome {
            ToolOutcome::Success { output, .. } | ToolOutcome::Error { output, .. } => {
                output.clone()
            }
            ToolOutcome::Cancelled { message } => message.clone(),
        }
    }

    #[tokio::test]
    async fn rejects_direct_subagent_cwd_at_filesystem_root() {
        let parent = TempDir::new().expect("parent tempdir");
        let mut rt = runtime_in_direct_mode(parent.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "inspect everything".to_string(),
                    cwd: Some("/".to_string()),
                    mode: Some(SubAgentMode::Explore),
                    model: None,
                    max_turns: None,
                    agent_type: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                assert!(result.is_error(), "root cwd must surface as tool error");
                let msg = tool_result_text(&result);
                assert!(
                    msg.contains("filesystem root"),
                    "error should explain root cwd rejection, got: {msg}"
                );
            }
            other => panic!("expected ToolComplete with cwd error, got {other:?}"),
        }
        assert_eq!(rt.active_work_subagents, 0);
    }

    #[tokio::test]
    async fn rejects_inherited_direct_subagent_cwd_at_filesystem_root() {
        let mut rt = runtime_in_direct_mode(std::path::Path::new("/"));

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "inherit root".to_string(),
                    cwd: None,
                    mode: Some(SubAgentMode::Explore),
                    model: None,
                    max_turns: None,
                    agent_type: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                assert!(result.is_error(), "inherited root cwd must be rejected");
                let msg = tool_result_text(&result);
                assert!(msg.contains("filesystem root"), "got: {msg}");
            }
            other => panic!("expected ToolComplete with cwd error, got {other:?}"),
        }
        assert_eq!(rt.active_work_subagents, 0);
    }

    #[tokio::test]
    async fn accepts_direct_subagent_cwd_in_deep_directory() {
        let parent = TempDir::new().expect("parent tempdir");
        let deep = parent.path().join("project/src");
        std::fs::create_dir_all(&deep).expect("deep dir");
        let mut rt = runtime_in_direct_mode(parent.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "inspect project".to_string(),
                    cwd: Some(deep.to_string_lossy().to_string()),
                    mode: Some(SubAgentMode::Explore),
                    model: None,
                    max_turns: None,
                    agent_type: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                let msg = tool_result_text(&result);
                assert!(
                    !msg.contains("filesystem root"),
                    "deep cwd should not trip root-floor guard; got: {msg}"
                );
            }
            Some(Event::SpawnAgentsComplete { .. }) => {}
            other => panic!("unexpected event: {other:?}"),
        }
    }

    /// Work sub-agent with a `cwd` pointing outside the parent's worktree
    /// is rejected with an error tool result -- the spawn never reaches
    /// the `RuntimeManager`.
    #[tokio::test]
    async fn rejects_work_subagent_cwd_outside_worktree() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let outside = TempDir::new().expect("outside tempdir");

        let mut rt = runtime_in_work_mode(worktree.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "do unsafe writes".to_string(),
                    cwd: Some(outside.path().to_string_lossy().to_string()),
                    mode: Some(SubAgentMode::Work),
                    model: None,
                    max_turns: None,
                    agent_type: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                assert!(result.is_error(), "rejection must surface as a tool error");
                let msg = tool_result_text(&result);
                assert!(
                    msg.contains("inside the parent's worktree"),
                    "error message should explain the cwd-scoping rule, got: {msg}"
                );
            }
            other => panic!("expected ToolComplete with error, got {other:?}"),
        }
        assert_eq!(
            rt.active_work_subagents, 0,
            "rejected spawn must not increment active_work_subagents"
        );
    }

    #[tokio::test]
    async fn accepts_unnamed_generic_work_subagent() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let (spawn_tx, mut spawn_rx) = mpsc::channel::<SubAgentSpawnRequest>(1);
        let (cancel_tx, _cancel_rx) = mpsc::channel(1);
        let mut rt = runtime_in_work_mode(worktree.path()).with_spawn_channels(spawn_tx, cancel_tx);

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "implement the fix".to_string(),
                    cwd: None,
                    mode: Some(SubAgentMode::Work),
                    model: None,
                    max_turns: None,
                    agent_type: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::SpawnAgentsComplete {
                spawned, result, ..
            }) => {
                assert!(!result.is_error(), "generic Work spawn should succeed");
                assert_eq!(spawned.len(), 1);
                assert_eq!(spawned[0].mode, SubAgentMode::Work);
            }
            other => panic!("expected SpawnAgentsComplete, got {other:?}"),
        }

        let request = spawn_rx
            .try_recv()
            .expect("generic Work sub-agent request should be sent");
        assert_eq!(request.spec.mode, SubAgentMode::Work);
        assert_eq!(request.spec.agent_name, None);
        assert_eq!(request.spec.persona, None);
        assert_eq!(request.spec.model_id, "test-model");
        assert_eq!(rt.active_work_subagents, 1);
    }

    /// An `agent_type` that matches no discovered agent is rejected before any
    /// sub-agent is spawned (REQ-AG-007). The empty worktree has no
    /// `.claude/agents/`, so discovery returns nothing and the lookup fails.
    #[tokio::test]
    async fn rejects_unknown_agent_type() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let mut rt = runtime_in_work_mode(worktree.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "review".to_string(),
                    cwd: None,
                    mode: None,
                    model: None,
                    max_turns: None,
                    agent_type: Some("ghost".to_string()),
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                assert!(
                    result.is_error(),
                    "unknown agent_type must surface as error"
                );
                let msg = tool_result_text(&result);
                assert!(
                    msg.contains("Unknown agent_type 'ghost'"),
                    "error should name the unknown agent_type, got: {msg}"
                );
            }
            other => panic!("expected ToolComplete with error, got {other:?}"),
        }
        assert_eq!(
            rt.active_work_subagents, 0,
            "rejected spawn must not increment active_work_subagents"
        );
    }

    /// A later task with an unknown explicit model is rejected during the
    /// build/validate pass, before any spawn request is sent — so a partial
    /// validation failure cannot orphan earlier sub-agents. With no spawn
    /// channel wired, surfacing the *model* error (not the "not configured"
    /// error that pass B would emit) proves validation runs before sending.
    #[tokio::test]
    async fn unknown_model_on_later_task_rejected_before_any_spawn() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let mut rt = runtime_in_work_mode(worktree.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![
                    SubAgentTask {
                        task: "first".to_string(),
                        cwd: None,
                        mode: Some(SubAgentMode::Explore),
                        model: None,
                        max_turns: None,
                        agent_type: None,
                    },
                    SubAgentTask {
                        task: "second".to_string(),
                        cwd: None,
                        mode: Some(SubAgentMode::Explore),
                        model: Some("ghost-model".to_string()),
                        max_turns: None,
                        agent_type: None,
                    },
                ],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                assert!(result.is_error());
                let msg = tool_result_text(&result);
                assert!(
                    msg.contains("ghost-model"),
                    "should fail on the unknown model before reaching the spawn channel, got: {msg}"
                );
            }
            other => panic!("expected ToolComplete with model error, got {other:?}"),
        }
        assert_eq!(rt.active_work_subagents, 0);
    }

    /// `agent_type` resolves against the catalog frozen on the runtime, not a
    /// fresh filesystem scan (REQ-AG-008): the worktree has no `.claude/agents`,
    /// yet a frozen-catalog agent resolves, so we reach the missing-spawn-channel
    /// path rather than an "Unknown `agent_type`" rejection.
    #[tokio::test]
    async fn agent_type_resolves_from_frozen_catalog_not_filesystem() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let catalog = std::sync::Arc::from(vec![phoenix_agents::AgentDefinition {
            name: "reviewer".to_string(),
            description: "Reviews".to_string(),
            body: "You are a reviewer.".to_string(),
            path: std::path::PathBuf::from("/virtual/reviewer.md"),
            source_dir: ".claude/agents".to_string(),
            model: None,
            mode: None,
            tools: None,
        }]);
        let mut rt = runtime_in_work_mode(worktree.path()).with_agent_catalog(catalog);

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "review".to_string(),
                    cwd: None,
                    mode: Some(SubAgentMode::Explore),
                    model: None,
                    max_turns: None,
                    agent_type: Some("reviewer".to_string()),
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                let msg = tool_result_text(&result);
                assert!(
                    !msg.contains("Unknown agent_type"),
                    "frozen-catalog agent should resolve despite empty filesystem, got: {msg}"
                );
                assert!(
                    msg.contains("not configured"),
                    "resolution should succeed and reach the missing-channel path, got: {msg}"
                );
            }
            other => panic!("expected ToolComplete, got {other:?}"),
        }
    }

    /// A Work sub-agent whose `cwd` is inside the worktree is NOT rejected
    /// by the scoping guard. (It will still fail downstream because no
    /// `spawn_tx` is wired in this test runtime, but that failure is
    /// distinguishable from the scoping rejection.)
    #[tokio::test]
    async fn accepts_work_subagent_cwd_inside_worktree() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let nested = worktree.path().join("sub/dir");
        std::fs::create_dir_all(&nested).expect("nested dir");

        let mut rt = runtime_in_work_mode(worktree.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "do scoped writes".to_string(),
                    cwd: Some(nested.to_string_lossy().to_string()),
                    mode: Some(SubAgentMode::Work),
                    model: None,
                    max_turns: None,
                    agent_type: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                let msg = tool_result_text(&result);
                assert!(
                    !msg.contains("inside the parent's worktree"),
                    "in-worktree cwd should not trip the scoping guard; got: {msg}"
                );
            }
            Some(Event::SpawnAgentsComplete { .. }) => {
                // Test runtime has no spawn channel wired, so we don't
                // normally reach SpawnAgentsComplete -- but if some future
                // refactor wires it, that's still a "did not reject" pass.
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    /// Symlinks that escape the worktree are rejected -- the guard
    /// canonicalises both sides before comparing.
    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_work_subagent_cwd_via_escaping_symlink() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let outside = TempDir::new().expect("outside tempdir");
        let symlink = worktree.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &symlink).expect("create symlink");

        let mut rt = runtime_in_work_mode(worktree.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "follow the symlink".to_string(),
                    cwd: Some(symlink.to_string_lossy().to_string()),
                    mode: Some(SubAgentMode::Work),
                    model: None,
                    max_turns: None,
                    agent_type: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                assert!(result.is_error());
            }
            other => panic!("expected symlink escape to be rejected, got {other:?}"),
        }
    }

    #[test]
    fn path_is_within_requires_root_to_exist() {
        // Worktree root that doesn't canonicalise -> fail closed. A
        // non-existent root would make the comparison meaningless.
        assert!(!path_is_within(
            "/nonexistent/root/sub",
            "/nonexistent/root"
        ));
    }

    #[test]
    fn path_is_within_accepts_nonexistent_leaf_under_real_root() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let path_in = worktree.path().join("not/yet/created");
        // Deepest existing ancestor of `path_in` is the worktree itself,
        // which canonicalises and starts_with itself.
        assert!(path_is_within(
            path_in.to_str().unwrap(),
            worktree.path().to_str().unwrap()
        ));
    }

    #[test]
    fn path_is_within_rejects_parent_dir_traversal_that_escapes() {
        // `/worktree/../escape` -> canonical /parent_of_worktree, which
        // is not a subpath of the worktree -> rejected. Canonicalise
        // handles the `..` directly (it's a `realpath` call).
        let worktree = TempDir::new().expect("worktree tempdir");
        let bad = format!("{}/../escape", worktree.path().display());
        assert!(!path_is_within(&bad, worktree.path().to_str().unwrap()));
    }

    #[test]
    fn path_is_within_accepts_internal_parent_dir_traversal() {
        // `/worktree/src/../tests` is a legitimate in-worktree path --
        // canonicalisation resolves it to `/worktree/tests`, which is a
        // subpath of the worktree. Don't blanket-reject `..`.
        let worktree = TempDir::new().expect("worktree tempdir");
        std::fs::create_dir_all(worktree.path().join("src")).expect("src dir");
        let inner = format!("{}/src/../tests", worktree.path().display());
        assert!(path_is_within(&inner, worktree.path().to_str().unwrap()));
    }

    #[test]
    fn path_is_within_rejects_relative_paths() {
        let worktree = TempDir::new().expect("worktree tempdir");
        assert!(!path_is_within(
            "sub/dir",
            worktree.path().to_str().unwrap()
        ));
        assert!(!path_is_within("./sub", worktree.path().to_str().unwrap()));
    }

    /// The intermediate-symlink escape: `/worktree/escape` is a symlink
    /// to `/outside`, override cwd is `/worktree/escape/newdir` (does
    /// not exist yet). Without the deepest-existing-ancestor canonical
    /// resolution, the leaf's canonicalisation fails and a lexical
    /// `starts_with` would accept the path. Resolving the deepest
    /// existing ancestor (the symlink itself, which canonicalises to
    /// `/outside`) rejects it.
    #[cfg(unix)]
    #[test]
    fn path_is_within_rejects_intermediate_symlink_escape() {
        let worktree = TempDir::new().expect("worktree tempdir");
        let outside = TempDir::new().expect("outside tempdir");
        let escape = worktree.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &escape).expect("create symlink");
        let bypass = escape.join("newdir");
        assert!(!path_is_within(
            bypass.to_str().unwrap(),
            worktree.path().to_str().unwrap()
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_parent_dir_traversal_in_override() {
        let worktree = TempDir::new().expect("worktree tempdir");
        // Sibling dir that EXISTS on disk, so canonicalize succeeds for
        // both sides -- the only thing keeping this safe is the `..`
        // rejection up front.
        let outside = TempDir::new().expect("outside tempdir");
        let traversing = format!(
            "{}/../{}",
            worktree.path().display(),
            outside
                .path()
                .file_name()
                .expect("outside dir name")
                .to_string_lossy()
        );

        let mut rt = runtime_in_work_mode(worktree.path());

        let result = rt
            .handle_spawn_agents_tool(spawn_tool(SpawnAgentsInput {
                tasks: vec![SubAgentTask {
                    task: "traverse out".to_string(),
                    cwd: Some(traversing),
                    mode: Some(SubAgentMode::Work),
                    model: None,
                    max_turns: None,
                    agent_type: None,
                }],
            }))
            .await
            .expect("handle_spawn_agents_tool returned error");

        match result {
            Some(Event::ToolComplete { result, .. }) => {
                assert!(result.is_error(), "rejection must surface as a tool error");
                let msg = tool_result_text(&result);
                assert!(
                    msg.contains("inside the parent's worktree"),
                    "error message should explain the cwd-scoping rule, got: {msg}"
                );
            }
            other => panic!("expected ..-traversal to be rejected, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod tool_output_to_outcome_tests {
    use super::tool_output_to_outcome;
    use crate::db::ToolOutcome;
    use crate::tools::{ToolImage, ToolOutput};

    #[test]
    fn success_output_maps_to_success_outcome() {
        let out = ToolOutput::success("ran clean")
            .with_display(serde_json::json!({ "k": "v" }))
            .with_images(vec![ToolImage {
                media_type: "image/png".to_string(),
                data: "Zm9v".to_string(),
            }]);

        match tool_output_to_outcome(out) {
            ToolOutcome::Success {
                output,
                display_data,
                images,
            } => {
                assert_eq!(output, "ran clean");
                assert_eq!(display_data, Some(serde_json::json!({ "k": "v" })));
                assert_eq!(images.len(), 1);
                assert_eq!(images[0].media_type, "image/png");
                assert_eq!(images[0].data, "Zm9v");
            }
            other => panic!("expected ToolOutcome::Success, got {other:?}"),
        }
    }

    #[test]
    fn error_output_maps_to_error_outcome() {
        // Error-shaped text the executor must NOT misclassify: with the old
        // `success: bool`, only the string distinguished this from a success.
        let out = ToolOutput::error("looks fine but failed");

        match tool_output_to_outcome(out) {
            ToolOutcome::Error { output, .. } => {
                assert_eq!(output, "looks fine but failed");
            }
            other => panic!("expected ToolOutcome::Error, got {other:?}"),
        }
    }
}

/// M3 (task 61004): a dropped oneshot sender — the background tool/LLM task
/// panicked or was aborted before it could `send` — must produce a typed
/// failure outcome rather than silently delivering nothing. Silent loss wedges
/// the conversation forever (`ToolExecuting` never sees `ToolComplete`;
/// `CancellingTool` waits and rejects all input until restart).
#[cfg(test)]
mod sender_drop_forwarder_tests {
    use super::{forward_llm_outcome, forward_tool_outcome};
    use crate::state_machine::outcome::{LlmOutcome, ToolExecOutcome};
    use tokio::sync::{mpsc, oneshot};

    #[tokio::test]
    async fn tool_sender_drop_yields_failed_outcome() {
        let (tool_tx, tool_rx) = oneshot::channel::<ToolExecOutcome>();
        let (tool_outcome_tx, mut tool_outcome_rx) = mpsc::channel::<(u64, ToolExecOutcome)>(4);

        // Simulate the spawned tool task panicking/aborting: the sender is
        // dropped without ever sending.
        drop(tool_tx);

        forward_tool_outcome(tool_rx, "tool-use-42".to_string(), 9, tool_outcome_tx).await;

        match tool_outcome_rx.try_recv() {
            Ok((generation, ToolExecOutcome::Failed { tool_use_id, error })) => {
                assert_eq!(
                    generation, 9,
                    "forwarder must stamp the dispatch generation"
                );
                assert_eq!(tool_use_id, "tool-use-42");
                assert!(
                    error.contains("aborted or panicked"),
                    "error should explain the sender-drop, got: {error}"
                );
            }
            other => panic!("expected a Failed tool outcome on sender-drop, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_normal_outcome_is_forwarded_unchanged() {
        let (tool_tx, tool_rx) = oneshot::channel::<ToolExecOutcome>();
        let (tool_outcome_tx, mut tool_outcome_rx) = mpsc::channel::<(u64, ToolExecOutcome)>(4);

        tool_tx
            .send(ToolExecOutcome::Failed {
                tool_use_id: "real-id".to_string(),
                error: "real error".to_string(),
            })
            .expect("send should succeed");

        forward_tool_outcome(tool_rx, "forwarder-id".to_string(), 3, tool_outcome_tx).await;

        match tool_outcome_rx.try_recv() {
            Ok((generation, ToolExecOutcome::Failed { tool_use_id, error })) => {
                // The forwarder must NOT clobber a real outcome with the
                // synthetic one.
                assert_eq!(
                    generation, 3,
                    "forwarder must stamp the dispatch generation"
                );
                assert_eq!(tool_use_id, "real-id");
                assert_eq!(error, "real error");
            }
            other => panic!("expected the real outcome forwarded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn llm_sender_drop_yields_network_error_outcome() {
        let (llm_tx, llm_rx) = oneshot::channel::<LlmOutcome>();
        let (llm_outcome_tx, mut llm_outcome_rx) = mpsc::channel::<(u64, LlmOutcome)>(4);

        drop(llm_tx);

        forward_llm_outcome(llm_rx, 7, llm_outcome_tx).await;

        match llm_outcome_rx.try_recv() {
            Ok((generation, LlmOutcome::NetworkError { message })) => {
                assert_eq!(
                    generation, 7,
                    "forwarder must stamp the dispatch generation"
                );
                assert!(
                    message.contains("aborted or panicked"),
                    "message should explain the sender-drop, got: {message}"
                );
            }
            other => panic!("expected a NetworkError LLM outcome on sender-drop, got {other:?}"),
        }
    }
}

/// A retry-backoff timer from a cancelled-then-resent turn must not fire a
/// second concurrent LLM request. Attempt numbers reset per turn, so the
/// reducer's `attempt == retry_attempt` guard alone cannot reject a stale
/// timer. The executor stamps each scheduled timer with a `retry_generation`
/// and discards a `RetryTimeout` whose generation has been superseded — by a
/// newer timer or by leaving the retry-scheduling state. The guard is
/// generation-based, not handle-presence-based: an old fire arriving while a
/// newer timer is pending is discarded without disturbing the new timer, so the
/// live fire is still honored when it arrives.
#[cfg(test)]
mod retry_timer_epoch_tests {
    use super::*;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::tools::BrowserSessionManager;
    use phoenix_llm::ModelRegistry;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    type TestRuntime =
        ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>;

    fn runtime_requesting() -> TestRuntime {
        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new("conv-retry", PathBuf::from("/tmp"), "test-model", 200_000);
        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);
        ConversationRuntime::new(
            context,
            ConvState::LlmRequesting { attempt: 1 },
            storage,
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        )
    }

    fn retryable_llm_error() -> Event {
        Event::LlmError {
            message: "connection reset".to_string(),
            error_kind: crate::db::ErrorKind::Network,
            attempt: 0,
            recovery_in_progress: false,
            resets_at: None,
        }
    }

    /// Mirror of the executor select-loop's retry-outcome arm: apply the
    /// generation guard, then route a current-generation `RetryTimeout` to
    /// `process_outcome`. Returns `true` if the timeout was applied, `false` if
    /// discarded as stale. Tests use this to exercise the real guard
    /// (`retry_timeout_is_stale`) deterministically without spinning the loop.
    async fn route_retry_timeout(rt: &mut TestRuntime, generation: u64, attempt: u32) -> bool {
        if rt.retry_timeout_is_stale(generation) {
            return false;
        }
        rt.process_outcome(EffectOutcome::RetryTimeout { attempt })
            .await
            .expect("current-generation retry timeout must apply cleanly");
        true
    }

    /// A retryable `LlmError` schedules a backoff timer (bumping
    /// `retry_generation`); cancelling the turn bumps the generation again so
    /// any fire from the old timer is stale and can never cross the turn
    /// boundary.
    #[tokio::test]
    async fn cancel_supersedes_pending_retry_timer() {
        let mut rt = runtime_requesting();

        rt.process_event(retryable_llm_error())
            .await
            .expect("retryable error transitions");
        assert!(
            matches!(rt.state, ConvState::LlmRequesting { attempt: 2 }),
            "retryable error should bump attempt and stay in LlmRequesting, got {:?}",
            rt.state.variant_name()
        );
        assert!(
            rt.retry_timer_handle.is_some(),
            "a retry timer must be tracked while a retry is pending"
        );
        let scheduled_gen = rt.retry_generation;

        rt.process_event(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .expect("cancel transitions");
        assert!(
            matches!(rt.state, ConvState::Idle),
            "cancel from LlmRequesting goes to Idle, got {:?}",
            rt.state.variant_name()
        );
        assert!(
            rt.retry_timer_handle.is_none(),
            "leaving the retry-scheduling state must abort and clear the timer handle"
        );
        assert!(
            rt.retry_generation > scheduled_gen,
            "leaving the retry-scheduling state must bump the generation past the timer's"
        );
        assert!(
            rt.retry_timeout_is_stale(scheduled_gen),
            "the cancelled timer's fire must now be stale"
        );
    }

    /// A stale `RetryTimeout` that raced onto the channel before the cancel is
    /// dropped by the generation guard rather than dispatching a second LLM
    /// request.
    #[tokio::test]
    async fn stale_retry_timeout_is_ignored_after_cancel() {
        let mut rt = runtime_requesting();

        rt.process_event(retryable_llm_error())
            .await
            .expect("retryable error transitions");
        let stale_gen = rt.retry_generation;
        rt.process_event(Event::UserCancel {
            reason: None,
            cause: crate::state_machine::event::CancelCause::UserRequested,
        })
        .await
        .expect("cancel transitions");
        assert!(matches!(rt.state, ConvState::Idle));

        // The stale timer fires attempt 2 after the turn was cancelled. Its
        // generation is superseded, so the guard discards it and the state
        // stays Idle — no second RequestLlm.
        let applied = route_retry_timeout(&mut rt, stale_gen, 2).await;
        assert!(
            !applied,
            "a superseded-generation RetryTimeout must be discarded"
        );
        assert!(
            matches!(rt.state, ConvState::Idle),
            "stale RetryTimeout must not move state out of Idle, got {:?}",
            rt.state.variant_name()
        );
    }

    /// The legitimate retry path is unaffected: a `RetryTimeout` whose
    /// generation matches the live timer dispatches the next attempt.
    #[tokio::test]
    async fn live_retry_timeout_still_fires() {
        let mut rt = runtime_requesting();

        rt.process_event(retryable_llm_error())
            .await
            .expect("retryable error transitions");
        assert!(rt.retry_timer_handle.is_some());
        let live_gen = rt.retry_generation;

        // The live timer fires attempt 2 while still in LlmRequesting{2}. Its
        // generation is current, so it is accepted, clears the handle, and keeps
        // the conversation progressing in LlmRequesting.
        let applied = route_retry_timeout(&mut rt, live_gen, 2).await;
        assert!(applied, "the live-generation RetryTimeout must be applied");
        assert!(
            matches!(rt.state, ConvState::LlmRequesting { attempt: 2 }),
            "live RetryTimeout re-requests the LLM in LlmRequesting, got {:?}",
            rt.state.variant_name()
        );
        assert!(
            rt.retry_timer_handle.is_none(),
            "the spent timer handle must be cleared after a live timeout fires"
        );
    }

    /// Finding #1: an OLD `RetryTimeout` is already queued AND a NEWER backoff
    /// timer is pending (handle = `Some` for the new timer). The old fire must
    /// be discarded by generation WITHOUT disturbing the new timer, and the
    /// new (live) timer's own fire must still be honored — the turn must not be
    /// wedged in backoff. A handle-presence guard would mishandle this: the old
    /// fire would clear the handle, the reducer would reject it on attempt
    /// mismatch, and the live fire would then be dropped as "no retry pending".
    #[tokio::test]
    async fn old_timer_does_not_wedge_a_newer_live_timer() {
        let mut rt = runtime_requesting();

        // First retryable error opens generation G1 (the OLD timer), attempt 2.
        rt.process_event(retryable_llm_error())
            .await
            .expect("first retryable error transitions");
        assert!(matches!(rt.state, ConvState::LlmRequesting { attempt: 2 }));
        let old_gen = rt.retry_generation;

        // A second retryable error re-schedules: this supersedes the old timer
        // (generation G2 = the NEW, live timer), attempt 3. The handle now
        // points at the new timer, so a handle-presence guard would treat the
        // stale old fire as live.
        rt.process_event(retryable_llm_error())
            .await
            .expect("second retryable error transitions");
        assert!(matches!(rt.state, ConvState::LlmRequesting { attempt: 3 }));
        let live_gen = rt.retry_generation;
        assert!(
            live_gen > old_gen,
            "re-scheduling must open a newer retry generation"
        );
        assert!(
            rt.retry_timer_handle.is_some(),
            "a (new) retry timer must be pending"
        );

        // The OLD timer's fire (attempt 2, generation G1) now arrives. It must
        // be discarded by the generation guard and must NOT touch the handle.
        let old_applied = route_retry_timeout(&mut rt, old_gen, 2).await;
        assert!(
            !old_applied,
            "the old, superseded timer fire must be discarded"
        );
        assert!(
            rt.retry_timer_handle.is_some(),
            "discarding the old fire must not disturb the live timer's handle"
        );
        assert!(
            matches!(rt.state, ConvState::LlmRequesting { attempt: 3 }),
            "the stale old fire must not move the turn, got {:?}",
            rt.state.variant_name()
        );

        // The LIVE timer's own fire (attempt 3, generation G2) arrives. It must
        // still be honored — the turn re-requests the LLM and is not wedged.
        let live_applied = route_retry_timeout(&mut rt, live_gen, 3).await;
        assert!(
            live_applied,
            "the live timer's current-generation fire must still be honored"
        );
        assert!(
            matches!(rt.state, ConvState::LlmRequesting { attempt: 3 }),
            "the live RetryTimeout re-requests the LLM, got {:?}",
            rt.state.variant_name()
        );
        assert!(
            rt.retry_timer_handle.is_none(),
            "the spent live timer handle is cleared after its fire is honored"
        );
    }
}

// ============================================================
// Decoupled fork-proposal persistence (REQ-PROJ-033)
// ============================================================
//
// Distilled from specs/bedrock/bedrock.allium (ForkProposalIntercepted):
// the synthetic success ack and the fork_proposals row must both be durable
// after a successful persist, and the stored task_file is repository-relative.

#[cfg(test)]
mod fork_proposal_persist_tests {
    use super::test_git_helpers::init_repo;
    use super::*;
    use crate::db::MessageContent;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::state_machine::state::AssistantMessage;
    use crate::state_machine::{CheckpointData, ConvContext};
    use crate::tools::BrowserSessionManager;
    use phoenix_llm::ModelRegistry;
    use std::path::Path;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    type TestRuntime =
        ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>;

    fn runtime_in_dir(conv_id: &str, working_dir: &Path) -> (TestRuntime, Arc<InMemoryStorage>) {
        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new(conv_id, working_dir.to_path_buf(), "test-model", 200_000);
        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);
        let rt = ConversationRuntime::new(
            context,
            ConvState::LlmRequesting { attempt: 1 },
            storage.clone(),
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        );
        (rt, storage)
    }

    fn fork_effect(proposal_id: &str, task_file: &str) -> Effect {
        let assistant = AssistantMessage::new(
            "asst-fork".to_string(),
            vec![phoenix_llm::ContentBlock::ToolUse {
                id: "tool-fork-1".to_string(),
                name: "propose_task".to_string(),
                input: serde_json::json!({ "task_file": task_file }),
            }],
            None,
            None,
        );
        let ack = ToolResult::success_with_display(
            "tool-fork-1".to_string(),
            "Fork proposal recorded — pending your review; continue your work".to_string(),
            Some(serde_json::json!({ "fork_proposal_id": proposal_id })),
        );
        let checkpoint = CheckpointData::tool_round(assistant, vec![ack]).expect("tool_round");
        Effect::PersistForkProposal {
            proposal_id: proposal_id.to_string(),
            task_file: task_file.to_string(),
            title: "Fix the bug".to_string(),
            priority: phoenix_core::task_source::Priority::P1,
            body: "# Fix the bug\n\nplan body for the fork".to_string(),
            checkpoint,
        }
    }

    fn assert_round_persisted(storage: &InMemoryStorage, conv_id: &str, proposal_id: &str) {
        let msgs = storage.get_all_messages(conv_id);
        assert!(
            msgs.iter().any(|m| m.message_id == "asst-fork"),
            "assistant message must be persisted"
        );
        let ack = msgs
            .iter()
            .find(|m| m.message_id == tool_result_message_id("tool-fork-1"))
            .expect("synthetic success ack must be persisted");
        assert!(
            matches!(&ack.content, MessageContent::Tool(tc) if !tc.is_error),
            "ack must be a success tool result"
        );
        assert_eq!(
            ack.display_data
                .as_ref()
                .and_then(|d| d.get("fork_proposal_id"))
                .and_then(|v| v.as_str()),
            Some(proposal_id),
            "ack display_data must carry the fork_proposal_id handle"
        );
    }

    /// Worktree-top origin: `working_dir` IS the repo root, so the stored
    /// `task_file` equals the working-dir-relative path unchanged.
    #[tokio::test]
    async fn persist_at_repo_root_keeps_task_file() {
        let (_tmp, root) = init_repo();
        let (mut rt, storage) = runtime_in_dir("conv-fork-root", &root);

        let task_file = "tasks/00042-p1-ready--fix-thing.md";
        rt.execute_effect(fork_effect("fp-root", task_file))
            .await
            .expect("PersistForkProposal must succeed");

        let proposals = storage.get_fork_proposals();
        assert_eq!(proposals.len(), 1);
        let p = &proposals[0];
        assert_eq!(p.id, "fp-root");
        assert_eq!(p.origin_conversation_id, "conv-fork-root");
        assert_eq!(p.status, crate::db::ForkProposalStatus::Pending);
        assert_eq!(p.task_file, task_file, "repo-root origin: path unchanged");
        assert_eq!(p.title, "Fix the bug");
        assert_eq!(p.priority, "p1");
        assert!(p.body.contains("plan body for the fork"));
        assert!(p.fork_conversation_id.is_none());
        assert!(p.resolved_at.is_none());

        assert_round_persisted(&storage, "conv-fork-root", "fp-root");
    }

    /// Direct-in-subdir origin: `working_dir` is a subdir of the repo root, so
    /// the stored `task_file` is prefixed with the subdir offset.
    #[tokio::test]
    async fn persist_in_subdir_prefixes_offset() {
        let (_tmp, root) = init_repo();
        let subdir = root.join("crate-a");
        std::fs::create_dir_all(&subdir).unwrap();
        let (mut rt, storage) = runtime_in_dir("conv-fork-sub", &subdir);

        let task_file = "tasks/00099-p2-ready--sub-thing.md";
        rt.execute_effect(fork_effect("fp-sub", task_file))
            .await
            .expect("PersistForkProposal must succeed");

        let proposals = storage.get_fork_proposals();
        assert_eq!(proposals.len(), 1);
        assert_eq!(
            proposals[0].task_file,
            format!("crate-a/{task_file}"),
            "subdir origin: stored path gains the subdir offset"
        );

        assert_round_persisted(&storage, "conv-fork-sub", "fp-sub");
    }
}

#[cfg(test)]
mod tool_output_cap_tests {
    use super::{cap_tool_output_text, MAX_TOOL_OUTPUT_BYTES};

    #[test]
    fn output_within_cap_is_unchanged() {
        let small = "a".repeat(MAX_TOOL_OUTPUT_BYTES);
        assert_eq!(cap_tool_output_text(small.clone()), small);
    }

    #[test]
    fn giant_single_line_is_truncated_with_marker_and_under_cap() {
        // The pathological case: a multi-MB single line with no newlines that
        // every line-based cap lets through.
        let original = "x".repeat(2 * 1024 * 1024);
        let capped = cap_tool_output_text(original);

        assert!(
            capped.len() <= MAX_TOOL_OUTPUT_BYTES,
            "capped length {} must be <= {MAX_TOOL_OUTPUT_BYTES}",
            capped.len()
        );
        assert!(
            capped.contains("…[truncated"),
            "truncation marker must be present: {:?}",
            capped.chars().take(120).collect::<String>()
        );
        // String is always valid UTF-8 in Rust; assert it explicitly survives a
        // round-trip through bytes to guard the char-boundary slicing logic.
        assert!(std::str::from_utf8(capped.as_bytes()).is_ok());
    }

    #[test]
    fn truncation_does_not_split_utf8_char() {
        // A long run of 4-byte chars (😀) ensures slice boundaries land mid-char
        // unless snapped. If snapping were wrong, the result would be invalid
        // UTF-8 (impossible to even construct) or drop/duplicate bytes.
        let emoji = "😀";
        assert_eq!(emoji.len(), 4);
        let original = emoji.repeat(MAX_TOOL_OUTPUT_BYTES); // ~400 KB
        let capped = cap_tool_output_text(original);

        assert!(capped.len() <= MAX_TOOL_OUTPUT_BYTES);
        assert!(std::str::from_utf8(capped.as_bytes()).is_ok());
        // Every retained char must be a whole emoji or part of the ASCII marker;
        // no replacement chars or partial sequences.
        assert!(!capped.contains('\u{FFFD}'));
    }
}

/// Stale tool-result clearing (specs/stale-tool-results): the planner advances a
/// monotonic watermark under context pressure, never into the recency floor and
/// only for re-queryable (clearable) results; the renderer placeholders cleared
/// results and preserves kept ones with their images.
#[cfg(test)]
mod stale_tool_result_clearing_tests {
    use super::*;
    use crate::db::{MessageContent, ToolContentImage};
    use crate::runtime::testing::InMemoryStorage;
    use crate::runtime::traits::{MessageStore, StateStore};
    use phoenix_llm::{ContentBlock, MessageRole};
    use std::collections::HashSet;
    use std::sync::Arc;

    // Each image is IMAGE_TOKEN_ESTIMATE (1500) tokens, divisor-independent — so
    // a result's weight is exactly known. Six images = 9000 tokens, above
    // CLEAR_AT_LEAST_TOKENS (8192).
    fn pngs(tag: &str, n: usize) -> Vec<ToolContentImage> {
        (0..n)
            .map(|i| ToolContentImage {
                media_type: "image/png".to_string(),
                data: format!("data-{tag}-{i}"),
            })
            .collect()
    }

    /// Persist a message and return its assigned `sequence_id`.
    async fn add(storage: &Arc<InMemoryStorage>, conv: &str, content: MessageContent) -> i64 {
        storage
            .add_message(
                &uuid::Uuid::new_v4().to_string(),
                conv,
                &content,
                None,
                None,
            )
            .await
            .expect("add_message")
            .sequence_id
    }

    /// Append one tool round: an assistant `tool_use` for `tool_name` plus one
    /// tool-result carrying `n_images` images. The `tool_use_id` is `{tag}-tool`;
    /// returns the tool-result message's `sequence_id` (the cleared-set key).
    async fn add_round(
        storage: &Arc<InMemoryStorage>,
        conv: &str,
        tag: &str,
        tool_name: &str,
        n_images: usize,
    ) -> i64 {
        let id = format!("{tag}-tool");
        add(
            storage,
            conv,
            MessageContent::agent(vec![ContentBlock::ToolUse {
                id: id.clone(),
                name: tool_name.to_string(),
                input: serde_json::json!({}),
            }]),
        )
        .await;
        add(
            storage,
            conv,
            MessageContent::tool_with_images(&id, "result body", false, pngs(tag, n_images)),
        )
        .await
    }

    fn clearable(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    /// Above the high-water mark, an old clearable round is swept: the watermark
    /// advances, the round is in the cleared set, the recent floor is untouched,
    /// and the freed-token / count figures are reported.
    #[tokio::test]
    async fn sweeps_old_clearable_round_under_pressure() {
        let storage = Arc::new(InMemoryStorage::new());
        let conv = "conv-sweep";
        // Round 0 is the only round outside the KEEP_RECENT_ROUNDS=3 floor.
        let old = add_round(&storage, conv, "old", "read_file", 6).await; // 9000 tokens
        let r1 = add_round(&storage, conv, "r1", "read_file", 1).await;
        let r2 = add_round(&storage, conv, "r2", "read_file", 1).await;
        let r3 = add_round(&storage, conv, "r3", "read_file", 1).await;

        let db = storage.get_messages(conv).await.unwrap();
        // Last turn's reported prompt (12_000) is above the trigger (0.7 * 1000).
        let plan =
            plan_tool_result_clearing(&db, &clearable(&["read_file"]), 0, Some(12_000), 1_000);

        assert!(plan.new_watermark > 0, "watermark must advance");
        assert!(
            plan.cleared_sequence_ids.contains(&old),
            "old clearable round must be cleared",
        );
        for kept in [&r1, &r2, &r3] {
            assert!(
                !plan.cleared_sequence_ids.contains(kept),
                "recency-floor round {kept} must never be cleared",
            );
        }
        assert!(plan.freed_tokens >= CLEAR_AT_LEAST_TOKENS);
        assert_eq!(plan.cleared_count, 1);
    }

    /// Below the high-water mark nothing is cleared, however old the rounds.
    #[tokio::test]
    async fn holds_below_high_water_mark() {
        let storage = Arc::new(InMemoryStorage::new());
        let conv = "conv-hold";
        add_round(&storage, conv, "old", "read_file", 6).await;
        add_round(&storage, conv, "r1", "read_file", 1).await;
        add_round(&storage, conv, "r2", "read_file", 1).await;
        add_round(&storage, conv, "r3", "read_file", 1).await;

        let db = storage.get_messages(conv).await.unwrap();
        // Reported prompt (5_000) sits far below the trigger (0.7 * 200_000).
        let plan =
            plan_tool_result_clearing(&db, &clearable(&["read_file"]), 0, Some(5_000), 200_000);

        assert_eq!(plan.new_watermark, 0);
        assert!(plan.cleared_sequence_ids.is_empty());
        assert_eq!(plan.freed_tokens, 0);
    }

    /// An unclearable tool's result is never cleared, even old and under pressure.
    #[tokio::test]
    async fn never_clears_unclearable_tool() {
        let storage = Arc::new(InMemoryStorage::new());
        let conv = "conv-unclearable";
        let old = add_round(&storage, conv, "old", "ask_user_question", 6).await;
        add_round(&storage, conv, "r1", "read_file", 1).await;
        add_round(&storage, conv, "r2", "read_file", 1).await;
        add_round(&storage, conv, "r3", "read_file", 1).await;

        let db = storage.get_messages(conv).await.unwrap();
        // read_file is clearable; ask_user_question is not. Over pressure (12_000).
        let plan =
            plan_tool_result_clearing(&db, &clearable(&["read_file"]), 0, Some(12_000), 1_000);

        assert!(
            !plan.cleared_sequence_ids.contains(&old),
            "unclearable result must never be cleared",
        );
    }

    /// A maximal sweep clears *every* eligible round below the floor in one move
    /// (not one round per turn), and once swept the watermark holds even while
    /// pressure persists — there is nothing new below the floor to clear, so the
    /// cached prefix stays stable across turns (REQ-STR-007).
    #[tokio::test]
    async fn maximal_sweep_clears_to_floor_then_holds() {
        let storage = Arc::new(InMemoryStorage::new());
        let conv = "conv-stable";
        let names = clearable(&["read_file"]);
        // Five rounds → floor_first_round = 2, so rounds 0 and 1 are both
        // pre-floor. Each carries 6 images (9000 tokens), so together they clear
        // far more than CLEAR_AT_LEAST_TOKENS.
        let old0 = add_round(&storage, conv, "old0", "read_file", 6).await;
        let old1 = add_round(&storage, conv, "old1", "read_file", 6).await;
        let r2 = add_round(&storage, conv, "r2", "read_file", 1).await;
        add_round(&storage, conv, "r3", "read_file", 1).await;
        add_round(&storage, conv, "r4", "read_file", 1).await;
        let window = 20_000; // trigger = 14_000

        let db = storage.get_messages(conv).await.unwrap();

        // Turn 1, over pressure: both pre-floor rounds are cleared in one sweep.
        let first = plan_tool_result_clearing(&db, &names, 0, Some(18_000), window);
        assert!(first.cleared_sequence_ids.contains(&old0));
        assert!(
            first.cleared_sequence_ids.contains(&old1),
            "maximal sweep clears the whole pre-floor prefix, not just the oldest",
        );
        assert!(
            !first.cleared_sequence_ids.contains(&r2),
            "the recency floor is never cleared",
        );
        assert_eq!(first.cleared_count, 2);

        // Turn 2, still over pressure, same history: nothing new is eligible below
        // the floor, so the watermark holds and no further tokens are freed.
        let second =
            plan_tool_result_clearing(&db, &names, first.new_watermark, Some(18_000), window);
        assert_eq!(
            second.new_watermark, first.new_watermark,
            "watermark holds once the pre-floor prefix is cleared, even under pressure",
        );
        assert!(second.cleared_sequence_ids.contains(&old0));
        assert!(second.cleared_sequence_ids.contains(&old1));
        assert_eq!(
            second.freed_tokens, 0,
            "no new tokens freed on a stable turn"
        );
    }

    /// `None` reported usage (no prior turn) is treated as not under pressure, so
    /// the watermark never advances on the very first turn however large history is.
    #[tokio::test]
    async fn no_reported_usage_holds_watermark() {
        let storage = Arc::new(InMemoryStorage::new());
        let conv = "conv-firstturn";
        add_round(&storage, conv, "old", "read_file", 6).await;
        add_round(&storage, conv, "r1", "read_file", 1).await;
        add_round(&storage, conv, "r2", "read_file", 1).await;
        add_round(&storage, conv, "r3", "read_file", 1).await;

        let db = storage.get_messages(conv).await.unwrap();
        let plan = plan_tool_result_clearing(&db, &clearable(&["read_file"]), 0, None, 1_000);

        assert_eq!(plan.new_watermark, 0);
        assert!(plan.cleared_sequence_ids.is_empty());
    }

    /// Production wraps the tool registry in `Arc`, and the runtime caches the
    /// clearable set from it. The `Arc<T>` blanket impl must forward
    /// `clearable_tool_names` — falling back to the empty default would silently
    /// make nothing clearable, disabling the whole feature in production while
    /// the planner unit tests (which pass the set directly) stay green.
    #[tokio::test]
    async fn arc_tool_executor_forwards_clearable_tool_names() {
        use crate::runtime::testing::MockToolExecutor;
        use crate::runtime::traits::ToolExecutor;

        let arc: Arc<MockToolExecutor> =
            Arc::new(MockToolExecutor::new().with_clearable_tool("bash"));
        assert!(
            arc.clearable_tool_names().contains("bash"),
            "Arc<T> must forward clearable_tool_names, not use the empty default",
        );
    }

    /// True if `tool_use_id` is rendered as a cleared placeholder (no images).
    fn is_rendered_cleared(messages: &[LlmMessage], tool_use_id: &str) -> bool {
        messages.iter().any(|m| {
            m.content.iter().any(|b| {
                matches!(b,
                    ContentBlock::ToolResult { tool_use_id: id, content, images, .. }
                    if id == tool_use_id
                        && content == CLEARED_TOOL_RESULT_PLACEHOLDER
                        && images.is_empty())
            })
        })
    }

    /// assemble path, happy case: over pressure → sweep applied and persisted.
    #[tokio::test]
    async fn assemble_sweeps_and_persists_under_pressure() {
        let storage = Arc::new(InMemoryStorage::new());
        let conv = "asm-happy";
        let names = clearable(&["read_file"]);
        add_round(&storage, conv, "old", "read_file", 6).await;
        add_round(&storage, conv, "r1", "read_file", 1).await;
        add_round(&storage, conv, "r2", "read_file", 1).await;
        add_round(&storage, conv, "r3", "read_file", 1).await;
        storage.set_last_prompt_tokens(conv, 12_000); // > trigger (0.7 * 1000)
        let db = storage.get_messages(conv).await.unwrap();
        let cache = std::sync::Mutex::new(None);

        let messages = assemble_cleared_messages(&*storage, conv, &db, &names, 1_000, &cache).await;
        assert!(
            is_rendered_cleared(&messages, "old-tool"),
            "old round rendered as placeholder"
        );
        assert!(
            storage.get_clear_watermark(conv).await.unwrap() > 0,
            "advance persisted",
        );
    }

    /// assemble path, watermark read failure with nothing cached: send the
    /// uncleared history rather than re-plan from an unknown baseline (REQ-STR-007).
    #[tokio::test]
    async fn assemble_read_failure_with_empty_cache_sends_uncleared() {
        let storage = Arc::new(InMemoryStorage::new());
        let conv = "asm-readfail";
        let names = clearable(&["read_file"]);
        add_round(&storage, conv, "old", "read_file", 6).await;
        add_round(&storage, conv, "r1", "read_file", 1).await;
        add_round(&storage, conv, "r2", "read_file", 1).await;
        add_round(&storage, conv, "r3", "read_file", 1).await;
        storage.set_last_prompt_tokens(conv, 12_000);
        storage.set_fail_watermark_read(true);
        let db = storage.get_messages(conv).await.unwrap();
        let cache = std::sync::Mutex::new(None);

        let messages = assemble_cleared_messages(&*storage, conv, &db, &names, 1_000, &cache).await;
        assert!(
            !is_rendered_cleared(&messages, "old-tool"),
            "read failure with empty cache → nothing cleared this turn",
        );
    }

    /// assemble path, watermark read failure *after* a prior successful turn: the
    /// in-memory cache holds the advanced watermark, so previously-cleared
    /// results stay cleared rather than being re-exposed (REQ-STR-007).
    #[tokio::test]
    async fn assemble_read_failure_preserves_cached_cleared_set() {
        let storage = Arc::new(InMemoryStorage::new());
        let conv = "asm-readfail-cached";
        let names = clearable(&["read_file"]);
        add_round(&storage, conv, "old", "read_file", 6).await;
        add_round(&storage, conv, "r1", "read_file", 1).await;
        add_round(&storage, conv, "r2", "read_file", 1).await;
        add_round(&storage, conv, "r3", "read_file", 1).await;
        storage.set_last_prompt_tokens(conv, 12_000);
        let db = storage.get_messages(conv).await.unwrap();
        let cache = std::sync::Mutex::new(None);

        // Turn 1: a real sweep advances and caches the watermark; `old` is cleared.
        let first = assemble_cleared_messages(&*storage, conv, &db, &names, 1_000, &cache).await;
        assert!(is_rendered_cleared(&first, "old-tool"));

        // Turn 2: the watermark read fails, but the cache holds the advanced
        // value, so the previously-cleared result stays cleared.
        storage.set_fail_watermark_read(true);
        let second = assemble_cleared_messages(&*storage, conv, &db, &names, 1_000, &cache).await;
        assert!(
            is_rendered_cleared(&second, "old-tool"),
            "read failure must preserve the cached cleared set, not un-clear it",
        );
    }

    /// assemble path, watermark write failure: render the prior watermark's set
    /// (here empty), matching what is durably recorded, and do not advance.
    #[tokio::test]
    async fn assemble_write_failure_holds_prior_set() {
        let storage = Arc::new(InMemoryStorage::new());
        let conv = "asm-writefail";
        let names = clearable(&["read_file"]);
        add_round(&storage, conv, "old", "read_file", 6).await;
        add_round(&storage, conv, "r1", "read_file", 1).await;
        add_round(&storage, conv, "r2", "read_file", 1).await;
        add_round(&storage, conv, "r3", "read_file", 1).await;
        storage.set_last_prompt_tokens(conv, 12_000);
        storage.set_fail_watermark_write(true);
        let db = storage.get_messages(conv).await.unwrap();
        let cache = std::sync::Mutex::new(None);

        let messages = assemble_cleared_messages(&*storage, conv, &db, &names, 1_000, &cache).await;
        assert!(
            !is_rendered_cleared(&messages, "old-tool"),
            "write failure → prior (empty) cleared set rendered, not the failed advance",
        );
        assert_eq!(
            storage.get_clear_watermark(conv).await.unwrap(),
            0,
            "watermark not advanced when the write failed",
        );
    }

    /// The renderer placeholders cleared results (no images) and sends kept
    /// results verbatim with their images.
    #[tokio::test]
    async fn render_placeholders_cleared_keeps_rest() {
        let storage = Arc::new(InMemoryStorage::new());
        let conv = "conv-render";
        let cleared_seq = add_round(&storage, conv, "old", "read_file", 2).await;
        add_round(&storage, conv, "new", "read_file", 2).await;

        let db = storage.get_messages(conv).await.unwrap();
        let cleared: HashSet<i64> = std::iter::once(cleared_seq).collect();
        let msgs = render_messages(&db, &cleared);

        let find = |id: &str| -> (String, usize) {
            for m in &msgs {
                if m.role == MessageRole::User {
                    for b in &m.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            images,
                            ..
                        } = b
                        {
                            if tool_use_id == id {
                                return (content.clone(), images.len());
                            }
                        }
                    }
                }
            }
            panic!("no tool result for {id}");
        };

        let (cleared_body, cleared_imgs) = find("old-tool");
        assert_eq!(cleared_body, CLEARED_TOOL_RESULT_PLACEHOLDER);
        assert_eq!(cleared_imgs, 0, "cleared result emits no images");

        let (kept_body, kept_imgs) = find("new-tool");
        assert_eq!(kept_body, "result body");
        assert_eq!(kept_imgs, 2, "kept result preserves its images");
    }
}

/// An intentional `Effect::AbortLlm` aborts the in-flight LLM task, dropping its
/// outcome sender — the forwarder maps that drop to a retryable `NetworkError`.
/// Without a generation guard, that synthetic outcome (which carries no request
/// identity) could be applied to a later, unrelated `LlmRequesting` turn and
/// trigger a spurious retry/error. Each dispatch opens a new generation and the
/// forwarder stamps it; an outcome whose generation is no longer current is
/// discarded.
#[cfg(test)]
mod llm_generation_guard_tests {
    use super::*;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::state_machine::outcome::LlmOutcome;
    use crate::tools::BrowserSessionManager;
    use phoenix_llm::ModelRegistry;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    type TestRuntime =
        ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>;

    fn runtime_requesting() -> TestRuntime {
        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new("conv-gen", PathBuf::from("/tmp"), "test-model", 200_000);
        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);
        ConversationRuntime::new(
            context,
            ConvState::LlmRequesting { attempt: 1 },
            storage,
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        )
    }

    /// `Effect::AbortLlm` bumps the in-flight generation, so an outcome stamped
    /// with the pre-abort generation is classified stale.
    #[tokio::test]
    async fn abort_supersedes_pre_abort_generation() {
        let mut rt = runtime_requesting();

        // Simulate a dispatch having opened generation 1.
        rt.llm_request_generation = 1;
        let dispatched_gen = rt.llm_request_generation;
        assert!(
            !rt.llm_outcome_is_stale(dispatched_gen),
            "the in-flight generation is current before any abort"
        );

        rt.execute_effect(Effect::AbortLlm)
            .await
            .expect("AbortLlm executes");

        assert!(
            rt.llm_request_generation > dispatched_gen,
            "AbortLlm must open a new generation"
        );
        assert!(
            rt.llm_outcome_is_stale(dispatched_gen),
            "the aborted request's outcome must be stale after AbortLlm"
        );
    }

    /// End-to-end of the select-loop guard: a stale-generation `NetworkError`
    /// (the synthetic outcome a forwarder emits when the aborted task drops its
    /// sender) is discarded — it does NOT transition the state. A current-gen
    /// outcome would be processed, proving the guard is selective, not a blanket
    /// drop.
    #[tokio::test]
    async fn stale_generation_network_error_is_discarded() {
        let mut rt = runtime_requesting();
        rt.llm_request_generation = 2; // current in-flight generation

        let stale_outcome = LlmOutcome::NetworkError {
            message: "LLM task aborted or panicked".to_string(),
        };

        // Mirror the select-arm decision: a stale generation is dropped without
        // touching state.
        let stale_gen = 1;
        assert!(
            rt.llm_outcome_is_stale(stale_gen),
            "generation 1 is stale relative to current generation 2"
        );

        let state_before = rt.state.variant_name();
        if !rt.llm_outcome_is_stale(stale_gen) {
            rt.process_outcome(EffectOutcome::Llm(stale_outcome))
                .await
                .expect("would process if current");
        }
        assert_eq!(
            rt.state.variant_name(),
            state_before,
            "a stale aborted outcome must not move state — no spurious retry/error"
        );
    }

    /// The current-generation outcome is honoured: not stale, so it flows into
    /// `process_outcome`. Guards against an over-broad guard that would wedge
    /// genuine outcomes (e.g. a real panic on the in-flight request).
    #[tokio::test]
    async fn current_generation_outcome_is_not_stale() {
        let mut rt = runtime_requesting();
        rt.llm_request_generation = 5;
        assert!(
            !rt.llm_outcome_is_stale(5),
            "an outcome stamped with the current generation must be processed"
        );
    }
}

/// The tool path's generation guard mirrors the LLM path's. An intentional
/// `Effect::AbortTool` / `CancellingTool` backstop teardown drops the aborted
/// tool task's outcome sender; the forwarder maps that drop to a synthetic
/// `Failed` outcome. The state machine's id/state gate accepts a tool outcome
/// only when its `tool_use_id` matches the current tool AND the conversation is
/// in a tool state — but `tool_use_id` uniqueness across turns is
/// provider-dependent, not structural. If a later turn reuses the same id while
/// the stale outcome is still in flight, the id/state gate alone would mistake
/// the stale failure for the new tool's result. The generation guard closes
/// this: each dispatch opens a new generation and the forwarder stamps it; an
/// outcome whose generation is no longer current is discarded before reaching
/// the reducer. REQ-BED-005a.
#[cfg(test)]
mod tool_generation_guard_tests {
    use super::*;
    use crate::runtime::testing::{InMemoryStorage, MockLlmClient, MockToolExecutor};
    use crate::state_machine::outcome::ToolExecOutcome;
    use crate::state_machine::state::{AssistantMessage, ToolCall, ToolInput};
    use crate::tools::BrowserSessionManager;
    use phoenix_llm::ContentBlock;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    type TestRuntime =
        ConversationRuntime<Arc<InMemoryStorage>, Arc<MockLlmClient>, Arc<MockToolExecutor>>;

    /// Build a runtime parked in `ToolExecuting` for `tool_use_id`, modelling a
    /// live tool round whose stale outcome could later collide on id reuse.
    fn runtime_tool_executing(tool_use_id: &str) -> TestRuntime {
        let storage = Arc::new(InMemoryStorage::new());
        let context = ConvContext::new(
            "conv-tool-gen",
            PathBuf::from("/tmp"),
            "test-model",
            200_000,
        );
        let (_event_tx, event_rx) = mpsc::channel(32);
        let event_tx_dup = mpsc::channel::<Event>(1).0;
        let broadcaster = SseBroadcaster::new(128, 0);

        let assistant_message = AssistantMessage::new(
            uuid::Uuid::new_v4().to_string(),
            vec![ContentBlock::ToolUse {
                id: tool_use_id.to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({}),
            }],
            None,
            None,
        );
        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                tool_use_id,
                ToolInput::Bash(crate::tools::BashToolInput::run("cmd")),
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message,
        };

        ConversationRuntime::new(
            context,
            state,
            storage,
            Arc::new(MockLlmClient::new("test-model")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx_dup,
            broadcaster,
        )
    }

    /// `Effect::AbortTool` must NOT bump the tool generation: it only cancels
    /// the token, and the cooperative tool task then produces a real `Aborted`
    /// outcome (carrying the dispatch generation) that the state machine
    /// consumes to reach Idle. Bumping here would classify that legitimate
    /// outcome stale and wedge cooperative cancellation.
    #[tokio::test]
    async fn abort_tool_does_not_supersede_cooperative_outcome() {
        let mut rt = runtime_tool_executing("X");

        // Simulate a dispatch having opened generation 1.
        rt.tool_request_generation = 1;
        let dispatched_gen = rt.tool_request_generation;

        rt.execute_effect(Effect::AbortTool {
            tool_use_id: "X".to_string(),
        })
        .await
        .expect("AbortTool executes");

        assert_eq!(
            rt.tool_request_generation, dispatched_gen,
            "AbortTool must NOT bump the generation — the cooperative Aborted outcome \
             carries this generation and must remain current"
        );
        assert!(
            !rt.tool_outcome_is_stale(dispatched_gen),
            "the cooperative tool's Aborted outcome must still be current after AbortTool"
        );
    }

    /// The #4 race the id/state gate alone CANNOT catch: a forced cancel reaches
    /// Idle, a stale tool outcome for `tool_use_id = "X"` is still in flight, and
    /// the NEXT tool round dispatches a tool that REUSES id "X". Because the
    /// id/state gate would match (same id, again in `ToolExecuting`), only the
    /// generation guard can reject the stale outcome. This pins that: the stale
    /// outcome carries the aborted round's generation, which is no longer
    /// current after the new dispatch bumped it, so it is classified stale and
    /// must NOT be applied to the new round.
    #[tokio::test]
    async fn stale_tool_outcome_with_reused_id_is_rejected_by_generation_guard() {
        // Round 1 dispatches a tool with id "X" → generation 1.
        let mut rt = runtime_tool_executing("X");
        rt.tool_request_generation = 1;
        let round1_gen = rt.tool_request_generation;

        // A forced cancellation reaches Idle. Round 1's wedged tool task is still
        // in flight; its forwarder will later enqueue a stale outcome stamped
        // with `round1_gen`. A new tool round then dispatches a tool that REUSES
        // id "X", bumping the generation (mirrors the dispatch site opening a
        // fresh generation on every tool round).
        rt.tool_request_generation = rt.tool_request_generation.wrapping_add(1);
        let round2_gen = rt.tool_request_generation;
        assert_ne!(
            round1_gen, round2_gen,
            "the reused-id round opens a new generation"
        );

        // The stale outcome for the SAME tool_use_id "X" from round 1 arrives.
        let stale_outcome = ToolExecOutcome::Failed {
            tool_use_id: "X".to_string(),
            error: "tool task aborted or panicked".to_string(),
        };

        // The id/state gate would accept this (id "X" matches the current tool,
        // state is ToolExecuting), but the generation guard rejects it first.
        assert!(
            rt.tool_outcome_is_stale(round1_gen),
            "round 1's generation is stale relative to the reused-id round's generation"
        );

        // Mirror the select-arm decision: a stale generation is dropped without
        // touching state. Only a current-generation outcome reaches the reducer.
        let state_before = rt.state.variant_name();
        if !rt.tool_outcome_is_stale(round1_gen) {
            rt.process_outcome(EffectOutcome::Tool(stale_outcome))
                .await
                .expect("would process if current");
        }
        assert_eq!(
            rt.state.variant_name(),
            state_before,
            "a stale tool outcome reusing the current tool_use_id must not settle the new round"
        );
        assert!(
            matches!(rt.state, ConvState::ToolExecuting { .. }),
            "the reused-id round must remain in ToolExecuting, awaiting its own outcome"
        );
    }

    /// The current-generation outcome is honoured: not stale, so it flows into
    /// `process_outcome`. Guards against an over-broad guard that would wedge
    /// genuine outcomes (e.g. a real failure on the in-flight tool).
    #[tokio::test]
    async fn current_generation_tool_outcome_is_not_stale() {
        let mut rt = runtime_tool_executing("X");
        rt.tool_request_generation = 5;
        assert!(
            !rt.tool_outcome_is_stale(5),
            "an outcome stamped with the current generation must be processed"
        );
    }
}
