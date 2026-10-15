//! Observation-only disposable runner; never claims durable reconciliation.
#[cfg(unix)]
fn main() -> Result<(), String> {
    use phoenix_deployment_controller::native_guardian::GuardianClient;
    use std::{
        path::Path,
        thread,
        time::{Duration, Instant},
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 4 || args[0] != "--socket-dir" || args[2] != "--observe-seconds" {
        return Err("usage: --socket-dir <existing private temporary sdc-disposable-* directory> --observe-seconds 1..60".into());
    }
    let seconds: u64 = args[3]
        .to_str()
        .ok_or("invalid duration")?
        .parse()
        .map_err(|_| "invalid duration")?;
    if !(1..=60).contains(&seconds) {
        return Err("duration outside 1..60".into());
    }
    let mut client = GuardianClient::connect(Path::new(&args[1]))
        .map_err(|error| format!("connect: {error:?}"))?;
    let hello = client
        .hello()
        .map_err(|error| format!("hello: {error:?}"))?;
    let slots = client
        .inventory()
        .map_err(|error| format!("inventory: {error:?}"))?;
    if hello.slots != slots {
        return Err("inventory changed during hello".into());
    }
    println!(
        "guardian instance={} state={:?} slots={}",
        hello.instance,
        hello.state,
        slots.len()
    );
    use std::io::Write;
    std::io::stdout()
        .flush()
        .map_err(|error| error.to_string())?;
    let until = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < until {
        let snapshot = client
            .snapshot()
            .map_err(|error| format!("snapshot: {error:?}"))?;
        if snapshot.instance != hello.instance || snapshot.slots != slots {
            return Err("guardian or slot inventory changed; unresolved supervision".into());
        }
        for slot in &slots {
            let observation = client
                .observe_slot(*slot)
                .map_err(|error| format!("slot: {error:?}"))?;
            println!("slot={} observation={observation:?}", slot.ordinal);
        }
        thread::sleep(Duration::from_millis(100));
    }
    println!("reconciliation: {:?}", client.reconcile().unwrap_err());
    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("disposable runner requires Unix");
    std::process::exit(2);
}
