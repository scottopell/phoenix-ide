//! Disposable durable-binding runner; reconciliation permits observations only.

#[cfg(unix)]
use phoenix_deployment_controller::{
    guardian_ipc::{WireNativeBinding, WireSnapshot},
    native_guardian::{verify_native_binding, GuardianClient, VerifiedBindingStatus},
    AttemptId, ControllerSqliteStore, PersistenceRevision, Phase, RecoveryDisposition,
};
#[cfg(unix)]
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
struct RunnerConfig {
    socket_dir: PathBuf,
    observe_seconds: u64,
    store: PathBuf,
    attempt: AttemptId,
}

#[cfg(unix)]
fn parse_args(args: &[OsString]) -> Result<RunnerConfig, String> {
    if args.len() != 8
        || args[0] != "--socket-dir"
        || args[2] != "--observe-seconds"
        || args[4] != "--store"
        || args[6] != "--attempt"
    {
        return Err("usage: --socket-dir <private fixture> --observe-seconds 1..60 --store <existing schema-v2 database> --attempt <u128>".into());
    }
    let observe_seconds = args[3]
        .to_str()
        .ok_or("invalid duration")?
        .parse()
        .map_err(|_| "invalid duration")?;
    if !(1..=60).contains(&observe_seconds) {
        return Err("duration outside 1..60".into());
    }
    let attempt = AttemptId::new(
        args[7]
            .to_str()
            .ok_or("invalid attempt")?
            .parse()
            .map_err(|_| "invalid attempt")?,
    );
    Ok(RunnerConfig {
        socket_dir: PathBuf::from(&args[1]),
        observe_seconds,
        store: PathBuf::from(&args[5]),
        attempt,
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
        .commit_detached_binding(&pending)
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

#[cfg(unix)]
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let config = parse_args(&args)?;
    let mut store =
        ControllerSqliteStore::open_existing(&config.store).map_err(|e| e.to_string())?;
    let (mut client, snapshot) = connect_consistent(&config.socket_dir)?;
    let status = verify_native_binding(&mut store, config.attempt, &snapshot)?;
    println!("verified binding: {status:?}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    if status == VerifiedBindingStatus::PendingBegin {
        catch_up_pending_begin(&mut client, &mut store, config.attempt, &snapshot)?;
    }
    if status != VerifiedBindingStatus::InitialActive {
        begin_reconciliation(&mut client, &mut store, config.attempt, &snapshot)?;
    }
    observe_until(&mut client, &snapshot, config.observe_seconds)
}

#[cfg(not(unix))]
fn main() {
    eprintln!("disposable runner requires Unix");
    std::process::exit(2);
}
