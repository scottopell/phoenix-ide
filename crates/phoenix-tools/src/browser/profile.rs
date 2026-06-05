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
use crate::{Tool, ToolContext, ToolOutput};
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
    Profile as CpuProfile, ProfileNode as CpuProfileNode, StartParams as ProfilerStartParams,
    StartPreciseCoverageParams, StopParams as ProfilerStopParams, StopPreciseCoverageParams,
    TakePreciseCoverageParams,
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
    /// `run_scenario`: discarded warmup runs. Default is 1
    /// (REQ-BT-019.16) — see [`resolve_warmup`]. `Some(0)` is honoured
    /// but raises a methodology warning.
    #[serde(default)]
    warmup: Option<u32>,
    /// `run_scenario`: throttle applied for the scenario only.
    #[serde(default)]
    throttle_rate: Option<f64>,
    /// `run_scenario`: force a full GC once per run and read the live
    /// heap at that single post-GC point (REQ-BT-019.15). Default true.
    /// When false, `js_heap_used` is null and a warning is raised.
    #[serde(default)]
    gc_per_run: Option<bool>,
    /// `run_scenario`: per-run reset (REQ-BT-019.18). Omitted = reload
    /// the current URL before every run. `{"kind":"navigate","url":...}`
    /// or `{"kind":"reload"}` for an explicit reset; the string `"none"`
    /// opts out (and raises a methodology warning).
    #[serde(default)]
    reset: Option<ResetSpec>,
    /// `heap_snapshot`: optional baseline snapshot path to diff against.
    #[serde(default)]
    baseline: Option<String>,
    /// `cpu_summary`: path to a saved CPU profile JSON to summarise.
    #[serde(default)]
    path: Option<String>,
}

/// Per-run reset directive (REQ-BT-019.18). Accepts either the string
/// `"none"` or an object `{kind: "navigate"|"reload", url?}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ResetSpec {
    /// The literal string `"none"` — opt out of per-run reset.
    None(NoneLiteral),
    /// `{kind:"navigate", url}` or `{kind:"reload"}`.
    Action(ResetAction),
}

/// Deserialises only from the exact string `"none"`. A misspelt opt-out
/// must not silently fall through to "no reset"; it errors at parse.
#[derive(Debug, Clone, Deserialize)]
enum NoneLiteral {
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ResetAction {
    Navigate { url: String },
    Reload,
}

/// Resolved per-run reset behaviour after applying the default.
#[derive(Debug, Clone)]
enum Reset {
    /// Reload the current URL (the default — REQ-BT-019.18).
    ReloadCurrent,
    /// Explicit `navigate{url}`.
    Navigate(String),
    /// Explicit `reload`.
    Reload,
    /// Explicit opt-out (`reset:"none"`) — raises a methodology warning.
    Skip,
}

impl Reset {
    fn resolve(spec: Option<&ResetSpec>) -> Reset {
        match spec {
            None => Reset::ReloadCurrent,
            Some(ResetSpec::None(_)) => Reset::Skip,
            Some(ResetSpec::Action(ResetAction::Navigate { url })) => Reset::Navigate(url.clone()),
            Some(ResetSpec::Action(ResetAction::Reload)) => Reset::Reload,
        }
    }
}

/// REQ-BT-019.16: `warmup` defaults to 1 (cold JIT/first-paint excluded
/// by default). An explicit `Some(0)` is honoured but flagged.
fn resolve_warmup(warmup: Option<u32>) -> u32 {
    warmup.unwrap_or(1)
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
    "cpu_summary",
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
                    "description": "run_scenario only: ordered steps. Each step is an object with a `kind`: click{selector}, type{selector,text}, key{key,modifiers?}, eval{expression}, wait_selector{selector,timeout?}, wait_timing{mark,timeout?}, wait_eval{expression,timeout?}. The page-anchored measurement window opens AFTER the FIRST wait_* readiness step satisfies (REQ-BT-019.20): steps up to and including it are UNTIMED setup (page load + framework mount + async settle), the remaining steps are measured. Put the readiness wait FIRST so mount/settle is excluded. NOTE: navigate/reload are NOT allowed in steps — put navigation in `reset` instead. With no wait_* step the window opens immediately after reset and methodology_warnings flags it (mount/settle then unavoidably in-window).",
                    "items": { "type": "object" }
                },
                "runs": {
                    "type": "integer",
                    "description": "run_scenario only: number of measured runs (>= 1)."
                },
                "warmup": {
                    "type": "integer",
                    "description": "run_scenario only: warmup runs discarded from results. Default 1 (cold JIT/first-paint excluded). An explicit 0 is honoured but raises a methodology warning."
                },
                "throttle_rate": {
                    "type": "number",
                    "description": "run_scenario only: CPU slowdown applied for the scenario, restored after (>= 1). Omitting it raises a methodology warning (host noise dominates)."
                },
                "gc_per_run": {
                    "type": "boolean",
                    "description": "run_scenario only: force a full GC once per run (outside the duration bracket) and read JSHeapUsedSize at that single post-GC point. Default true. When false, each sample's js_heap_used is null and a methodology warning is raised."
                },
                "reset": {
                    "description": "run_scenario only: per-run reset for determinism. Omitted = reload the current URL before every run. Object {\"kind\":\"navigate\",\"url\":...} or {\"kind\":\"reload\"} for an explicit reset; the string \"none\" opts out (raises a methodology warning). Reset runs before the before-snapshot, so this is where navigation belongs (NOT in steps).",
                    "oneOf": [
                        { "type": "string", "enum": ["none"] },
                        { "type": "object" }
                    ]
                },
                "baseline": {
                    "type": "string",
                    "description": "heap_snapshot only: path to a baseline .heapsnapshot to diff against."
                },
                "path": {
                    "type": "string",
                    "description": "cpu_summary only: path to a saved CPU profile JSON (from cpu_stop). Returns top hot functions by self/total time — no browser needed."
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

        // help and cpu_summary need no browser (cpu_summary parses a
        // file on disk — works even after the session died).
        if input.action == "help" {
            return ToolOutput::success(help_text());
        }
        if input.action == "cpu_summary" {
            return action_cpu_summary(input.path.as_deref()).await;
        }

        // run_scenario's structural validation (empty steps, navigate/
        // reload-in-steps, runs/throttle bounds) is pure input checking
        // and MUST run BEFORE acquiring the browser — otherwise an
        // invalid scenario in a no-Chrome env returns "Failed to get
        // browser" instead of the real validation error, defeating the
        // non-browser validation guarantee (Codex P2). Mirrors how
        // `help` / `cpu_summary` are dispatched pre-browser.
        if input.action == "run_scenario" {
            let plan = match validate_run_scenario(&input) {
                Ok(p) => p,
                Err(e) => return e,
            };
            let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
                Ok(s) => s,
                Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
            };
            return action_run_scenario(&session, &input, plan).await;
        }

