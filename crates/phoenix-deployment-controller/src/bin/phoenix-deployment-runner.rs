//! Disposable sampling runner with distinct ACK and reconciliation recovery. Not the safe controller CLI.

#[cfg(unix)]
use phoenix_deployment_controller::{
    guardian_ipc::{WireNativeBinding, WireSnapshot, WireState},
    native_guardian::{
        recover_native_child_list_ack, recover_native_sampling_ack, verify_native_binding,
        GuardianClient, VerifiedBindingStatus,
    },
    AckDisposition, AttemptId, BirthToken, CommittedTransition, ControllerSqliteStore,
    CpuContentionBasisPoints, DeploymentAttempt, GuardianLifetimeNonce, LoopProgress,
    MemoryPressure, MonotonicTick, NativeChildListStatus, NativeGuardianSlot, NativeSamplingStatus,
    OsEffect, PersistenceRevision, Phase, PolicyLoop, ProcessId, ProcessIdentity,
    RecoveryDisposition, ResourcePolicy,
};
#[cfg(unix)]
use std::{
    ffi::{CString, OsString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    num::{NonZeroU32, NonZeroUsize},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawFd, FromRawFd},
    },
    path::{Component, Path, PathBuf},
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
        fixture: InitialFixture,
    },
    RecoverAppliedAck(AckRecoveryFixture),
    RecoverUnappliedSample {
        fixture: UnappliedRecoveryFixture,
    },
    RecoverAndFinish {
        fixture: FinishFixture,
    },
    RecoverFinishAndSample {
        fixture: SampleFixture,
    },
    RecoverFinishAndChildList {
        fixture: ChildListFixture,
    },
    RecoverChildListAck(ChildListAckFixture),
}

#[cfg(unix)]
enum InitialFixture {
    Run,
    PauseAfterObservation(ReleaseSource),
    PauseAfterApplication(ReleaseSource),
}

#[cfg(unix)]
enum AckRecoveryFixture {
    Run,
    PauseAfterConfirmation(ReleaseSource),
}

#[cfg(unix)]
#[derive(Clone)]
enum ReleaseSource {
    Stdin,
    DisposableFile(PathBuf),
}

