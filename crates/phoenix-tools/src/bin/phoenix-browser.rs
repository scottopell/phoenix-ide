#[cfg(unix)]
#[path = "phoenix-browser/cli.rs"]
mod cli;
#[cfg(unix)]
#[path = "phoenix-browser/daemon.rs"]
mod daemon;
#[cfg(unix)]
#[path = "phoenix-browser/protocol.rs"]
mod protocol;

#[cfg(unix)]
fn main() {
    if std::env::args().nth(1).as_deref() == Some("__daemon") {
        if let Err(error) = daemon::close_inherited_descriptors() {
            eprintln!("daemon descriptor cleanup failed: {error}");
            std::process::exit(1);
        }
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("create browser runtime");
    std::process::exit(runtime.block_on(cli::main_entry()));
}

#[cfg(not(unix))]
fn main() {
    eprintln!("phoenix-browser requires macOS or Linux (local Unix sockets)");
    std::process::exit(1);
}
