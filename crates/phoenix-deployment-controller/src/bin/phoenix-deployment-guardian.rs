//! Disposable native transport fixture. This is not the safe controller CLI.
#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use phoenix_deployment_controller::native_guardian::run_disposable_guardian;
    use std::{io, path::Path};
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 12
        || args[0] != "--socket-dir"
        || args[2] != "--child-seconds"
        || args[4] != "--serve-seconds"
        || args[6] != "--attempt"
        || args[8] != "--deadline-nanoseconds"
        || args[10] != "--store"
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "usage: --socket-dir <new private temporary sdc-disposable-* directory> --child-seconds 1..120 --serve-seconds 1..120 --attempt <u128> --deadline-nanoseconds <u64> --store <existing controller-owned schema-v4 database>").into());
    }
    let child = args[3]
        .to_str()
        .ok_or_else(|| io::Error::other("invalid child seconds"))?
        .parse()?;
    let serve = args[5]
        .to_str()
        .ok_or_else(|| io::Error::other("invalid serve seconds"))?
        .parse()?;
    let attempt = phoenix_deployment_controller::AttemptId::new(
        args[7].to_str().ok_or("invalid attempt")?.parse()?,
    );
    let deadline = phoenix_deployment_controller::MonotonicTick::new(
        args[9].to_str().ok_or("invalid deadline")?.parse()?,
    );
    run_disposable_guardian(
        Path::new(&args[1]),
        child,
        serve,
        attempt,
        deadline,
        Path::new(&args[11]),
    )
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("disposable native guardian requires macOS");
    std::process::exit(2);
}
