use crate::coordinator_query::sqlite_symbolic_code;
#[cfg(test)]
use libsqlite3_sys as ffi;
use sqlx::error::DatabaseError;
use std::time::Duration;
use tracing::{field, Span};

pub(crate) const SLOW_ACQUIRE: Duration = Duration::from_millis(100);
pub(crate) const SLOW_TRANSACTION: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DbOperation {
    BeginWorkflowAttempt,
    ClaimDirectTurn,
    ClaimWakeObservation,
}

impl DbOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::BeginWorkflowAttempt => "workflow.begin_attempt",
            Self::ClaimDirectTurn => "workflow.claim_direct_turn",
            Self::ClaimWakeObservation => "workflow.claim_wake_observation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DbOutcome {
    Success,
    Ineligible,
    AuthorityConflict,
    UnsupportedCodec,
    ContentionRetry,
    RetryExhausted,
    Failure,
}

impl DbOutcome {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Ineligible => "ineligible",
            Self::AuthorityConflict => "authority_conflict",
            Self::UnsupportedCodec => "unsupported_codec",
            Self::ContentionRetry => "contention_retry",
            Self::RetryExhausted => "retry_exhausted",
            Self::Failure => "failure",
        }
    }
}

impl From<phoenix_workflow::ClaimOutcome> for DbOutcome {
    fn from(outcome: phoenix_workflow::ClaimOutcome) -> Self {
        match outcome {
            phoenix_workflow::ClaimOutcome::Started => Self::Success,
            phoenix_workflow::ClaimOutcome::Ineligible => Self::Ineligible,
            phoenix_workflow::ClaimOutcome::AuthorityConflict => Self::AuthorityConflict,
            phoenix_workflow::ClaimOutcome::UnsupportedCodec => Self::UnsupportedCodec,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DbBeginMode {
    Deferred,
}

impl DbBeginMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Deferred => "deferred",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SqliteErrorClass {
    pub(crate) primary: &'static str,
    pub(crate) extended: &'static str,
}

impl SqliteErrorClass {
    pub(crate) fn from_database_error(error: &dyn DatabaseError) -> Option<Self> {
        let extended_code = error.code()?.parse::<i32>().ok()?;
        Some(Self::from_extended_code(extended_code))
    }

    pub(crate) fn from_extended_code(extended_code: i32) -> Self {
        Self {
            primary: sqlite_symbolic_code(extended_code & 0xff),
            extended: sqlite_symbolic_code(extended_code),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetryAccounting {
    attempts: u32,
    retries: u32,
}

impl RetryAccounting {
    pub(crate) fn start_attempt(&mut self) -> u32 {
        self.attempts += 1;
        self.attempts
    }

    pub(crate) fn record_retry(&mut self) {
        self.retries += 1;
    }

    pub(crate) const fn attempts(self) -> u32 {
        self.attempts
    }

    pub(crate) const fn retries(self) -> u32 {
        self.retries
    }
}

pub(crate) fn operation_span(operation: DbOperation) -> Span {
    tracing::info_span!(
        target: "phoenix_db::otel",
        "db.operation",
        db.system = "sqlite",
        db.operation = operation.as_str(),
        db.outcome = field::Empty,
        db.attempt_count = field::Empty,
        db.retry_count = field::Empty,
        db.elapsed_ms = field::Empty,
    )
}

pub(crate) fn acquisition_span(operation: DbOperation, attempt: u32, parent: &Span) -> Span {
    tracing::info_span!(
        target: "phoenix_db::otel",
        parent: parent,
        "db.pool.acquire",
        db.system = "sqlite",
        db.operation = operation.as_str(),
        db.attempt = attempt,
        db.outcome = field::Empty,
        db.elapsed_ms = field::Empty,
    )
}

pub(crate) fn transaction_span(
    operation: DbOperation,
    begin_mode: DbBeginMode,
    attempt: u32,
    parent: &Span,
) -> Span {
    tracing::info_span!(
        target: "phoenix_db::otel",
        parent: parent,
        "db.transaction",
        db.system = "sqlite",
        db.operation = operation.as_str(),
        db.begin_mode = begin_mode.as_str(),
        db.attempt = attempt,
        db.outcome = field::Empty,
        db.retry_count = field::Empty,
        db.elapsed_ms = field::Empty,
        db.sqlite.primary = field::Empty,
        db.sqlite.extended = field::Empty,
    )
}

fn elapsed_millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn record_operation(
    span: &Span,
    outcome: DbOutcome,
    accounting: RetryAccounting,
    elapsed: Duration,
) {
    span.record("db.outcome", outcome.as_str());
    span.record("db.attempt_count", i64::from(accounting.attempts()));
    span.record("db.retry_count", i64::from(accounting.retries()));
    span.record("db.elapsed_ms", elapsed_millis(elapsed));
}

pub(crate) fn record_acquisition(
    span: &Span,
    operation: DbOperation,
    elapsed: Duration,
    success: bool,
) {
    let outcome = if success {
        DbOutcome::Success
    } else {
        DbOutcome::Failure
    };
    let elapsed_ms = elapsed_millis(elapsed);
    span.record("db.outcome", outcome.as_str());
    span.record("db.elapsed_ms", elapsed_ms);
    if !success {
        tracing::warn!(
            target: "phoenix_db::observability",
            db_operation = operation.as_str(),
            db_outcome = outcome.as_str(),
            db_elapsed_ms = elapsed_ms,
            "SQLite pool acquisition failed"
        );
    } else if elapsed >= SLOW_ACQUIRE {
        tracing::warn!(
            target: "phoenix_db::observability",
            db_operation = operation.as_str(),
            db_outcome = outcome.as_str(),
            db_elapsed_ms = elapsed_ms,
            "slow SQLite pool acquisition"
        );
    }
}

pub(crate) fn record_transaction(
    span: &Span,
    operation: DbOperation,
    outcome: DbOutcome,
    accounting: RetryAccounting,
    elapsed: Duration,
    sqlite: Option<SqliteErrorClass>,
) {
    let elapsed_ms = elapsed_millis(elapsed);
    span.record("db.outcome", outcome.as_str());
    span.record("db.retry_count", i64::from(accounting.retries()));
    span.record("db.elapsed_ms", elapsed_ms);
    if let Some(sqlite) = sqlite {
        span.record("db.sqlite.primary", sqlite.primary);
        span.record("db.sqlite.extended", sqlite.extended);
    }

    if outcome == DbOutcome::ContentionRetry {
        tracing::info!(
            target: "phoenix_db::observability",
            db_operation = operation.as_str(),
            db_outcome = outcome.as_str(),
            db_attempt = accounting.attempts(),
            db_retry_count = accounting.retries(),
            db_elapsed_ms = elapsed_ms,
            db_sqlite_primary = sqlite.map(|value| value.primary),
            db_sqlite_extended = sqlite.map(|value| value.extended),
            "retrying SQLite transaction after contention"
        );
    } else if matches!(outcome, DbOutcome::Failure | DbOutcome::RetryExhausted) {
        tracing::error!(
            target: "phoenix_db::observability",
            db_operation = operation.as_str(),
            db_outcome = outcome.as_str(),
            db_attempt = accounting.attempts(),
            db_retry_count = accounting.retries(),
            db_elapsed_ms = elapsed_ms,
            db_sqlite_primary = sqlite.map(|value| value.primary),
            db_sqlite_extended = sqlite.map(|value| value.extended),
            "SQLite transaction failed"
        );
    } else if elapsed >= SLOW_TRANSACTION {
        tracing::warn!(
            target: "phoenix_db::observability",
            db_operation = operation.as_str(),
            db_outcome = outcome.as_str(),
            db_attempt = accounting.attempts(),
            db_retry_count = accounting.retries(),
            db_elapsed_ms = elapsed_ms,
            "slow SQLite transaction"
        );
    }
}

pub(crate) fn sqlite_class(error: &sqlx::Error) -> Option<SqliteErrorClass> {
    error
        .as_database_error()
        .and_then(SqliteErrorClass::from_database_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_vocabulary_is_bounded_and_stable() {
        assert_eq!(
            DbOperation::BeginWorkflowAttempt.as_str(),
            "workflow.begin_attempt"
        );
        assert_eq!(DbBeginMode::Deferred.as_str(), "deferred");
        assert_eq!(DbOutcome::ContentionRetry.as_str(), "contention_retry");
        assert_eq!(DbOutcome::RetryExhausted.as_str(), "retry_exhausted");
    }

    #[test]
    fn classifies_every_claim_outcome_without_collapsing_ineligible() {
        use phoenix_workflow::ClaimOutcome;

        assert_eq!(DbOutcome::from(ClaimOutcome::Started), DbOutcome::Success);
        assert_eq!(
            DbOutcome::from(ClaimOutcome::Ineligible),
            DbOutcome::Ineligible
        );
        assert_eq!(
            DbOutcome::from(ClaimOutcome::AuthorityConflict),
            DbOutcome::AuthorityConflict
        );
        assert_eq!(
            DbOutcome::from(ClaimOutcome::UnsupportedCodec),
            DbOutcome::UnsupportedCodec
        );
    }

    #[test]
    fn classifies_primary_and_extended_sqlite_codes_symbolically() {
        let busy_snapshot = SqliteErrorClass::from_extended_code(ffi::SQLITE_BUSY_SNAPSHOT);
        assert_eq!(busy_snapshot.primary, "SQLITE_BUSY");
        assert_eq!(busy_snapshot.extended, "SQLITE_BUSY_SNAPSHOT");

        let foreign_key = SqliteErrorClass::from_extended_code(ffi::SQLITE_CONSTRAINT_FOREIGNKEY);
        assert_eq!(foreign_key.primary, "SQLITE_CONSTRAINT");
        assert_eq!(foreign_key.extended, "SQLITE_CONSTRAINT_FOREIGNKEY");
    }

    #[test]
    fn unknown_extended_codes_fall_back_to_bounded_primary_symbol() {
        let unknown = SqliteErrorClass::from_extended_code(ffi::SQLITE_BUSY | (255 << 8));
        assert_eq!(unknown.primary, "SQLITE_BUSY");
        assert_eq!(unknown.extended, "SQLITE_BUSY");
    }

    #[test]
    fn retry_accounting_counts_attempts_separately_from_retries() {
        let mut accounting = RetryAccounting::default();
        assert_eq!(accounting.start_attempt(), 1);
        accounting.record_retry();
        assert_eq!(accounting.start_attempt(), 2);
        accounting.record_retry();
        assert_eq!(accounting.start_attempt(), 3);

        assert_eq!(accounting.attempts(), 3);
        assert_eq!(accounting.retries(), 2);
    }
}
