//! `browser_profile` — systematic web performance testing (REQ-BT-019).
//!
//! Behavioural contract: `specs/browser-tool/browser-profiling.allium`
//! Requirements: `specs/browser-tool/requirements.md` REQ-BT-019.1..12.
//!
//! ## Deliberate single-tool-with-action divergence
//!
//! Phoenix's norm is one struct per `Tool` name. This module deviates: a
//! single [`BrowserProfileTool`] exposes many capabilities through an
//! `action` discriminator. This is intentional and is mandated by
//! REQ-BT-019 ("A single `browser_profile` tool exposes performance
//! measurement and root-cause analysis through an `action` discriminator").
//! The capabilities form three start/stop sub-machines plus one-shot reads
//! that share conversation-scoped profiling state on the `BrowserSession`;
//! splitting them into separate tool structs would scatter that state and
//! the precondition gates the Allium spec requires.
//!
//! ## Raw-samples hard constraint (REQ-BT-019.5 / invariant
//! `RawSamplesNeverReduced`)
//!
//! `run_scenario` returns the raw per-run sample array verbatim. The harness
//! computes no mean/variance/significance. See `run_scenario` and the
//! `raw_samples` JSON key — there is exactly one place samples are emitted
//! and it is the untouched `Vec<RunSample>`.

use super::session::BrowserSession;
use crate::tools::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use chromiumoxide::cdp::browser_protocol::{
    emulation::SetCpuThrottlingRateParams,
    performance::{EnableParams as PerfEnableParams, GetMetricsParams},
    tracing::{EndParams, StartParams as TraceStartParams, TraceConfig},
};
use chromiumoxide::cdp::js_protocol::heap_profiler::{
    CollectGarbageParams, EnableParams as HeapEnableParams, EventAddHeapSnapshotChunk,
    TakeHeapSnapshotParams,
};
use chromiumoxide::cdp::js_protocol::profiler::{
    DisableParams as ProfilerDisableParams, EnableParams as ProfilerEnableParams,
    StartParams as ProfilerStartParams, StartPreciseCoverageParams,
    StopParams as ProfilerStopParams, StopPreciseCoverageParams, TakePreciseCoverageParams,
};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Output larger than this is escaped to a /tmp file; the summary line
/// references the path. Mirrors `browser_eval`'s 4096-byte escape.
const OUTPUT_ESCAPE_BYTES: usize = 4096;

/// Bound on awaiting `Tracing.tracingComplete` after `Tracing.end`.
const TRACE_COMPLETE_TIMEOUT: Duration = Duration::from_secs(30);

/// A long task in a trace is one whose `dur` (microseconds) exceeds this.
/// 50 ms is the standard "long task" threshold (REQ-BT-019.9).
const LONG_TASK_US: f64 = 50_000.0;

/// Macro counters the harness brackets each run with (REQ-BT-019.3).
const TRACKED_METRICS: &[&str] = &[
    "ScriptDuration",
    "TaskDuration",
    "LayoutCount",
    "RecalcStyleCount",
    "JSHeapUsedSize",
    "Nodes",
    "JSEventListeners",
];

// ============================================================================
// Input
// ============================================================================

#[derive(Debug, Deserialize)]
struct ProfileInput {
    action: String,
    /// `trace_start`: comma-separated category override.
    #[serde(default)]
    categories: Option<String>,
    /// `throttle`: slowdown factor (>= 1; 1 = no throttle).
    #[serde(default)]
    rate: Option<f64>,
    /// `run_scenario`: ordered steps.
    #[serde(default)]
    steps: Option<Vec<Step>>,
    /// `run_scenario`: measured runs (>= 1).
    #[serde(default)]
    runs: Option<u32>,
    /// `run_scenario`: discarded warmup runs (default 0).
    #[serde(default)]
    warmup: Option<u32>,
    /// `run_scenario`: throttle applied for the scenario only.
    #[serde(default)]
    throttle_rate: Option<f64>,
    /// `heap_snapshot`: optional baseline snapshot path to diff against.
    #[serde(default)]
    baseline: Option<String>,
}

/// One scenario step (REQ-BT-019.1). Readiness steps (`wait_*`) block the
/// run until satisfied or fail the whole operation on timeout.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Step {
    Navigate {
        url: String,
    },
    Reload,
    Click {
        selector: String,
    },
    Type {
        selector: String,
        text: String,
    },
    Key {
        key: String,
        #[serde(default)]
        modifiers: Vec<String>,
    },
    Eval {
        expression: String,
    },
    WaitSelector {
        selector: String,
        #[serde(default)]
        timeout: Option<String>,
    },
    WaitTiming {
        mark: String,
        #[serde(default)]
        timeout: Option<String>,
    },
    WaitEval {
        expression: String,
        #[serde(default)]
        timeout: Option<String>,
    },
}

/// Parse duration like "5s", "500ms", "1m". Mirrors `tools.rs::parse_duration`.
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        ms.trim().parse().ok().map(Duration::from_millis)
    } else if let Some(v) = s.strip_suffix('s') {
        v.trim().parse().ok().map(Duration::from_secs)
    } else if let Some(m) = s.strip_suffix('m') {
        m.trim()
            .parse::<u64>()
            .ok()
            .map(|m| Duration::from_secs(m * 60))
    } else {
        s.parse().ok().map(Duration::from_secs)
    }
}

const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Run `f` with the conversation's [`ProfilingState`] mutably locked,
/// without holding the session read-guard across the std `Mutex` (which
/// would create a borrow that outlives the guard). Returns `f`'s value, or
/// `None` if the state lock is poisoned.
async fn with_profiling<R>(
    session: &Arc<RwLock<BrowserSession>>,
    f: impl FnOnce(&mut super::session::ProfilingState) -> R,
) -> Option<R> {
    let profiling = {
        let guard = session.read().await;
        guard.profiling.clone()
    };
    let mut st = profiling.lock().ok()?;
    Some(f(&mut st))
}

// ============================================================================
// Tool
// ============================================================================

pub struct BrowserProfileTool;

