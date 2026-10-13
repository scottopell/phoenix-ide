//! Explicit, isolated actual-server opening qualification. Never linked in shipping builds.
#![allow(clippy::too_many_lines)]

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use phoenix_workflow::ClientTurnKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::watch;
use tokio::time::{timeout, timeout_at, Instant};

use crate::db::{ContinuationOpeningRead, Database};

const ARM_BUDGET: Duration = Duration::from_secs(60);
const DRIVER_BUDGET: Duration = Duration::from_secs(600);
static GATE: OnceLock<Arc<Gate>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
struct Reservation {
    predecessor: String,
    key: ClientTurnKey,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct Evidence {
    product_conversation_id: String,
    predecessor_transcript_row_id: String,
    successor_transcript_row_id: String,
    continuation_message_id: String,
    client_turn_key: String,
    edge: &'static str,
}

#[derive(Clone, Debug)]
enum State {
    Unarmed,
    Armed {
        reservation: Reservation,
        deadline: Instant,
    },
    Entered {
        reservation: Reservation,
        deadline: Instant,
        evidence: Evidence,
    },
    Released(Reservation),
    Failed,
    Stopped,
}

struct Gate {
    state: watch::Sender<State>,
}

impl Gate {
    fn new() -> Self {
        Self {
            state: watch::channel(State::Unarmed).0,
        }
    }

    fn arm(&self, predecessor: String, key: String) -> Result<(), String> {
        if predecessor.is_empty() || predecessor.len() > 128 || key.len() > 256 {
            return Err("invalid bounded reservation".into());
        }
        let reservation = Reservation {
            predecessor,
            key: ClientTurnKey::try_from(key).map_err(|e| e.to_string())?,
        };
        let mut accepted = false;
        self.state.send_if_modified(|state| {
            if matches!(state, State::Unarmed) {
                *state = State::Armed {
                    reservation,
                    deadline: Instant::now() + ARM_BUDGET,
                };
                accepted = true;
                true
            } else {
                false
            }
        });
        if accepted {
            Ok(())
        } else {
            Err("one-shot gate already used".into())
        }
    }

    fn release(&self, predecessor: &str, key: &str) -> Result<(), String> {
        let mut accepted = false;
        self.state.send_if_modified(|state| {
            if let State::Entered {
                reservation,
                deadline,
                ..
            } = state
            {
                if reservation.predecessor == predecessor
                    && reservation.key.as_str() == key
                    && Instant::now() < *deadline
                {
                    *state = State::Released(reservation.clone());
                    accepted = true;
                    return true;
                }
            }
            false
        });
        if accepted {
            Ok(())
        } else {
            Err("release requires exact entered reservation before deadline".into())
        }
    }

    fn stop(&self) {
        self.state.send_if_modified(|state| {
            if matches!(state, State::Failed) {
                false
            } else {
                *state = State::Stopped;
                true
            }
        });
    }

    fn fail(&self) {
        self.state.send_replace(State::Failed);
    }

