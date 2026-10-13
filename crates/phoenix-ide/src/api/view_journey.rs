use axum::{Json, http::StatusCode};
use serde::Deserialize;

macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Deserialize, PartialEq)]
        enum $name {
            $(#[serde(rename = $wire)] $variant),+
        }
        impl $name {
            fn as_str(&self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }
    };
}
wire_enum!(Outcome { Ready => "ready", Failed => "failed", Canceled => "canceled", Incomplete => "incomplete" });
wire_enum!(Route { Global => "global", Ordinary => "ordinary" });
wire_enum!(Trigger { Navigation => "navigation", Reload => "reload", BackForward => "back_forward", PwaResume => "pwa_resume" });
wire_enum!(Platform { Ios => "ios", Android => "android", Desktop => "desktop", Unknown => "unknown" });
wire_enum!(Evidence { Commit => "react_commit_estimate" });
wire_enum!(CorrelationKind { Request => "request", Sse => "sse", Trace => "trace" });

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marks {
    shell: Option<f64>,
    snapshot_received: Option<f64>,
    hydration_ready: Option<f64>,
    meaningful_transcript: Option<f64>,
    chapter_data_ready: Option<f64>,
    breadcrumb_presented: Option<f64>,
    breadcrumb_enabled: Option<f64>,
    anchor_settled: Option<f64>,
}