/// Every action this tool exposes. Kept in one place so `input_schema` and
/// the dispatch match cannot drift, and the registration test can assert
/// the full surface.
pub const PROFILE_ACTIONS: &[&str] = &[
    "help",
    "metrics",
    "throttle",
    "gc_heap",
    "run_scenario",
    "cpu_start",
    "cpu_stop",
    "trace_start",
    "trace_stop",
    "why_render",
    "heap_snapshot",
    "coverage_start",
    "coverage_stop",
];

#[async_trait]
impl Tool for BrowserProfileTool {
    fn name(&self) -> &'static str {
        "browser_profile"
    }

    fn description(&self) -> String {
        "Systematic web performance testing and root-cause analysis (REQ-BT-019). \
         One tool, many actions via the `action` field. Call action=\"help\" for the \
         full action reference. Key actions: metrics (Performance.getMetrics snapshot), \
         run_scenario (deterministic multi-run harness returning RAW per-run samples — \
         never averaged; you compute significance), throttle (CPU slowdown), gc_heap \
         (forced GC then heap read), cpu_start/cpu_stop, trace_start/trace_stop, \
         coverage_start/coverage_stop, why_render, heap_snapshot."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": PROFILE_ACTIONS,
                    "description": "The profiling action. Call action=\"help\" for details."
                },
                "categories": {
                    "type": "string",
                    "description": "trace_start only: comma-separated trace categories override."
                },
                "rate": {
                    "type": "number",
                    "description": "throttle only: CPU slowdown factor (>= 1; 1 clears throttle)."
                },
                "steps": {
                    "type": "array",
                    "description": "run_scenario only: ordered steps. Each step is an object with a `kind`: navigate{url}, reload, click{selector}, type{selector,text}, key{key,modifiers?}, eval{expression}, wait_selector{selector,timeout?}, wait_timing{mark,timeout?}, wait_eval{expression,timeout?}.",
                    "items": { "type": "object" }
                },
                "runs": {
                    "type": "integer",
                    "description": "run_scenario only: number of measured runs (>= 1)."
                },
                "warmup": {
                    "type": "integer",
                    "description": "run_scenario only: warmup runs discarded from results (default 0)."
                },
                "throttle_rate": {
                    "type": "number",
                    "description": "run_scenario only: CPU slowdown applied for the scenario, restored after (>= 1)."
                },
                "baseline": {
                    "type": "string",
                    "description": "heap_snapshot only: path to a baseline .heapsnapshot to diff against."
                }
            },
            "required": ["action"]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: ProfileInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        // help needs no browser.
        if input.action == "help" {
            return ToolOutput::success(help_text());
        }

        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        match input.action.as_str() {
            "metrics" => action_metrics(&session).await,
            "throttle" => action_throttle(&session, input.rate).await,
            "gc_heap" => action_gc_heap(&session).await,
            "run_scenario" => action_run_scenario(&session, &input).await,
            "cpu_start" => action_cpu_start(&session).await,
            "cpu_stop" => action_cpu_stop(&session).await,
            "trace_start" => action_trace_start(&session, input.categories.as_deref()).await,
            "trace_stop" => action_trace_stop(&session).await,
            "why_render" => action_why_render(&session).await,
            "heap_snapshot" => action_heap_snapshot(&session, input.baseline.as_deref()).await,
            "coverage_start" => action_coverage_start(&session).await,
            "coverage_stop" => action_coverage_stop(&session).await,
            other => ToolOutput::error(format!(
                "Unknown action {other:?} — call action=\"help\" for the list"
            )),
        }
    }
}

// ============================================================================
// help (Tier 0)
// ============================================================================

