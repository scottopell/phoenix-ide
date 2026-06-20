//! Background sweep that returns usage-limit-errored conversations to Idle
//! once their reported quota window has elapsed (clear-to-ready).
//!
//! When an OpenAI/Codex request hits a usage limit, the conversation lands in
//! `ConvState::Error { error_kind: UsageLimitReached, resets_at: Some(t), .. }`,
//! where `t` is the upstream's reported window-reset instant. Without this
//! sweep the error is sticky until the user manually dismisses or retries —
//! even after the window has reopened, and even though the banner already tells
//! the user the limit "resets at" `t`.
//!
//! Clear-to-ready: at/after `resets_at` the sweep clears the error
//! (`Error -> Idle`) so the conversation is usable again. It does **not**
//! auto-resume the turn — the user sends the next message. The clear goes
//! through `Event::DismissError` rather than a direct DB write so the dismissal
//! marker (suppresses restart auto-continue), the SSE state-change (live UI
//! update), and any in-memory executor state all stay consistent — exactly the
//! path a manual Dismiss takes.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::db::ErrorKind;
use crate::runtime::RuntimeManager;
use crate::state_machine::{ConvState, Event};

/// How often to scan for elapsed usage-limit windows. Clear-to-ready needs no
/// second-precision: nothing auto-runs at the boundary, so a coarse tick keeps
/// the conversation list query cheap while still clearing promptly enough that
/// a returning user finds the conversation ready.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Run the sweep loop forever. Spawn once at startup. The first tick fires
/// immediately, so a restart with already-elapsed windows clears them on boot.
pub async fn run(runtime: Arc<RuntimeManager>) {
    let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
    // A slow scan must not let ticks pile up into a burst afterwards.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        sweep_once(&runtime).await;
    }
}

async fn sweep_once(runtime: &Arc<RuntimeManager>) {
    // `list_conversations` already restricts to non-archived, user-initiated
    // (top-level) conversations — sub-agents are driven by their parent and
    // are not independently resumable, so they are correctly excluded.
    let convs = match runtime.db().list_conversations().await {
        Ok(convs) => convs,
        Err(e) => {
            tracing::warn!(error = %e, "usage-limit sweep: failed to list conversations");
            return;
        }
    };

    let now = Utc::now();
    let mut cleared = 0usize;
    for conv in convs {
        if !is_due_usage_limit_error(&conv.state, now) {
            continue;
        }
        // DismissError is a no-op for any conversation whose live executor
        // state is no longer Error (e.g. the user retried between the list and
        // this send), so the send is safe even against a stale snapshot.
        match runtime.send_event(&conv.id, Event::DismissError).await {
            Ok(()) => {
                cleared += 1;
                tracing::info!(
                    conv_id = %conv.id,
                    "usage-limit window elapsed; cleared error to Idle"
                );
            }
            Err(e) => {
                tracing::warn!(
                    conv_id = %conv.id,
                    error = %e,
                    "usage-limit sweep: failed to clear error"
                );
            }
        }
    }

    if cleared > 0 {
        tracing::info!(count = cleared, "usage-limit sweep cleared elapsed windows");
    }
}

/// True iff `state` is a usage-limit error whose reset window has passed.
fn is_due_usage_limit_error(state: &ConvState, now: DateTime<Utc>) -> bool {
    matches!(
        state,
        ConvState::Error {
            error_kind: ErrorKind::UsageLimitReached,
            resets_at: Some(resets_at),
            ..
        } if *resets_at <= now
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn usage_limit_error(resets_at: Option<DateTime<Utc>>) -> ConvState {
        ConvState::Error {
            message: "You've hit your usage limit.".to_string(),
            error_kind: ErrorKind::UsageLimitReached,
            resets_at,
        }
    }

    #[test]
    fn due_when_reset_in_the_past() {
        assert!(is_due_usage_limit_error(
            &usage_limit_error(Some(at(100))),
            at(200)
        ));
    }

    #[test]
    fn due_exactly_at_reset_instant() {
        assert!(is_due_usage_limit_error(
            &usage_limit_error(Some(at(100))),
            at(100)
        ));
    }

    #[test]
    fn not_due_before_reset() {
        assert!(!is_due_usage_limit_error(
            &usage_limit_error(Some(at(300))),
            at(200)
        ));
    }

    #[test]
    fn not_due_without_reset_time() {
        // A usage-limit error whose 429 carried no reset timestamp is never
        // swept — there is no window to wait out, so it stays user-driven.
        assert!(!is_due_usage_limit_error(&usage_limit_error(None), at(200)));
    }

    #[test]
    fn other_error_kinds_are_never_swept() {
        let state = ConvState::Error {
            message: "boom".to_string(),
            error_kind: ErrorKind::ServerError,
            resets_at: Some(at(100)),
        };
        assert!(!is_due_usage_limit_error(&state, at(200)));
    }

    #[test]
    fn non_error_states_are_never_swept() {
        assert!(!is_due_usage_limit_error(&ConvState::Idle, at(200)));
    }
}