#[cfg(unix)]
enum UnappliedRecoveryFixture {
    Run,
    PauseAfterVerification,
    PauseAfterVerificationFile(ReleaseSource),
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum FinishFixture {
    Run,
    PauseBeforeBegin,
    ActivationBarriers,
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum SampleFixture {
    Run,
    ActivationAndSampleBarriers,
}

#[cfg(unix)]
#[derive(Clone)]
enum ChildListFixture {
    Run,
    PauseAfterApplication(ReleaseSource),
}

#[cfg(unix)]
#[derive(Clone)]
enum ChildListAckFixture {
    Run,
    PauseAfterConfirmation(ReleaseSource),
}

#[cfg(unix)]
#[derive(Clone)]
enum RecoveryRun {
    Finish(FinishFixture),
    FinishAndSample(SampleFixture),
    FinishAndChildList(ChildListFixture),
}

#[cfg(unix)]
impl RecoveryRun {
    fn activation_fixture(&self) -> FinishFixture {
        match self {
            Self::Finish(fixture) => *fixture,
            Self::FinishAndSample(SampleFixture::Run) | Self::FinishAndChildList(_) => {
                FinishFixture::Run
            }
            Self::FinishAndSample(SampleFixture::ActivationAndSampleBarriers) => {
                FinishFixture::ActivationBarriers
            }
        }
    }
}

#[cfg(unix)]
fn parse_number(args: &[OsString], index: usize, usage: &str) -> Result<u64, String> {
    args.get(index)
        .and_then(|arg| arg.to_str())
        .ok_or(usage.to_owned())?
        .parse()
        .map_err(|_| usage.to_owned())
}

#[cfg(unix)]
fn parse_initial_sample(args: &[OsString], usage: &str) -> Result<RunnerMode, String> {
    if !(16..=19).contains(&args.len())
        || args[8] != "--deadline-nanoseconds"
        || args[10] != "--root-pid"
        || args[12] != "--root-birth-seconds"
        || args[14] != "--root-birth-nanos"
        || (args.len() != 16
            && args[16] != "--fixture-pause-after-apply"
            && args[16] != "--fixture-pause-after-observation")
    {
        return Err(usage.into());
    }
    let pid = u32::try_from(parse_number(args, 11, usage)?)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or(usage)?;
    let nanos = u32::try_from(parse_number(args, 15, usage)?).map_err(|_| usage)?;
    let deadline = MonotonicTick::new(parse_number(args, 9, usage)?);
    let root = ProcessIdentity::new(
        ProcessId::new(pid),
        BirthToken::new(parse_number(args, 13, usage)?, nanos).map_err(|_| usage)?,
    );
    let release = match args.len() {
        16 | 17 => ReleaseSource::Stdin,
        19 if args[17] == "--fixture-release-file" => {
            let path = PathBuf::from(&args[18]);
            validate_release_path(&path)?;
            ReleaseSource::DisposableFile(path)
        }
        _ => return Err(usage.into()),
    };
    let fixture = match args.get(16).and_then(|flag| flag.to_str()) {
        None if args.len() == 16 => InitialFixture::Run,
        Some("--fixture-pause-after-observation") => InitialFixture::PauseAfterObservation(release),
        Some("--fixture-pause-after-apply") => InitialFixture::PauseAfterApplication(release),
        _ => return Err(usage.into()),
    };
    Ok(RunnerMode::InitialSample {
        deadline,
        root,
        fixture,
    })
}

#[cfg(unix)]
fn parse_recover_applied_ack(args: &[OsString], usage: &str) -> Result<RunnerMode, String> {
    let fixture = match args {
        [_, _, _, _, _, _, _, _] => AckRecoveryFixture::Run,
        [_, _, _, _, _, _, _, _, pause, release, path]
            if pause == "--fixture-pause-after-recovered-ack"
                && release == "--fixture-release-file" =>
        {
            let path = PathBuf::from(path);
            validate_release_path(&path)?;
            AckRecoveryFixture::PauseAfterConfirmation(ReleaseSource::DisposableFile(path))
        }
        _ => return Err(usage.into()),
    };
    Ok(RunnerMode::RecoverAppliedAck(fixture))
}

#[cfg(unix)]
fn parse_recover_child_list(args: &[OsString], usage: &str) -> Result<RunnerMode, String> {
    let fixture = match args {
        [_, _, _, _, _, _, _, _] => ChildListFixture::Run,
        [_, _, _, _, _, _, _, _, pause] if pause == "--fixture-pause-after-child-list-apply" => {
            ChildListFixture::PauseAfterApplication(ReleaseSource::Stdin)
        }
        [_, _, _, _, _, _, _, _, pause, release, path]
            if pause == "--fixture-pause-after-child-list-apply"
                && release == "--fixture-release-file" =>
        {
            let path = PathBuf::from(path);
            validate_release_path(&path)?;
            ChildListFixture::PauseAfterApplication(ReleaseSource::DisposableFile(path))
        }
        _ => return Err(usage.into()),
    };
    Ok(RunnerMode::RecoverFinishAndChildList { fixture })
}

#[cfg(unix)]
fn parse_recover_child_list_ack(args: &[OsString], usage: &str) -> Result<RunnerMode, String> {
    let fixture = match args {
        [_, _, _, _, _, _, _, _] => ChildListAckFixture::Run,
        [_, _, _, _, _, _, _, _, pause, release, path]
            if pause == "--fixture-pause-after-child-list-ack"
                && release == "--fixture-release-file" =>
        {
            let path = PathBuf::from(path);
            validate_release_path(&path)?;
            ChildListAckFixture::PauseAfterConfirmation(ReleaseSource::DisposableFile(path))
        }
        _ => return Err(usage.into()),
    };
    Ok(RunnerMode::RecoverChildListAck(fixture))
}

#[cfg(unix)]
fn parse_args(args: &[OsString]) -> Result<RunnerConfig, String> {
    let usage = "usage: --mode <observe-only|initial-sample|recover-applied-ack|recover-unapplied-sample|recover-and-finish|recover-finish-and-sample|recover-finish-and-child-list|recover-child-list-ack> --socket-dir <private fixture> --store <existing schema-v6 store> --attempt <u128> [--observe-seconds 1..60 | --deadline-nanoseconds <u64> --root-pid <u32> --root-birth-seconds <u64> --root-birth-nanos <u32> [--fixture-pause-after-observation|--fixture-pause-after-apply [--fixture-release-file /tmp/sdc-disposable-*/runner-release.signal]] | --fixture-pause-after-recovered-ack --fixture-release-file /tmp/sdc-disposable-*/runner-release.signal (recover-applied-ack only) | --fixture-pause-after-recovery-verification [--fixture-release-file /tmp/sdc-disposable-*/runner-release.signal] (recover-unapplied-sample only) | --fixture-pause-before-begin or --fixture-activation-barriers (recover-and-finish only) | --fixture-activation-and-sample-barriers (recover-finish-and-sample only) | --fixture-pause-after-child-list-apply [--fixture-release-file /tmp/sdc-disposable-*/runner-release.signal] (recover-finish-and-child-list only)]";
    if args.len() < 8
        || args[0] != "--mode"
        || args[2] != "--socket-dir"
        || args[4] != "--store"
        || args[6] != "--attempt"
    {
        return Err(usage.into());
    }
    let mode = match args[1].to_str().ok_or(usage)? {
        "observe-only" if args.len() == 10 && args[8] == "--observe-seconds" => {
            let seconds = parse_number(args, 9, usage)?;
            if !(1..=60).contains(&seconds) {
                return Err(usage.into());
            }
            RunnerMode::ObserveOnly {
                observe_seconds: seconds,
            }
        }
        "recover-applied-ack" => parse_recover_applied_ack(args, usage)?,
        "recover-unapplied-sample" if args.len() == 8 => RunnerMode::RecoverUnappliedSample {
            fixture: UnappliedRecoveryFixture::Run,
        },
        "recover-unapplied-sample"
            if args.len() == 9 && args[8] == "--fixture-pause-after-recovery-verification" =>
        {
            RunnerMode::RecoverUnappliedSample {
                fixture: UnappliedRecoveryFixture::PauseAfterVerification,
            }
        }
        "recover-unapplied-sample"
            if args.len() == 11
                && args[8] == "--fixture-pause-after-recovery-verification"
                && args[9] == "--fixture-release-file" =>
        {
            let path = PathBuf::from(&args[10]);
            validate_release_path(&path)?;
            RunnerMode::RecoverUnappliedSample {
                fixture: UnappliedRecoveryFixture::PauseAfterVerificationFile(
                    ReleaseSource::DisposableFile(path),
                ),
            }
        }
        "recover-and-finish" if args.len() == 8 => RunnerMode::RecoverAndFinish {
            fixture: FinishFixture::Run,
        },
        "recover-and-finish" if args.len() == 9 && args[8] == "--fixture-activation-barriers" => {
            RunnerMode::RecoverAndFinish {
                fixture: FinishFixture::ActivationBarriers,
            }
        }
        "recover-and-finish" if args.len() == 9 && args[8] == "--fixture-pause-before-begin" => {
            RunnerMode::RecoverAndFinish {
                fixture: FinishFixture::PauseBeforeBegin,
            }
        }
        "recover-finish-and-sample" if args.len() == 8 => RunnerMode::RecoverFinishAndSample {
            fixture: SampleFixture::Run,
        },
        "recover-finish-and-sample"
            if args.len() == 9 && args[8] == "--fixture-activation-and-sample-barriers" =>
        {
            RunnerMode::RecoverFinishAndSample {
                fixture: SampleFixture::ActivationAndSampleBarriers,
            }
        }
        "recover-finish-and-child-list" => parse_recover_child_list(args, usage)?,
        "recover-child-list-ack" => parse_recover_child_list_ack(args, usage)?,
        "initial-sample" => parse_initial_sample(args, usage)?,
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
    println!("pending_begin_fenced generation={}", caught_up.generation);
    std::io::stdout().flush().map_err(|e| e.to_string())?;
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
    fixture: FinishFixture,
) -> Result<phoenix_deployment_controller::guardian_ipc::WireSession, String> {
    let interruption = commit_interruption(store, attempt)?;
    let binding = store
        .load_native_binding(attempt)
        .map_err(|e| e.to_string())?
        .ok_or("missing binding")?;
    if MonotonicTick::native_now().map_err(|e| e.to_string())? >= binding.deadline() {
        return Err("recovery deadline expired before fresh binding".into());
    }
    let next = store
        .commit_fresh_interruption_binding(&binding, interruption)
        .map_err(|e| e.to_string())?;
    if matches!(fixture, FinishFixture::PauseBeforeBegin) {
        println!(
            "pending_begin generation={} interruption={}",
            next.current_session().generation,
            interruption.get()
        );
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        let until = Instant::now() + Duration::from_secs(30);
        while Instant::now() < until {
            // test-timing-allow: bounded killable pause before live begin in the disposable fixture.
            thread::sleep(Duration::from_millis(100));
        }
        return Err("fixture pending-begin pause expired".into());
    }
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
    Ok(session)
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
        CpuContentionBasisPoints::new(10_000).expect("fixture constant"),
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
    fixture: &AckRecoveryFixture,
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
    let gate = match fixture {
        AckRecoveryFixture::PauseAfterConfirmation(release) => Some(release.prepare()?),
        AckRecoveryFixture::Run => None,
    };
    println!("sample_ack_confirmed generation={confirming_session_generation}");
    std::io::stdout()
        .flush()
        .map_err(|error| error.to_string())?;
    if let Some(gate) = gate {
        gate.wait()?;
    }
    Ok(())
}

fn preflight_recover_and_finish(
    store: &mut ControllerSqliteStore,
    attempt: AttemptId,
) -> Result<(), String> {
    let deadline = store
        .load_attempt(attempt)
        .map_err(|e| e.to_string())?
        .ok_or("missing recovery attempt")?
        .deadline();
    if MonotonicTick::native_now().map_err(|e| e.to_string())? >= deadline {
        return Err("recovery deadline expired".into());
    }
    store
        .preflight_native_sampling_recovery(attempt)
        .map_err(|e| e.to_string())
}

// Disposable fixture synchronization; no policy effect or production entry point.
fn activation_barrier(fixture: FinishFixture) -> Result<(), String> {
    if !matches!(fixture, FinishFixture::ActivationBarriers) {
        return Ok(());
    }
    let mut descriptor = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    if unsafe { libc::poll(&raw mut descriptor, 1, 5000) } != 1 {
        return Err("activation fixture barrier timed out".into());
    }
    let mut byte = [0];
    std::io::Read::read_exact(&mut std::io::stdin(), &mut byte).map_err(|e| e.to_string())?;
    if byte != [b'!'] {
        return Err("invalid activation fixture acknowledgement".into());
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn run_recover_and_finish(
    store: &mut ControllerSqliteStore,
    client: &mut GuardianClient,
    attempt: AttemptId,
    original: &WireSnapshot,
    session: phoenix_deployment_controller::guardian_ipc::WireSession,
    run: RecoveryRun,
) -> Result<(), String> {
    let fixture = run.activation_fixture();
    let loaded = store
        .load_attempt(attempt)
        .map_err(|e| e.to_string())?
        .ok_or("missing interrupted attempt")?;
    let binding = store
        .load_native_binding(attempt)
        .map_err(|e| e.to_string())?
        .ok_or("missing reconciling binding")?;
    let interruption = binding.current_session().interruption_revision;
    let committed = store
        .commit_native_sampling_recovery_before_deadline(loaded, &binding)
        .map_err(|e| e.to_string())?;
    let revision = interruption
        .checked_next()
        .ok_or("recovery revision exhausted")?;
    if committed.checkpoint().attempt_id() != attempt
        || committed.checkpoint().persistence_revision() != Some(revision)
        || committed.checkpoint().deadline() != binding.deadline()
        || !committed.is_exact_sampling_plan()
    {
        return Err("unexpected recovery plan".into());
    }
    let mut policy = PolicyLoop::from_committed(committed);
    let retained_ticket = policy
        .issue_next()
        .ok_or("missing retained recovery ticket")?;
    let (_, prepared_binding, completion) = store
        .read_native_reconciliation_evidence(attempt, binding.current_session().generation)
        .map_err(|e| e.to_string())?
        .ok_or("missing prepared completion")?;
    if prepared_binding != binding || completion.recovery_revision() != revision {
        return Err("prepared completion differs from recovery".into());
    }
    let prepared = client
        .finish_reconciliation(session, revision)
        .map_err(|e| format!("finish: {e:?}"))?;
    let prepared_snapshot = client
        .snapshot()
        .map_err(|e| format!("prepared snapshot: {e:?}"))?;
    if prepared_snapshot.state != WireState::Reconciling || prepared_snapshot.session != session {
        return Err("prepare installed live authority".into());
    }
    println!(
        "prepared_snapshot={}",
        serde_json::to_string(&prepared_snapshot).map_err(|e| e.to_string())?
    );
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    activation_barrier(fixture)?;
    let (active_binding, activation, committed_activation) = store
        .commit_native_reconciliation_activation(&binding, completion, prepared)
        .map_err(|e| format!("activation commit: {e}"))?;
    if active_binding.current_session().state
        != phoenix_deployment_controller::NativeSessionState::Active
        || active_binding.dispatch_floor() != revision
        || activation.completion() != completion
    {
        return Err("durable activation mismatch".into());
    }
    let durable_snapshot = client
        .snapshot()
        .map_err(|e| format!("durable snapshot: {e:?}"))?;
    if durable_snapshot.state != WireState::Reconciling || durable_snapshot.session != session {
        return Err("live Active preceded durable activation".into());
    }
    println!(
        "durable_activation_snapshot={}",
        serde_json::to_string(&durable_snapshot).map_err(|e| e.to_string())?
    );
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    activation_barrier(fixture)?;
    client
        .activate_reconciliation(committed_activation)
        .map_err(|e| format!("activate: {e:?}"))?;
    let active = client
        .snapshot()
        .map_err(|e| format!("active snapshot: {e:?}"))?;
    if active.state != WireState::Active
        || active.session != session
        || active.instance != original.instance
        || active.attempt != original.attempt
        || active.deadline != original.deadline
        || active.slots != original.slots
        || !same_native_identity(
            active.native_binding.as_ref(),
            original.native_binding.as_ref(),
        )
        || !active.retained.is_empty()
    {
        return Err("reconciliation did not activate the same guardian".into());
    }
    println!(
        "reconciliation_active generation={} interruption={} recovery={}",
        session.generation,
        interruption.get(),
        revision.get()
    );
    println!(
        "active_snapshot={}",
        serde_json::to_string(&active).map_err(|e| e.to_string())?
    );
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let (sample_fixture, child_list_fixture) = match run {
        RecoveryRun::FinishAndSample(fixture) => (Some(fixture), None),
        RecoveryRun::FinishAndChildList(fixture) => (None, Some(fixture)),
        RecoveryRun::Finish(_) => return Ok(()),
    };
    if matches!(
        sample_fixture,
        Some(SampleFixture::ActivationAndSampleBarriers)
    ) {
        println!("post_activation_sample_pending");
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        activation_barrier(FinishFixture::ActivationBarriers)?;
    }
    let issue = store
        .commit_native_sampling_issue(&retained_ticket, &active_binding)
        .map_err(|e| format!("recovered sample issue: {e}"))?;
    let dispatched = dispatch_sampling(store, client, retained_ticket, issue)?;
    let verified = client
        .observe_native_sampling(dispatched)
        .map_err(|e| format!("recovered sample observe: {e:?}"))?;
    let progress = policy
        .apply_native_sampling(verified, store)
        .map_err(|e| format!("recovered sample apply: {e:?}"))?;
    if !matches!(progress, LoopProgress::Applied { key, ack: AckDisposition::AcknowledgeNow, .. } if key == issue.key())
    {
        return Err(format!(
            "recovered sample did not produce exact immediate ACK: {progress:?}"
        ));
    }
    let confirmed = recover_native_sampling_ack(store, client, issue.key())
        .map_err(|e| format!("recovered sample ACK: {e:?}"))?;
    if !matches!(confirmed, NativeSamplingStatus::SampleAckConfirmed { issue: stored, sample, confirming_session_generation, .. }
        if stored == issue && sample.sequence == issue.sequence() && confirming_session_generation.get() == session.generation)
    {
        return Err("recovered sample ACK confirmation mismatch".into());
    }
    println!(
        "recovered_sample_applied_ack attempt={} revision={} ordinal={} sequence={}",
        issue.key().attempt_id().get(),
        issue.key().revision().get(),
        issue.key().ordinal(),
        issue.sequence()
    );
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    match child_list_fixture {
        Some(fixture) => run_recovered_child_list(
            store,
            client,
            attempt,
            session,
            policy,
            &active_binding,
            fixture,
        ),
        None => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_recovered_child_list(
    store: &mut ControllerSqliteStore,
    client: &mut GuardianClient,
    attempt: AttemptId,
    session: phoenix_deployment_controller::guardian_ipc::WireSession,
    mut policy: PolicyLoop,
    binding: &phoenix_deployment_controller::NativeGuardianBinding,
    fixture: ChildListFixture,
) -> Result<(), String> {
    let child_ticket = policy
        .issue_next()
        .ok_or("missing ListChildren successor after recovered sample")?;
    let OsEffect::ListChildren { parent } = child_ticket.effect() else {
        return Err("recovered sample successor was not ListChildren".into());
    };
    let root = binding
        .slots()
        .first()
        .ok_or("missing harmless root slot")?
        .process;
    if *parent != root {
        return Err("ListChildren successor did not target harmless root".into());
    }
    let child_issue = store
        .commit_native_child_list_issue(&child_ticket, binding)
        .map_err(|error| format!("child-list issue: {error}"))?;
    let dispatched = dispatch_child_list(client, child_ticket, child_issue)?;
    let verified = client
        .observe_native_child_list(dispatched)
        .map_err(|error| format!("child-list observe: {error:?}"))?;
    let progress = policy
        .apply_native_child_list(verified, store)
        .map_err(|error| format!("child-list apply: {error:?}"))?;
    if !matches!(progress, LoopProgress::Applied { key, ack: AckDisposition::AcknowledgeNow, .. } if key == child_issue.key())
    {
        return Err(format!(
            "child-list did not produce exact immediate ACK: {progress:?}"
        ));
    }
    let pending = store
        .load_native_child_list_status(child_issue.key())
        .map_err(|error| error.to_string())?
        .ok_or("child-list application was not durable")?;
    if !matches!(pending, NativeChildListStatus::AppliedPendingAck { issue, children, .. } if issue == child_issue && children.is_empty())
    {
        return Err("child-list completion was not successful and empty".into());
    }
    if let ChildListFixture::PauseAfterApplication(release) = fixture {
        let gate = release.prepare()?;
        println!(
            "child_list_application_committed_pending_ack revision={}",
            child_issue.key().revision().get()
        );
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        gate.wait()?;
    }
    let confirmed = recover_native_child_list_ack(store, client, child_issue.key())
        .map_err(|error| format!("child-list ACK: {error:?}"))?;
    if !matches!(confirmed, NativeChildListStatus::ChildrenAckConfirmed { issue, children, confirming_session_generation, .. } if issue == child_issue && children.is_empty() && confirming_session_generation.get() == session.generation)
    {
        return Err("child-list ACK confirmation mismatch".into());
    }
    policy
        .finish_ownership_scan(store)
        .map_err(|error| format!("finish ownership scan: {error:?}"))?;
    let final_attempt = store
        .load_attempt(attempt)
        .map_err(|error| error.to_string())?
        .ok_or("missing final attempt")?;
    if final_attempt.phase() != Phase::CleaningOwnedTree {
        return Err(format!(
            "child-list recovery did not reach CleaningOwnedTree: {:?}",
            final_attempt.phase()
        ));
    }
    println!(
        "child_list_ack_confirmed attempt={} revision={} generation={} phase=CleaningOwnedTree; observe_identity_not_issued",
        child_issue.key().attempt_id().get(),
        child_issue.key().revision().get(),
        session.generation,
    );
    std::io::stdout().flush().map_err(|error| error.to_string())
}

#[cfg(unix)]
fn run_recover_child_list_ack(
    store: &mut ControllerSqliteStore,
    client: &mut GuardianClient,
    key: phoenix_deployment_controller::DispatchKey,
    fixture: &ChildListAckFixture,
) -> Result<(), String> {
    let confirmed = recover_native_child_list_ack(store, client, key)
        .map_err(|error| format!("child-list ACK recovery: {error:?}"))?;
    if !matches!(confirmed, NativeChildListStatus::ChildrenAckConfirmed { issue, children, .. } if issue.key() == key && children.is_empty())
    {
        return Err("recovered child-list ACK did not confirm exact empty result".into());
    }
    let attempt = store
        .load_attempt(key.attempt_id())
        .map_err(|error| error.to_string())?
        .ok_or("missing attempt after child-list ACK")?;
    println!(
        "child_list_ack_confirmed attempt={} revision={} phase={:?}; recovery_gap=PolicyLoop authority is intentionally not reconstructed, so ownership scan cannot safely finish",
        key.attempt_id().get(),
        key.revision().get(),
        attempt.phase(),
    );
    let gate = match fixture {
        ChildListAckFixture::PauseAfterConfirmation(release) => Some(release.prepare()?),
        ChildListAckFixture::Run => None,
    };
    std::io::stdout()
        .flush()
        .map_err(|error| error.to_string())?;
    if let Some(gate) = gate {
        gate.wait()?;
    }
    Ok(())
}

fn run_recover_unapplied_sample(
    store: &mut ControllerSqliteStore,
    client: &mut GuardianClient,
    attempt: AttemptId,
    session: phoenix_deployment_controller::guardian_ipc::WireSession,
    fixture: &UnappliedRecoveryFixture,
) -> Result<(), String> {
    let loaded = store
        .load_attempt(attempt)
        .map_err(|error| error.to_string())?
        .ok_or("missing interrupted attempt")?;
    let candidate = store
        .prepare_native_sampling_recovery(loaded)
        .map_err(|error| format!("prepare recovered sample: {error}"))?;
    let issue = candidate.issue();
    let verified = client
        .verify_recovered_native_sampling(candidate)
        .map_err(|error| format!("verify recovered sample: {error:?}"))?;
    if !matches!(fixture, UnappliedRecoveryFixture::Run) {
        let gate = match fixture {
            UnappliedRecoveryFixture::PauseAfterVerificationFile(release) => {
                Some(release.prepare()?)
            }
            UnappliedRecoveryFixture::Run | UnappliedRecoveryFixture::PauseAfterVerification => {
                None
            }
        };
        println!(
            "recovered_sample_verified_pending_application revision={} sequence={}",
            issue.key().revision().get(),
            issue.sequence()
        );
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        if let Some(gate) = gate {
            gate.wait()?;
        } else {
            activation_barrier(FinishFixture::ActivationBarriers)?;
        }
    }
    let applied = store
        .apply_recovered_native_sampling(verified)
        .map_err(|error| format!("apply recovered sample: {error}"))?;
    if !matches!(applied, NativeSamplingStatus::AppliedPendingAck { issue: stored, sample, .. }
        if stored == issue && sample.sequence == issue.sequence())
    {
        return Err("recovered sample did not produce exact pending ACK".into());
    }
    let confirmed = recover_native_sampling_ack(store, client, issue.key())
        .map_err(|error| format!("recovered sample ACK: {error:?}"))?;
    if !matches!(confirmed, NativeSamplingStatus::SampleAckConfirmed {
        issue: stored, sample, confirming_session_generation, ..
    } if stored == issue && sample.sequence == issue.sequence()
        && confirming_session_generation.get() == session.generation)
    {
        return Err("recovered sample ACK confirmation mismatch".into());
    }
    println!(
        "recovered_sample_applied_ack attempt={} revision={} ordinal={} sequence={}",
        issue.key().attempt_id().get(),
        issue.key().revision().get(),
        issue.key().ordinal(),
        issue.sequence()
    );
    std::io::stdout().flush().map_err(|error| error.to_string())
}

fn dispatch_sampling(
    store: &mut ControllerSqliteStore,
    client: &mut GuardianClient,
    ticket: phoenix_deployment_controller::DispatchTicket,
    issue: phoenix_deployment_controller::NativeSamplingIssue,
) -> Result<phoenix_deployment_controller::NativeSamplingDispatch, String> {
    use phoenix_deployment_controller::native_guardian::NativeSamplingDispatchError;
    match client.dispatch_native_sampling(ticket, issue) {
        Ok(dispatched) => Ok(dispatched),
        Err(NativeSamplingDispatchError::KnownUnsubmitted { error, unsubmitted }) => {
            store
                .cancel_unsubmitted_native_sampling(*unsubmitted)
                .map_err(|cancel| {
                    format!("dispatch known unsubmitted: {error:?}; cancellation: {cancel}")
                })?;
            Err(format!(
                "dispatch known unsubmitted (issue cancelled): {error:?}"
            ))
        }
        Err(NativeSamplingDispatchError::SubmissionUnknown(error)) => Err(format!(
            "dispatch submission unknown (issue retained): {error:?}"
        )),
    }
}

#[cfg(unix)]
fn dispatch_child_list(
    client: &mut GuardianClient,
    ticket: phoenix_deployment_controller::DispatchTicket,
    issue: phoenix_deployment_controller::NativeChildListIssue,
) -> Result<phoenix_deployment_controller::NativeChildListDispatch, String> {
    use phoenix_deployment_controller::native_guardian::NativeChildListDispatchError;
    match client.dispatch_native_child_list(ticket, issue) {
        Ok(dispatched) => Ok(dispatched),
        Err(NativeChildListDispatchError::KnownUnsubmitted { error, .. }) => Err(format!(
            "child-list dispatch pre-submit (issue retained; no cancellation API): {error:?}"
        )),
        Err(NativeChildListDispatchError::SubmissionUnknown(error)) => Err(format!(
            "child-list dispatch ambiguous after submit (issue retained): {error:?}"
        )),
    }
}

#[cfg(unix)]
fn release_leaf(path: &Path) -> Result<OsString, String> {
    let components: Vec<_> = path.components().collect();
    if !matches!(components.as_slice(), [Component::RootDir, Component::Normal(tmp), Component::Normal(root), Component::Normal(file)]
        if *tmp == "tmp" && root.to_str().is_some_and(|s| s.starts_with("sdc-disposable-") && s.len() > "sdc-disposable-".len()) && *file == "runner-release.signal")
    {
        return Err("release file must be directly below a disposable /tmp root".into());
    }
    Ok(path
        .file_name()
        .ok_or("missing release leaf")?
        .to_os_string())
}

#[cfg(unix)]
type ReleaseIdentity = (u64, u64);

#[cfg(unix)]
fn metadata_identity(metadata: &fs::Metadata) -> ReleaseIdentity {
    (metadata.dev(), metadata.ino())
}

#[cfg(target_os = "macos")]
fn stat_identity(stat: &libc::stat) -> Result<ReleaseIdentity, String> {
    let device = u64::try_from(stat.st_dev).map_err(|_| "negative release device id")?;
    Ok((device, stat.st_ino))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn stat_identity(stat: &libc::stat) -> ReleaseIdentity {
    (stat.st_dev, stat.st_ino)
}

#[cfg(unix)]
fn baseline_leaf(directory: &File, leaf: &OsString) -> Result<Option<ReleaseIdentity>, String> {
    let leaf = CString::new(leaf.as_bytes()).map_err(|e| e.to_string())?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: leaf is NUL-terminated, directory owns a live fd, and stat is writable.
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            leaf.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error.to_string())
        };
    }
    // SAFETY: successful fstatat initialized stat.
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG
        || stat.st_uid != unsafe { libc::geteuid() }
        || stat.st_mode & 0o777 != 0o600
    {
        return Err("release file is not a private regular file".into());
    }
    #[cfg(target_os = "macos")]
    let identity = stat_identity(&stat)?;
    #[cfg(not(target_os = "macos"))]
    let identity = stat_identity(&stat);
    Ok(Some(identity))
}

#[cfg(unix)]
enum PreparedReleaseGate {
    Stdin,
    File {
        directory: File,
        leaf: OsString,
        before: Option<ReleaseIdentity>,
    },
}

#[cfg(unix)]
impl ReleaseSource {
    fn prepare(&self) -> Result<PreparedReleaseGate, String> {
        match self {
            Self::Stdin => Ok(PreparedReleaseGate::Stdin),
            Self::DisposableFile(path) => {
                let leaf = release_leaf(path)?;
                let root = path.parent().ok_or("missing disposable root")?;
                let directory = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(root)
                    .map_err(|e| e.to_string())?;
                let metadata = directory.metadata().map_err(|e| e.to_string())?;
                if !metadata.is_dir()
                    || metadata.uid() != unsafe { libc::geteuid() }
                    || metadata.mode() & 0o777 != 0o700
                {
                    return Err("release directory is not private".into());
                }
                let before = baseline_leaf(&directory, &leaf)?;
                Ok(PreparedReleaseGate::File {
                    directory,
                    leaf,
                    before,
                })
            }
        }
    }
}

#[cfg(unix)]
fn validate_release_path(path: &Path) -> Result<(), String> {
    ReleaseSource::DisposableFile(path.to_owned())
        .prepare()
        .map(|_| ())
}

#[cfg(unix)]
impl PreparedReleaseGate {
    fn poll_once(&self) -> Result<bool, String> {
        let Self::File {
            directory,
            leaf,
            before,
        } = self
        else {
            return Err("stdin release cannot be polled".into());
        };
        let leaf = CString::new(leaf.as_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: the pinned directory fd is live and the leaf is NUL-terminated.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                leaf.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(error.to_string())
            };
        }
        // SAFETY: openat returned a new owned descriptor.
        let mut file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o600
            || metadata.len() != 1
        {
            return Err("release file is not a one-byte private regular file".into());
        }
        if Some(metadata_identity(&metadata)) == *before {
            return Ok(false);
        }
        let mut byte = [0];
        file.read_exact(&mut byte).map_err(|e| e.to_string())?;
        if byte != [b'R'] {
            return Err("invalid fixture gate release".into());
        }
        Ok(true)
    }

    fn wait(self) -> Result<(), String> {
        if matches!(&self, Self::Stdin) {
            let mut byte = [0];
            std::io::stdin()
                .read_exact(&mut byte)
                .map_err(|error| format!("fixture gate EOF: {error}"))?;
            return if byte == [b'R'] {
                Ok(())
            } else {
                Err("invalid fixture gate release".into())
            };
        }
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if self.poll_once()? {
                return Ok(());
            }
            if Instant::now() >= until {
                return Err("fixture release file timed out".into());
            }
            // test-timing-allow: deadline is only an outer liveness guard; file observation is causal.
            thread::sleep(Duration::from_millis(20));
        }
    }
}

fn run_initial_sample(
    config: &RunnerConfig,
    store: &mut ControllerSqliteStore,
    client: &mut GuardianClient,
    committed: CommittedTransition,
    fixture: &InitialFixture,
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
    let dispatched = dispatch_sampling(store, client, ticket, issue)?;
    let verified = client
        .observe_native_sampling(dispatched)
        .map_err(|error| format!("observe sample: {error:?}"))?;
    if let InitialFixture::PauseAfterObservation(release) = fixture {
        let snapshot = client
            .snapshot()
            .map_err(|error| format!("observed snapshot: {error:?}"))?;
        if snapshot.state != WireState::Active
            || snapshot.retained.len() != 1
            || snapshot.retained[0].key
                != phoenix_deployment_controller::guardian_ipc::WireKey::try_from(issue.key())
                    .map_err(|error| format!("issue key: {error:?}"))?
            || !matches!(snapshot.retained[0].completion, phoenix_deployment_controller::guardian_ipc::WireCompletion::SampleResources { sequence, result: Ok(_), } if sequence == issue.sequence())
        {
            return Err("observation fixture lacks exact active retained sample".into());
        }
        let gate = release.prepare()?;
        println!(
            "sample_observed_pending_application revision={} sequence={} snapshot={}",
            issue.key().revision().get(),
            issue.sequence(),
            serde_json::to_string(&snapshot).map_err(|error| error.to_string())?
        );
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        gate.wait()?;
    }
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
    if let InitialFixture::PauseAfterApplication(release) = fixture {
        let gate = release.prepare()?;
        println!(
            "application_committed_pending_ack revision={}",
            key.revision().get()
        );
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        gate.wait()?;
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
#[allow(clippy::too_many_lines)]
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let config = parse_args(&args)?;
    let mut store =
        ControllerSqliteStore::open_existing(&config.store).map_err(|e| e.to_string())?;
    if matches!(
        config.mode,
        RunnerMode::RecoverAndFinish { .. }
            | RunnerMode::RecoverFinishAndSample { .. }
            | RunnerMode::RecoverFinishAndChildList { .. }
    ) {
        preflight_recover_and_finish(&mut store, config.attempt)?;
    }
    let pending_recovery_key = if matches!(config.mode, RunnerMode::RecoverAppliedAck(_)) {
        Some(
            store
                .pending_native_sampling_ack(config.attempt)
                .map_err(|e| e.to_string())?
                .ok_or("no unique successful pending sample ACK")?
                .0,
        )
    } else if matches!(config.mode, RunnerMode::RecoverChildListAck(_)) {
        Some(
            store
                .pending_native_child_list_ack(config.attempt)
                .map_err(|e| e.to_string())?
                .ok_or("no unique successful pending child-list ACK")?
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
    if matches!(
        config.mode,
        RunnerMode::RecoverAppliedAck(_)
            | RunnerMode::RecoverUnappliedSample { .. }
            | RunnerMode::RecoverAndFinish { .. }
            | RunnerMode::RecoverFinishAndSample { .. }
            | RunnerMode::RecoverFinishAndChildList { .. }
            | RunnerMode::RecoverChildListAck(_)
    ) && status == VerifiedBindingStatus::InitialActive
    {
        return Err("recovery requires detached guardian".into());
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
    let reconciliation_session = if initial_active {
        None
    } else {
        Some(begin_reconciliation(
            &mut client,
            &mut store,
            config.attempt,
            &snapshot,
            match config.mode {
                RunnerMode::RecoverAndFinish { fixture } => fixture,
                RunnerMode::RecoverFinishAndSample { .. }
                | RunnerMode::RecoverFinishAndChildList { .. }
                | RunnerMode::ObserveOnly { .. }
                | RunnerMode::InitialSample { .. }
                | RunnerMode::RecoverAppliedAck(_)
                | RunnerMode::RecoverUnappliedSample { .. }
                | RunnerMode::RecoverChildListAck(_) => FinishFixture::Run,
            },
        )?)
    };
    match &config.mode {
        RunnerMode::ObserveOnly { observe_seconds } => {
            observe_until(&mut client, &snapshot, *observe_seconds)
        }
        RunnerMode::RecoverAndFinish { fixture } => run_recover_and_finish(
            &mut store,
            &mut client,
            config.attempt,
            &snapshot,
            reconciliation_session.ok_or("missing fresh reconciliation session")?,
            RecoveryRun::Finish(*fixture),
        ),
        RunnerMode::RecoverFinishAndSample { fixture } => run_recover_and_finish(
            &mut store,
            &mut client,
            config.attempt,
            &snapshot,
            reconciliation_session.ok_or("missing fresh reconciliation session")?,
            RecoveryRun::FinishAndSample(*fixture),
        ),
        RunnerMode::RecoverFinishAndChildList { fixture } => run_recover_and_finish(
            &mut store,
            &mut client,
            config.attempt,
            &snapshot,
            reconciliation_session.ok_or("missing fresh reconciliation session")?,
            RecoveryRun::FinishAndChildList(fixture.clone()),
        ),
        RunnerMode::RecoverUnappliedSample { fixture } => run_recover_unapplied_sample(
            &mut store,
            &mut client,
            config.attempt,
            reconciliation_session.ok_or("missing fresh reconciliation session")?,
            fixture,
        ),
        RunnerMode::RecoverAppliedAck(fixture) => run_recover_applied_ack(
            &mut store,
            &mut client,
            pending_recovery_key.ok_or("missing preflight pending ACK")?,
            fixture,
        ),
        RunnerMode::RecoverChildListAck(fixture) => run_recover_child_list_ack(
            &mut store,
            &mut client,
            pending_recovery_key.ok_or("missing preflight pending child-list ACK")?,
            fixture,
        ),
        RunnerMode::InitialSample { fixture, .. } => run_initial_sample(
            &config,
            &mut store,
            &mut client,
            committed.ok_or("missing committed transition")?,
            fixture,
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
    use std::os::unix::fs::PermissionsExt;

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
    fn release_file_is_only_allowed_with_one_initial_pause_and_private_disposable_path() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = tempfile::Builder::new()
            .prefix("sdc-disposable-")
            .tempdir_in("/tmp")
            .unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("runner-release.signal");
        assert!(validate_release_path(&path).is_ok());
        let mut initial = args("initial-sample");
        initial.extend(
            [
                "--deadline-nanoseconds",
                "100",
                "--root-pid",
                "42",
                "--root-birth-seconds",
                "1",
                "--root-birth-nanos",
                "0",
            ]
            .map(OsString::from),
        );
        let mut paused = initial.clone();
        paused.push("--fixture-pause-after-apply".into());
        paused.push("--fixture-release-file".into());
        paused.push(path.as_os_str().into());
        assert!(matches!(
            parse_args(&paused),
            Ok(RunnerConfig {
                mode: RunnerMode::InitialSample {
                    fixture: InitialFixture::PauseAfterApplication(ReleaseSource::DisposableFile(
                        _
                    )),
                    ..
                },
                ..
            })
        ));
        let mut observe = args("observe-only");
        observe.extend([OsString::from("--observe-seconds"), OsString::from("5")]);
        for mut invalid in [initial.clone(), args("recover-applied-ack"), observe] {
            invalid.extend([
                OsString::from("--fixture-release-file"),
                path.as_os_str().into(),
            ]);
            assert!(parse_args(&invalid).is_err());
        }
        for tail in [
            vec!["--fixture-release-file"],
            vec!["--fixture-release-file", "--fixture-pause-after-apply"],
            vec!["--fixture-release-file", "/tmp/elsewhere"],
        ] {
            let mut invalid = paused.clone();
            invalid.extend(tail.into_iter().map(OsString::from));
            assert!(parse_args(&invalid).is_err());
        }
        let mut missing = paused.clone();
        missing.pop();
        assert!(parse_args(&missing).is_err());
        let mut wrong = paused.clone();
        wrong[18] = root.path().join("other.signal").into_os_string();
        assert!(parse_args(&wrong).is_err());
        for path in [
            Path::new("/tmp/sdc-disposable-x/../runner-release.signal"),
            Path::new("/tmp/sdc-disposable-x/nested/runner-release.signal"),
            Path::new("/var/tmp/sdc-disposable-x/runner-release.signal"),
        ] {
            assert!(validate_release_path(path).is_err());
        }
        symlink("/tmp/target", &path).unwrap();
        assert!(validate_release_path(&path).is_err());
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"R").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(validate_release_path(&path).is_ok());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(validate_release_path(&path).is_err());
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validate_release_path(&root.path().join("runner-release.signal")).is_err());
    }

    #[test]
    fn prepared_file_gate_accepts_release_created_after_prepare() {
        let root = tempfile::Builder::new()
            .prefix("sdc-disposable-")
            .tempdir_in("/tmp")
            .unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("runner-release.signal");
        let gate = ReleaseSource::DisposableFile(path.clone())
            .prepare()
            .unwrap();
        fs::write(&path, b"R").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        gate.wait().unwrap();
    }

    #[test]
    fn prepared_file_gate_refuses_fifo_without_blocking() {
        let root = tempfile::Builder::new()
            .prefix("sdc-disposable-")
            .tempdir_in("/tmp")
            .unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("runner-release.signal");
        let gate = ReleaseSource::DisposableFile(path.clone())
            .prepare()
            .unwrap();
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: mkfifo reads the NUL-terminated name and does not retain it.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let start = Instant::now();
        assert!(gate.poll_once().is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn prepared_file_gate_ignores_replaced_parent_path() {
        use std::os::unix::fs::symlink;
        let root = tempfile::Builder::new()
            .prefix("sdc-disposable-")
            .tempdir_in("/tmp")
            .unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let outside = tempfile::tempdir_in("/tmp").unwrap();
        let path = root.path().join("runner-release.signal");
        let gate = ReleaseSource::DisposableFile(path.clone())
            .prepare()
            .unwrap();
        let renamed = root.path().with_extension("moved");
        fs::rename(root.path(), &renamed).unwrap();
        let outside_release = outside.path().join("runner-release.signal");
        fs::write(&outside_release, b"R").unwrap();
        fs::set_permissions(&outside_release, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(outside.path(), root.path()).unwrap();
        assert!(!gate.poll_once().unwrap());
        fs::remove_file(root.path()).unwrap();
        fs::remove_dir_all(renamed).unwrap();
    }

    #[test]
    fn unapplied_verification_file_gate_requires_private_disposable_path() {
        let root = tempfile::Builder::new()
            .prefix("sdc-disposable-")
            .tempdir_in("/tmp")
            .unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = args("recover-unapplied-sample");
        command.extend([
            OsString::from("--fixture-pause-after-recovery-verification"),
            OsString::from("--fixture-release-file"),
            root.path().join("runner-release.signal").into_os_string(),
        ]);
        assert!(matches!(
            parse_args(&command),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverUnappliedSample {
                    fixture: UnappliedRecoveryFixture::PauseAfterVerificationFile(
                        ReleaseSource::DisposableFile(_)
                    )
                },
                ..
            })
        ));
        command[10] = OsString::from("/tmp/other/runner-release.signal");
        assert!(parse_args(&command).is_err());
    }

    #[test]
    fn recovered_ack_pause_requires_exact_file_gate_and_mode() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::Builder::new()
            .prefix("sdc-disposable-")
            .tempdir_in("/tmp")
            .unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("runner-release.signal");
        let mut paused = args("recover-applied-ack");
        paused.extend([
            OsString::from("--fixture-pause-after-recovered-ack"),
            OsString::from("--fixture-release-file"),
            path.as_os_str().into(),
        ]);
        assert!(matches!(
            parse_args(&paused),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverAppliedAck(AckRecoveryFixture::PauseAfterConfirmation(
                    ReleaseSource::DisposableFile(_)
                )),
                ..
            })
        ));
        for size in [9, 10] {
            assert!(
                parse_args(&paused[..size]).is_err(),
                "missing release arg {size}"
            );
        }
        let mut invalid = paused.clone();
        invalid[9] = "--fixture-pause-after-recovered-ack".into();
        assert!(parse_args(&invalid).is_err(), "duplicate pause flag");
        invalid = paused.clone();
        invalid.extend(paused[8..].iter().cloned());
        assert!(parse_args(&invalid).is_err(), "duplicate flag group");
        invalid = paused.clone();
        invalid[10] = root.path().join("other.signal").into_os_string();
        assert!(parse_args(&invalid).is_err(), "wrong release path");
        for mode in [
            "observe-only",
            "initial-sample",
            "recover-unapplied-sample",
            "recover-and-finish",
            "recover-finish-and-sample",
        ] {
            let mut candidate = args(mode);
            if mode == "observe-only" {
                candidate.extend(["--observe-seconds", "5"].map(OsString::from));
            }
            if mode == "initial-sample" {
                candidate.extend(
                    [
                        "--deadline-nanoseconds",
                        "100",
                        "--root-pid",
                        "42",
                        "--root-birth-seconds",
                        "1",
                        "--root-birth-nanos",
                        "0",
                    ]
                    .map(OsString::from),
                );
            }
            candidate.extend(paused[8..].iter().cloned());
            assert!(parse_args(&candidate).is_err(), "cross-mode {mode}");
        }
        let mut no_file = args("recover-applied-ack");
        no_file.push("--fixture-pause-after-recovered-ack".into());
        assert!(parse_args(&no_file).is_err(), "stdin not accepted");
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
                mode: RunnerMode::RecoverAppliedAck(AckRecoveryFixture::Run),
                ..
            })
        ));
        assert!(matches!(
            parse_args(&args("recover-and-finish")),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverAndFinish {
                    fixture: FinishFixture::Run
                },
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
                    fixture: InitialFixture::PauseAfterApplication(ReleaseSource::Stdin),
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn unapplied_recovery_and_initial_fixtures_are_mode_specific() {
        assert!(matches!(
            parse_args(&args("recover-unapplied-sample")),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverUnappliedSample {
                    fixture: UnappliedRecoveryFixture::Run
                },
                ..
            })
        ));
        let mut initial = args("initial-sample");
        initial.extend(
            [
                "--deadline-nanoseconds",
                "100",
                "--root-pid",
                "42",
                "--root-birth-seconds",
                "1",
                "--root-birth-nanos",
                "0",
            ]
            .into_iter()
            .map(OsString::from),
        );
        assert!(matches!(
            parse_args(&initial),
            Ok(RunnerConfig {
                mode: RunnerMode::InitialSample {
                    fixture: InitialFixture::Run,
                    ..
                },
                ..
            })
        ));
        for flag in [
            "--fixture-pause-after-observation",
            "--fixture-pause-after-apply",
        ] {
            let mut selected = initial.clone();
            selected.push(flag.into());
            assert!(matches!(
                parse_args(&selected),
                Ok(RunnerConfig {
                    mode: RunnerMode::InitialSample {
                        fixture: InitialFixture::PauseAfterObservation(ReleaseSource::Stdin)
                            | InitialFixture::PauseAfterApplication(ReleaseSource::Stdin),
                        ..
                    },
                    ..
                })
            ));
            for second in [
                "--fixture-pause-after-observation",
                "--fixture-pause-after-apply",
            ] {
                let mut duplicate = selected.clone();
                duplicate.push(second.into());
                assert!(parse_args(&duplicate).is_err(), "{flag} + {second}");
            }
            for mode in [
                "recover-applied-ack",
                "recover-unapplied-sample",
                "recover-and-finish",
                "recover-finish-and-sample",
            ] {
                let mut invalid = args(mode);
                invalid.push(flag.into());
                assert!(parse_args(&invalid).is_err(), "{mode} + {flag}");
            }
        }
        for flag in [
            "--fixture-pause-before-begin",
            "--fixture-activation-barriers",
            "--fixture-activation-and-sample-barriers",
        ] {
            let mut invalid = args("recover-unapplied-sample");
            invalid.push(flag.into());
            assert!(parse_args(&invalid).is_err(), "unapplied + {flag}");
        }
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
                fixture: InitialFixture::Run,
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
    #[allow(clippy::too_many_lines)]
    fn initial_dispatch_deadline_and_admission_refusal_cancel_the_unsubmitted_issue() {
        use phoenix_deployment_controller::{
            guardian_ipc::{
                read_frame, write_frame, BuildIdentity, Request, Response, WireError, WireSession,
            },
            native_guardian::bind_disposable,
            NativeGuardianIdentity,
        };
        for expired in [true, false] {
            let directory = tempfile::Builder::new()
                .prefix("sdc-runner-cancel-")
                .tempdir_in("/tmp")
                .unwrap();
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let socket_dir = directory.path().join("sdc-disposable-runner");
            let listener = bind_disposable(&socket_dir).unwrap();
            let path = directory.path().join("controller.sqlite");
            let attempt = AttemptId::new(95);
            let deadline = if expired {
                MonotonicTick::new(0)
            } else {
                MonotonicTick::native_now()
                    .unwrap()
                    .checked_add(Duration::from_secs(120))
                    .unwrap()
            };
            let root = ProcessIdentity::new(
                ProcessId::new(NonZeroU32::new(42).unwrap()),
                BirthToken::new(1, 0).unwrap(),
            );
            let mut store = ControllerSqliteStore::open(&path).unwrap();
            let committed = DeploymentAttempt::start(
                attempt,
                deadline,
                ResourcePolicy::new(
                    NonZeroUsize::new(2).unwrap(),
                    0,
                    0,
                    CpuContentionBasisPoints::new(0).unwrap(),
                    MemoryPressure::Critical,
                ),
                root,
            )
            .persist(&mut store)
            .unwrap();
            let identity = NativeGuardianIdentity {
                nonce: GuardianLifetimeNonce::new([5; 16]).unwrap(),
                process: root,
            };
            store
                .commit_initial_binding(
                    attempt,
                    deadline,
                    PersistenceRevision::first(),
                    identity,
                    &[],
                )
                .unwrap();
            let snapshot = WireSnapshot {
                instance: 1,
                attempt: attempt.get(),
                deadline: deadline.get(),
                state: WireState::Active,
                session: WireSession {
                    instance: 1,
                    generation: 1,
                },
                slots: vec![],
                retained: vec![],
                native_binding: Some(WireNativeBinding {
                    attempt: attempt.get(),
                    deadline: deadline.get(),
                    nonce: identity.nonce.bytes(),
                    guardian: identity.process.into(),
                    slots: vec![],
                }),
            };
            let config = RunnerConfig {
                socket_dir: socket_dir.clone(),
                store: path.clone(),
                attempt,
                mode: RunnerMode::InitialSample {
                    deadline,
                    root,
                    fixture: InitialFixture::Run,
                },
            };
            std::thread::scope(|scope| {
                scope.spawn(move || {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    assert!(matches!(
                        read_frame::<Request>(&mut stream).unwrap(),
                        Request::Hello { .. }
                    ));
                    write_frame(
                        &mut stream,
                        &Response::Hello {
                            build: BuildIdentity::compiled(),
                            snapshot: snapshot.clone(),
                        },
                    )
                    .unwrap();
                    assert_eq!(
                        read_frame::<Request>(&mut stream).unwrap(),
                        Request::Snapshot
                    );
                    write_frame(&mut stream, &Response::Snapshot(snapshot)).unwrap();
                    if !expired {
                        assert!(matches!(
                            read_frame::<Request>(&mut stream).unwrap(),
                            Request::DispatchAdmit { .. }
                        ));
                        write_frame(&mut stream, &Response::Refused(WireError::MissingProof))
                            .unwrap();
                    }
                    let mut byte = [0];
                    assert_eq!(
                        std::io::Read::read(&mut stream, &mut byte).unwrap(),
                        0,
                        "no DispatchSubmit after refusal/deadline"
                    );
                });
                let mut client = GuardianClient::connect(&socket_dir).unwrap();
                client.hello().unwrap();
                let error = run_initial_sample(
                    &config,
                    &mut store,
                    &mut client,
                    committed,
                    &InitialFixture::Run,
                )
                .unwrap_err();
                assert!(
                    error.starts_with("dispatch known unsubmitted (issue cancelled):"),
                    "{error}"
                );
            });
            let db = rusqlite::Connection::open(&path).unwrap();
            for table in [
                "native_sampling_issues",
                "native_sampling_applications",
                "native_sampling_ack_confirmations",
            ] {
                assert_eq!(
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    0
                );
            }
            let loaded = store.load_attempt(attempt).unwrap().unwrap();
            assert_eq!(loaded.phase(), Phase::ResourceSampling);
            assert_eq!(loaded.persistence_revision(), PersistenceRevision::first());
        }
    }

    #[test]
    fn expired_recovery_uses_runner_preflight_without_any_durable_mutation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("expired.sqlite");
        let mut store = ControllerSqliteStore::open(&path).unwrap();
        let attempt = AttemptId::new(9);
        let deadline = MonotonicTick::new(0);
        let root = ProcessIdentity::new(
            ProcessId::new(NonZeroU32::new(42).unwrap()),
            BirthToken::new(1, 0).unwrap(),
        );
        let committed = DeploymentAttempt::start(
            attempt,
            deadline,
            ResourcePolicy::new(
                NonZeroUsize::new(2).unwrap(),
                0,
                0,
                CpuContentionBasisPoints::new(0).unwrap(),
                MemoryPressure::Critical,
            ),
            root,
        )
        .persist(&mut store)
        .unwrap();
        let binding = store
            .commit_initial_binding(
                attempt,
                deadline,
                PersistenceRevision::first(),
                phoenix_deployment_controller::NativeGuardianIdentity {
                    nonce: GuardianLifetimeNonce::new([1; 16]).unwrap(),
                    process: root,
                },
                &[],
            )
            .unwrap();
        let binding = store.commit_detached_binding(&binding, &[]).unwrap();
        let interrupted = committed
            .into_parts()
            .0
            .interrupt()
            .unwrap()
            .persist(&mut store)
            .unwrap();
        let binding = store
            .commit_fresh_interruption_binding(
                &binding,
                interrupted.checkpoint().persistence_revision().unwrap(),
            )
            .unwrap();
        let snapshot = || {
            let raw = rusqlite::Connection::open(&path).unwrap();
            [
                "attempts",
                "resource_policies",
                "resource_samples",
                "sampling_obligations",
                "guardian_bindings",
                "guardian_sessions",
                "guardian_slots",
                "native_reconciliation_completions",
                "native_reconciliation_activations",
            ]
            .into_iter()
            .map(|table| {
                let mut statement = raw
                    .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                    .unwrap();
                let columns = statement.column_count();
                let rows = statement
                    .query_map([], |row| {
                        (0..columns)
                            .map(|column| row.get::<_, rusqlite::types::Value>(column))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .unwrap();
                rows.collect::<Result<Vec<_>, _>>().unwrap()
            })
            .collect::<Vec<_>>()
        };
        let before = snapshot();
        assert_eq!(
            preflight_recover_and_finish(&mut store, attempt).unwrap_err(),
            "recovery deadline expired"
        );
        drop(store);
        let mut reopened = ControllerSqliteStore::open_existing(&path).unwrap();
        assert_eq!(snapshot(), before);
        assert_eq!(
            reopened.load_native_binding(attempt).unwrap().unwrap(),
            binding
        );
        let checkpoint = reopened.load_attempt(attempt).unwrap().unwrap();
        assert_eq!(checkpoint.phase(), Phase::Interrupted);
        assert_eq!(checkpoint.deadline(), deadline);
        assert_eq!(
            checkpoint.persistence_revision(),
            binding.current_session().interruption_revision
        );
    }

    #[test]
    fn pending_begin_fixture_is_typed_and_only_available_to_recover_and_finish() {
        let mut finish = args("recover-and-finish");
        finish.push("--fixture-pause-before-begin".into());
        assert!(matches!(
            parse_args(&finish),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverAndFinish {
                    fixture: FinishFixture::PauseBeforeBegin
                },
                ..
            })
        ));
        for mode in ["observe-only", "initial-sample", "recover-applied-ack"] {
            let mut invalid = args(mode);
            let valid_options: &[&str] = match mode {
                "observe-only" => &["--observe-seconds", "5"],
                "initial-sample" => &[
                    "--deadline-nanoseconds",
                    "100",
                    "--root-pid",
                    "42",
                    "--root-birth-seconds",
                    "1",
                    "--root-birth-nanos",
                    "0",
                ],
                "recover-applied-ack" => &[],
                _ => unreachable!(),
            };
            invalid.extend(valid_options.iter().map(OsString::from));
            assert!(parse_args(&invalid).is_ok(), "valid {mode} baseline");
            invalid.push("--fixture-pause-before-begin".into());
            assert!(parse_args(&invalid).is_err(), "{mode}");
        }
        finish.push("--fixture-pause-after-apply".into());
        assert!(parse_args(&finish).is_err());
    }

    #[test]
    fn recovery_verification_gate_is_only_available_to_unapplied_recovery() {
        let flag = "--fixture-pause-after-recovery-verification";
        let mut recovery = args("recover-unapplied-sample");
        recovery.push(flag.into());
        assert!(matches!(
            parse_args(&recovery),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverUnappliedSample {
                    fixture: UnappliedRecoveryFixture::PauseAfterVerification
                },
                ..
            })
        ));
        recovery.push(flag.into());
        assert!(parse_args(&recovery).is_err(), "duplicate fixture");
        for mode in [
            "observe-only",
            "initial-sample",
            "recover-applied-ack",
            "recover-and-finish",
            "recover-finish-and-sample",
        ] {
            let mut invalid = args(mode);
            let options: &[&str] = match mode {
                "observe-only" => &["--observe-seconds", "5"],
                "initial-sample" => &[
                    "--deadline-nanoseconds",
                    "100",
                    "--root-pid",
                    "42",
                    "--root-birth-seconds",
                    "1",
                    "--root-birth-nanos",
                    "0",
                ],
                _ => &[],
            };
            invalid.extend(options.iter().map(OsString::from));
            assert!(parse_args(&invalid).is_ok(), "valid {mode} baseline");
            invalid.push(flag.into());
            assert!(parse_args(&invalid).is_err(), "{mode}");
        }
    }

    #[test]
    fn sample_continuation_and_third_barrier_are_structurally_mode_specific() {
        assert!(matches!(
            parse_args(&args("recover-finish-and-sample")),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverFinishAndSample {
                    fixture: SampleFixture::Run
                },
                ..
            })
        ));
        let mut sample = args("recover-finish-and-sample");
        sample.push("--fixture-activation-and-sample-barriers".into());
        assert!(matches!(
            parse_args(&sample),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverFinishAndSample {
                    fixture: SampleFixture::ActivationAndSampleBarriers
                },
                ..
            })
        ));
        sample.push("--fixture-activation-and-sample-barriers".into());
        assert!(parse_args(&sample).is_err(), "duplicate fixture");
        let mut activation = args("recover-and-finish");
        activation.push("--fixture-activation-barriers".into());
        assert!(matches!(
            parse_args(&activation),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverAndFinish {
                    fixture: FinishFixture::ActivationBarriers
                },
                ..
            })
        ));
    }

    #[test]
    fn child_list_modes_have_isolated_fixtures_and_explicit_ack_mode() {
        assert!(matches!(
            parse_args(&args("recover-child-list-ack")),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverChildListAck(ChildListAckFixture::Run),
                ..
            })
        ));
        let mut ack_with_fixture = args("recover-child-list-ack");
        ack_with_fixture.push("--fixture-pause-after-child-list-apply".into());
        assert!(parse_args(&ack_with_fixture).is_err());

        assert!(matches!(
            parse_args(&args("recover-finish-and-child-list")),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverFinishAndChildList {
                    fixture: ChildListFixture::Run
                },
                ..
            })
        ));
        let mut child_pause = args("recover-finish-and-child-list");
        child_pause.push("--fixture-pause-after-child-list-apply".into());
        assert!(matches!(
            parse_args(&child_pause),
            Ok(RunnerConfig {
                mode: RunnerMode::RecoverFinishAndChildList {
                    fixture: ChildListFixture::PauseAfterApplication(ReleaseSource::Stdin)
                },
                ..
            })
        ));
        let mut child_with_sample_fixture = args("recover-finish-and-child-list");
        child_with_sample_fixture.push("--fixture-activation-and-sample-barriers".into());
        assert!(parse_args(&child_with_sample_fixture).is_err());
        let mut sample_with_child_fixture = args("recover-finish-and-sample");
        sample_with_child_fixture.push("--fixture-pause-after-child-list-apply".into());
        assert!(parse_args(&sample_with_child_fixture).is_err());
    }

    #[test]
    fn sample_barrier_fixtures_are_restricted_to_their_modes() {
        for mode in [
            "observe-only",
            "initial-sample",
            "recover-applied-ack",
            "recover-unapplied-sample",
            "recover-and-finish",
            "recover-finish-and-sample",
        ] {
            let mut baseline = args(mode);
            let options: &[&str] = match mode {
                "observe-only" => &["--observe-seconds", "5"],
                "initial-sample" => &[
                    "--deadline-nanoseconds",
                    "100",
                    "--root-pid",
                    "42",
                    "--root-birth-seconds",
                    "1",
                    "--root-birth-nanos",
                    "0",
                ],
                _ => &[],
            };
            baseline.extend(options.iter().map(OsString::from));
            assert!(parse_args(&baseline).is_ok());
            for (flag, owner) in [
                ("--fixture-pause-after-observation", "initial-sample"),
                ("--fixture-pause-after-apply", "initial-sample"),
                ("--fixture-pause-after-recovered-ack", "none"),
                ("--fixture-pause-before-begin", "recover-and-finish"),
                ("--fixture-activation-barriers", "recover-and-finish"),
                (
                    "--fixture-activation-and-sample-barriers",
                    "recover-finish-and-sample",
                ),
            ] {
                let mut candidate = baseline.clone();
                candidate.push(flag.into());
                assert_eq!(
                    parse_args(&candidate).is_ok(),
                    mode == owner,
                    "{mode}: {flag}"
                );
            }
        }
    }

    #[test]
    fn sample_mode_refuses_other_mode_options_with_its_usage() {
        for options in [
            vec!["--observe-seconds", "5"],
            vec![
                "--deadline-nanoseconds",
                "100",
                "--root-pid",
                "42",
                "--root-birth-seconds",
                "1",
                "--root-birth-nanos",
                "0",
            ],
        ] {
            let mut invalid = args("recover-finish-and-sample");
            invalid.extend(options.iter().map(OsString::from));
            let usage = parse_args(&invalid)
                .err()
                .expect("cross-mode flags refused");
            assert!(usage.contains("recover-finish-and-sample"));
            assert!(usage.contains(
                "--fixture-activation-and-sample-barriers (recover-finish-and-sample only)"
            ));
        }
    }

    #[test]
    fn runner_modes_reject_cross_mode_arguments_and_missing_initial_authority() {
        let mut observe_with_pause = args("observe-only");
        observe_with_pause.push(OsString::from("--fixture-pause-after-apply"));
        assert!(parse_args(&observe_with_pause).is_err());
        let mut finish_with_observe = args("recover-and-finish");
        finish_with_observe.extend([OsString::from("--observe-seconds"), OsString::from("5")]);
        assert!(parse_args(&finish_with_observe).is_err());
        assert!(parse_args(&args("initial-sample")).is_err());
    }
}