fn help_text() -> String {
    "browser_profile — systematic web performance testing (REQ-BT-019).

Actions:
  help            — Show this message.

  metrics         — Performance.getMetrics snapshot as an aligned table
                    (ScriptDuration, TaskDuration, LayoutCount,
                    RecalcStyleCount, JSHeapUsedSize, Nodes,
                    JSEventListeners, ...).

  throttle        — Set CPU throttling rate. Params: rate (number, >= 1;
                    1 clears the override). Persisted on the session and
                    applied until cleared.

  gc_heap         — Force GC (HeapProfiler.collectGarbage) then read
                    JSHeapUsedSize, so the memory number is deterministic.

  run_scenario    — THE harness. Params: steps (non-empty array), runs
                    (int >= 1), warmup (int >= 0, default 0), throttle_rate
                    (optional, restored after). Step kinds: navigate{url},
                    reload, click{selector}, type{selector,text},
                    key{key,modifiers?}, eval{expression},
                    wait_selector{selector,timeout?},
                    wait_timing{mark,timeout?},
                    wait_eval{expression,timeout?}.
                    Returns the RAW per-run sample array — never a mean,
                    stddev, or any reduction. YOU own the statistics. If a
                    readiness step times out the whole operation fails,
                    names the blocking step, and returns ZERO samples.

  cpu_start       — Start a Profiler CPU sampling session.
  cpu_stop        — Stop it; save the profile JSON (loadable in the
                    DevTools Performance tab). Returns the file path.

  trace_start     — Start a Tracing session. Param: categories (optional,
                    comma-separated; default devtools.timeline,
                    disabled-by-default-v8.cpu_profiler,
                    blink.user_timing).
  trace_stop      — End tracing, await tracingComplete, write
                    {\"traceEvents\":[...]} JSON, and summarise tasks > 50ms.

  why_render      — Best-effort why-did-render: per re-rendered component,
                    the changed prop keys and changed hook indices.

  heap_snapshot   — Take a .heapsnapshot to disk. Param: baseline
                    (optional path) — when given, diff: node-count delta,
                    self-size delta (retained-size approximate), and
                    detached-DOM-node count.

  coverage_start  — Start precise JS coverage (call_count + detailed).
  coverage_stop   — Stop it; save per-script coverage JSON.

Sub-machine preconditions: cpu/trace/coverage are independent start/stop
pairs. Double-start is a success no-op; stop-when-idle is an error.

Typical workflow:
  throttle(rate=4) → run_scenario(steps=[...], runs=30, warmup=3)
  then compute mean/variance/significance YOURSELF over raw_samples.

Root cause:
  cpu_start → reproduce → cpu_stop;  trace_start → reproduce → trace_stop;
  heap_snapshot (baseline) → repeat mount/unmount → heap_snapshot(baseline=...)."
        .to_string()
}

// ============================================================================
// metrics (Tier 0, REQ-BT-019.3)
// ============================================================================

/// Read `Performance.getMetrics` (enabling the domain first). Returns the
/// metric map; callers format / extract from it.
async fn read_metrics(
    session: &Arc<RwLock<BrowserSession>>,
) -> Result<std::collections::BTreeMap<String, f64>, String> {
    let guard = session.read().await;
    guard
        .page
        .execute(PerfEnableParams::default())
        .await
        .map_err(|e| format!("Performance.enable failed: {e}"))?;
    let resp = guard
        .page
        .execute(GetMetricsParams::default())
        .await
        .map_err(|e| format!("Performance.getMetrics failed: {e}"))?;
    Ok(resp
        .result
        .metrics
        .iter()
        .map(|m| (m.name.clone(), m.value))
        .collect())
}

async fn action_metrics(session: &Arc<RwLock<BrowserSession>>) -> ToolOutput {
    let metrics = match read_metrics(session).await {
        Ok(m) => m,
        Err(e) => return ToolOutput::error(e),
    };
    if metrics.is_empty() {
        return ToolOutput::success("No performance metrics available.");
    }
    let max_len = metrics.keys().map(String::len).max().unwrap_or(0);
    let mut out = format!("Performance metrics ({} entries):\n\n", metrics.len());
    // Tracked headline metrics first, then everything else.
    for name in TRACKED_METRICS {
        if let Some(v) = metrics.get(*name) {
            let _ = writeln!(out, "  {name:<max_len$}  {v}");
        }
    }
    out.push('\n');
    for (name, v) in &metrics {
        if !TRACKED_METRICS.contains(&name.as_str()) {
            let _ = writeln!(out, "  {name:<max_len$}  {v}");
        }
    }
    ToolOutput::success(out).with_display(json!({ "metrics": metrics }))
}

// ============================================================================
// throttle (Tier 0, REQ-BT-019.2)
// ============================================================================

/// Issue `Emulation.setCPUThrottlingRate` on the page. `rate == 1` means
/// "no throttle" (CDP accepts 1 as the identity).
async fn apply_throttle(session: &Arc<RwLock<BrowserSession>>, rate: f64) -> Result<(), String> {
    let guard = session.read().await;
    guard
        .page
        .execute(SetCpuThrottlingRateParams::new(rate))
        .await
        .map_err(|e| format!("Emulation.setCPUThrottlingRate failed: {e}"))?;
    Ok(())
}

async fn action_throttle(session: &Arc<RwLock<BrowserSession>>, rate: Option<f64>) -> ToolOutput {
    let Some(rate) = rate else {
        return ToolOutput::error("throttle requires `rate` (number >= 1; 1 = no throttle)");
    };
    // Allium ThrottleRateWellFormed / invariant: rate < 1 must never be
    // representable as established override state.
    if rate < 1.0 {
        return ToolOutput::error(format!(
            "Invalid rate {rate}: must be >= 1 (1 = no throttling)"
        ));
    }
    if let Err(e) = apply_throttle(session, rate).await {
        return ToolOutput::error(e);
    }
    // ThrottleSetEstablishesOverride / ThrottleClearRestoresDefault:
    // rate == 1 clears the override (None), else records it. rate is
    // already validated >= 1, so `<= 1.0` is exactly "rate is 1".
    let clears = rate <= 1.0;
    with_profiling(session, |st| {
        st.throttle_rate = if clears { None } else { Some(rate) };
    })
    .await;
    if clears {
        ToolOutput::success("CPU throttling cleared (rate=1, browser default).")
    } else {
        ToolOutput::success(format!("CPU throttling set to {rate}x slowdown."))
    }
}

// ============================================================================
// gc_heap (Tier 0, REQ-BT-019.6)
// ============================================================================

async fn action_gc_heap(session: &Arc<RwLock<BrowserSession>>) -> ToolOutput {
    {
        let guard = session.read().await;
        if let Err(e) = guard.page.execute(HeapEnableParams::default()).await {
            return ToolOutput::error(format!("HeapProfiler.enable failed: {e}"));
        }
        if let Err(e) = guard.page.execute(CollectGarbageParams::default()).await {
            return ToolOutput::error(format!("HeapProfiler.collectGarbage failed: {e}"));
        }
    }
    let metrics = match read_metrics(session).await {
        Ok(m) => m,
        Err(e) => return ToolOutput::error(e),
    };
    match metrics.get("JSHeapUsedSize") {
        Some(used) => ToolOutput::success(format!(
            "Forced GC complete. JSHeapUsedSize = {used} bytes."
        ))
        .with_display(json!({ "js_heap_used_size": used })),
        None => {
            ToolOutput::error("GC done but JSHeapUsedSize not reported by Performance.getMetrics")
        }
    }
}

// ============================================================================
// run_scenario (Tier 0, REQ-BT-019.1/.3/.4/.5) — THE harness
// ============================================================================

/// One raw per-run sample. Serialised verbatim into `raw_samples`; the
/// harness never reduces these (invariant `RawSamplesNeverReduced`).
#[derive(Debug, Clone, serde::Serialize)]
struct RunSample {
    run_index: u32,
    script_duration: f64,
    task_duration: f64,
    layout_count: f64,
    recalc_style_count: f64,
    js_heap_used_size: f64,
    nodes: f64,
    js_event_listeners: f64,
    react_commits: u64,
    react_actual_ms: f64,
}

fn metric_or_zero(m: &std::collections::BTreeMap<String, f64>, k: &str) -> f64 {
    m.get(k).copied().unwrap_or(0.0)
}

/// Why a single run did not yield a sample.
enum RunError {
    /// A readiness step did not satisfy in time — fails the whole op,
    /// names the step (REQ-BT-019.1 / `BlockedScenarioYieldsNoSamples`).
    Blocked(String),
    /// Infrastructure failure (metrics snapshot) — abort the op.
    Infra(String),
}

/// Execute one scenario run with atomic before/after bracketing
/// (Allium @guidance). `Ok(sample)` carries the bracketed metrics for
/// this single execution; the caller decides whether to keep it (warmup
/// runs are executed identically but discarded).
async fn run_one(
    session: &Arc<RwLock<BrowserSession>>,
    steps: &[Step],
    run_index: u32,
) -> Result<RunSample, RunError> {
    let before = read_metrics(session)
        .await
        .map_err(|e| RunError::Infra(format!("metrics snapshot failed: {e}")))?;
    {
        let guard = session.read().await;
        let _ = guard
            .page
            .evaluate(
                "window.__phoenix && window.__phoenix.__resetCommits && window.__phoenix.__resetCommits()",
            )
            .await;
    }
    for step in steps {
        if let Err(reason) = run_step(session, step).await {
            return Err(RunError::Blocked(reason));
        }
    }
    let after = read_metrics(session)
        .await
        .map_err(|e| RunError::Infra(format!("metrics snapshot failed: {e}")))?;
    let (react_commits, react_actual_ms) = read_commit_totals(session).await;
    Ok(RunSample {
        run_index,
        script_duration: metric_or_zero(&after, "ScriptDuration")
            - metric_or_zero(&before, "ScriptDuration"),
        task_duration: metric_or_zero(&after, "TaskDuration")
            - metric_or_zero(&before, "TaskDuration"),
        layout_count: metric_or_zero(&after, "LayoutCount")
            - metric_or_zero(&before, "LayoutCount"),
        recalc_style_count: metric_or_zero(&after, "RecalcStyleCount")
            - metric_or_zero(&before, "RecalcStyleCount"),
        js_heap_used_size: metric_or_zero(&after, "JSHeapUsedSize"),
        nodes: metric_or_zero(&after, "Nodes"),
        js_event_listeners: metric_or_zero(&after, "JSEventListeners"),
        react_commits,
        react_actual_ms,
    })
}

/// Build the success `ToolOutput`, escaping large raw-sample arrays to
/// `/tmp`. `samples` is the untouched per-run vector — never reduced.
async fn scenario_success_output(samples: &[RunSample], runs: u32, warmup: u32) -> ToolOutput {
    // HARD CONSTRAINT (REQ-BT-019.5 / invariant RawSamplesNeverReduced):
    // emit the RAW per-run array verbatim. No mean/stddev/any reduction.
    let raw = serde_json::to_value(samples).unwrap_or(Value::Array(vec![]));
    let payload = json!({
        "outcome": "completed",
        "requested_runs": runs,
        "warmup": warmup,
        "raw_samples": raw,
        "note": "RAW per-run samples. The harness computes NO statistics — \
                 compute mean/variance/significance yourself.",
    });
    let pretty = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    if pretty.len() > OUTPUT_ESCAPE_BYTES {
        let path = format!("/tmp/phoenix-scenario-{}.json", uuid::Uuid::new_v4());
        if let Err(e) = tokio::fs::write(&path, &pretty).await {
            return ToolOutput::error(format!("Failed to write scenario output: {e}"));
        }
        ToolOutput::success(format!(
            "run_scenario completed: {runs} raw per-run samples (warmup {warmup} discarded). \
             Full raw samples written to {path} (use `cat`). NOT reduced — compute stats yourself."
        ))
        .with_display(payload)
    } else {
        ToolOutput::success(pretty).with_display(payload)
    }
}

async fn action_run_scenario(
    session: &Arc<RwLock<BrowserSession>>,
    input: &ProfileInput,
) -> ToolOutput {
    // Allium RunScenarioCollectsRawSamples preconditions.
    let steps = match &input.steps {
        Some(s) if !s.is_empty() => s.clone(),
        _ => return ToolOutput::error("run_scenario requires a non-empty `steps` array"),
    };
    let runs = input.runs.unwrap_or(1);
    if runs < 1 {
        return ToolOutput::error("run_scenario requires `runs` >= 1");
    }
    let warmup = input.warmup.unwrap_or(0);
    if let Some(tr) = input.throttle_rate {
        if tr < 1.0 {
            return ToolOutput::error(format!(
                "Invalid throttle_rate {tr}: must be >= 1 (1 = no throttling)"
            ));
        }
    }

    // Capture the prior throttle so it can be restored on return — whether
    // the operation completes OR is blocked (Allium @guidance).
    let prior_throttle = with_profiling(session, |st| st.throttle_rate)
        .await
        .flatten();

    // Apply the scenario throttle for the duration of the operation.
    if let Some(tr) = input.throttle_rate {
        if let Err(e) = apply_throttle(session, tr).await {
            return ToolOutput::error(e);
        }
    }

    let total_runs = warmup + runs;
    let mut samples: Vec<RunSample> = Vec::with_capacity(runs as usize);

    // Warmup runs execute identically but their samples are discarded
    // (Allium @guidance). Throttle is restored on every exit path below.
    for global_idx in 0..total_runs {
        let is_warmup = global_idx < warmup;
        match run_one(session, &steps, global_idx.saturating_sub(warmup)).await {
            Ok(sample) => {
                if !is_warmup {
                    samples.push(sample);
                }
            }
            Err(RunError::Blocked(reason)) => {
                // Restore throttle then fail with ZERO samples (REQ-BT-019.1
                // / BlockedScenarioYieldsNoSamples). Never a partial set.
                restore_throttle(session, prior_throttle).await;
                return ToolOutput::error(format!(
                    "run_scenario blocked: {reason}. No samples returned (a measurement \
                     against an indeterminate state is worse than no measurement)."
                ))
                .with_display(json!({
                    "outcome": "blocked",
                    "blocked_step": reason,
                    "raw_samples": [],
                }));
            }
            Err(RunError::Infra(msg)) => {
                restore_throttle(session, prior_throttle).await;
                return ToolOutput::error(msg);
            }
        }
    }

    // Restore throttle on success too (Allium @guidance).
    restore_throttle(session, prior_throttle).await;
    scenario_success_output(&samples, runs, warmup).await
}

/// Restore the throttle to its pre-scenario value. `None` means "browser
/// default" → re-issue rate 1 to clear any scenario override.
async fn restore_throttle(session: &Arc<RwLock<BrowserSession>>, prior: Option<f64>) {
    let rate = prior.unwrap_or(1.0);
    if let Err(e) = apply_throttle(session, rate).await {
        // Capability gap: cannot restore throttle (page gone, etc.).
        tracing::debug!(error = %e, "run_scenario: failed to restore CPU throttle");
    }
    with_profiling(session, |st| st.throttle_rate = prior).await;
}

/// Read total React commit count + summed actualDuration (ms) from the
/// __phoenix buffer for the current run. Zeroes on non-React pages.
async fn read_commit_totals(session: &Arc<RwLock<BrowserSession>>) -> (u64, f64) {
    let guard = session.read().await;
    let script = "(function(){\
        try {\
          if (!window.__phoenix || !window.__phoenix.__getCommits) return [0,0];\
          var c = window.__phoenix.__getCommits();\
          var total = 0;\
          for (var i=0;i<c.length;i++) total += (c[i].totalActualDuration||0);\
          return [c.length, total];\
        } catch(e) { return [0,0]; }\
      })()";
    match guard.page.evaluate(script).await {
        Ok(res) => match res.into_value::<(u64, f64)>() {
            Ok((n, ms)) => (n, ms),
            Err(_) => (0, 0.0),
        },
        Err(_) => (0, 0.0),
    }
}

/// Execute one scenario step. `Err(reason)` means a readiness step did not
/// satisfy within its timeout — the caller fails the whole operation.
async fn run_step(session: &Arc<RwLock<BrowserSession>>, step: &Step) -> Result<(), String> {
    let guard = session.read().await;
    match step {
        Step::Navigate { url } => guard
            .page
            .goto(url)
            .await
            .map(|_| ())
            .map_err(|e| format!("navigate to {url} failed: {e}")),
        Step::Reload => guard
            .page
            .reload()
            .await
            .map(|_| ())
            .map_err(|e| format!("reload failed: {e}")),
        Step::Click { selector } => {
            let el = guard
                .page
                .find_element(selector)
                .await
                .map_err(|e| format!("click: element {selector} not found: {e}"))?;
            el.click()
                .await
                .map(|_| ())
                .map_err(|e| format!("click on {selector} failed: {e}"))
        }
        Step::Type { selector, text } => {
            let el = guard
                .page
                .find_element(selector)
                .await
                .map_err(|e| format!("type: element {selector} not found: {e}"))?;
            el.click()
                .await
                .map_err(|e| format!("type: focus {selector} failed: {e}"))?;
            el.type_str(text)
                .await
                .map(|_| ())
                .map_err(|e| format!("type into {selector} failed: {e}"))
        }
        Step::Key { key, modifiers } => {
            let ctrl = modifiers.iter().any(|m| m == "ctrl" || m == "control");
            let shift = modifiers.iter().any(|m| m == "shift");
            let alt = modifiers.iter().any(|m| m == "alt");
            let meta = modifiers
                .iter()
                .any(|m| m == "meta" || m == "cmd" || m == "command");
            let js = format!(
                "(function(){{var o={{key:{key:?},code:{key:?},ctrlKey:{ctrl},shiftKey:{shift},\
                 altKey:{alt},metaKey:{meta},bubbles:true,cancelable:true,composed:true}};\
                 window.dispatchEvent(new KeyboardEvent('keydown',o));\
                 window.dispatchEvent(new KeyboardEvent('keyup',o));return 'ok';}})()"
            );
            guard
                .page
                .evaluate(js)
                .await
                .map(|_| ())
                .map_err(|e| format!("key {key} failed: {e}"))
        }
        Step::Eval { expression } => guard
            .page
            .evaluate(expression.clone())
            .await
            .map(|_| ())
            .map_err(|e| format!("eval failed: {e}")),
        Step::WaitSelector { selector, timeout } => {
            let to = timeout
                .as_deref()
                .and_then(parse_duration)
                .unwrap_or(DEFAULT_WAIT_TIMEOUT);
            let check = format!(
                "document.querySelector({}) !== null",
                serde_json::to_string(selector).unwrap_or_default()
            );
            poll(&guard, &check, to, &format!("wait_selector {selector}")).await
        }
        Step::WaitTiming { mark, timeout } => {
            let to = timeout
                .as_deref()
                .and_then(parse_duration)
                .unwrap_or(DEFAULT_WAIT_TIMEOUT);
            let check = format!(
                "performance.getEntriesByName({}, 'mark').length > 0",
                serde_json::to_string(mark).unwrap_or_default()
            );
            poll(&guard, &check, to, &format!("wait_timing {mark}")).await
        }
        Step::WaitEval {
            expression,
            timeout,
        } => {
            let to = timeout
                .as_deref()
                .and_then(parse_duration)
                .unwrap_or(DEFAULT_WAIT_TIMEOUT);
            poll(&guard, &format!("!!({expression})"), to, "wait_eval").await
        }
    }
}

/// Poll a boolean JS predicate until truthy or the timeout elapses.
/// `Err` names the blocking step so the harness can report it.
async fn poll(
    guard: &tokio::sync::RwLockReadGuard<'_, BrowserSession>,
    predicate: &str,
    timeout: Duration,
    step_name: &str,
) -> Result<(), String> {
    let start = Instant::now();
    let interval = Duration::from_millis(100);
    loop {
        if let Ok(res) = guard.page.evaluate(predicate.to_string()).await {
            if let Ok(true) = res.into_value::<bool>() {
                return Ok(());
            }
        }
        if start.elapsed() >= timeout {
            return Err(format!("{step_name} not satisfied within {timeout:?}"));
        }
        tokio::time::sleep(interval).await;
    }
}

// ============================================================================
// CPU sampling sub-machine (Tier 1, REQ-BT-019.7)
// ============================================================================

async fn action_cpu_start(session: &Arc<RwLock<BrowserSession>>) -> ToolOutput {
    // Allium CpuStartWhenIdle: a start while already active is a success
    // no-op (must NOT restart the profiler and discard the in-progress one).
    if with_profiling(session, |st| st.cpu_active).await == Some(true) {
        return ToolOutput::success("CPU profiling already active");
    }
    {
        let guard = session.read().await;
        if let Err(e) = guard.page.execute(ProfilerEnableParams::default()).await {
            return ToolOutput::error(format!("Profiler.enable failed: {e}"));
        }
        if let Err(e) = guard.page.execute(ProfilerStartParams::default()).await {
            return ToolOutput::error(format!("Profiler.start failed: {e}"));
        }
    }
    with_profiling(session, |st| st.cpu_active = true).await;
    ToolOutput::success("CPU profiling started.")
}

async fn action_cpu_stop(session: &Arc<RwLock<BrowserSession>>) -> ToolOutput {
    // Allium CpuStopWhenActive: stop-when-idle is an error (the
    // forgot-to-start case is structurally explicit, not an empty profile).
    if with_profiling(session, |st| st.cpu_active).await != Some(true) {
        return ToolOutput::error("CPU profiling is not active — call cpu_start first");
    }
    let profile = {
        let guard = session.read().await;
        let resp = match guard.page.execute(ProfilerStopParams::default()).await {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(format!("Profiler.stop failed: {e}")),
        };
        if let Err(e) = guard.page.execute(ProfilerDisableParams::default()).await {
            tracing::debug!(error = %e, "Profiler.disable failed after stop");
        }
        resp.result.profile.clone()
    };
    with_profiling(session, |st| st.cpu_active = false).await;
    let path = format!("/tmp/phoenix-cpu-profile-{}.json", uuid::Uuid::new_v4());
    let data = match serde_json::to_string(&profile) {
        Ok(d) => d,
        Err(e) => return ToolOutput::error(format!("Failed to serialise CPU profile: {e}")),
    };
    if let Err(e) = tokio::fs::write(&path, data).await {
        return ToolOutput::error(format!("Failed to write CPU profile: {e}"));
    }
    ToolOutput::success(format!(
        "CPU profile saved to {path} (load in Chrome DevTools → Performance)."
    ))
}

// ============================================================================
// Tracing sub-machine (Tier 1, REQ-BT-019.9/.12)
// ============================================================================

async fn action_trace_start(
    session: &Arc<RwLock<BrowserSession>>,
    categories: Option<&str>,
) -> ToolOutput {
    // Allium TraceStartWhenIdle: idempotent success no-op on double-start;
    // ensures trace_event_count = 0.
    if with_profiling(session, |st| st.tracing_active).await == Some(true) {
        return ToolOutput::success("Tracing already active");
    }

    let cats: Vec<String> = match categories {
        Some(s) if !s.trim().is_empty() => s.split(',').map(|c| c.trim().to_string()).collect(),
        _ => vec![
            "devtools.timeline".to_string(),
            "disabled-by-default-v8.cpu_profiler".to_string(),
            "blink.user_timing".to_string(),
        ],
    };

    // Allium @guidance: clear buffer + arm (listener is already armed at
    // session creation) BEFORE Tracing.start so no early events are lost.
    with_profiling(session, |st| {
        st.trace_events.clear();
        st.tracing_active = true;
    })
    .await;

    let cfg = TraceConfig::builder()
        .included_categories(cats.clone())
        .build();
    let params = TraceStartParams::builder().trace_config(cfg).build();
    let start_err = {
        let guard = session.read().await;
        guard.page.execute(params).await.err()
    };
    if let Some(e) = start_err {
        // Roll back the active flag so we don't strand the sub-machine.
        with_profiling(session, |st| {
            st.tracing_active = false;
            st.trace_events.clear();
        })
        .await;
        return ToolOutput::error(format!("Tracing.start failed: {e}"));
    }
    ToolOutput::success(format!(
        "Tracing started (categories: {}).",
        cats.join(", ")
    ))
}

async fn action_trace_stop(session: &Arc<RwLock<BrowserSession>>) -> ToolOutput {
    // Allium TraceStopWhenActive: stop-when-idle is an error.
    if with_profiling(session, |st| st.tracing_active).await != Some(true) {
        return ToolOutput::error("Tracing is not active — call trace_start first");
    }

    let notify = {
        let guard = session.read().await;
        guard.trace_complete.clone()
    };

    // Arm the wait BEFORE Tracing.end so a fast tracingComplete can't race
    // ahead of the waiter. `notified()` only registers the waiter when first
    // polled, so `enable()` forces registration now — before end is issued —
    // closing the lost-wakeup window (notify_waiters stores no permit).
    let notified = notify.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();

    {
        let guard = session.read().await;
        if let Err(e) = guard.page.execute(EndParams::default()).await {
            return ToolOutput::error(format!("Tracing.end failed: {e}"));
        }
    }

    // Events arrive asynchronously via Tracing.dataCollected after end;
    // wait (bounded) for Tracing.tracingComplete before draining.
    let timed_out = tokio::time::timeout(TRACE_COMPLETE_TIMEOUT, notified)
        .await
        .is_err();
    if timed_out {
        tracing::debug!("trace_stop: timed out waiting for tracingComplete; draining what arrived");
    }

    // Drain-then-clear the buffer and reset tracing=idle together
    // (Allium TraceStopWhenActive ensures trace_event_count = 0).
    let events: Vec<Value> = match with_profiling(session, |st| {
        st.tracing_active = false;
        std::mem::take(&mut st.trace_events)
    })
    .await
    {
        Some(ev) => ev,
        None => return ToolOutput::error("profiling state lock poisoned"),
    };

    // Long-task extraction (REQ-BT-019.9): events with dur > 50_000us.
    let mut long_tasks: Vec<(String, f64)> = events
        .iter()
        .filter_map(|e| {
            let dur = e.get("dur").and_then(Value::as_f64)?;
            if dur > LONG_TASK_US {
                let name = e
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("(unnamed)")
                    .to_string();
                Some((name, dur))
            } else {
                None
            }
        })
        .collect();
    long_tasks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let long_count = long_tasks.len();
    let long_total_ms: f64 = long_tasks.iter().map(|(_, d)| d / 1000.0).sum();
    let top: Vec<String> = long_tasks
        .iter()
        .take(5)
        .map(|(n, d)| format!("    {n}: {:.1}ms", d / 1000.0))
        .collect();

    let wrapper = json!({ "traceEvents": events });
    let path = format!("/tmp/phoenix-trace-{}.json", uuid::Uuid::new_v4());
    let data = match serde_json::to_string(&wrapper) {
        Ok(d) => d,
        Err(e) => return ToolOutput::error(format!("Failed to serialise trace: {e}")),
    };
    if let Err(e) = tokio::fs::write(&path, data).await {
        return ToolOutput::error(format!("Failed to write trace: {e}"));
    }

    let mut summary = format!(
        "Trace saved to {path} ({} events). Long tasks (>50ms): {long_count}, \
         total {long_total_ms:.1}ms.",
        events.len()
    );
    if !top.is_empty() {
        summary.push_str("\n  Top long tasks:\n");
        summary.push_str(&top.join("\n"));
    }
    if timed_out {
        summary.push_str("\n  (note: tracingComplete timed out; trace may be partial)");
    }
    ToolOutput::success(summary)
}

// ============================================================================
// why_render (Tier 1, REQ-BT-019.8)
// ============================================================================

async fn action_why_render(session: &Arc<RwLock<BrowserSession>>) -> ToolOutput {
    let guard = session.read().await;
    let script = "(function(){\
        try {\
          if (!window.__phoenix || !window.__phoenix.__getWhyRender) return null;\
          return JSON.stringify(window.__phoenix.__getWhyRender());\
        } catch(e) { return null; }\
      })()";
    match guard.page.evaluate(script).await {
        Ok(res) => match res.into_value::<Option<String>>() {
            Ok(Some(s)) => {
                let parsed: Value = serde_json::from_str(&s).unwrap_or(Value::Array(vec![]));
                let pretty =
                    serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| "[]".to_string());
                if pretty == "[]" {
                    ToolOutput::success(
                        "No re-renders with attributable prop/hook changes recorded \
                         (run a scenario or interact first, then call why_render).",
                    )
                } else {
                    ToolOutput::success(pretty).with_display(json!({ "why_render": parsed }))
                }
            }
            _ => ToolOutput::success(
                "why-did-render unavailable (no __phoenix helper — not a React page?).",
            ),
        },
        Err(e) => ToolOutput::error(format!("why_render eval failed: {e}")),
    }
}