    async fn before_message(
        &self,
        db: &Database,
        successor: &str,
        key: &str,
    ) -> Result<(), String> {
        let mut rx = self.state.subscribe();
        let (reservation, deadline) = match rx.borrow().clone() {
            State::Armed {
                reservation,
                deadline,
            }
            | State::Entered {
                reservation,
                deadline,
                ..
            } if reservation.key.as_str() == key => (reservation, deadline),
            State::Failed | State::Stopped => return Err("qualifier stopped".into()),
            State::Unarmed | State::Armed { .. } | State::Entered { .. } | State::Released(_) => {
                return Ok(())
            }
        };
        if Instant::now() >= deadline {
            self.fail();
            return Err("qualifier arm deadline expired".into());
        }
        let intent = timeout_at(
            deadline,
            db.continuation_dispatch_intent(&reservation.predecessor),
        )
        .await
        .map_err(|_| "intent read deadline")?
        .map_err(|e| e.to_string())?;
        if !intent.is_some_and(|intent| {
            intent.successor_conversation_id == successor && intent.message_id == reservation.key
        }) {
            return Ok(());
        }
        // The read transaction is closed by this API before any gate wait.
        let opening = timeout_at(deadline, db.read_continuation_opening(successor))
            .await
            .map_err(|_| "opening read deadline")?
            .map_err(|e| e.to_string())?;
        if matches!(&*self.state.borrow(), State::Released(released) if *released == reservation) {
            return Ok(());
        }
        let ContinuationOpeningRead::Pending {
            edge,
            client_turn_key,
            ..
        } = opening
        else {
            return Err("qualifier requires committed pending intent".into());
        };
        if edge.predecessor_transcript_row_id != reservation.predecessor
            || client_turn_key != reservation.key
        {
            return Ok(());
        }
        let evidence = Evidence {
            product_conversation_id: edge.product_conversation_id.as_str().to_owned(),
            predecessor_transcript_row_id: edge.predecessor_transcript_row_id,
            successor_transcript_row_id: edge.successor_transcript_row_id,
            continuation_message_id: edge.continuation_message_id,
            client_turn_key: client_turn_key.as_str().to_owned(),
            edge: "committed_intent_before_first_message_persistence",
        };
        self.state.send_if_modified(|state| {
            if let State::Armed {
                reservation: current,
                ..
            } = state
            {
                if *current == reservation && Instant::now() < deadline {
                    *state = State::Entered {
                        reservation: reservation.clone(),
                        deadline,
                        evidence,
                    };
                    return true;
                }
            }
            false
        });
        loop {
            match rx.borrow_and_update().clone() {
                State::Released(released) if released == reservation => return Ok(()),
                State::Failed | State::Stopped => {
                    return Err("qualifier failed/stopped before persistence".into())
                }
                State::Armed { .. } | State::Entered { .. } => {}
                State::Unarmed | State::Released(_) => {
                    return Err("qualifier reservation changed".into())
                }
            }
            if timeout_at(deadline, rx.changed()).await.is_err() {
                self.fail();
                return Err("qualifier arm deadline expired".into());
            }
        }
    }
}

pub(crate) async fn before_authoritative_message(
    db: &Database,
    turn: u64,
    message: &str,
) -> Result<(), String> {
    let Some(gate) = GATE.get() else {
        return Ok(());
    };
    if !matches!(&*gate.state.borrow(), State::Armed { reservation, .. } | State::Entered { reservation, .. } if reservation.key.as_str() == message)
    {
        return match &*gate.state.borrow() {
            State::Failed | State::Stopped => Err("qualifier stopped".into()),
            State::Unarmed | State::Armed { .. } | State::Entered { .. } | State::Released(_) => {
                Ok(())
            }
        };
    }
    let deadline = match &*gate.state.borrow() {
        State::Armed { deadline, .. } | State::Entered { deadline, .. } => *deadline,
        State::Released(_) => return Ok(()),
        State::Unarmed | State::Failed | State::Stopped => return Err("qualifier stopped".into()),
    };
    let read = async {
        let (successor, actual_key): (String, String) = sqlx::query_as(
            "SELECT conversation_id, client_turn_key FROM durable_turns WHERE turn_id = ?1",
        )
        .bind(i64::try_from(turn).map_err(|e| e.to_string())?)
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.to_string())?;
        if actual_key != message {
            return Err("qualifier turn/key mismatch".into());
        }
        gate.before_message(db, &successor, message).await
    };
    let result = timeout_at(deadline, read)
        .await
        .unwrap_or_else(|_| Err("qualifier arm deadline expired".into()));
    if result.is_err() {
        gate.fail();
    }
    result
}

pub(crate) async fn before_reserved_message(
    db: &Database,
    successor: &str,
    message: &str,
) -> Result<(), String> {
    let Some(gate) = GATE.get() else {
        return Ok(());
    };
    let result = gate.before_message(db, successor, message).await;
    if result.is_err() {
        gate.fail();
    }
    result
}

