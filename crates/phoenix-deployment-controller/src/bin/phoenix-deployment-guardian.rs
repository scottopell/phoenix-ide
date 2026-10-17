//! Disposable native transport fixture. This is not the safe controller CLI.
#[cfg(target_os = "macos")]
fn fixture_flags(args: &[std::ffi::OsString]) -> Result<(bool, bool), std::io::Error> {
    let ack = args.len() == 13 && args[12] == "--fixture-ack-response-barrier";
    let failure = args.len() == 13 && args[12] == "--fixture-sample-unsupported";
    if !(args.len() == 12 || ack || failure)
        || args[0] != "--socket-dir"
        || args[2] != "--child-seconds"
        || args[4] != "--serve-seconds"
        || args[6] != "--attempt"
        || args[8] != "--deadline-nanoseconds"
        || args[10] != "--store"
    {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput,
            "usage: --socket-dir <new private temporary sdc-disposable-* directory> --child-seconds 1..120 --serve-seconds 1..120 --attempt <u128> --deadline-nanoseconds <u64> --store <existing controller-owned schema-v8 database> [--fixture-ack-response-barrier|--fixture-sample-unsupported]"));
    }
    Ok((ack, failure))
}

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use phoenix_deployment_controller::native_guardian::run_disposable_guardian;
    use std::{io, path::Path};
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let (ack_response_barrier, fail_sample) = fixture_flags(&args)?;
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
        ack_response_barrier,
        fail_sample,
    )
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("disposable native guardian requires macOS");
    std::process::exit(2);
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn fixture_result_is_only_bounded_unsupported_in_private_guardian_mode() {
        let base: Vec<OsString> = [
            "--socket-dir",
            "/tmp/sdc-disposable-fixture",
            "--child-seconds",
            "40",
            "--serve-seconds",
            "50",
            "--attempt",
            "64",
            "--deadline-nanoseconds",
            "100",
            "--store",
            "/tmp/store.sqlite",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(fixture_flags(&base).unwrap(), (false, false));
        let mut ack = base.clone();
        ack.push("--fixture-ack-response-barrier".into());
        assert_eq!(fixture_flags(&ack).unwrap(), (true, false));
        let mut failure = base.clone();
        failure.push("--fixture-sample-unsupported".into());
        assert_eq!(fixture_flags(&failure).unwrap(), (false, true));
        for invalid in [
            "--fixture-sample-malformed",
            "--fixture-sample-temporarily-unavailable",
            "--sample-result",
            "Unsupported",
        ] {
            let mut args = base.clone();
            args.push(invalid.into());
            assert!(fixture_flags(&args).is_err(), "accepted {invalid}");
        }
        failure.push("--fixture-ack-response-barrier".into());
        assert!(fixture_flags(&failure).is_err());
        let mut switched = base.clone();
        switched[0] = "--store".into();
        assert!(fixture_flags(&switched).is_err());
        assert!(fixture_flags(&base[..11]).is_err());
    }
}