// ============================================================================
// heap_snapshot (Tier 1, REQ-BT-019.10)
// ============================================================================

/// Capture a heap snapshot to `path` by collecting
/// `HeapProfiler.addHeapSnapshotChunk` events emitted during
/// `HeapProfiler.takeHeapSnapshot`.
async fn capture_heap_snapshot(
    session: &Arc<RwLock<BrowserSession>>,
    path: &str,
) -> Result<(), String> {
    let mut chunks = {
        let guard = session.read().await;
        guard
            .page
            .event_listener::<EventAddHeapSnapshotChunk>()
            .await
            .map_err(|e| format!("failed to subscribe to heap chunks: {e}"))?
    };

    let collector = tokio::spawn(async move {
        let mut buf = String::new();
        // Chunks stop arriving once the snapshot is fully streamed; the
        // takeHeapSnapshot command returns after the last chunk, so a short
        // post-completion drain window is enough.
        while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(2), chunks.next()).await {
            buf.push_str(&ev.chunk);
        }
        buf
    });

    {
        let guard = session.read().await;
        guard
            .page
            .execute(HeapEnableParams::default())
            .await
            .map_err(|e| format!("HeapProfiler.enable failed: {e}"))?;
        let params = TakeHeapSnapshotParams {
            report_progress: Some(false),
            capture_numeric_value: Some(true),
            expose_internals: None,
        };
        guard
            .page
            .execute(params)
            .await
            .map_err(|e| format!("HeapProfiler.takeHeapSnapshot failed: {e}"))?;
    }

    let data = collector
        .await
        .map_err(|e| format!("heap chunk collector join failed: {e}"))?;
    if data.is_empty() {
        return Err("heap snapshot produced no data".to_string());
    }
    tokio::fs::write(path, data)
        .await
        .map_err(|e| format!("failed to write heap snapshot: {e}"))
}

