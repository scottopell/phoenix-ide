//! Disposable one-sample policy runner and ACK-only recovery. Not the safe controller CLI.

#[cfg(unix)]
use phoenix_deployment_controller::{
    guardian_ipc::{WireNativeBinding, WireSnapshot, WireState},
    native_guardian::{
        recover_native_sampling_ack, verify_native_binding, GuardianClient, VerifiedBindingStatus,
    },
    AckDisposition, AttemptId, BirthToken, CommittedTransition, ControllerSqliteStore,
    CpuContentionBasisPoints, DeploymentAttempt, GuardianLifetimeNonce, LoopProgress,
    MemoryPressure, MonotonicTick, NativeGuardianSlot, NativeSamplingStatus, PersistenceRevision,
    Phase, PolicyLoop, ProcessId, ProcessIdentity, RecoveryDisposition, ResourcePolicy,
};
#[cfg(unix)]
use std::{
    ffi::OsString,
    io::{Read, Write},
    num::{NonZeroU32, NonZeroUsize},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
struct RunnerConfig {
    socket_dir: PathBuf,
    store: PathBuf,
    attempt: AttemptId,
    mode: RunnerMode,
}

#[cfg(unix)]
enum RunnerMode {
    ObserveOnly {
        observe_seconds: u64,
    },
    InitialSample {
        deadline: MonotonicTick,
        root: ProcessIdentity,
        pause_after_apply: bool,
    },
    RecoverAppliedAck,
}

#[cfg(unix)]
fn parse_args(args: &[OsString]) -> Result<RunnerConfig, String> {
    let usage = "usage: --mode <observe-only|initial-sample|recover-applied-ack> --socket-dir <private fixture> --store <existing schema-v4 store> --attempt <u128> [--observe-seconds 1..60 | --deadline-nanoseconds <u64> --root-pid <u32> --root-birth-seconds <u64> --root-birth-nanos <u32> [--fixture-pause-after-apply]]";
    if args.len() < 8
        || args[0] != "--mode"
        || args[2] != "--socket-dir"
        || args[4] != "--store"
        || args[6] != "--attempt"
    {
        return Err(usage.into());
    }
    let number = |index: usize| -> Result<u64, String> {
        args.get(index)
            .and_then(|arg| arg.to_str())
            .ok_or(usage.to_owned())?
            .parse()
            .map_err(|_| usage.to_owned())
    };
    let mode = match args[1].to_str().ok_or(usage)? {
        "observe-only" if args.len() == 10 && args[8] == "--observe-seconds" => {
            let seconds = number(9)?;
            if !(1..=60).contains(&seconds) {
                return Err(usage.into());
            }
            RunnerMode::ObserveOnly {
                observe_seconds: seconds,
            }
        }
        "recover-applied-ack" if args.len() == 8 => RunnerMode::RecoverAppliedAck,
        "initial-sample"
            if (args.len() == 16 || args.len() == 17)
                && args[8] == "--deadline-nanoseconds"
                && args[10] == "--root-pid"
                && args[12] == "--root-birth-seconds"
                && args[14] == "--root-birth-nanos"
                && (args.len() == 16 || args[16] == "--fixture-pause-after-apply") =>
        {
            let pid = u32::try_from(number(11)?)
                .ok()
                .and_then(NonZeroU32::new)
                .ok_or(usage)?;
            let nanos = u32::try_from(number(15)?).map_err(|_| usage)?;
            RunnerMode::InitialSample {
                deadline: MonotonicTick::new(number(9)?),
                root: ProcessIdentity::new(
                    ProcessId::new(pid),
                    BirthToken::new(number(13)?, nanos).map_err(|_| usage)?,
                ),
                pause_after_apply: args.len() == 17,
            }
        }
        _ => return Err(usage.into()),
    };
    Ok(RunnerConfig {
        socket_dir: PathBuf::from(&args[3]),
        store: PathBuf::from(&args[5]),
        attempt: AttemptId::new(args[7].to_str().ok_or(usage)?.parse().map_err(|_| usage)?),
        mode,
    })
}

#[cfg(unix)]
fn same_native_identity(a: Option<&WireNativeBinding>, b: Option<&WireNativeBinding>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            a.attempt == b.attempt
                && a.deadline == b.deadline
                && a.nonce == b.nonce
                && a.guardian == b.guardian
                && a.slots.len() == b.slots.len()
                && a.slots
                    .iter()
                    .zip(&b.slots)
                    .all(|(a, b)| a.identity == b.identity)
        }
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

#[cfg(unix)]
fn connect_consistent(socket_dir: &Path) -> Result<(GuardianClient, WireSnapshot), String> {
    let mut client = GuardianClient::connect(socket_dir).map_err(|e| format!("connect: {e:?}"))?;
    let hello = client.hello().map_err(|e| format!("hello: {e:?}"))?;
    let inventory = client
        .inventory()
        .map_err(|e| format!("inventory: {e:?}"))?;
    let snapshot = client.snapshot().map_err(|e| format!("snapshot: {e:?}"))?;
    if hello.instance != snapshot.instance
        || hello.attempt != snapshot.attempt
        || hello.deadline != snapshot.deadline
        || hello.slots != inventory
        || snapshot.slots != inventory
        || !same_native_identity(
            hello.native_binding.as_ref(),
            snapshot.native_binding.as_ref(),
        )
    {
        return Err("guardian inventory changed during hello".into());
    }
    Ok((client, snapshot))
}

#[cfg(unix)]
fn catch_up_pending_begin(
    client: &mut GuardianClient,
    store: &mut ControllerSqliteStore,
    attempt: AttemptId,
    snapshot: &WireSnapshot,
    observed_slots: &[NativeGuardianSlot],
) -> Result<(), String> {
    let pending = store
        .load_native_binding(attempt)
        .map_err(|e| e.to_string())?
        .ok_or("missing pending binding")?;
    let caught_up = client
        .begin_reconciliation(snapshot, pending.current_session().interruption_revision)
        .map_err(|e| format!("catch up: {e:?}"))?;
    if caught_up.generation != pending.current_session().generation.get() {
        return Err("pending session generation mismatch".into());
    }
    client
        .fence(caught_up)
        .map_err(|e| format!("fence pending: {e:?}"))?;
    store
        .commit_detached_binding(&pending, observed_slots)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(unix)]
fn commit_interruption(
    store: &mut ControllerSqliteStore,
    attempt: AttemptId,
) -> Result<PersistenceRevision, String> {
    let loaded = store
        .load_attempt(attempt)
        .map_err(|e| e.to_string())?
        .ok_or("missing attempt")?;
    let transition = match loaded.phase() {
        Phase::Interrupted => loaded
            .refresh_interruption()
            .map_err(|e| format!("refresh: {e:?}"))?,
        Phase::ResourceSampling
        | Phase::OwnershipDiscovery
        | Phase::AwaitingBirthEvidence
        | Phase::CleaningOwnedTree
        | Phase::VerifyingCleanup
        | Phase::ReadyForDisruption
        | Phase::ResourceBlocked
        | Phase::Recovering
        | Phase::ForwardRecovery
        | Phase::AdministrativeCloseoutRequired
        | Phase::AdministrativelyClosed => match loaded
            .prepare_recovery()
            .map_err(|e| format!("recovery: {e:?}"))?
        {
            RecoveryDisposition::Checkpoint(transition) => *transition,
            RecoveryDisposition::Stable(_) => return Err("attempt cannot be interrupted".into()),
        },
    };
    let committed = transition.persist(store).map_err(|e| e.to_string())?;
    if committed.checkpoint().phase() != Phase::Interrupted || !committed.after_persist().is_empty()
    {
        return Err("interruption must be effect-free".into());
    }
    committed
        .checkpoint()
        .persistence_revision()
        .ok_or_else(|| "missing interruption revision".into())
}

#[cfg(unix)]
fn begin_reconciliation(
    client: &mut GuardianClient,
    store: &mut ControllerSqliteStore,
    attempt: AttemptId,
    snapshot: &WireSnapshot,
) -> Result<(), String> {
    let interruption = commit_interruption(store, attempt)?;
    let binding = store
        .load_native_binding(attempt)
        .map_err(|e| e.to_string())?
        .ok_or("missing binding")?;
    let next = store
        .commit_fresh_interruption_binding(&binding, interruption)
        .map_err(|e| e.to_string())?;
    let session = client
        .begin_reconciliation(snapshot, interruption)
        .map_err(|e| format!("begin: {e:?}"))?;
    if session.generation != next.current_session().generation.get()
        || session.instance != snapshot.instance
    {
        return Err("guardian generation differs from durable session".into());
    }
    println!(
        "reconciling generation={} interruption={}",
        session.generation,
        interruption.get()
    );
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(unix)]
fn observe_until(
    client: &mut GuardianClient,
    original: &WireSnapshot,
    observe_seconds: u64,
) -> Result<(), String> {
    let until = Instant::now() + Duration::from_secs(observe_seconds);
    while Instant::now() < until {
        let current = client.snapshot().map_err(|e| format!("snapshot: {e:?}"))?;
        if current.instance != original.instance
            || current.attempt != original.attempt
            || current.deadline != original.deadline
            || current.slots != original.slots
            || !same_native_identity(
                current.native_binding.as_ref(),
                original.native_binding.as_ref(),
            )
        {
            return Err("guardian or slot identity changed".into());
        }
        for slot in &current.slots {
            let observation = client
                .observe_slot(*slot)
                .map_err(|e| format!("observe: {e:?}"))?;
            println!("slot={} observation={observation:?}", slot.ordinal);
        }
        // test-timing-allow: bounded liveness polling of the disposable fixture.
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn validate_initial_snapshot(
    attempt: AttemptId,
    deadline: MonotonicTick,
    snapshot: &WireSnapshot,
) -> Result<(), String> {
    let reject = || "initial guardian identity or deadline mismatch".to_owned();
    if snapshot.attempt != attempt.get()
        || snapshot.deadline != deadline.get()
        || snapshot.state != WireState::Active
        || snapshot.session.instance != snapshot.instance
        || snapshot.session.generation != 1
    {
        return Err(reject());
    }
    let native = snapshot.native_binding.as_ref().ok_or_else(reject)?;
    if native.attempt != snapshot.attempt
        || native.deadline != snapshot.deadline
        || native.slots.len() != snapshot.slots.len()
        || snapshot.slots.iter().enumerate().any(|(index, slot)| {
            slot.instance != snapshot.instance || slot.ordinal != index as u64 + 1
        })
        || GuardianLifetimeNonce::new(native.nonce).is_none()
        || ProcessIdentity::try_from(native.guardian).is_err()
        || native
            .slots
            .iter()
            .any(|slot| ProcessIdentity::try_from(slot.identity).is_err())
        || MonotonicTick::native_now().map_err(|error| format!("{error:?}"))? >= deadline
    {
        return Err(reject());
    }
    Ok(())
}

fn prepare_initial_attempt(
    config: &RunnerConfig,
    snapshot: &WireSnapshot,
    store: &mut ControllerSqliteStore,
) -> Result<Option<CommittedTransition>, String> {
    let RunnerMode::InitialSample { deadline, root, .. } = &config.mode else {
        return Ok(None);
    };
    validate_initial_snapshot(config.attempt, *deadline, snapshot)?;
    let policy = ResourcePolicy::new(
        NonZeroUsize::new(2).expect("fixture constant"),
        0,
        0,
        CpuContentionBasisPoints::new(0).expect("fixture constant"),
        MemoryPressure::Critical,
    );
    DeploymentAttempt::start(config.attempt, *deadline, policy, *root)
        .persist(store)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn run_recover_applied_ack(
    store: &mut ControllerSqliteStore,
    client: &mut GuardianClient,
    key: phoenix_deployment_controller::DispatchKey,
) -> Result<(), String> {
    let confirmed = recover_native_sampling_ack(store, client, key)
        .map_err(|error| format!("ACK recovery: {error:?}"))?;
    let NativeSamplingStatus::SampleAckConfirmed {
        confirming_session_generation,
        ..
    } = confirmed
    else {
        return Err("recovery did not confirm ACK".into());
    };
    println!("sample_ack_confirmed generation={confirming_session_generation}");
    std::io::stdout().flush().map_err(|error| error.to_string())
}

fn run_initial_sample(
    config: &RunnerConfig,
    store: &mut ControllerSqliteStore,
    client: &mut GuardianClient,
    committed: CommittedTransition,
    pause_after_apply: bool,
) -> Result<(), String> {
    let binding = store
        .load_native_binding(config.attempt)
        .map_err(|error| error.to_string())?
        .ok_or("missing initial binding")?;
    let mut policy = PolicyLoop::from_committed(committed);
    let ticket = policy.issue_next().ok_or("missing first sample ticket")?;
    let issue = store
        .commit_native_sampling_issue(&ticket, &binding)
        .map_err(|error| error.to_string())?;
    let dispatched = client
        .dispatch_native_sampling(ticket, issue)
        .map_err(|error| format!("dispatch: {error:?}"))?;
    let verified = client
        .observe_native_sampling(dispatched)
        .map_err(|error| format!("observe sample: {error:?}"))?;
    let progress = policy
        .apply_native_sampling(verified, store)
        .map_err(|error| format!("apply sample: {error:?}"))?;
    if !matches!(progress, LoopProgress::Applied { key, ack: AckDisposition::AcknowledgeNow, .. } if key == issue.key())
    {
        return Err(format!(
            "sample did not produce immediate ACK: {progress:?}"
        ));
    }
    let mut independent =
        ControllerSqliteStore::open_read_only(&config.store).map_err(|error| error.to_string())?;
    let Some((key, NativeSamplingStatus::AppliedPendingAck { .. })) = independent
        .pending_native_sampling_ack(config.attempt)
        .map_err(|error| error.to_string())?
    else {
        return Err("application not durable pending ACK".into());
    };
    if key != issue.key() {
        return Err("wrong pending ACK".into());
    }
    if pause_after_apply {
        println!(
            "application_committed_pending_ack revision={}",
            key.revision().get()
        );
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        let mut byte = [0];
        std::io::stdin()
            .read_exact(&mut byte)
            .map_err(|error| format!("fixture gate EOF: {error}"))?;
        if byte != [b'R'] {
            return Err("invalid fixture gate release".into());
        }
    }
    let confirmed = recover_native_sampling_ack(store, client, key)
        .map_err(|error| format!("ACK: {error:?}"))?;
    if !matches!(confirmed, NativeSamplingStatus::SampleAckConfirmed { .. }) {
        return Err("ACK was not confirmed".into());
    }
    println!("sample_ack_confirmed generation=1");
    std::io::stdout().flush().map_err(|error| error.to_string())
}

#[cfg(unix)]
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let config = parse_args(&args)?;
    let mut store =
        ControllerSqliteStore::open_existing(&config.store).map_err(|e| e.to_string())?;
    let pending_recovery_key = if matches!(config.mode, RunnerMode::RecoverAppliedAck) {
        Some(
            store
                .pending_native_sampling_ack(config.attempt)
                .map_err(|e| e.to_string())?
                .ok_or("no unique successful pending ACK")?
                .0,
        )
    } else {
        None
    };
    if matches!(config.mode, RunnerMode::InitialSample { .. })
        && store
            .load_attempt(config.attempt)
            .map_err(|e| e.to_string())?
            .is_some()
    {
        return Err("initial sample requires an empty attempt".into());
    }
    let (mut client, snapshot) = connect_consistent(&config.socket_dir)?;
    let committed = prepare_initial_attempt(&config, &snapshot, &mut store)?;
    let status = verify_native_binding(&mut store, config.attempt, &snapshot)?;
    if matches!(config.mode, RunnerMode::InitialSample { .. })
        && status != VerifiedBindingStatus::InitialActive
    {
        return Err("initial sample requires active initial binding".into());
    }
    if matches!(config.mode, RunnerMode::RecoverAppliedAck)
        && status == VerifiedBindingStatus::InitialActive
    {
        return Err("ACK recovery requires detached guardian".into());
    }
    let initial_active = status == VerifiedBindingStatus::InitialActive;
    if initial_active {
        let binding = store
            .load_native_binding(config.attempt)
            .map_err(|e| e.to_string())?
            .ok_or("missing committed initial binding")?;
        client
            .release_initial(&snapshot, binding.dispatch_floor())
            .map_err(|e| format!("initial release: {e:?}"))?;
    }
    println!("verified binding: {status:?}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    if let VerifiedBindingStatus::PendingBegin { observed_slots } = &status {
        catch_up_pending_begin(
            &mut client,
            &mut store,
            config.attempt,
            &snapshot,
            observed_slots,
        )?;
    }
    if !initial_active {
        begin_reconciliation(&mut client, &mut store, config.attempt, &snapshot)?;
    }
    match config.mode {
        RunnerMode::ObserveOnly { observe_seconds } => {
            observe_until(&mut client, &snapshot, observe_seconds)
        }
        RunnerMode::RecoverAppliedAck => run_recover_applied_ack(
            &mut store,
            &mut client,
            pending_recovery_key.ok_or("missing preflight pending ACK")?,
        ),
        RunnerMode::InitialSample {
            pause_after_apply, ..
        } => run_initial_sample(
            &config,
            &mut store,
            &mut client,
            committed.ok_or("missing committed transition")?,
            pause_after_apply,
        ),
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("disposable runner requires Unix");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(mode: &str) -> Vec<OsString> {
        [
            "--mode",
            mode,
            "--socket-dir",
            "/tmp/sdc-disposable-test",
            "--store",
            "/tmp/controller.sqlite",
            "--attempt",
            "9",
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    #[test]
    fn runner_modes_parse_into_variant_specific_configs() {
        let mut observe = args("observe-only");
        observe.extend([OsString::from("--observe-seconds"), OsString::from("5")]);
        assert!(matches!(
            parse_args(&observe),
            Ok(RunnerConfig {
                mode: RunnerMode::ObserveOnly { observe_seconds: 5 },
                ..
            })
        ));
        assert!(matches!(
            parse_args(&args("recover-applied-ack")),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverAppliedAck,
                ..
            })
        ));

        let mut initial = args("initial-sample");
        for value in [
            "--deadline-nanoseconds",
            "100",
            "--root-pid",
            "42",
            "--root-birth-seconds",
            "1",
            "--root-birth-nanos",
            "0",
            "--fixture-pause-after-apply",
        ] {
            initial.push(OsString::from(value));
        }
        assert!(matches!(
            parse_args(&initial),
            Ok(RunnerConfig {
                mode: RunnerMode::InitialSample {
                    pause_after_apply: true,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn invalid_initial_snapshot_does_not_persist_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let store_path = directory.path().join("controller.sqlite");
        let mut store = ControllerSqliteStore::open(&store_path).unwrap();
        let attempt = AttemptId::new(9);
        let deadline = MonotonicTick::new(100);
        let config = RunnerConfig {
            socket_dir: directory.path().join("socket"),
            store: store_path,
            attempt,
            mode: RunnerMode::InitialSample {
                deadline,
                root: ProcessIdentity::new(
                    ProcessId::new(NonZeroU32::new(42).unwrap()),
                    BirthToken::new(1, 0).unwrap(),
                ),
                pause_after_apply: false,
            },
        };
        let snapshot = WireSnapshot {
            instance: 1,
            attempt: attempt.get(),
            deadline: deadline.get(),
            state: WireState::Active,
            session: phoenix_deployment_controller::guardian_ipc::WireSession {
                instance: 1,
                generation: 1,
            },
            slots: vec![],
            retained: vec![],
            native_binding: None,
        };
        assert!(prepare_initial_attempt(&config, &snapshot, &mut store).is_err());
        assert!(store.load_attempt(attempt).unwrap().is_none());
    }

    #[test]
    fn runner_modes_reject_cross_mode_arguments_and_missing_initial_authority() {
        let mut observe_with_pause = args("observe-only");
        observe_with_pause.push(OsString::from("--fixture-pause-after-apply"));
        assert!(parse_args(&observe_with_pause).is_err());
        assert!(parse_args(&args("initial-sample")).is_err());
    }
}
