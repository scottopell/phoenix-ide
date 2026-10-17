//! Disposable sampling runner with distinct ACK and reconciliation recovery. Not the safe controller CLI.

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
        fixture: InitialFixture,
    },
    RecoverAppliedAck,
    RecoverUnappliedSample,
    RecoverAndFinish {
        fixture: FinishFixture,
    },
    RecoverFinishAndSample {
        fixture: SampleFixture,
    },
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum InitialFixture {
    Run,
    PauseAfterObservation,
    PauseAfterApplication,
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
#[derive(Clone, Copy)]
enum RecoveryRun {
    Finish(FinishFixture),
    FinishAndSample(SampleFixture),
}

#[cfg(unix)]
impl RecoveryRun {
    fn activation_fixture(self) -> FinishFixture {
        match self {
            Self::Finish(fixture) => fixture,
            Self::FinishAndSample(SampleFixture::Run) => FinishFixture::Run,
            Self::FinishAndSample(SampleFixture::ActivationAndSampleBarriers) => {
                FinishFixture::ActivationBarriers
            }
        }
    }
}

#[cfg(unix)]
fn parse_args(args: &[OsString]) -> Result<RunnerConfig, String> {
    let usage = "usage: --mode <observe-only|initial-sample|recover-applied-ack|recover-unapplied-sample|recover-and-finish|recover-finish-and-sample> --socket-dir <private fixture> --store <existing schema-v5 store> --attempt <u128> [--observe-seconds 1..60 | --deadline-nanoseconds <u64> --root-pid <u32> --root-birth-seconds <u64> --root-birth-nanos <u32> [--fixture-pause-after-observation|--fixture-pause-after-apply] | --fixture-pause-before-begin or --fixture-activation-barriers (recover-and-finish only) | --fixture-activation-and-sample-barriers (recover-finish-and-sample only)]";
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
        "recover-unapplied-sample" if args.len() == 8 => RunnerMode::RecoverUnappliedSample,
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
        "initial-sample"
            if (args.len() == 16 || args.len() == 17)
                && args[8] == "--deadline-nanoseconds"
                && args[10] == "--root-pid"
                && args[12] == "--root-birth-seconds"
                && args[14] == "--root-birth-nanos"
                && (args.len() == 16
                    || args[16] == "--fixture-pause-after-apply"
                    || args[16] == "--fixture-pause-after-observation") =>
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
                fixture: match args.get(16).and_then(|flag| flag.to_str()) {
                    None => InitialFixture::Run,
                    Some("--fixture-pause-after-observation") => {
                        InitialFixture::PauseAfterObservation
                    }
                    Some("--fixture-pause-after-apply") => InitialFixture::PauseAfterApplication,
                    _ => return Err(usage.into()),
                },
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
    let RecoveryRun::FinishAndSample(sample_fixture) = run else {
        return Ok(());
    };
    if matches!(sample_fixture, SampleFixture::ActivationAndSampleBarriers) {
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
    std::io::stdout().flush().map_err(|e| e.to_string())
}

fn run_recover_unapplied_sample(
    store: &mut ControllerSqliteStore,
    client: &mut GuardianClient,
    attempt: AttemptId,
    session: phoenix_deployment_controller::guardian_ipc::WireSession,
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

fn initial_fixture_gate() -> Result<(), String> {
    let mut byte = [0];
    std::io::stdin()
        .read_exact(&mut byte)
        .map_err(|error| format!("fixture gate EOF: {error}"))?;
    if byte != [b'R'] {
        return Err("invalid fixture gate release".into());
    }
    Ok(())
}

fn run_initial_sample(
    config: &RunnerConfig,
    store: &mut ControllerSqliteStore,
    client: &mut GuardianClient,
    committed: CommittedTransition,
    fixture: InitialFixture,
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
    if matches!(fixture, InitialFixture::PauseAfterObservation) {
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
        println!(
            "sample_observed_pending_application revision={} sequence={} snapshot={}",
            issue.key().revision().get(),
            issue.sequence(),
            serde_json::to_string(&snapshot).map_err(|error| error.to_string())?
        );
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        initial_fixture_gate()?;
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
    if matches!(fixture, InitialFixture::PauseAfterApplication) {
        println!(
            "application_committed_pending_ack revision={}",
            key.revision().get()
        );
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        initial_fixture_gate()?;
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
        RunnerMode::RecoverAndFinish { .. } | RunnerMode::RecoverFinishAndSample { .. }
    ) {
        preflight_recover_and_finish(&mut store, config.attempt)?;
    }
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
    if matches!(
        config.mode,
        RunnerMode::RecoverAppliedAck
            | RunnerMode::RecoverUnappliedSample
            | RunnerMode::RecoverAndFinish { .. }
            | RunnerMode::RecoverFinishAndSample { .. }
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
                | RunnerMode::ObserveOnly { .. }
                | RunnerMode::InitialSample { .. }
                | RunnerMode::RecoverAppliedAck
                | RunnerMode::RecoverUnappliedSample => FinishFixture::Run,
            },
        )?)
    };
    match config.mode {
        RunnerMode::ObserveOnly { observe_seconds } => {
            observe_until(&mut client, &snapshot, observe_seconds)
        }
        RunnerMode::RecoverAndFinish { fixture } => run_recover_and_finish(
            &mut store,
            &mut client,
            config.attempt,
            &snapshot,
            reconciliation_session.ok_or("missing fresh reconciliation session")?,
            RecoveryRun::Finish(fixture),
        ),
        RunnerMode::RecoverFinishAndSample { fixture } => run_recover_and_finish(
            &mut store,
            &mut client,
            config.attempt,
            &snapshot,
            reconciliation_session.ok_or("missing fresh reconciliation session")?,
            RecoveryRun::FinishAndSample(fixture),
        ),
        RunnerMode::RecoverUnappliedSample => run_recover_unapplied_sample(
            &mut store,
            &mut client,
            config.attempt,
            reconciliation_session.ok_or("missing fresh reconciliation session")?,
        ),
        RunnerMode::RecoverAppliedAck => run_recover_applied_ack(
            &mut store,
            &mut client,
            pending_recovery_key.ok_or("missing preflight pending ACK")?,
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
                    fixture: InitialFixture::PauseAfterApplication,
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
                mode: RunnerMode::RecoverUnappliedSample,
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
        for (flag, fixture) in [
            (
                "--fixture-pause-after-observation",
                InitialFixture::PauseAfterObservation,
            ),
            (
                "--fixture-pause-after-apply",
                InitialFixture::PauseAfterApplication,
            ),
        ] {
            let mut selected = initial.clone();
            selected.push(flag.into());
            assert!(matches!(
                (parse_args(&selected), fixture),
                (
                    Ok(RunnerConfig {
                        mode: RunnerMode::InitialSample {
                            fixture: InitialFixture::PauseAfterObservation,
                            ..
                        },
                        ..
                    }),
                    InitialFixture::PauseAfterObservation
                ) | (
                    Ok(RunnerConfig {
                        mode: RunnerMode::InitialSample {
                            fixture: InitialFixture::PauseAfterApplication,
                            ..
                        },
                        ..
                    }),
                    InitialFixture::PauseAfterApplication
                )
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
                    InitialFixture::Run,
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
