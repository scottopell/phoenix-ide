//! Observation-only disposable runner; durable binding is diagnostic, never an execution permit.
#[cfg(unix)]
fn same_native_identity(
    a: Option<&phoenix_deployment_controller::guardian_ipc::WireNativeBinding>,
    b: Option<&phoenix_deployment_controller::guardian_ipc::WireNativeBinding>,
) -> bool {
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
        _ => false,
    }
}

#[cfg(unix)]
fn main() -> Result<(), String> {
    use phoenix_deployment_controller::{
        native_guardian::{verify_native_binding, GuardianClient},
        AttemptId, ControllerSqliteStore,
    };
    use std::{
        io::Write,
        path::Path,
        thread,
        time::{Duration, Instant},
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 8
        || args[0] != "--socket-dir"
        || args[2] != "--observe-seconds"
        || args[4] != "--store"
        || args[6] != "--attempt"
    {
        return Err("usage: --socket-dir <private fixture> --observe-seconds 1..60 --store <existing schema-v2 database> --attempt <u128>".into());
    }
    let seconds: u64 = args[3]
        .to_str()
        .ok_or("invalid duration")?
        .parse()
        .map_err(|_| "invalid duration")?;
    if !(1..=60).contains(&seconds) {
        return Err("duration outside 1..60".into());
    }
    let attempt = AttemptId::new(
        args[7]
            .to_str()
            .ok_or("invalid attempt")?
            .parse()
            .map_err(|_| "invalid attempt")?,
    );
    let mut store =
        ControllerSqliteStore::open_existing(Path::new(&args[5])).map_err(|e| e.to_string())?;
    let mut client =
        GuardianClient::connect(Path::new(&args[1])).map_err(|e| format!("connect: {e:?}"))?;
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
    let status = verify_native_binding(&mut store, attempt, &snapshot)?;
    println!("verified binding: {status:?}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let until = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < until {
        let current = client.snapshot().map_err(|e| format!("snapshot: {e:?}"))?;
        if current.instance != snapshot.instance
            || current.attempt != snapshot.attempt
            || current.deadline != snapshot.deadline
            || current.slots != snapshot.slots
            || !same_native_identity(
                current.native_binding.as_ref(),
                snapshot.native_binding.as_ref(),
            )
        {
            return Err("guardian or slot identity changed".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
    // No reconcile or mutation capability is constructed from the binding.
    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("disposable runner requires Unix");
    std::process::exit(2);
}