/// Minimal V8 .heapsnapshot stats. A .heapsnapshot is JSON:
/// `{snapshot:{meta:{node_fields,...},node_count}, nodes:[flat ints], strings:[...]}`.
struct HeapStats {
    node_count: usize,
    /// Sum of the `self_size` field across all nodes. NOTE: this is
    /// *self* size, not *retained* size — true retained-size needs the full
    /// dominator-tree analysis (heavy). We report self-size deltas and call
    /// retained-size approximate (documented in the diff output).
    total_self_size: u64,
    detached_dom_nodes: usize,
}

fn parse_heap_stats(json_text: &str) -> Result<HeapStats, String> {
    let v: Value =
        serde_json::from_str(json_text).map_err(|e| format!("invalid heapsnapshot JSON: {e}"))?;
    let meta = v
        .get("snapshot")
        .and_then(|s| s.get("meta"))
        .ok_or("heapsnapshot missing snapshot.meta")?;
    let node_fields: Vec<String> = meta
        .get("node_fields")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .ok_or("heapsnapshot missing node_fields")?;
    let field_count = node_fields.len();
    if field_count == 0 {
        return Err("heapsnapshot node_fields empty".to_string());
    }
    let name_idx = node_fields.iter().position(|f| f == "name");
    let self_size_idx = node_fields.iter().position(|f| f == "self_size");
    let nodes = v
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or("heapsnapshot missing nodes array")?;
    let strings: Vec<&str> = v
        .get("strings")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let node_count = nodes.len() / field_count;
    let mut total_self_size: u64 = 0;
    let mut detached = 0usize;
    for i in 0..node_count {
        let base = i * field_count;
        if let Some(ss_i) = self_size_idx {
            if let Some(ss) = nodes.get(base + ss_i).and_then(Value::as_u64) {
                total_self_size += ss;
            }
        }
        if let Some(n_i) = name_idx {
            if let Some(name_ref) = nodes
                .get(base + n_i)
                .and_then(Value::as_u64)
                .and_then(|r| usize::try_from(r).ok())
            {
                if let Some(name) = strings.get(name_ref) {
                    if name.contains("Detached") {
                        detached += 1;
                    }
                }
            }
        }
    }
    Ok(HeapStats {
        node_count,
        total_self_size,
        detached_dom_nodes: detached,
    })
}

