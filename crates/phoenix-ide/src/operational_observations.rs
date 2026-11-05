//! Bounded process-local observations, independent of persistence and tracing (REQ-DEPLOY-006A).

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FailureBoundary {
    OrdinaryToolSettlement,
    StartupRequiredAuthority,
    SettlementOwnerDisappeared,
    OtherLocalAuthority,
    StructuredLogWrite,
    StructuredLogFlush,
    StdoutWrite,
    StdoutFlush,
    FatalDiagnosticWrite,
    TraceExport,
}

const BOUNDARIES: [FailureBoundary; 10] = [
    FailureBoundary::OrdinaryToolSettlement,
    FailureBoundary::StartupRequiredAuthority,
    FailureBoundary::SettlementOwnerDisappeared,
    FailureBoundary::OtherLocalAuthority,
    FailureBoundary::StructuredLogWrite,
    FailureBoundary::StructuredLogFlush,
    FailureBoundary::StdoutWrite,
    FailureBoundary::StdoutFlush,
    FailureBoundary::FatalDiagnosticWrite,
    FailureBoundary::TraceExport,
];

#[derive(Debug)]
pub(crate) struct FailureCounter {
    count: AtomicU64,
    last_unix_millis: AtomicU64,
}

impl FailureCounter {
    pub(crate) const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            last_unix_millis: AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    pub(crate) fn record(&self) {
        let millis = u64::try_from(Utc::now().timestamp_millis()).unwrap_or(1);
        self.last_unix_millis.fetch_max(millis, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Release);
    }

    fn snapshot(&self, boundary: FailureBoundary) -> FailureObservation {
        let count = self.count.load(Ordering::Acquire);
        let millis = self.last_unix_millis.load(Ordering::Relaxed);
        FailureObservation {
            boundary,
            count,
            last_observed_at: if count == 0 {
                None
            } else {
                i64::try_from(millis)
                    .ok()
                    .and_then(DateTime::from_timestamp_millis)
            },
        }
    }
}

static FAILURES: [FailureCounter; 10] = [const { FailureCounter::new() }; 10];

pub(crate) fn counter(boundary: FailureBoundary) -> &'static FailureCounter {
    &FAILURES[boundary as usize]
}

pub(crate) fn record_authority_failure(boundary: &'static str) {
    let boundary = if boundary == "ordinary_tool_outcome_settlement" {
        FailureBoundary::OrdinaryToolSettlement
    } else if boundary.ends_with("owner_disappeared") {
        FailureBoundary::SettlementOwnerDisappeared
    } else if boundary.starts_with("startup_") {
        FailureBoundary::StartupRequiredAuthority
    } else {
        FailureBoundary::OtherLocalAuthority
    };
    counter(boundary).record();
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct FailureObservation {
    pub boundary: FailureBoundary,
    pub count: u64,
    pub last_observed_at: Option<DateTime<Utc>>,
}

pub(crate) fn snapshot() -> Vec<FailureObservation> {
    BOUNDARIES
        .into_iter()
        .map(|boundary| counter(boundary).snapshot(boundary))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_are_fixed_size_and_keep_only_count_and_time() {
        let counter = FailureCounter::new();
        assert!(counter
            .snapshot(FailureBoundary::TraceExport)
            .last_observed_at
            .is_none());
        for _ in 0..10_000 {
            counter.record();
        }
        let snapshot = counter.snapshot(FailureBoundary::TraceExport);
        assert_eq!(snapshot.count, 10_000);
        assert!(snapshot.last_observed_at.is_some());
        assert_eq!(super::snapshot().len(), 10);
    }
}
