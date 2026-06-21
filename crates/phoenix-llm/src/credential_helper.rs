//! Interactive credential helper lifecycle management.
//!
//! `CredentialHelper` manages the full lifecycle of a shell-based credential helper:
//! idle → running → valid/failed, with SSE fan-out to multiple concurrent subscribers.

use crate::registry::CredentialSource;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex as TokioMutex};

/// Events emitted by the credential helper stream.
#[derive(Debug, Clone)]
pub enum HelperEvent {
    /// An instruction line from the helper (all lines except the last).
    Line(String),
    /// Helper exited 0; credential is now cached.
    Complete,
    /// Helper exited non-zero or failed to spawn.
    Error {
        exit_code: Option<i32>,
        stderr: String,
    },
}

#[derive(Debug)]
enum HelperInner {
    Idle,
    Running {
        lines_so_far: Vec<String>,
        subscribers: Vec<mpsc::Sender<HelperEvent>>,
    },
    Valid {
        credential: String,
        expires_at: Instant,
    },
    Failed {
        #[allow(dead_code)] // written on failure, read via Debug
        exit_code: Option<i32>,
        #[allow(dead_code)] // written on failure, read via Debug
        stderr: String,
    },
}

/// Observable status of the helper, suitable for API responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStatus {
    Idle,
    Running,
    Valid,
    Failed,
}

/// Manages the lifecycle of an interactive shell credential helper.
pub struct CredentialHelper {
    command: String,
    ttl: Duration,
    inner: TokioMutex<HelperInner>,
    /// Signalled when the helper task transitions out of Running (to Valid or Failed).
    settled: tokio::sync::Notify,
    /// Weak self-reference for auto-triggering from `get()`.
    self_ref: std::sync::OnceLock<std::sync::Weak<Self>>,
}

impl std::fmt::Debug for CredentialHelper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialHelper")
            .field("command", &"[redacted]")
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

impl CredentialHelper {
    #[must_use]
    pub fn new(command: String, ttl: Duration) -> Arc<Self> {
        let this = Arc::new(Self {
            command,
            ttl,
            inner: TokioMutex::new(HelperInner::Idle),
            settled: tokio::sync::Notify::new(),
            self_ref: std::sync::OnceLock::new(),
        });
        let _ = this.self_ref.set(Arc::downgrade(&this));
        this
    }

    /// Wait until the helper transitions out of Running (to Valid or Failed).
    /// Returns immediately if not currently Running.
    pub async fn wait_for_settlement(&self) {
        // Register the notification future BEFORE checking the lock.
        // If we checked first then registered, a settlement between
        // the check and the registration would be lost (TOCTOU).
        let notified = self.settled.notified();
        if !matches!(&*self.inner.lock().await, HelperInner::Running { .. }) {
            return;
        }
        notified.await;
    }

    /// Return the current observable status. Transitions Valid→Idle if TTL has expired.
    pub async fn credential_status(&self) -> CredentialStatus {
        {
            let inner = self.inner.lock().await;
            match &*inner {
                HelperInner::Idle => return CredentialStatus::Idle,
                HelperInner::Running { .. } => return CredentialStatus::Running,
                HelperInner::Failed { .. } => return CredentialStatus::Failed,
                HelperInner::Valid { expires_at, .. } => {
                    if Instant::now() < *expires_at {
                        return CredentialStatus::Valid;
                    }
                    // Expired — fall through to expire_if_needed
                }
            }
        }
        self.expire_if_needed().await;
        CredentialStatus::Idle
    }

    /// If the inner state is Valid but the TTL has elapsed, transition to Idle.
    pub async fn expire_if_needed(&self) {
        let mut inner = self.inner.lock().await;
        if let HelperInner::Valid { expires_at, .. } = &*inner {
            if Instant::now() >= *expires_at {
                *inner = HelperInner::Idle;
            }
        }
    }