pub(crate) async fn shutdown_requested() {
    let Some(gate) = GATE.get() else {
        std::future::pending::<()>().await;
        return;
    };
    let mut rx = gate.state.subscribe();
    loop {
        if matches!(&*rx.borrow_and_update(), State::Failed | State::Stopped) {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Arm {
        predecessor: String,
        client_turn_key: String,
    },
    Entered,
    Release {
        predecessor: String,
        client_turn_key: String,
    },
    Stop,
}

fn control(gate: &Gate, command: Command) -> Result<Value, String> {
    match command {
        Command::Arm {
            predecessor,
            client_turn_key,
        } => {
            gate.arm(predecessor, client_turn_key)?;
            Ok(json!({"state":"armed", "deadline_ms":60_000}))
        }
        Command::Entered => match &*gate.state.borrow() {
            State::Entered { evidence, .. } => Ok(json!({"state":"entered", "evidence":evidence})),
            State::Unarmed => Ok(json!({"state":"unarmed"})),
            State::Armed { .. } => Ok(json!({"state":"armed"})),
            State::Released(_) => Ok(json!({"state":"released"})),
            State::Failed | State::Stopped => Err("qualifier stopped".into()),
        },
        Command::Release {
            predecessor,
            client_turn_key,
        } => {
            gate.release(&predecessor, &client_turn_key)?;
            Ok(json!({"state":"released"}))
        }
        Command::Stop => {
            gate.stop();
            Ok(json!({"state":"stopped"}))
        }
    }
}

struct Cleanup {
    gate: Arc<Gate>,
    socket: std::path::PathBuf,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        self.gate.stop();
        let _ = std::fs::remove_file(&self.socket);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "isolated actual-server qualifier; run exact test alone with owned IPC socket"]
async fn actual_server_opening_qualifier() {
    use std::os::unix::fs::PermissionsExt;
    let args: Vec<_> = std::env::args().collect();
    for required in [
        "--exact",
        "--ignored",
        "--test-threads=1",
        "opening_qualifier::actual_server_opening_qualifier",
    ] {
        assert!(
            args.iter().any(|arg| arg == required),
            "driver requires exact standalone libtest invocation: {required}"
        );
    }
    let socket = std::path::PathBuf::from(
        std::env::var_os("PHOENIX_TEST_OPENING_CONTROL").expect("owned control socket required"),
    );
    assert!(socket.is_absolute());
    let parent = socket.parent().unwrap().canonicalize().unwrap();
    assert!(
        parent.starts_with(std::env::temp_dir().canonicalize().unwrap()),
        "socket must be under isolated temporary root"
    );
    assert_eq!(
        std::fs::metadata(&parent).unwrap().permissions().mode() & 0o077,
        0,
        "IPC root must be owner-only"
    );
    let port: u16 = std::env::var("PHOENIX_PORT")
        .expect("reserved loopback port required")
        .parse()
        .unwrap();
    assert!(port != 0);
    let home = tempfile::Builder::new()
        .prefix("opening-server-")
        .tempdir_in(&parent)
        .unwrap();
    // This ignored driver is executed as the only test in its own process.
    for (key, _) in std::env::vars_os() {
        std::env::remove_var(key);
    }
    std::env::set_var("HOME", home.path());
    std::env::set_var("PHOENIX_TMP_DIR", home.path().join("tmp"));
    std::env::set_var("TMPDIR", home.path());
    std::fs::write(home.path().join("config.toml"), "").unwrap();
    std::env::set_var("CONFIG_PATH", home.path().join("config.toml"));
    std::env::set_var("PHOENIX_BIND_ADDR", "127.0.0.1");
    std::env::set_var("PHOENIX_PORT", port.to_string());
    std::env::set_var("PHOENIX_TLS", "off");
    std::env::set_var("PHOENIX_ENABLE_MOCK_MODEL", "1");
    std::env::set_var("PHOENIX_LOG_STDOUT", "1");
    let gate = Arc::new(Gate::new());
    assert!(GATE.set(gate.clone()).is_ok());
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let _cleanup = Cleanup {
        gate: gate.clone(),
        socket,
    };
    eprintln!(
        "OPENING_QUALIFIER {}",
        json!({"git_sha":env!("PHOENIX_GIT_SHA"), "exe":std::env::current_exe().unwrap(), "home":home.path(), "db":home.path().join(".phoenix-ide/phoenix.db"), "bind":"127.0.0.1", "port":port, "instrumented_test_binary":true})
    );
    let server = crate::run_server();
    tokio::pin!(server);
    let deadline = Instant::now() + DRIVER_BUDGET;
    let mut ticks = tokio::time::interval(Duration::from_millis(25));
    let controller = async {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => return Err::<(), String>(error.to_string()),
            };
            let response = timeout(Duration::from_secs(2), async {
                let mut bytes = Vec::new();
                loop {
                    let byte = stream.read_u8().await.map_err(|e| e.to_string())?;
                    if byte == b'\n' {
                        break;
                    }
                    if bytes.len() >= 1024 {
                        return Err("command exceeds 1024 bytes".into());
                    }
                    bytes.push(byte);
                }
                let command = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                control(&gate, command)
            })
            .await
            .unwrap_or_else(|_| Err("IPC deadline".into()));
            let response = match response {
                Ok(value) => json!({"ok":true, "result":value}),
                Err(error) => json!({"ok":false, "error":error}),
            };
            let _ = timeout(
                Duration::from_secs(2),
                stream.write_all(format!("{response}\n").as_bytes()),
            )
            .await;
        }
    };
    tokio::pin!(controller);
    let outcome = loop {
        tokio::select! {
            result = &mut server => break result.map_err(|e| e.to_string()),
            result = &mut controller => {
                gate.fail();
                let _ = timeout(Duration::from_secs(60), &mut server).await;
                break result;
            },
            _ = tokio::time::sleep_until(deadline) => {
                gate.fail();
                let _ = timeout(Duration::from_secs(60), &mut server).await;
                break Err("driver deadline".to_owned());
            },
            _ = ticks.tick() => {
                let expired = matches!(&*gate.state.borrow(), State::Armed { deadline, .. } | State::Entered { deadline, .. } if Instant::now() >= *deadline);
                if expired { gate.fail(); }
            },
        }
    };
    let failed = matches!(&*gate.state.borrow(), State::Failed);
    gate.stop();
    assert!(!failed, "qualifier gate deadline/fail-stop");
    assert!(
        outcome.is_ok(),
        "actual server qualifier failed: {outcome:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ContinueOutcome, MessageContent, NewContinuationDispatchIntent};
    use phoenix_core::domain::db_schema::ContinuationContent;

    async fn pending() -> (Arc<Database>, String, String) {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let root = db
            .create_conversation("qualifier", "qualifier", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "boundary",
            &root.id,
            &MessageContent::Continuation(ContinuationContent {
                summary: "opening".into(),
            }),
            None,
            None,
        )
        .await
        .unwrap();
        db.update_conversation_state(
            &root.id,
            &crate::state_machine::ConvState::ContextExhausted {
                summary: "opening".into(),
            },
        )
        .await
        .unwrap();
        let (outcome, _) = db
            .continue_conversation_with_intent(
                &root.id,
                NewContinuationDispatchIntent::user_authorized(
                    ClientTurnKey::try_from("reserved:key").unwrap(),
                    "opening".into(),
                    None,
                ),
            )
            .await
            .unwrap();
        let ContinueOutcome::Created(successor) = outcome else {
            panic!("expected successor");
        };
        (db, root.id, successor.id)
    }

    async fn entered(gate: &Gate) {
        let mut rx = gate.state.subscribe();
        timeout(Duration::from_secs(2), async {
            loop {
                if matches!(&*rx.borrow_and_update(), State::Entered { .. }) {
                    break;
                }
                rx.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn wrong_identity_and_failed_release_do_not_unlock() {
        let (db, predecessor, successor) = pending().await;
        let unrelated = Gate::new();
        unrelated
            .arm("wrong-predecessor".into(), "reserved:key".into())
            .unwrap();
        unrelated
            .before_message(&db, &successor, "reserved:key")
            .await
            .unwrap();
        assert!(matches!(&*unrelated.state.borrow(), State::Armed { .. }));
        let gate = Arc::new(Gate::new());
        gate.arm(predecessor.clone(), "reserved:key".into())
            .unwrap();
        gate.before_message(&db, &successor, "wrong:key")
            .await
            .unwrap();
        assert!(matches!(&*gate.state.borrow(), State::Armed { .. }));
        assert!(gate.release(&predecessor, "reserved:key").is_err());
        let task = {
            let gate = gate.clone();
            let db = db.clone();
            tokio::spawn(async move { gate.before_message(&db, &successor, "reserved:key").await })
        };
        entered(&gate).await;
        assert!(gate.release("wrong-predecessor", "reserved:key").is_err());
        assert!(gate.release(&predecessor, "wrong:key").is_err());
        assert!(!task.is_finished());
        gate.stop();
        assert!(timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(gate.arm(predecessor, "reserved:key".into()).is_err());
    }

    #[tokio::test]
    async fn pending_reads_remain_available_and_all_waiters_release_before_acceptance() {
        let (db, predecessor, successor) = pending().await;
        let gate = Arc::new(Gate::new());
        gate.arm(predecessor.clone(), "reserved:key".into())
            .unwrap();
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let gate = gate.clone();
            let db = db.clone();
            let successor = successor.clone();
            tasks.push(tokio::spawn(async move {
                gate.before_message(&db, &successor, "reserved:key").await
            }));
        }
        entered(&gate).await;
        let opening = timeout(
            Duration::from_secs(2),
            db.read_continuation_opening(&successor),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(opening.is_pending());
        let State::Entered { evidence, .. } = gate.state.borrow().clone() else {
            panic!("expected entered");
        };
        assert_eq!(evidence.predecessor_transcript_row_id, predecessor);
        assert_eq!(evidence.successor_transcript_row_id, successor);
        assert_eq!(evidence.continuation_message_id, "boundary");
        assert_eq!(evidence.client_turn_key, "reserved:key");
        gate.release(&predecessor, "reserved:key").unwrap();
        for task in tasks {
            timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
        db.add_message(
            "reserved:key",
            &successor,
            &MessageContent::user("opening"),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(
            matches!(db.read_continuation_opening(&successor).await.unwrap(), ContinuationOpeningRead::Accepted { accepted_message_id, .. } if accepted_message_id == "reserved:key")
        );
    }

    #[tokio::test]
    async fn absolute_deadline_fails_closed_and_cleans_waiter() {
        let (db, predecessor, successor) = pending().await;
        let gate = Arc::new(Gate::new());
        gate.arm(predecessor.clone(), "reserved:key".into())
            .unwrap();
        let opening = db.read_continuation_opening(&successor).await.unwrap();
        assert!(opening.is_pending());
        gate.state.send_if_modified(|state| {
            if let State::Armed { deadline, .. } = state {
                *deadline = Instant::now();
                true
            } else {
                false
            }
        });
        assert!(gate
            .before_message(&db, &successor, "reserved:key")
            .await
            .is_err());
        gate.stop();
        assert!(matches!(&*gate.state.borrow(), State::Failed));
        assert!(gate.release(&predecessor, "reserved:key").is_err());
        assert!(db
            .read_continuation_opening(&successor)
            .await
            .unwrap()
            .is_pending());
    }

    #[test]
    fn cleanup_unlinks_only_owned_socket_and_stops_gate() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("control.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let gate = Arc::new(Gate::new());
        gate.arm("p".into(), "k".into()).unwrap();
        drop(Cleanup {
            gate: gate.clone(),
            socket: socket.clone(),
        });
        assert!(!socket.exists());
        assert!(matches!(&*gate.state.borrow(), State::Stopped));
        drop(listener);
    }

    #[test]
    fn control_is_bounded_and_has_no_database_commands() {
        assert!(serde_json::from_str::<Command>(r#"{"command":"write_db"}"#).is_err());
        assert!(serde_json::from_str::<Command>(
            r#"{"command":"arm","predecessor":"p","client_turn_key":"k","sql":"anything"}"#
        )
        .is_err());
        let gate = Gate::new();
        assert!(gate.arm("predecessor".into(), String::new()).is_err());
        assert!(gate.arm("predecessor".into(), "x".repeat(257)).is_err());
    }
}
