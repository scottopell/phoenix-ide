use crate::{LlmAttemptCapture, LlmResponse, ProviderStreamTelemetry, StreamTelemetryOutputKind};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GenerationKind {
    Text,
    Reasoning,
    Tool,
    Structured,
}

#[derive(Debug, Clone, Copy, Default)]
struct ObservedOutputKinds(u8);

impl ObservedOutputKinds {
    const TEXT: u8 = 1;
    const REASONING: u8 = 1 << 1;
    const TOOL: u8 = 1 << 2;
    const STRUCTURED: u8 = 1 << 3;

    fn record(&mut self, kind: GenerationKind) {
        self.0 |= match kind {
            GenerationKind::Text => Self::TEXT,
            GenerationKind::Reasoning => Self::REASONING,
            GenerationKind::Tool => Self::TOOL,
            GenerationKind::Structured => Self::STRUCTURED,
        };
    }

    fn output_kind(self) -> StreamTelemetryOutputKind {
        match self.0 {
            0 => StreamTelemetryOutputKind::None,
            Self::TEXT => StreamTelemetryOutputKind::Text,
            Self::REASONING => StreamTelemetryOutputKind::Reasoning,
            Self::TOOL => StreamTelemetryOutputKind::Tool,
            Self::STRUCTURED => StreamTelemetryOutputKind::Structured,
            _ => StreamTelemetryOutputKind::Mixed,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StreamTelemetryRecorder {
    dispatch_at: Instant,
    capture: Option<LlmAttemptCapture>,
    first_provider_event_at: Option<Instant>,
    first_generation_event_at: Option<Instant>,
    first_visible_text_at: Option<Instant>,
    last_provider_event_at: Option<Instant>,
    last_generation_event_at: Option<Instant>,
    provider_event_count: u32,
    generation_event_count: u32,
    visible_text_event_count: u32,
    max_provider_gap_ms: Option<u64>,
    max_generation_gap_ms: Option<u64>,
    output_kinds: ObservedOutputKinds,
}

impl StreamTelemetryRecorder {
    pub(crate) fn new(dispatch_at: Instant, capture: Option<LlmAttemptCapture>) -> Self {
        Self {
            dispatch_at,
            capture,
            first_provider_event_at: None,
            first_generation_event_at: None,
            first_visible_text_at: None,
            last_provider_event_at: None,
            last_generation_event_at: None,
            provider_event_count: 0,
            generation_event_count: 0,
            visible_text_event_count: 0,
            max_provider_gap_ms: None,
            max_generation_gap_ms: None,
            output_kinds: ObservedOutputKinds::default(),
        }
    }

    pub(crate) fn record_provider_event_at(&mut self, at: Instant) {
        self.provider_event_count = self.provider_event_count.saturating_add(1);
        if let Some(previous) = self.last_provider_event_at {
            let gap = millis_between(previous, at);
            self.max_provider_gap_ms = Some(self.max_provider_gap_ms.map_or(gap, |m| m.max(gap)));
        }
        self.last_provider_event_at = Some(at);
        self.first_provider_event_at.get_or_insert(at);
        self.publish_progress();
    }

    pub(crate) fn record_generation_event_at(&mut self, at: Instant, kind: GenerationKind) {
        self.generation_event_count = self.generation_event_count.saturating_add(1);
        if let Some(previous) = self.last_generation_event_at {
            let gap = millis_between(previous, at);
            self.max_generation_gap_ms =
                Some(self.max_generation_gap_ms.map_or(gap, |m| m.max(gap)));
        }
        self.last_generation_event_at = Some(at);
        self.first_generation_event_at.get_or_insert(at);
        self.output_kinds.record(kind);
        self.publish_progress();
    }

    pub(crate) fn record_visible_text_at(&mut self, at: Instant) {
        self.visible_text_event_count = self.visible_text_event_count.saturating_add(1);
        self.first_visible_text_at.get_or_insert(at);
        self.output_kinds.record(GenerationKind::Text);
        self.publish_progress();
    }

    fn publish_progress(&self) {
        if let Some(capture) = &self.capture {
            capture.publish_progress(self.snapshot(false));
        }
    }

    pub(crate) fn snapshot(&self, completed: bool) -> ProviderStreamTelemetry {
        ProviderStreamTelemetry {
            dispatch_to_first_provider_event_ms: self
                .first_provider_event_at
                .map(|at| millis_between(self.dispatch_at, at)),
            dispatch_to_first_generation_event_ms: self
                .first_generation_event_at
                .map(|at| millis_between(self.dispatch_at, at)),
            dispatch_to_first_visible_text_ms: self
                .first_visible_text_at
                .map(|at| millis_between(self.dispatch_at, at)),
            provider_event_count: self.provider_event_count,
            generation_event_count: self.generation_event_count,
            visible_text_event_count: self.visible_text_event_count,
            max_provider_gap_ms: self.max_provider_gap_ms,
            max_generation_gap_ms: self.max_generation_gap_ms,
            output_kind: self.output_kinds.output_kind(),
            completed,
        }
    }

    pub(crate) fn finish_success(self) -> ProviderStreamTelemetry {
        self.finish(true)
    }

    fn finish(self, completed: bool) -> ProviderStreamTelemetry {
        self.snapshot(completed)
    }

    pub(crate) fn attach_success(self, response: &mut LlmResponse) {
        response.stream_telemetry = self.finish_success();
    }
}

fn millis_between(start: Instant, end: Instant) -> u64 {
    u64::try_from(end.duration_since(start).as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_tracks_firsts_counts_gaps_and_mixed_output() {
        let start = Instant::now();
        let t1 = start + std::time::Duration::from_millis(5);
        let t2 = start + std::time::Duration::from_millis(9);
        let t3 = start + std::time::Duration::from_millis(25);
        let t4 = start + std::time::Duration::from_millis(30);
        let mut recorder = StreamTelemetryRecorder::new(start, None);
        recorder.record_provider_event_at(t1);
        recorder.record_generation_event_at(t2, GenerationKind::Reasoning);
        recorder.record_provider_event_at(t3);
        recorder.record_generation_event_at(t3, GenerationKind::Tool);
        recorder.record_visible_text_at(t4);
        recorder.record_generation_event_at(t4, GenerationKind::Text);

        let snapshot = recorder.finish_success();
        assert_eq!(snapshot.dispatch_to_first_provider_event_ms, Some(5));
        assert_eq!(snapshot.dispatch_to_first_generation_event_ms, Some(9));
        assert_eq!(snapshot.dispatch_to_first_visible_text_ms, Some(30));
        assert_eq!(snapshot.provider_event_count, 2);
        assert_eq!(snapshot.generation_event_count, 3);
        assert_eq!(snapshot.visible_text_event_count, 1);
        assert_eq!(snapshot.max_provider_gap_ms, Some(20));
        assert_eq!(snapshot.max_generation_gap_ms, Some(16));
        assert_eq!(snapshot.output_kind, StreamTelemetryOutputKind::Mixed);
        assert!(snapshot.completed);
    }
}