        let session: Arc<RwLock<BrowserSession>> = match ctx.browser().await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Failed to get browser: {e}")),
        };

        match input.action.as_str() {
            "metrics" => action_metrics(&session).await,
            "throttle" => action_throttle(&session, input.rate).await,
            "gc_heap" => action_gc_heap(&session).await,
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
                    (int >= 1), warmup (int >= 0, DEFAULT 1 — cold
                    JIT/first-paint excluded; explicit 0 is allowed but
                    warned), throttle_rate (optional, restored after),
                    gc_per_run (bool, DEFAULT true), reset (see below).
                    Step kinds: click{selector}, type{selector,text},
                    key{key,modifiers?}, eval{expression},
                    wait_selector{selector,timeout?},
                    wait_timing{mark,timeout?},
                    wait_eval{expression,timeout?}.
                    navigate/reload are REJECTED inside steps — put
                    navigation in `reset`.
                    PAGE-ANCHORED WINDOW (REQ-BT-019.20): the measured
                    window opens AFTER the FIRST wait_* readiness step
                    satisfies. Steps up to and including it are UNTIMED
                    setup (page load + framework mount + async settle);
                    the remaining steps are measured. Put the readiness
                    wait FIRST so mount/settle is excluded (the F3 fix).
                    The window boundaries are performance.now() marks the
                    page sets — NOT host-side CDP getMetrics reads (F5).
                    reset (per-run determinism, REQ-BT-019.18): omitted =
                    reload the current URL before every run;
                    {\"kind\":\"navigate\",\"url\":...} or {\"kind\":\"reload\"}
                    for an explicit reset; the string \"none\" opts out
                    (and raises a methodology warning).
                    Returns the RAW per-run sample array — never a mean,
                    stddev, or any reduction. YOU own the statistics. If a
                    readiness step times out the whole operation fails,
                    names the blocking step, and returns ZERO samples.
                    Each sample carries: script_ms (sum of in-window
                    longtask durations, ms — NOT a CDP ScriptDuration
                    delta), long_tasks (count of >50ms longtasks
                    in-window), wall_ms (performance.now() span of the
                    window), dom_nodes (getElementsByTagName('*') at
                    window close), react_status (measured | absent |
                    no_profiling_build — a not-measured React timing is
                    null, NEVER 0), react_commits (null only when React
                    is absent), react_actual_ms (null unless measured),
                    gc_ran (bool) and js_heap_used (post-GC live heap;
                    null unless gc_ran). The result also carries a
                    methodology_warnings list ALONGSIDE raw_samples
                    (metadata only — not a statistical reduction): it
                    flags no throttle, warmup=0, no readiness step, GC
                    disabled, or reset disabled.

  cpu_start       — Start a Profiler CPU sampling session.
  cpu_stop        — Stop it; save the profile JSON (loadable in the
                    DevTools Performance tab) AND return an inline
                    top-function ranking by self/total time.
  cpu_summary     — Re-summarise a saved CPU profile. Param: path (a
                    JSON from cpu_stop). No browser needed — parses the
                    file. Top hot functions by self (where CPU is
                    spent) and by call-tree total time.

  trace_start     — Start a Tracing session. Param: categories (optional,
                    comma-separated; default devtools.timeline,
                    disabled-by-default-v8.cpu_profiler,
                    blink.user_timing).
  trace_stop      — End tracing, await tracingComplete, write
                    {\"traceEvents\":[...]} JSON, and summarise tasks > 50ms.

  why_render      — Best-effort why-did-render: per re-rendered component,
                    the changed props (each labelled reference_changed |
                    value_changed | unknown) and changed hook indices,
                    plus a note that the compare is a shallow reference
                    compare (an inline object/array/fn prop changes
                    reference every render — labelled, NOT a root cause).

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

/// React measurement capability for a run (REQ-BT-019.13 / Allium
/// `ReactStatus`). Serialises to the snake-case discriminator string.
/// The whole point: `absent` / `no_profiling_build` are NOT a numeric 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReactStatus {
    Measured,
    Absent,
    NoProfilingBuild,
}

/// The in-page accumulators read once at window close via
/// `window.__phoenix.__perfRead()` (REQ-BT-019.20). `__perfRead` returns
/// a JSON *string* (from `evaluate`), which is parsed into this; the
/// harness never derives these from host-side CDP counter deltas (F5).
#[derive(Debug, Clone, Deserialize)]
struct PerfReading {
    /// Sum of `longtask` durations within the page-anchored window (ms).
    script_ms: f64,
    /// Count of `longtask` entries (>50 ms) within the window.
    long_tasks: u64,
    /// `performance.now()` span of the window; null if it never opened.
    wall_ms: Option<f64>,
    /// `document.getElementsByTagName('*').length` at window close.
    dom_nodes: u64,
    /// `measured` | `absent` | `no_profiling_build` discriminator.
    react_status: ReactStatus,
    /// Commit count over the window; null when React is absent.
    react_commits: Option<u64>,
    /// Summed root-fiber `actualDuration` over the window; null unless
    /// `react_status == measured`.
    react_actual_ms: Option<f64>,
}

impl PerfReading {
    /// The defensive default if `__perfRead` is missing (non-React /
    /// exotic page) — mirrors the in-page absent defaults.
    fn absent_default() -> Self {
        PerfReading {
            script_ms: 0.0,
            long_tasks: 0,
            wall_ms: None,
            dom_nodes: 0,
            react_status: ReactStatus::Absent,
            react_commits: None,
            react_actual_ms: None,
        }
    }
}

/// One raw per-run sample from the PAGE-ANCHORED window (REQ-BT-019.20).
/// Serialised verbatim into `raw_samples`; the harness never reduces
/// these (invariant `RawSamplesNeverReduced`).
///
/// `script_ms`/`long_tasks`/`wall_ms`/`dom_nodes` come from in-page
/// accumulators reset after readiness and read at window close — NOT
/// host-bracketed CDP counter deltas (F5: those collapse to ~0 for real
/// in-window work).
///
/// Every "not measured" field is `Option<T>` WITHOUT
/// `skip_serializing_if`: the key is always present and serialises as
/// JSON `null` so a not-taken measurement is visibly absent rather than
/// looking like a real zero (REQ-BT-019.13/.15/.19, invariants
/// `ReactTimingOnlyWhenMeasured` / `ReactCommitsAbsentOnlyWhenNoReact` /
/// `HeapOnlyWhenGc`).
#[derive(Debug, Clone, serde::Serialize)]
struct RunSample {
    run_index: u32,
    /// Sum of `longtask` durations within the page-anchored window (ms).
    script_ms: f64,
    /// Count of `longtask` entries (>50 ms) within the window.
    long_tasks: u64,
    /// `performance.now()` span of the measured window; `None` (JSON
    /// null) only defensively when the window never opened.
    wall_ms: Option<f64>,
    /// `document.getElementsByTagName('*').length` at window close.
    dom_nodes: u64,
    /// True iff a forced GC ran for this run (REQ-BT-019.15).
    gc_ran: bool,
    /// Post-full-GC live-heap read. `Some` ONLY when `gc_ran`; `None`
    /// (JSON null) otherwise — invariant `HeapOnlyWhenGc`.
    js_heap_used: Option<f64>,
    /// Always present; discriminates the two Option fields below.
    react_status: ReactStatus,
    /// `Some` whenever React is on the page (incl. production builds —
    /// the commit hook fires regardless); `None` (null) when React is
    /// absent — invariant `ReactCommitsAbsentOnlyWhenNoReact`.
    react_commits: Option<u64>,
    /// `Some` ONLY when `react_status == measured`; `None` (null)
    /// otherwise — invariant `ReactTimingOnlyWhenMeasured`.
    react_actual_ms: Option<f64>,
}

/// Why a single run did not yield a sample.
enum RunError {
    /// A readiness step did not satisfy in time — fails the whole op,
    /// names the step (REQ-BT-019.1 / `BlockedScenarioYieldsNoSamples`).
    Blocked(String),
    /// Infrastructure failure (metrics snapshot) — abort the op.
    Infra(String),
}

/// Perform the per-run reset (REQ-BT-019.18) BEFORE the before-snapshot.
/// A reset failure is an infra error for the run — never a bogus sample.
async fn do_reset(session: &Arc<RwLock<BrowserSession>>, reset: &Reset) -> Result<(), RunError> {
    let guard = session.read().await;
    match reset {
        Reset::Skip => Ok(()),
        Reset::ReloadCurrent | Reset::Reload => guard
            .page
            .reload()
            .await
            .map(|_| ())
            .map_err(|e| RunError::Infra(format!("per-run reset (reload) failed: {e}"))),
        Reset::Navigate(url) => guard
            .page
            .goto(url)
            .await
            .map(|_| ())
            .map_err(|e| RunError::Infra(format!("per-run reset (navigate {url}) failed: {e}"))),
    }
}

/// Execute one scenario run with a PAGE-ANCHORED measurement window
/// (REQ-BT-019.18/.20, Allium @guidance). `Ok(sample)` carries the
/// windowed metrics for this single execution; the caller decides
/// whether to keep it (warmup runs are executed identically but
/// discarded).
///
/// Per-run sequence (REQ-BT-019.18/.20, Allium @guidance):
///   reset → dispatch steps up to and INCLUDING the first readiness step
///   as UNTIMED setup → OPEN the window (`__phoenix.__perfReset()`:
///   `t0 = performance.now()`, longtask accumulators zeroed, React
///   commit buffer cleared) → dispatch the REMAINING measured steps → READ the
///   window once (`__phoenix.__perfRead()`) → if `gc_per_run`:
///   collectGarbage then a heap-only read (strictly outside the window,
///   F5 does not apply to a one-shot gauge) → emit one sample.
async fn run_one(
    session: &Arc<RwLock<BrowserSession>>,
    steps: &[Step],
    run_index: u32,
    reset: &Reset,
    gc_per_run: bool,
) -> Result<RunSample, RunError> {
    // Reset to a fixed state so runs are mutually comparable. Reset
    // failure aborts the run cleanly.
    do_reset(session, reset).await?;

    // REQ-BT-019.18: the reset AND the first readiness step are UNTIMED
    // setup — page load + framework mount + async settle happen before
    // the window opens. `setup_end` is the index of the FIRST readiness
    // step; steps `0..=setup_end` are dispatched untimed. With no
    // readiness step the window opens immediately after reset (degraded;
    // build_methodology_warnings flags it).
    let setup_end = steps.iter().position(|s| {
        matches!(
            s,
            Step::WaitSelector { .. } | Step::WaitTiming { .. } | Step::WaitEval { .. }
        )
    });

    // Dispatch the UNTIMED setup steps (`0..=setup_end`, or none).
    let measured_start = match setup_end {
        Some(idx) => {
            for step in &steps[..=idx] {
                if let Err(reason) = run_step(session, step).await {
                    return Err(RunError::Blocked(reason));
                }
            }
            idx + 1
        }
        None => 0,
    };

    // OPEN the page-anchored window (REQ-BT-019.20). Best-effort: if
    // `__perfReset` is missing (non-React/exotic page) the read still
    // returns defaults, and the longtask observer is installed regardless
    // via document-start injection.
    {
        let guard = session.read().await;
        let _ = guard
            .page
            .evaluate("window.__phoenix && window.__phoenix.__perfReset && window.__phoenix.__perfReset()")
            .await;
    }

    // Dispatch the REMAINING (measured) steps. A readiness failure here
    // also blocks the whole operation (REQ-BT-019.1).
    for step in &steps[measured_start..] {
        if let Err(reason) = run_step(session, step).await {
            return Err(RunError::Blocked(reason));
        }
    }

    // CLOSE the window: read the in-page accumulators in one call
    // (REQ-BT-019.20). `__perfRead` returns a JSON *string* (evaluate
    // returns a String); parse it into the typed reading. A missing
    // helper / shape mismatch is the absent default, never a fabricated
    // value.
    let reading: PerfReading = {
        let guard = session.read().await;
        let script = "window.__phoenix && window.__phoenix.__perfRead \
            ? window.__phoenix.__perfRead() \
            : JSON.stringify({script_ms:0,long_tasks:0,wall_ms:null,dom_nodes:0,\
            react_status:'absent',react_commits:null,react_actual_ms:null})";
        match guard.page.evaluate(script).await {
            Ok(res) => match res.into_value::<String>() {
                Ok(json) => {
                    serde_json::from_str(&json).unwrap_or_else(|_| PerfReading::absent_default())
                }
                Err(_) => PerfReading::absent_default(),
            },
            Err(_) => PerfReading::absent_default(),
        }
    };

    // Forced GC + heap read, strictly AFTER the window closes
    // (REQ-BT-019.15 / invariant HeapOnlyWhenGc). The heap is a one-shot
    // post-GC gauge — F5 does not apply to a gauge. When GC is disabled
    // the heap field is null + gc_ran=false — never a mid-cycle value.
    let (gc_ran, js_heap_used) = if gc_per_run {
        {
            let guard = session.read().await;
            if let Err(e) = guard.page.execute(HeapEnableParams::default()).await {
                return Err(RunError::Infra(format!(
                    "per-run forced GC (HeapProfiler.enable) failed: {e}"
                )));
            }
            if let Err(e) = guard.page.execute(CollectGarbageParams::default()).await {
                return Err(RunError::Infra(format!(
                    "per-run forced GC (collectGarbage) failed: {e}"
                )));
            }
        }
        // Heap-only read at the single post-full-GC point.
        match read_metrics(session).await {
            Ok(m) => (true, m.get("JSHeapUsedSize").copied()),
            Err(e) => return Err(RunError::Infra(format!("post-GC heap read failed: {e}"))),
        }
    } else {
        (false, None)
    };

    Ok(RunSample {
        run_index,
        script_ms: reading.script_ms,
        long_tasks: reading.long_tasks,
        wall_ms: reading.wall_ms,
        dom_nodes: reading.dom_nodes,
        gc_ran,
        js_heap_used,
        react_status: reading.react_status,
        react_commits: reading.react_commits,
        react_actual_ms: reading.react_actual_ms,
    })
}

/// Build the success `ToolOutput`, escaping large raw-sample arrays to
/// `/tmp`. `samples` is the untouched per-run vector — never reduced.
/// `warnings` is carried ALONGSIDE the samples (REQ-BT-019.16) — it is
/// metadata, never a reduction of the samples (REQ-BT-019.5 still holds).
async fn scenario_success_output(
    samples: &[RunSample],
    runs: u32,
    warmup: u32,
    warnings: &[String],
) -> ToolOutput {
    // HARD CONSTRAINT (REQ-BT-019.5 / invariant RawSamplesNeverReduced):
    // emit the RAW per-run array verbatim. No mean/stddev/any reduction.
    let raw = serde_json::to_value(samples).unwrap_or(Value::Array(vec![]));
    let payload = json!({
        "outcome": "completed",
        "requested_runs": runs,
        "warmup": warmup,
        "raw_samples": raw,
        "methodology_warnings": warnings,
        "note": "RAW per-run samples. The harness computes NO statistics — \
                 compute mean/variance/significance yourself. \
                 methodology_warnings is metadata only (not a reduction).",
    });
    let pretty = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    if pretty.len() > OUTPUT_ESCAPE_BYTES {
        let path = format!("/tmp/phoenix-scenario-{}.json", uuid::Uuid::new_v4());
        if let Err(e) = tokio::fs::write(&path, &pretty).await {
            return ToolOutput::error(format!("Failed to write scenario output: {e}"));
        }
        // Surface the saved-samples path on the structured payload too, so
        // the UI renderer can show a visible/copyable footnote — the text
        // body's path reference is the only one rendered without it.
        let mut escaped_payload = payload;
        if let Value::Object(ref mut map) = escaped_payload {
            map.insert("samples_path".to_string(), Value::String(path.clone()));
        }
        ToolOutput::success(format!(
            "run_scenario completed: {runs} raw per-run samples (warmup {warmup} discarded). \
             Full raw samples written to {path} (use `cat`). NOT reduced — compute stats yourself."
        ))
        .with_display(escaped_payload)
    } else {
        ToolOutput::success(pretty).with_display(payload)
    }
}

/// REQ-BT-019.16/.18: build the `methodology_warnings` list — metadata
/// flagging a run that is unguarded in a way that invalidates a naive
/// reading. This is NOT a statistical reduction; it rides alongside the
/// raw samples (REQ-BT-019.5 still holds).
fn build_methodology_warnings(
    input: &ProfileInput,
    steps: &[Step],
    gc_per_run: bool,
    reset: &Reset,
) -> Vec<String> {
    let mut w: Vec<String> = Vec::new();
    if input.throttle_rate.is_none() {
        w.push(
            "no CPU throttle — host/thermal noise dominates; results are not \
             comparable across machines or thermal states"
                .to_string(),
        );
    }
    if input.warmup == Some(0) {
        w.push(
            "warmup=0 — cold JIT/first-paint is in the sample set; the first \
             run(s) measure compilation, not steady state"
                .to_string(),
        );
    }
    let has_readiness = steps.iter().any(|s| {
        matches!(
            s,
            Step::WaitSelector { .. } | Step::WaitTiming { .. } | Step::WaitEval { .. }
        )
    });
    if !has_readiness {
        w.push(
            "no readiness step — measuring an indeterminate point (no \
             wait_selector/wait_timing/wait_eval in steps)"
                .to_string(),
        );
    }
    if !gc_per_run {
        w.push("per-run GC disabled — js_heap_used is null, heap not measured".to_string());
    }
    if matches!(reset, Reset::Skip) {
        w.push(
            "per-run reset disabled (reset=\"none\") — state bleeds across \
             runs; runs are not mutually comparable"
                .to_string(),
        );
    }
    w
}

/// The validated, browser-independent plan for one `run_scenario`
/// invocation. Produced by [`validate_run_scenario`] BEFORE the browser
/// is acquired, so a structurally invalid scenario fails with the real
/// error even when Chrome is unavailable (Codex P2). Single validation
/// site — no parallel re-checking in [`action_run_scenario`].
struct ScenarioPlan {
    steps: Vec<Step>,
    runs: u32,
    warmup: u32,
    gc_per_run: bool,
    reset: Reset,
}

/// Pure, no-browser validation of a `run_scenario` request. Mirrors the
/// preconditions in `browser-profiling.allium`
/// (`RunScenarioCollectsRawSamples` / `RunScenarioRejectsInlineNavigation`).
fn validate_run_scenario(input: &ProfileInput) -> Result<ScenarioPlan, ToolOutput> {
    let steps = match &input.steps {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            return Err(ToolOutput::error(
                "run_scenario requires a non-empty `steps` array",
            ))
        }
    };

    // REQ-BT-019.14 / RunScenarioRejectsInlineNavigation: a navigate/
    // reload inside `steps` would reset cumulative page state mid-run.
    // Reject before anything runs — no sample set to misread. This is
    // the path the non-browser validation test depends on; it must not
    // require Chrome to reach it.
    for (i, step) in steps.iter().enumerate() {
        let kind = match step {
            Step::Navigate { .. } => Some("navigate"),
            Step::Reload => Some("reload"),
            Step::Click { .. }
            | Step::Type { .. }
            | Step::Key { .. }
            | Step::Eval { .. }
            | Step::WaitSelector { .. }
            | Step::WaitTiming { .. }
            | Step::WaitEval { .. } => None,
        };
        if let Some(kind) = kind {
            return Err(ToolOutput::error(format!(
                "run_scenario: steps[{i}] is a `{kind}` step. Navigation/reload \
                 inside `steps` resets cumulative page state mid-run. Put \
                 navigation in the per-run `reset` instead (it runs before the \
                 measured window opens). No samples produced."
            )));
        }
    }

    let runs = input.runs.unwrap_or(1);
    if runs < 1 {
        return Err(ToolOutput::error("run_scenario requires `runs` >= 1"));
    }
    if let Some(tr) = input.throttle_rate {
        if tr < 1.0 {
            return Err(ToolOutput::error(format!(
                "Invalid throttle_rate {tr}: must be >= 1 (1 = no throttling)"
            )));
        }
    }

    Ok(ScenarioPlan {
        steps,
        runs,
        // REQ-BT-019.16: warmup defaults to 1 (cold JIT/first-paint excluded).
        warmup: resolve_warmup(input.warmup),
        gc_per_run: input.gc_per_run.unwrap_or(true),
        reset: Reset::resolve(input.reset.as_ref()),
    })
}

async fn action_run_scenario(
    session: &Arc<RwLock<BrowserSession>>,
    input: &ProfileInput,
    plan: ScenarioPlan,
) -> ToolOutput {
    let ScenarioPlan {
        steps,
        runs,
        warmup,
        gc_per_run,
        reset,
    } = plan;

    // REQ-BT-019.16/.18: methodology warnings — metadata flagging an
    // unguarded run. Carried ALONGSIDE samples, never in place of them.
    let warnings = build_methodology_warnings(input, &steps, gc_per_run, &reset);

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
        // Warmup runs execute identically (including the per-run reset)
        // but are discarded.
        match run_one(
            session,
            &steps,
            global_idx.saturating_sub(warmup),
            &reset,
            gc_per_run,
        )
        .await
        {
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
                    "methodology_warnings": warnings,
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
    scenario_success_output(&samples, runs, warmup, &warnings).await
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
    // REQ-BT-019.7: return the summary INLINE, not just a file path. A
    // file an agent cannot read is an artifact, not an answer; the file
    // is still kept for a human / DevTools deep-dive.
    //
    // Compute rankings ONCE and use them to render both the text summary
    // and the structured display_data — re-running build_cpu_rankings on
    // a large profile is wasteful and a future-divergence risk.
    let rankings = build_cpu_rankings(&profile, CPU_SUMMARY_TOP_N);
    let summary_text = rankings
        .as_ref()
        .map_or_else(|| cpu_empty_text(&profile), render_cpu_summary_text);
    let out = ToolOutput::success(format!(
        "CPU profile saved to {path} (load in Chrome DevTools → Performance for the full tree).\n\n{summary_text}"
    ));
    match rankings.as_ref().and_then(|s| cpu_summary_json(s, &path)) {
        Some(display) => out.with_display(display),
        None => out,
    }
}

/// `cpu_summary`: re-summarise a saved CPU profile JSON without a
/// browser. Lets an agent re-read an earlier `cpu_stop` profile (or one
/// captured elsewhere) for the hot-function ranking.
async fn action_cpu_summary(path: Option<&str>) -> ToolOutput {
    let Some(path) = path else {
        return ToolOutput::error("cpu_summary requires `path` (a saved CPU profile JSON)");
    };
    let data = match tokio::fs::read_to_string(path).await {
        Ok(d) => d,
        Err(e) => return ToolOutput::error(format!("Failed to read {path}: {e}")),
    };
    let profile: CpuProfile = match serde_json::from_str(&data) {
        Ok(p) => p,
        Err(e) => {
            return ToolOutput::error(format!(
                "{path} is not a valid CPU profile JSON (expected Profiler.Profile shape): {e}"
            ))
        }
    };
    // Same once-per-request ranking computation as action_cpu_stop.
    let rankings = build_cpu_rankings(&profile, CPU_SUMMARY_TOP_N);
    let summary_text = rankings
        .as_ref()
        .map_or_else(|| cpu_empty_text(&profile), render_cpu_summary_text);
    let out = ToolOutput::success(format!("CPU profile {path}\n\n{summary_text}"));
    match rankings.as_ref().and_then(|s| cpu_summary_json(s, path)) {
        Some(display) => out.with_display(display),
        None => out,
    }
}

/// How many hot functions to show in a CPU summary.
const CPU_SUMMARY_TOP_N: usize = 15;

type CpuNodeMap<'a> = std::collections::HashMap<i64, &'a CpuProfileNode>;

/// Per-node self microseconds (or hit counts when sampling data is
/// absent). Returns `(self_by_id, total, used_hitcount_fallback)`.
///
/// Each `timeDeltas[i]` (microseconds) is attributed to `samples[i]` —
/// the standard `.cpuprofile` attribution (matches `speedscope` /
/// `DevTools` for ranking). When `samples`/`timeDeltas` are absent the
/// profile carries only `hitCount`, so the ranking falls back to hit
/// counts (relative weight, no absolute time) and the summary says so.
//
// cast_precision_loss: sample µs deltas and hit counts are small
// positive integers, orders of magnitude below f64's 2^52 — no real
// precision is at stake here.
#[allow(clippy::cast_precision_loss)]
fn cpu_self_times(p: &CpuProfile) -> (std::collections::HashMap<i64, f64>, f64, bool) {
    let mut self_us: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
    let mut total = 0.0_f64;
    match (&p.samples, &p.time_deltas) {
        (Some(samples), Some(deltas)) if !samples.is_empty() => {
            for (i, &sid) in samples.iter().enumerate() {
                let d = deltas.get(i).copied().unwrap_or(0).max(0) as f64;
                *self_us.entry(sid).or_default() += d;
                total += d;
            }
            (self_us, total, false)
        }
        _ => {
            for n in &p.nodes {
                let h = n.hit_count.unwrap_or(0).max(0) as f64;
                *self_us.entry(n.id).or_default() += h;
                total += h;
            }
            (self_us, total, true)
        }
    }
}

/// Per-node total = self + sum(children totals). Memoised, with a
/// visited-stack guard so a malformed (cyclic) children graph cannot
/// infinitely recurse.
fn cpu_node_total(
    id: i64,
    nodes: &CpuNodeMap,
    self_us: &std::collections::HashMap<i64, f64>,
    memo: &mut std::collections::HashMap<i64, f64>,
    stack: &mut Vec<i64>,
) -> f64 {
    if let Some(&v) = memo.get(&id) {
        return v;
    }
    if stack.contains(&id) {
        return self_us.get(&id).copied().unwrap_or(0.0); // break cycle
    }
    let mut t = self_us.get(&id).copied().unwrap_or(0.0);
    if let Some(node) = nodes.get(&id) {
        if let Some(children) = &node.children {
            stack.push(id);
            for &c in children {
                t += cpu_node_total(c, nodes, self_us, memo, stack);
            }
            stack.pop();
        }
    }
    memo.insert(id, t);
    t
}

/// `name  url:line` (1-based line) for a node id; `(anonymous)` for
/// nameless frames, `<node N>` for a dangling id.
fn cpu_node_label(nodes: &CpuNodeMap, id: i64) -> String {
    match nodes.get(&id) {
        Some(n) => {
            let cf = &n.call_frame;
            let name = if cf.function_name.is_empty() {
                "(anonymous)"
            } else {
                cf.function_name.as_str()
            };
            if cf.url.is_empty() {
                name.to_string()
            } else {
                format!("{name}  {}:{}", cf.url, cf.line_number + 1)
            }
        }
        None => format!("<node {id}>"),
    }
}

/// One hot-function row in the structured CPU summary payload. `value`
/// is wall-time milliseconds when sampled, hit counts when the profile
/// only carries `hitCount` (discriminated by `hitcount_fallback` on the
/// parent struct — units are inseparable from that flag).
#[derive(Debug, Clone, serde::Serialize)]
struct CpuHotEntry {
    label: String,
    value: f64,
    percent: f64,
}

/// Structured form of a CPU profile summary. Produced once from a
/// `Profiler.Profile`; both the text summary and the `display_data`
/// payload are rendered from this. `None` from [`build_cpu_rankings`]
/// signals "no nodes" or "no samples" — the text fallback handles those.
#[derive(Debug, Clone, serde::Serialize)]
struct CpuProfileSummary {
    hitcount_fallback: bool,
    total: f64,
    top_by_self: Vec<CpuHotEntry>,
    top_by_total: Vec<CpuHotEntry>,
}

/// Compute hot-function rankings from a `Profiler.Profile`. Returns
/// `None` when the profile is empty or carries no attributable
/// samples/hits — callers use a text-only "empty" / "no samples" branch.
fn build_cpu_rankings(p: &CpuProfile, top_n: usize) -> Option<CpuProfileSummary> {
    use std::collections::HashMap;

    if p.nodes.is_empty() {
        return None;
    }
    let nodes: CpuNodeMap = p.nodes.iter().map(|n| (n.id, n)).collect();
    let (self_us, total_us, hitcount_fallback) = cpu_self_times(p);
    if total_us <= 0.0 {
        return None;
    }

    let conv = |us: f64| {
        if hitcount_fallback {
            us
        } else {
            us / 1000.0
        }
    };

    let mut agg: HashMap<String, f64> = HashMap::new();
    for (&id, &us) in &self_us {
        *agg.entry(cpu_node_label(&nodes, id)).or_default() += us;
    }
    let mut by_self: Vec<(String, f64)> = agg.into_iter().collect();
    by_self.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let top_by_self: Vec<CpuHotEntry> = by_self
        .into_iter()
        .take(top_n)
        .map(|(label, us)| CpuHotEntry {
            label,
            value: conv(us),
            percent: us / total_us * 100.0,
        })
        .collect();

    let mut memo = HashMap::new();
    let mut by_total: Vec<(i64, f64)> = p
        .nodes
        .iter()
        .map(|n| {
            let mut stack = Vec::new();
            (
                n.id,
                cpu_node_total(n.id, &nodes, &self_us, &mut memo, &mut stack),
            )
        })
        .collect();
    by_total.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let top_by_total: Vec<CpuHotEntry> = by_total
        .into_iter()
        .take_while(|(_, us)| *us > 0.0)
        .take(top_n)
        .map(|(id, us)| CpuHotEntry {
            label: cpu_node_label(&nodes, id),
            value: conv(us),
            percent: us / total_us * 100.0,
        })
        .collect();

    Some(CpuProfileSummary {
        hitcount_fallback,
        total: conv(total_us),
        top_by_self,
        top_by_total,
    })
}

/// Render the structured summary as the human/agent-readable text block
/// (REQ-BT-019.7). Self is aggregated per function (the robust "where is
/// CPU spent" metric, no double-count); total (self + descendants) is
/// per node and labelled as possibly double-counting recursion.
fn render_cpu_summary_text(summary: &CpuProfileSummary) -> String {
    use std::fmt::Write as _;

    let unit = if summary.hitcount_fallback {
        "hits"
    } else {
        "ms"
    };
    let mut out = String::new();
    if summary.hitcount_fallback {
        out.push_str(
            "NOTE: profile has no samples/timeDeltas — ranking by hitCount \
             (relative weight, NOT absolute time).\n\n",
        );
    } else {
        let _ = writeln!(out, "Sampled wall time: {:.1}ms.\n", summary.total);
    }
    let _ = writeln!(
        out,
        "Top {} by SELF time (aggregated per function — where CPU is actually spent):",
        summary.top_by_self.len()
    );
    for entry in &summary.top_by_self {
        let _ = writeln!(
            out,
            "  {:>9.1}{unit}  {:>5.1}%  {}",
            entry.value, entry.percent, entry.label
        );
    }
    let _ = writeln!(
        out,
        "\nTop {} call-tree nodes by TOTAL time (self + descendants; \
         per node — may double-count recursion):",
        summary.top_by_total.len()
    );
    for entry in &summary.top_by_total {
        let _ = writeln!(
            out,
            "  {:>9.1}{unit}  {:>5.1}%  {}",
            entry.value, entry.percent, entry.label
        );
    }
    out
}

/// Text fallback when the profile carries no rankable data — the
/// empty/no-samples cases that [`build_cpu_rankings`] returns `None` for.
fn cpu_empty_text(p: &CpuProfile) -> String {
    if p.nodes.is_empty() {
        "CPU profile is empty (no nodes — was the session too short?).".to_string()
    } else {
        "CPU profile carries no samples or hit counts (session too short to sample).".to_string()
    }
}

/// Wrap a pre-computed [`CpuProfileSummary`] as the `display_data`
/// payload fragment (`cpu_summary` object containing path + rankings).
/// Callers that already have a `CpuProfileSummary` use this to avoid
/// recomputing rankings via [`cpu_summary_display_data`].
fn cpu_summary_json(summary: &CpuProfileSummary, path: &str) -> Option<Value> {
    let mut value = serde_json::to_value(summary).ok()?;
    if let Value::Object(ref mut map) = value {
        map.insert("path".to_string(), Value::String(path.to_string()));
    }
    Some(json!({ "cpu_summary": value }))
}

/// Render a `Profiler.Profile` as a human/agent-readable hot-function
/// ranking (REQ-BT-019.7). Thin test shim — production callers compute
/// rankings once and route through [`render_cpu_summary_text`] +
/// [`cpu_summary_json`] directly.
#[cfg(test)]
fn summarize_cpu_profile(p: &CpuProfile, top_n: usize) -> String {
    build_cpu_rankings(p, top_n)
        .as_ref()
        .map_or_else(|| cpu_empty_text(p), render_cpu_summary_text)
}

/// Build the `display_data` payload fragment for a CPU profile. Thin
/// test shim — production callers compute rankings once and call
/// [`cpu_summary_json`] with the pre-computed summary.
#[cfg(test)]
fn cpu_summary_display_data(p: &CpuProfile, top_n: usize, path: &str) -> Option<Value> {
    let summary = build_cpu_rankings(p, top_n)?;
    cpu_summary_json(&summary, path)
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

    let event_count = events.len();
    let mut summary = format!(
        "Trace saved to {path} ({event_count} events). Long tasks (>50ms): {long_count}, \
         total {long_total_ms:.1}ms."
    );
    if !top.is_empty() {
        summary.push_str("\n  Top long tasks:\n");
        summary.push_str(&top.join("\n"));
    }
    if timed_out {
        summary.push_str("\n  (note: tracingComplete timed out; trace may be partial)");
    }

    // Structured payload for the UI renderer. `long_tasks` carries the
    // full sorted list (text shows only top 5); `ms` is wall-time
    // milliseconds, derived from the CDP `dur` field (microseconds).
    let long_tasks_payload: Vec<Value> = long_tasks
        .iter()
        .map(|(name, dur_us)| {
            json!({
                "name": name,
                "ms": dur_us / 1000.0,
            })
        })
        .collect();
    let display = json!({
        "trace": {
            "path": &path,
            "event_count": event_count,
            "long_task_count": long_count,
            "long_task_total_ms": long_total_ms,
            "long_tasks": long_tasks_payload,
            "timed_out": timed_out,
        }
    });
    ToolOutput::success(summary).with_display(display)
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
                // __getWhyRender() now returns {note, components:[...]}
                // (REQ-BT-019.17): each changedProps entry is {key, kind}
                // where kind ∈ reference_changed | value_changed | unknown.
                let parsed: Value = serde_json::from_str(&s).unwrap_or(Value::Null);
                let components = parsed.get("components").and_then(Value::as_array);
                match components {
                    Some(comps) if !comps.is_empty() => {
                        let note = parsed
                            .get("note")
                            .and_then(Value::as_str)
                            .unwrap_or("shallow reference compare");
                        let pretty = serde_json::to_string_pretty(&parsed)
                            .unwrap_or_else(|_| "{}".to_string());
                        ToolOutput::success(format!(
                            "why-did-render ({} component(s)). NOTE: {note}\n\n{pretty}",
                            comps.len()
                        ))
                        .with_display(json!({ "why_render": parsed }))
                    }
                    _ => ToolOutput::success(
                        "No re-renders with attributable prop/hook changes recorded \
                         (run a scenario or interact first, then call why_render).",
                    ),
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

    /// REQ-BT-019.13/.15 + cross-cutting: a not-measured field MUST
    /// serialise as JSON `null` with the key PRESENT — absence visible,
    /// never a real-looking zero. `react_status` / `gc_ran` always present.
    #[test]
    fn run_sample_not_measured_serializes_as_present_null() {
        let s = RunSample {
            run_index: 0,
            script_ms: 1.0,
            long_tasks: 2,
            wall_ms: Some(42.0),
            dom_nodes: 10,
            gc_ran: false,
            js_heap_used: None,
            react_status: ReactStatus::Absent,
            react_commits: None,
            react_actual_ms: None,
        };
        let v = serde_json::to_value(&s).expect("serialize");
        let obj = v.as_object().expect("object");
        // Keys PRESENT and explicitly null (not skipped).
        for k in ["js_heap_used", "react_commits", "react_actual_ms"] {
            assert!(obj.contains_key(k), "key {k} must be present");
            assert!(
                v[k].is_null(),
                "key {k} must serialize as JSON null, got {}",
                v[k]
            );
        }
        // Always-present discriminators.
        assert_eq!(v["react_status"], "absent");
        assert_eq!(v["gc_ran"], false);

        // Measured + GC variant: the same keys carry real numbers.
        let s2 = RunSample {
            react_status: ReactStatus::Measured,
            react_commits: Some(4),
            react_actual_ms: Some(12.5),
            gc_ran: true,
            js_heap_used: Some(2048.0),
            ..s
        };
        let v2 = serde_json::to_value(&s2).expect("serialize2");
        assert_eq!(v2["react_status"], "measured");
        assert_eq!(v2["react_commits"], 4);
        assert_eq!(v2["react_actual_ms"], 12.5);
        assert_eq!(v2["gc_ran"], true);
        assert_eq!(v2["js_heap_used"], 2048.0);

        // no_profiling_build: commits present, timing still null.
        let s3 = RunSample {
            react_status: ReactStatus::NoProfilingBuild,
            react_commits: Some(7),
            react_actual_ms: None,
            ..s2
        };
        let v3 = serde_json::to_value(&s3).expect("serialize3");
        assert_eq!(v3["react_status"], "no_profiling_build");
        assert_eq!(v3["react_commits"], 7);
        assert!(
            v3["react_actual_ms"].is_null(),
            "no_profiling_build must keep react_actual_ms null"
        );
    }

    /// REQ-BT-019.16: warmup defaults to 1 when omitted; an explicit
    /// value (including 0) is honoured verbatim.
    #[test]
    fn warmup_defaults_to_one() {
        assert_eq!(resolve_warmup(None), 1, "omitted warmup must default to 1");
        assert_eq!(resolve_warmup(Some(0)), 0, "explicit 0 honoured");
        assert_eq!(resolve_warmup(Some(5)), 5, "explicit value honoured");
    }

    /// REQ-BT-019.18: reset resolution. Omitted = reload current URL;
    /// the string "none" = opt out; objects map to explicit actions.
    #[test]
    fn reset_resolves_per_spec() {
        assert!(matches!(Reset::resolve(None), Reset::ReloadCurrent));

        let none: ResetSpec = serde_json::from_value(json!("none")).expect("none string");
        assert!(matches!(Reset::resolve(Some(&none)), Reset::Skip));

        let nav: ResetSpec = serde_json::from_value(json!({"kind":"navigate","url":"about:blank"}))
            .expect("navigate reset");
        assert!(matches!(Reset::resolve(Some(&nav)), Reset::Navigate(u) if u == "about:blank"));

        let rel: ResetSpec =
            serde_json::from_value(json!({"kind":"reload"})).expect("reload reset");
        assert!(matches!(Reset::resolve(Some(&rel)), Reset::Reload));

        // A misspelt opt-out must NOT silently become "no reset".
        assert!(serde_json::from_value::<ResetSpec>(json!("non")).is_err());
    }

    /// REQ-BT-019.7: a saved CPU profile is summarised into a readable
    /// hot-function ranking — the agent gets an answer, not just a file.
    #[test]
    fn cpu_summary_ranks_hot_functions() {
        // Two fns; `hot` sampled 9ms, `cold` 1ms (timeDeltas in µs).
        let profile = json!({
            "nodes": [
                { "id": 1, "callFrame": { "functionName": "(root)", "scriptId": "0",
                    "url": "", "lineNumber": -1, "columnNumber": -1 }, "children": [2, 3] },
                { "id": 2, "callFrame": { "functionName": "hot", "scriptId": "1",
                    "url": "app.js", "lineNumber": 41, "columnNumber": 2 } },
                { "id": 3, "callFrame": { "functionName": "cold", "scriptId": "1",
                    "url": "app.js", "lineNumber": 99, "columnNumber": 4 } }
            ],
            "startTime": 0, "endTime": 10000,
            "samples":    [2, 2, 2, 3],
            "timeDeltas": [3000, 3000, 3000, 1000]
        });
        let p: CpuProfile = serde_json::from_value(profile).expect("valid Profile");
        let out = summarize_cpu_profile(&p, 15);
        assert!(out.contains("Sampled wall time: 10.0ms"), "{out}");
        // hot = 9ms self, must rank above cold and show its location.
        let hot_idx = out
            .find("hot  app.js:42")
            .expect("hot listed with 1-based line");
        let cold_idx = out.find("cold  app.js:100").expect("cold listed");
        assert!(hot_idx < cold_idx, "hot must rank before cold:\n{out}");
        assert!(out.contains("90.0%"), "hot self share shown:\n{out}");
    }

    /// hitCount fallback when samples/timeDeltas absent — labelled, not
    /// silently presented as absolute time.
    #[test]
    fn cpu_summary_hitcount_fallback_is_labelled() {
        let profile = json!({
            "nodes": [
                { "id": 1, "callFrame": { "functionName": "f", "scriptId": "1",
                    "url": "a.js", "lineNumber": 0, "columnNumber": 0 }, "hitCount": 7 }
            ],
            "startTime": 0, "endTime": 1
        });
        let p: CpuProfile = serde_json::from_value(profile).expect("valid Profile");
        let out = summarize_cpu_profile(&p, 5);
        assert!(
            out.contains("hitCount"),
            "fallback must be disclosed:\n{out}"
        );
        assert!(
            out.contains("7.0hits"),
            "hit weight shown, not fake ms:\n{out}"
        );
    }

    /// Empty profile says so rather than emitting a misleading table.
    #[test]
    fn cpu_summary_empty_profile_is_honest() {
        let p: CpuProfile = serde_json::from_value(json!({
            "nodes": [], "startTime": 0, "endTime": 0
        }))
        .expect("valid empty Profile");
        assert!(summarize_cpu_profile(&p, 5).contains("empty"));
    }

    /// Structured display_data: cpu_summary payload carries path + ms-typed
    /// hot-function rankings in the order they're rendered in text.
    #[test]
    fn cpu_summary_display_data_carries_structured_rankings() {
        let profile = json!({
            "nodes": [
                { "id": 1, "callFrame": { "functionName": "(root)", "scriptId": "0",
                    "url": "", "lineNumber": -1, "columnNumber": -1 }, "children": [2, 3] },
                { "id": 2, "callFrame": { "functionName": "hot", "scriptId": "1",
                    "url": "app.js", "lineNumber": 41, "columnNumber": 2 } },
                { "id": 3, "callFrame": { "functionName": "cold", "scriptId": "1",
                    "url": "app.js", "lineNumber": 99, "columnNumber": 4 } }
            ],
            "startTime": 0, "endTime": 10000,
            "samples":    [2, 2, 2, 3],
            "timeDeltas": [3000, 3000, 3000, 1000]
        });
        let p: CpuProfile = serde_json::from_value(profile).expect("valid Profile");
        let display = cpu_summary_display_data(&p, 15, "/tmp/some-profile.json")
            .expect("structured payload should be produced");

        let cs = &display["cpu_summary"];
        assert_eq!(cs["path"], "/tmp/some-profile.json");
        assert_eq!(cs["hitcount_fallback"], false);
        // total wall time = 10ms.
        let total = cs["total"].as_f64().expect("total ms");
        assert!((total - 10.0).abs() < 0.01, "total should be 10ms: {total}");

        let by_self = cs["top_by_self"].as_array().expect("top_by_self array");
        assert!(!by_self.is_empty(), "rankings non-empty");
        // hot ranks first (9ms self), cold second (1ms).
        assert!(by_self[0]["label"]
            .as_str()
            .unwrap()
            .contains("hot  app.js:42"));
        let hot_val = by_self[0]["value"].as_f64().expect("hot value");
        assert!((hot_val - 9.0).abs() < 0.01, "hot value should be 9ms");
        let hot_pct = by_self[0]["percent"].as_f64().expect("hot percent");
        assert!((hot_pct - 90.0).abs() < 0.01, "hot is 90% of total");
    }

    /// hitCount fallback surfaces in display_data with the flag set; `value`
    /// carries raw hit counts (units inseparable from the flag).
    #[test]
    fn cpu_summary_display_data_hitcount_fallback_flag() {
        let profile = json!({
            "nodes": [
                { "id": 1, "callFrame": { "functionName": "f", "scriptId": "1",
                    "url": "a.js", "lineNumber": 0, "columnNumber": 0 }, "hitCount": 7 }
            ],
            "startTime": 0, "endTime": 1
        });
        let p: CpuProfile = serde_json::from_value(profile).expect("valid Profile");
        let display = cpu_summary_display_data(&p, 5, "/tmp/x.json")
            .expect("non-empty profile yields payload");
        assert_eq!(display["cpu_summary"]["hitcount_fallback"], true);
        let by_self = display["cpu_summary"]["top_by_self"]
            .as_array()
            .expect("rankings");
        assert!((by_self[0]["value"].as_f64().unwrap() - 7.0).abs() < 0.01);
    }

    /// Empty / no-sample profiles yield no display_data — the text output
    /// already explains the absence, and a payload with empty arrays
    /// would invite the UI to render a confusing empty table.
    #[test]
    fn cpu_summary_display_data_returns_none_on_empty() {
        let empty: CpuProfile = serde_json::from_value(json!({
            "nodes": [], "startTime": 0, "endTime": 0
        }))
        .expect("empty Profile");
        assert!(cpu_summary_display_data(&empty, 5, "/tmp/x.json").is_none());

        let no_samples: CpuProfile = serde_json::from_value(json!({
            "nodes": [
                { "id": 1, "callFrame": { "functionName": "f", "scriptId": "1",
                    "url": "a.js", "lineNumber": 0, "columnNumber": 0 } }
            ],
            "startTime": 0, "endTime": 0
        }))
        .expect("Profile");
        assert!(cpu_summary_display_data(&no_samples, 5, "/tmp/x.json").is_none());
    }
}
