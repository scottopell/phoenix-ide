//! Disposable native transport fixture. This is not the safe controller CLI.
#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use phoenix_deployment_controller::native_guardian::run_disposable_guardian;
    use std::{io, path::Path};
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 6
        || args[0] != "--socket-dir"
        || args[2] != "--child-seconds"
        || args[4] != "--serve-seconds"
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "usage: --socket-dir <new private temporary sdc-disposable-* directory> --child-seconds 1..120 --serve-seconds 1..120").into());
    }
    let child = args[3]
        .to_str()
        .ok_or_else(|| io::Error::other("invalid child seconds"))?
        .parse()?;
    let serve = args[5]
        .to_str()
        .ok_or_else(|| io::Error::other("invalid serve seconds"))?
        .parse()?;
    run_disposable_guardian(Path::new(&args[1]), child, serve)
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("disposable native guardian requires macOS");
    std::process::exit(2);
}