async fn action_heap_snapshot(
    session: &Arc<RwLock<BrowserSession>>,
    baseline: Option<&str>,
) -> ToolOutput {
    let path = format!("/tmp/phoenix-heap-{}.heapsnapshot", uuid::Uuid::new_v4());
    if let Err(e) = capture_heap_snapshot(session, &path).await {
        return ToolOutput::error(format!("heap_snapshot failed: {e}"));
    }

    let Some(baseline_path) = baseline else {
        return ToolOutput::success(format!(
            "Heap snapshot saved to {path} (load in Chrome DevTools → Memory). \
             Pass this path as `baseline` to a later heap_snapshot for a diff."
        ));
    };

    // Allium HeapDiffRequiresTwoSnapshots: a diff against a missing
    // baseline is an error, not a zero-delta result.
    let baseline_text = match tokio::fs::read_to_string(baseline_path).await {
        Ok(t) => t,
        Err(e) => {
            return ToolOutput::error(format!(
                "baseline snapshot {baseline_path} not readable: {e} \
                 (\"no leak\" and \"never measured\" must stay distinguishable)"
            ));
        }
    };
    let post_text = match tokio::fs::read_to_string(&path).await {
        Ok(t) => t,
        Err(e) => return ToolOutput::error(format!("post snapshot not readable: {e}")),
    };
    let base_stats = match parse_heap_stats(&baseline_text) {
        Ok(s) => s,
        Err(e) => return ToolOutput::error(format!("parsing baseline: {e}")),
    };
    let post_stats = match parse_heap_stats(&post_text) {
        Ok(s) => s,
        Err(e) => return ToolOutput::error(format!("parsing post snapshot: {e}")),
    };

    let node_delta = i64::try_from(post_stats.node_count).unwrap_or(i64::MAX)
        - i64::try_from(base_stats.node_count).unwrap_or(i64::MAX);
    let size_delta = i64::try_from(post_stats.total_self_size).unwrap_or(i64::MAX)
        - i64::try_from(base_stats.total_self_size).unwrap_or(i64::MAX);
    let summary = format!(
        "Heap diff (post {path} vs baseline {baseline_path}):\n\
         \u{20}\u{20}node count: {} → {} (Δ {node_delta:+})\n\
         \u{20}\u{20}self_size:  {} → {} bytes (Δ {size_delta:+})\n\
         \u{20}\u{20}detached DOM nodes: baseline {} → post {}\n\
         \u{20}\u{20}NOTE: retained-size is APPROXIMATED by self_size delta — true \
         retained size needs the full dominator-tree walk (not done here for cost).",
        base_stats.node_count,
        post_stats.node_count,
        base_stats.total_self_size,
        post_stats.total_self_size,
        base_stats.detached_dom_nodes,
        post_stats.detached_dom_nodes,
    );
    ToolOutput::success(summary).with_display(json!({
        "baseline": baseline_path,
        "post": path,
        "node_count_delta": node_delta,
        "self_size_delta_bytes": size_delta,
        "retained_size_approximate": true,
        "detached_dom_nodes": {
            "baseline": base_stats.detached_dom_nodes,
            "post": post_stats.detached_dom_nodes,
        },
    }))
}

