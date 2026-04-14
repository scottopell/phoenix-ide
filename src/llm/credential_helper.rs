//! Interactive credential helper lifecycle management.
//!
//! `CredentialHelper` manages the full lifecycle of a shell-based credential helper:
//! idle → running → valid/failed, with SSE fan-out to multiple concurrent subscribers.

use crate::llm::registry::CredentialSource;
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
/// Duration to wait for a non-interactive helper to return before giving up.
/// If the helper needs OIDC interaction, it will hang past this timeout and
/// the UI credential panel takes over.
const AUTO_TRIGGER_TIMEOUT: Duration = Duration::from_secs(3);

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

#[async_trait::async_trait]
impl CredentialSource for CredentialHelper {
    async fn get(&self) -> Option<String> {
        // Fast path: return cached credential if still valid.
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
                    // Expired — fall through to auto-trigger.
                    *inner = HelperInner::Idle;
                }
                HelperInner::Running { .. } => {
                    // Already running (e.g. triggered by UI panel) — wait briefly for result.
                    drop(inner);
                    if tokio::time::timeout(AUTO_TRIGGER_TIMEOUT, self.settled.notified())
                        .await
                        .is_ok()
                    {
                        let inner = self.inner.lock().await;
                        if let HelperInner::Valid { credential, .. } = &*inner {
                            return Some(credential.clone());
                        }
                    }
                    return None;
                }
                HelperInner::Idle | HelperInner::Failed { .. } => {}
            }
        }

        // Auto-trigger: spawn the helper and wait up to AUTO_TRIGGER_TIMEOUT.
        // If the helper returns quickly (cached token), the LLM request succeeds
        // without UI interaction. If it hangs (OIDC), timeout fires and the UI
        // credential panel takes over.
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

                    if tokio::time::timeout(AUTO_TRIGGER_TIMEOUT, self.settled.notified())
                        .await
                        .is_ok()
                    {
                        let inner = self.inner.lock().await;
                        if let HelperInner::Valid { credential, .. } = &*inner {
                            return Some(credential.clone());
                        }
                    }
                }
            }
        }

        None
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