    /// Run the helper (or join an in-progress run) and return a stream of events.
    ///
    /// - Already `Valid` (not expired): returns a stream with one `Complete` event.
    /// - `Running`: replays buffered lines then streams live events.
    /// - `Idle` or `Failed`: starts a fresh run.
    pub async fn run_and_stream(
        self: Arc<Self>,
    ) -> tokio_stream::wrappers::ReceiverStream<HelperEvent> {
        let (tx, rx) = mpsc::channel::<HelperEvent>(256);

        // Snapshot replay lines while holding the lock; release before any async send.
        let replay_lines: Vec<String> = {
            let mut inner = self.inner.lock().await;

            match &mut *inner {
                HelperInner::Valid { expires_at, .. } => {
                    if Instant::now() < *expires_at {
                        // Already valid — send Complete and return.
                        let _ = tx.send(HelperEvent::Complete).await;
                        return tokio_stream::wrappers::ReceiverStream::new(rx);
                    }
                    // Expired — start fresh.
                    *inner = HelperInner::Running {
                        lines_so_far: vec![],
                        subscribers: vec![tx.clone()],
                    };
                    drop(inner);
                    Self::spawn_helper_task(Arc::clone(&self));
                    vec![]
                }
                HelperInner::Running {
                    lines_so_far,
                    subscribers,
                } => {
                    // Join existing run: snapshot replay buffer, add our sender.
                    let replay = lines_so_far.clone();
                    subscribers.push(tx.clone());
                    replay
                    // inner drops here (lock released)
                }
                HelperInner::Idle | HelperInner::Failed { .. } => {
                    *inner = HelperInner::Running {
                        lines_so_far: vec![],
                        subscribers: vec![tx.clone()],
                    };
                    drop(inner);
                    Self::spawn_helper_task(Arc::clone(&self));
                    vec![]
                }
            }
        };

        // Replay buffered lines into tx (no lock held).
        for line in replay_lines {
            let _ = tx.send(HelperEvent::Line(line)).await;
        }

        tokio_stream::wrappers::ReceiverStream::new(rx)
    }

    #[allow(clippy::too_many_lines)]
    fn spawn_helper_task(this: Arc<Self>) {
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};

