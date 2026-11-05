use crate::operational_observations::{counter, FailureBoundary, FailureCounter};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SpanData, SpanExporter};
use opentelemetry_sdk::Resource;
use serde::Serialize;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use tracing_appender::non_blocking::ErrorCounter;
use ts_rs::TS;

static QUEUE_DROPS: OnceLock<ErrorCounter> = OnceLock::new();
static TRACE_COVERAGE: AtomicU8 = AtomicU8::new(0);

#[derive(Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum TraceFailureCoverage {
    NotConfigured,
    OtlpExportResults,
    DatadogUnavailable,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct DiagnosticSinkObservation {
    pub stdout_configured: bool,
    pub structured_file_configured: bool,
    pub fatal_diagnostic_configured: bool,
    pub structured_queue_drops: Option<u64>,
    pub trace_failure_coverage: TraceFailureCoverage,
}

pub(super) fn configure_trace(exporter: super::TraceExporter) {
    TRACE_COVERAGE.store(
        match exporter {
            super::TraceExporter::None => 0,
            super::TraceExporter::Otlp => 1,
            #[cfg(feature = "datadog-tracing")]
            super::TraceExporter::Datadog => 2,
        },
        Ordering::Relaxed,
    );
}

pub(super) fn install_queue_counter(counter: ErrorCounter) {
    let _ = QUEUE_DROPS.set(counter);
}

pub(crate) fn snapshot(log: &crate::api::LogInfo) -> DiagnosticSinkObservation {
    DiagnosticSinkObservation {
        stdout_configured: log.stdout,
        structured_file_configured: log.file.is_some(),
        fatal_diagnostic_configured: log.fatal_file.is_some(),
        structured_queue_drops: QUEUE_DROPS
            .get()
            .map(|counter| counter.dropped_lines() as u64),
        trace_failure_coverage: match TRACE_COVERAGE.load(Ordering::Relaxed) {
            1 => TraceFailureCoverage::OtlpExportResults,
            2 => TraceFailureCoverage::DatadogUnavailable,
            _ => TraceFailureCoverage::NotConfigured,
        },
    }
}

pub(super) struct ObservedWriter<W> {
    pub inner: W,
    pub write_errors: &'static FailureCounter,
    pub flush_errors: &'static FailureCounter,
}

impl<W: Write> Write for ObservedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = self.inner.write(bytes);
        if result.is_err() {
            self.write_errors.record();
        }
        result
    }

    fn flush(&mut self) -> io::Result<()> {
        let result = self.inner.flush();
        if result.is_err() {
            self.flush_errors.record();
        }
        result
    }
}

#[derive(Debug)]
pub(super) struct ObservedExporter<E>(pub E);

impl<E: SpanExporter> SpanExporter for ObservedExporter<E> {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let result = self.0.export(batch).await;
        if result.is_err() {
            counter(FailureBoundary::TraceExport).record();
        }
        result
    }

    fn shutdown_with_timeout(&mut self, timeout: Duration) -> OTelSdkResult {
        self.0.shutdown_with_timeout(timeout)
    }

    fn shutdown(&mut self) -> OTelSdkResult {
        self.0.shutdown()
    }

    fn force_flush(&mut self) -> OTelSdkResult {
        self.0.force_flush()
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.0.set_resource(resource);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct FailingSink;

    impl Write for FailingSink {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::StorageFull))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }

    #[test]
    fn writer_records_write_and_flush_errors_without_a_logger() {
        let writes = Box::leak(Box::new(FailureCounter::new()));
        let flushes = Box::leak(Box::new(FailureCounter::new()));
        let mut observed_sink = ObservedWriter {
            inner: FailingSink,
            write_errors: writes,
            flush_errors: flushes,
        };
        assert_eq!(
            observed_sink.write(b"private data").unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        assert_eq!(
            observed_sink.flush().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(writes.count(), 1);
        assert_eq!(flushes.count(), 1);
    }

    #[derive(Debug)]
    struct FailingExporter;

    impl SpanExporter for FailingExporter {
        async fn export(&self, _: Vec<SpanData>) -> OTelSdkResult {
            Err(opentelemetry_sdk::error::OTelSdkError::InternalFailure(
                "private error".into(),
            ))
        }
    }

    #[tokio::test]
    async fn exporter_records_failures_without_logging_or_persistence() {
        let before = counter(FailureBoundary::TraceExport).count();
        assert!(ObservedExporter(FailingExporter)
            .export(vec![])
            .await
            .is_err());
        assert!(counter(FailureBoundary::TraceExport).count() > before);
    }
}