impl Marks {
    fn values(&self) -> [Option<f64>; 8] {
        [
            self.shell,
            self.snapshot_received,
            self.hydration_ready,
            self.meaningful_transcript,
            self.chapter_data_ready,
            self.breadcrumb_presented,
            self.breadcrumb_enabled,
            self.anchor_settled,
        ]
    }
    fn delta(&self) -> Option<f64> {
        Some(self.breadcrumb_presented?.max(self.breadcrumb_enabled?) - self.meaningful_transcript?)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Correlation {
    kind: CorrelationKind,
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Report {
    open_id: uuid::Uuid,
    reporter_id: uuid::Uuid,
    counters_saturated: bool,
    product_conversation_id: Option<uuid::Uuid>,
    segment_id: Option<uuid::Uuid>,
    route: Route,
    trigger: Trigger,
    standalone: bool,
    platform: Platform,
    frontend_source: Option<String>,
    visible: bool,
    marks: Marks,
    outcome: Outcome,
    total_ms: f64,
    background_ms: f64,
    timeline_saturated: bool,
    breadcrumb_ready_minus_meaningful_ms: Option<f64>,
    presentation_evidence: Evidence,
    correlations: Vec<Correlation>,
    dropped_correlations: u32,
    sample_rate: f64,
    admitted_count: u32,
    sampled_count: u32,
    transport_dropped_count: u32,
    transport_failed_count: u32,
}

impl Report {
    fn valid(&self) -> bool {
        let bounded = |n: f64| n.is_finite() && (0.0..=300_000.0).contains(&n);
        let hex = |s: &str| s.bytes().all(|b| b.is_ascii_hexdigit());
        bounded(self.total_ms)
            && bounded(self.background_ms)
            && self.background_ms <= self.total_ms
            && self
                .marks
                .values()
                .into_iter()
                .flatten()
                .all(|n| bounded(n) && n <= self.total_ms)
            && (!self.timeline_saturated || self.total_ms == 300_000.0)
            && self.breadcrumb_ready_minus_meaningful_ms
                == if self.timeline_saturated {
                    None
                } else {
                    self.marks.delta()
                }
            && (self.outcome != Outcome::Ready
                || (!self.timeline_saturated
                    && self.marks.delta().is_some()
                    && self.marks.anchor_settled.is_some()))
            && self
                .frontend_source
                .as_deref()
                .is_none_or(|s| (7..=40).contains(&s.len()) && hex(s))
            && self.correlations.len() <= 16
            && self.correlations.iter().all(|c| match c.kind {
                CorrelationKind::Trace => c.id.len() == 32 && hex(&c.id),
                CorrelationKind::Request | CorrelationKind::Sse => {
                    uuid::Uuid::parse_str(&c.id).is_ok()
                }
            })
            && self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.sample_rate <= 1.0
            && self.sampled_count <= self.admitted_count
            && self.counters_saturated
                == [
                    self.admitted_count,
                    self.sampled_count,
                    self.transport_dropped_count,
                    self.transport_failed_count,
                ]
                .contains(&10_000)
            && [
                self.admitted_count,
                self.sampled_count,
                self.dropped_correlations,
                self.transport_dropped_count,
                self.transport_failed_count,
            ]
            .into_iter()
            .all(|n| n <= 10_000)
    }
}

pub(super) async fn report(Json(report): Json<Report>) -> StatusCode {
    if !report.valid() {
        return StatusCode::BAD_REQUEST;
    }
    let span = tracing::info_span!(target: "phoenix_ide::otel", "browser.conversation_view",
        "open.id" = %report.open_id,
        "browser.reporter_id" = %report.reporter_id,
        "browser.counters_saturated" = report.counters_saturated,
        "product_conversation.id" = report.product_conversation_id.as_ref().map(uuid::Uuid::to_string),
        "segment.id" = report.segment_id.as_ref().map(uuid::Uuid::to_string),
        "browser.route" = report.route.as_str(), "browser.trigger" = report.trigger.as_str(),
        "browser.outcome" = report.outcome.as_str(), "browser.platform" = report.platform.as_str(),
        "browser.standalone" = report.standalone, "browser.visible" = report.visible,
        "browser.frontend_source" = report.frontend_source.as_deref(),
        "server.backend_source" = env!("PHOENIX_GIT_SHA"),
        "browser.presentation_evidence" = report.presentation_evidence.as_str(),
        "browser.total_ms" = report.total_ms, "browser.background_ms" = report.background_ms,
        "browser.timeline_saturated" = report.timeline_saturated,
        "browser.shell_ms" = report.marks.shell,
        "browser.snapshot_received_ms" = report.marks.snapshot_received,
        "browser.hydration_ready_ms" = report.marks.hydration_ready,
        "browser.meaningful_transcript_ms" = report.marks.meaningful_transcript,
        "browser.chapter_data_ready_ms" = report.marks.chapter_data_ready,
        "browser.breadcrumb_presented_ms" = report.marks.breadcrumb_presented,
        "browser.breadcrumb_enabled_ms" = report.marks.breadcrumb_enabled,
        "browser.anchor_settled_ms" = report.marks.anchor_settled,
        "browser.breadcrumb_ready_minus_meaningful_ms" = report.breadcrumb_ready_minus_meaningful_ms,
        "browser.sample_rate" = report.sample_rate,
        "browser.admitted_count" = report.admitted_count, "browser.sampled_count" = report.sampled_count,
        "browser.dropped_correlations" = report.dropped_correlations,
        "browser.transport_dropped_count" = report.transport_dropped_count,
        "browser.transport_failed_count" = report.transport_failed_count,
    );
    let _entered = span.enter();
    for correlation in report.correlations {
        let child = tracing::info_span!(target: "phoenix_ide::otel", "browser.view_correlation",
            "open.id" = %report.open_id, "correlation.kind" = correlation.kind.as_str(),
            "correlation.id" = correlation.id);
        drop(child.enter());
    }
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> serde_json::Value {
        serde_json::json!({
            "open_id": uuid::Uuid::new_v4(), "product_conversation_id": null, "segment_id": null,
            "reporter_id": uuid::Uuid::new_v4(), "counters_saturated": false,
            "route": "global", "trigger": "pwa_resume", "standalone": true, "platform": "ios",
            "frontend_source": "b8fc23f", "visible": true,
            "marks": { "shell": 0, "snapshot_received": null, "hydration_ready": null,
                "meaningful_transcript": 50, "chapter_data_ready": null, "breadcrumb_presented": 10,
                "breadcrumb_enabled": 10, "anchor_settled": null },
            "outcome": "canceled", "total_ms": 100, "background_ms": 40, "timeline_saturated": false,
            "breadcrumb_ready_minus_meaningful_ms": -40, "presentation_evidence": "react_commit_estimate",
            "correlations": [], "dropped_correlations": 0, "sample_rate": 0.25,
            "admitted_count": 4, "sampled_count": 1, "transport_dropped_count": 0, "transport_failed_count": 0
        })
    }

    #[test]
    fn accepts_missing_canceled_reordered_marks_but_not_fabricated_success() {
        let value = payload();
        assert!(
            serde_json::from_value::<Report>(value.clone())
                .unwrap()
                .valid()
        );
        let mut success = value;
        success["outcome"] = "ready".into();
        assert!(!serde_json::from_value::<Report>(success).unwrap().valid());
    }

    #[test]
    fn rejects_content_and_forged_delta_and_excess_correlations() {
        let mut value = payload();
        value["message_text"] = "private".into();
        assert!(serde_json::from_value::<Report>(value).is_err());
        let mut value = payload();
        value["breadcrumb_ready_minus_meaningful_ms"] = 0.into();
        assert!(!serde_json::from_value::<Report>(value).unwrap().valid());
        let mut value = payload();
        value["correlations"] = serde_json::json!(vec![
            serde_json::json!({ "kind": "request", "id": uuid::Uuid::new_v4() });
            17
        ]);
        assert!(!serde_json::from_value::<Report>(value).unwrap().valid());
    }

    #[test]
    fn saturated_timeline_requires_censored_delta() {
        let mut value = payload();
        value["timeline_saturated"] = true.into();
        value["total_ms"] = 300_000.into();
        assert!(
            !serde_json::from_value::<Report>(value.clone())
                .unwrap()
                .valid()
        );
        value["breadcrumb_ready_minus_meaningful_ms"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Report>(value).unwrap().valid());
    }
}