            let mut child = match tokio::process::Command::new("sh")
                .args(["-c", &this.command])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "credential helper failed to spawn");
                    let mut inner = this.inner.lock().await;
                    let subs = Self::drain_subscribers(
                        &mut inner,
                        HelperInner::Failed {
                            exit_code: None,
                            stderr: e.to_string(),
                        },
                    );
                    drop(inner);
                    for sub in subs {
                        let _ = sub
                            .send(HelperEvent::Error {
                                exit_code: None,
                                stderr: e.to_string(),
                            })
                            .await;
                    }
                    return;
                }
            };

            let stdout = child.stdout.take().expect("stdout piped");
            let stderr_handle = child.stderr.take().expect("stderr piped");

            // Stream stderr lines as HelperEvent::Line (instruction/progress output).
            // Many credential helpers (e.g. ddtool) write interactive instructions
            // (OIDC device URLs, codes) to stderr and reserve stdout for the token.
            let this_for_stderr = Arc::clone(&this);
            let stderr_task = tokio::spawn(async move {
                let mut stderr_lines = BufReader::new(stderr_handle).lines();
                let mut collected = Vec::<String>::new();
                while let Ok(Some(line)) = stderr_lines.next_line().await {
                    if line.trim().is_empty() {
                        continue;
                    }
                    collected.push(line.clone());
                    let subs = {
                        let mut inner = this_for_stderr.inner.lock().await;
                        if let HelperInner::Running {
                            lines_so_far,
                            subscribers,
                        } = &mut *inner
                        {
                            lines_so_far.push(line.clone());
                            subscribers.clone()
                        } else {
                            vec![]
                        }
                    };
                    for sub in subs {
                        let _ = sub.send(HelperEvent::Line(line.clone())).await;
                    }
                }
                collected.join("\n")
            });

            // Read stdout lines — the last non-empty line is the credential.
            let mut stdout_lines = BufReader::new(stdout).lines();
            let mut pending: Option<String> = None;

            while let Ok(Some(line)) = stdout_lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(prev) = pending.replace(line.clone()) {
                    // prev is a non-final stdout instruction line — broadcast it
                    let subs = {
                        let mut inner = this.inner.lock().await;
                        if let HelperInner::Running {
                            lines_so_far,
                            subscribers,
                        } = &mut *inner
                        {
                            lines_so_far.push(prev.clone());
                            subscribers.clone()
                        } else {
                            vec![]
                        }
                    };
                    for sub in subs {
                        let _ = sub.send(HelperEvent::Line(prev.clone())).await;
                    }
                }
            }

            // Wait for stderr task and collect output for error reporting.
            let stderr_str = stderr_task.await.unwrap_or_default();

            let status = child.wait().await;
            let exit_code = status.ok().and_then(|s| s.code());
            let success = exit_code == Some(0);

            let mut inner = this.inner.lock().await;

            if let (true, Some(credential)) = (success, pending) {
                let expires_at = Instant::now() + this.ttl;
                let subs = Self::drain_subscribers(
                    &mut inner,
                    HelperInner::Valid {
                        credential,
                        expires_at,
                    },
                );
                drop(inner);
                this.settled.notify_waiters();
                for sub in subs {
                    let _ = sub.send(HelperEvent::Complete).await;
                }
            } else {
                let subs = Self::drain_subscribers(
                    &mut inner,
                    HelperInner::Failed {
                        exit_code,
                        stderr: stderr_str.clone(),
                    },
                );
                drop(inner);
                this.settled.notify_waiters();
                for sub in subs {
                    let _ = sub
                        .send(HelperEvent::Error {
                            exit_code,
                            stderr: stderr_str.clone(),
                        })
                        .await;
                }
            }
        });
    }

    /// Replace inner state and return the subscriber list that was held in the Running variant.
    fn drain_subscribers(
        inner: &mut HelperInner,
        new_state: HelperInner,
    ) -> Vec<mpsc::Sender<HelperEvent>> {
        let old = std::mem::replace(inner, new_state);
        if let HelperInner::Running { subscribers, .. } = old {
            subscribers
        } else {
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::CredentialSource;
    use futures::StreamExt;
    use tokio::time::timeout;

    const TICK: Duration = Duration::from_secs(5);
    const LONG_TTL: Duration = Duration::from_secs(3600);

    fn helper(cmd: &str, ttl: Duration) -> Arc<CredentialHelper> {
        CredentialHelper::new(cmd.to_string(), ttl)
    }

    /// Drain a stream up to and including its first terminal event
    /// (`Complete` or `Error`), bounding each read so a hung helper
    /// fails the test instead of hanging the suite.
    async fn drain(
        mut s: tokio_stream::wrappers::ReceiverStream<HelperEvent>,
    ) -> Vec<HelperEvent> {
        let mut out = Vec::new();
        while let Ok(Some(ev)) = timeout(TICK, s.next()).await {
            let terminal = matches!(ev, HelperEvent::Complete | HelperEvent::Error { .. });
            out.push(ev);
            if terminal {
                break;
            }
        }
        out
    }

    fn lines(events: &[HelperEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                HelperEvent::Line(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn new_helper_starts_idle() {
        let h = helper("printf 'tok\\n'", LONG_TTL);
        assert_eq!(h.credential_status().await, CredentialStatus::Idle);
        assert!(!h.is_recovering().await);
    }

    #[tokio::test]
    async fn successful_run_streams_complete_and_caches_last_line() {
        let h = helper("printf 'tok\\n'", LONG_TTL);
        let events = drain(Arc::clone(&h).run_and_stream().await).await;

        assert!(
            matches!(events.last(), Some(HelperEvent::Complete)),
            "expected terminal Complete, got {events:?}"
        );
        assert_eq!(h.credential_status().await, CredentialStatus::Valid);
        assert_eq!(h.get().await.as_deref(), Some("tok"));
    }

    #[tokio::test]
    async fn non_final_stdout_lines_become_line_events() {
        let h = helper("printf 'a\\nb\\nTOK\\n'", LONG_TTL);
        let events = drain(Arc::clone(&h).run_and_stream().await).await;

        assert_eq!(lines(&events), vec!["a", "b"]);
        assert!(matches!(events.last(), Some(HelperEvent::Complete)));
        // The final non-empty stdout line is the credential, not a Line event.
        assert_eq!(h.get().await.as_deref(), Some("TOK"));
    }

    #[tokio::test]
    async fn stderr_lines_become_line_events_before_complete() {
        // Helpers commonly write instructions (device URLs, codes) to stderr
        // and reserve stdout for the token.
        let h = helper("printf 'visit https://x\\n' 1>&2; printf 'TOK\\n'", LONG_TTL);
        let events = drain(Arc::clone(&h).run_and_stream().await).await;

        assert_eq!(lines(&events), vec!["visit https://x"]);
        assert!(matches!(events.last(), Some(HelperEvent::Complete)));
        assert_eq!(h.get().await.as_deref(), Some("TOK"));
    }

    #[tokio::test]
    async fn blank_lines_are_skipped() {
        let h = helper("printf '\\n\\nTOK\\n'", LONG_TTL);
        let events = drain(Arc::clone(&h).run_and_stream().await).await;

        assert!(lines(&events).is_empty(), "blank lines must not stream");
        assert_eq!(h.get().await.as_deref(), Some("TOK"));
    }

    #[tokio::test]
    async fn nonzero_exit_streams_error_and_sets_failed() {
        let h = helper("printf 'boom\\n' 1>&2; exit 3", LONG_TTL);
        let events = drain(Arc::clone(&h).run_and_stream().await).await;

        match events.last() {
            Some(HelperEvent::Error { exit_code, stderr }) => {
                assert_eq!(*exit_code, Some(3));
                assert!(stderr.contains("boom"), "stderr was {stderr:?}");
            }
            other => panic!("expected terminal Error, got {other:?}"),
        }
        assert_eq!(h.credential_status().await, CredentialStatus::Failed);
        assert_eq!(h.get().await, None);
    }

    #[tokio::test]
    async fn stdout_token_with_nonzero_exit_is_a_failure() {
        // A printed token does not count if the helper itself exits non-zero.
        let h = helper("printf 'tok\\n'; exit 1", LONG_TTL);
        let events = drain(Arc::clone(&h).run_and_stream().await).await;

        assert!(
            matches!(events.last(), Some(HelperEvent::Error { exit_code: Some(1), .. })),
            "got {events:?}"
        );
        assert_eq!(h.credential_status().await, CredentialStatus::Failed);
    }

    #[tokio::test]
    async fn cached_valid_credential_replays_complete_without_respawn() {
        let h = helper("printf 'first\\nTOK\\n'", LONG_TTL);
        let first = drain(Arc::clone(&h).run_and_stream().await).await;
        assert_eq!(lines(&first), vec!["first"]);

        // Second call must short-circuit on the cached Valid state: only a
        // single Complete, no re-run (which would re-emit the "first" line).
        let second = drain(Arc::clone(&h).run_and_stream().await).await;
        assert!(matches!(second.as_slice(), [HelperEvent::Complete]), "got {second:?}");
    }

    #[tokio::test]
    async fn ttl_zero_expires_valid_to_idle() {
        let h = helper("printf 'TOK\\n'", Duration::ZERO);
        drain(Arc::clone(&h).run_and_stream().await).await;

        // expires_at == issue instant, so any later observation is expired.
        assert_eq!(h.credential_status().await, CredentialStatus::Idle);
        assert_eq!(h.get().await, None);
    }

    #[tokio::test]
    async fn expire_if_needed_is_noop_while_valid() {
        let h = helper("printf 'TOK\\n'", LONG_TTL);
        drain(Arc::clone(&h).run_and_stream().await).await;

        h.expire_if_needed().await;
        assert_eq!(h.credential_status().await, CredentialStatus::Valid);
    }

    #[tokio::test]
    async fn invalidate_clears_valid_then_reports_already_idle() {
        let h = helper("printf 'TOK\\n'", LONG_TTL);
        drain(Arc::clone(&h).run_and_stream().await).await;

        assert!(h.invalidate().await, "first invalidate should clear Valid");
        assert_eq!(h.credential_status().await, CredentialStatus::Idle);
        assert!(!h.invalidate().await, "second invalidate is a no-op");
    }

    #[tokio::test]
    async fn wait_for_settlement_returns_immediately_when_not_running() {
        let h = helper("printf 'TOK\\n'", LONG_TTL);
        // Idle: must not block.
        timeout(TICK, h.wait_for_settlement())
            .await
            .expect("wait_for_settlement must return promptly when idle");
    }

    #[tokio::test]
    async fn wait_for_settlement_unblocks_after_run_completes() {
        let h = helper("sleep 0.2; printf 'TOK\\n'", LONG_TTL);
        // Kick off the run (fire-and-forget stream); the task keeps running.
        let _stream = Arc::clone(&h).run_and_stream().await;

        timeout(TICK, h.wait_for_settlement())
            .await
            .expect("settlement should fire within the timeout");
        assert_eq!(h.credential_status().await, CredentialStatus::Valid);
    }

    #[tokio::test]
    async fn get_auto_triggers_when_idle_and_returns_none_while_running() {
        let h = helper("sleep 0.2; printf 'TOK\\n'", LONG_TTL);

        // Idle get(): fire-and-forget spawn, immediate None.
        assert_eq!(h.get().await, None);
        assert!(h.is_recovering().await, "get() on idle must start a run");
        // While running, get() still returns None and does not double-spawn.
        assert_eq!(h.get().await, None);

        timeout(TICK, h.wait_for_settlement()).await.unwrap();
        assert_eq!(h.get().await.as_deref(), Some("TOK"));
    }

    #[tokio::test]
    async fn second_subscriber_joins_running_helper_and_replays_buffer() {
        // The helper emits an instruction, then sleeps long enough for a
        // second subscriber to attach before the token is produced.
        let h = helper("printf 'instr-1\\n' 1>&2; sleep 1; printf 'TOK\\n'", LONG_TTL);

        let mut s1 = Arc::clone(&h).run_and_stream().await;
        // Once subscriber 1 has observed the instruction line, it is provably
        // in the shared replay buffer (the buffer push and the send share the
        // same lock), so a late subscriber must replay it.
        let first = timeout(TICK, s1.next()).await.unwrap();
        assert!(matches!(first, Some(HelperEvent::Line(ref l)) if l == "instr-1"));
        assert_eq!(h.credential_status().await, CredentialStatus::Running);

        let mut s2 = Arc::clone(&h).run_and_stream().await;
        let replayed = timeout(TICK, s2.next()).await.unwrap();
        assert!(
            matches!(replayed, Some(HelperEvent::Line(ref l)) if l == "instr-1"),
            "late subscriber should replay buffered line, got {replayed:?}"
        );

        // Both subscribers converge on Complete.
        timeout(TICK, h.wait_for_settlement()).await.unwrap();
        assert_eq!(h.get().await.as_deref(), Some("TOK"));
    }
}

#[async_trait::async_trait]
impl CredentialSource for CredentialHelper {
    /// Returns the cached credential if valid, or `None` immediately.
    /// Auto-triggers the helper subprocess if idle/failed (fire-and-forget).
    /// Never blocks -- the state machine handles waiting via `AwaitingRecovery`.
    async fn get(&self) -> Option<String> {
        {
            let mut inner = self.inner.lock().await;
            match &*inner {
                HelperInner::Valid {
                    credential,
                    expires_at,
                } => {
                    if Instant::now() < *expires_at {
                        return Some(credential.clone());
                    }
                    *inner = HelperInner::Idle;
                    // Fall through to auto-trigger.
                }
                HelperInner::Running { .. } => {
                    // Already running -- return None, caller checks is_recovering().
                    return None;
                }
                HelperInner::Idle | HelperInner::Failed { .. } => {}
            }
        }

        // Auto-trigger: spawn the helper (fire-and-forget, no waiting).
        if let Some(weak) = self.self_ref.get() {
            if let Some(arc_self) = weak.upgrade() {
                let mut inner = self.inner.lock().await;
                if matches!(&*inner, HelperInner::Idle | HelperInner::Failed { .. }) {
                    *inner = HelperInner::Running {
                        lines_so_far: vec![],
                        subscribers: vec![],
                    };
                    drop(inner);
                    Self::spawn_helper_task(arc_self);
                }
            }
        }

        None
    }

    /// Returns true when the helper subprocess is actively running.
    async fn is_recovering(&self) -> bool {
        matches!(&*self.inner.lock().await, HelperInner::Running { .. })
    }

    async fn invalidate(&self) -> bool {
        let mut inner = self.inner.lock().await;
        if matches!(&*inner, HelperInner::Valid { .. }) {
            *inner = HelperInner::Idle;
            true
        } else {
            false
        }
    }
}