// ============================================================================
// JS coverage sub-machine (Tier 2, REQ-BT-019.11)
// ============================================================================

async fn action_coverage_start(session: &Arc<RwLock<BrowserSession>>) -> ToolOutput {
    // Allium CoverageStartWhenIdle: idempotent success no-op.
    if with_profiling(session, |st| st.coverage_active).await == Some(true) {
        return ToolOutput::success("Coverage already active");
    }
    {
        let guard = session.read().await;
        if let Err(e) = guard.page.execute(ProfilerEnableParams::default()).await {
            return ToolOutput::error(format!("Profiler.enable failed: {e}"));
        }
        let params = StartPreciseCoverageParams {
            call_count: Some(true),
            detailed: Some(true),
            allow_triggered_updates: None,
        };
        if let Err(e) = guard.page.execute(params).await {
            return ToolOutput::error(format!("Profiler.startPreciseCoverage failed: {e}"));
        }
    }
    with_profiling(session, |st| st.coverage_active = true).await;
    ToolOutput::success("JS coverage collection started.")
}

async fn action_coverage_stop(session: &Arc<RwLock<BrowserSession>>) -> ToolOutput {
    // Allium CoverageStopWhenActive: stop-when-idle is an error.
    if with_profiling(session, |st| st.coverage_active).await != Some(true) {
        return ToolOutput::error("Coverage is not active — call coverage_start first");
    }
    let coverage = {
        let guard = session.read().await;
        let resp = match guard
            .page
            .execute(TakePreciseCoverageParams::default())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return ToolOutput::error(format!("Profiler.takePreciseCoverage failed: {e}"))
            }
        };
        if let Err(e) = guard
            .page
            .execute(StopPreciseCoverageParams::default())
            .await
        {
            tracing::debug!(error = %e, "Profiler.stopPreciseCoverage failed");
        }
        if let Err(e) = guard.page.execute(ProfilerDisableParams::default()).await {
            tracing::debug!(error = %e, "Profiler.disable failed after coverage stop");
        }
        resp.result.result.clone()
    };
    with_profiling(session, |st| st.coverage_active = false).await;
    let script_count = coverage.len();
    let path = format!("/tmp/phoenix-coverage-{}.json", uuid::Uuid::new_v4());
    let data = match serde_json::to_string(&coverage) {
        Ok(d) => d,
        Err(e) => return ToolOutput::error(format!("Failed to serialise coverage: {e}")),
    };
    if let Err(e) = tokio::fs::write(&path, data).await {
        return ToolOutput::error(format!("Failed to write coverage: {e}"));
    }
    ToolOutput::success(format!(
        "Coverage saved to {path} ({script_count} scripts)."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_schema_lists_all_actions() {
        let tool = BrowserProfileTool;
        let schema = tool.input_schema();
        let enum_vals = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum present");
        let names: Vec<&str> = enum_vals.iter().filter_map(Value::as_str).collect();
        for a in PROFILE_ACTIONS {
            assert!(names.contains(a), "schema missing action {a}");
        }
        assert_eq!(names.len(), PROFILE_ACTIONS.len());
    }

    #[test]
    fn help_needs_no_browser_and_lists_run_scenario() {
        let h = help_text();
        assert!(h.contains("run_scenario"));
        assert!(h.contains("RAW per-run"));
    }

    #[test]
    fn step_deserializes_tagged() {
        let s: Step = serde_json::from_value(json!({"kind":"navigate","url":"about:blank"}))
            .expect("navigate step");
        matches!(s, Step::Navigate { .. });
        let w: Step = serde_json::from_value(json!({"kind":"wait_timing","mark":"ready"}))
            .expect("wait_timing");
        matches!(w, Step::WaitTiming { .. });
    }
}
