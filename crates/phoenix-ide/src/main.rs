#[derive(Debug, Clone, PartialEq, Eq)]
enum Invocation {
    Server,
    MigrateOnly,
    ServerCommand,
    Observation {
        fixture: std::path::PathBuf,
        target: String,
        port: u16,
    },
}

const USAGE: &str = "usage: phoenix_ide [--migrate-only | --observe-fixture ABSOLUTE_DB --target REFERENCE [--port PORT]]";

fn parse_invocation(args: impl IntoIterator<Item = String>) -> Result<Invocation, String> {
    let args: Vec<String> = args.into_iter().collect();
    match args.as_slice() {
        [] => Ok(Invocation::Server),
        [arg] if arg == "--migrate-only" => Ok(Invocation::MigrateOnly),
        [arg] if arg == "--build-identity" => Ok(Invocation::ServerCommand),
        [subcommand, rest @ ..]
            if subcommand == "suggest"
                || (subcommand == "--sandbox-exec"
                    && matches!(rest, [separator, _] if separator == "--")) =>
        {
            Ok(Invocation::ServerCommand)
        }
        [flag, fixture, target_flag, target, rest @ ..]
            if flag == "--observe-fixture" && target_flag == "--target" =>
        {
            let fixture = std::path::PathBuf::from(fixture);
            if !fixture.is_absolute() || target.is_empty() || target.starts_with("--") {
                return Err(
                    "observation requires an absolute fixture path and nonempty target".to_owned(),
                );
            }
            let port = match rest {
                [] => 8043,
                [flag, value] if flag == "--port" => value
                    .parse::<u16>()
                    .map_err(|_| "observation port must be in 0..=65535".to_owned())?,
                _ => return Err(USAGE.to_owned()),
            };
            Ok(Invocation::Observation {
                fixture,
                target: target.clone(),
                port,
            })
        }
        _ => Err(USAGE.to_owned()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerFailureDisposition {
    RecordDiagnostic,
    ExitFatalLocalAuthority,
}

fn server_failure_disposition(
    error: &(dyn std::error::Error + 'static),
) -> ServerFailureDisposition {
    if error
        .downcast_ref::<phoenix_ide::FatalLocalAuthorityExit>()
        .is_some()
    {
        ServerFailureDisposition::ExitFatalLocalAuthority
    } else {
        ServerFailureDisposition::RecordDiagnostic
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let invocation = parse_invocation(std::env::args().skip(1))?;
    let records_fatal_diagnostics =
        matches!(invocation, Invocation::Server | Invocation::ServerCommand);
    if records_fatal_diagnostics {
        phoenix_ide::install_fatal_diagnostic_hook();
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            if records_fatal_diagnostics {
                phoenix_ide::record_fatal_diagnostic(&error);
            }
            return Err(error.into());
        }
    };
    match invocation {
        Invocation::Server | Invocation::ServerCommand => {
            let result = runtime.block_on(phoenix_ide::run_server());
            if let Err(error) = &result {
                match server_failure_disposition(error.as_ref()) {
                    ServerFailureDisposition::ExitFatalLocalAuthority => {
                        std::process::exit(phoenix_ide::FATAL_LOCAL_AUTHORITY_EXIT);
                    }
                    ServerFailureDisposition::RecordDiagnostic => {
                        phoenix_ide::record_fatal_diagnostic(error);
                    }
                }
            }
            result
        }
        Invocation::MigrateOnly => runtime.block_on(phoenix_ide::migrate_database()),
        Invocation::Observation {
            fixture,
            target,
            port,
        } => runtime
            .block_on(phoenix_ide::observation_startup::run(
                &fixture, &target, port,
            ))
            .map_err(|error| error as Box<dyn std::error::Error>),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_invocation, server_failure_disposition, Invocation, ServerFailureDisposition,
    };

    #[test]
    fn parses_supported_invocations() {
        assert_eq!(parse_invocation([]).unwrap(), Invocation::Server);
        assert_eq!(
            parse_invocation(["--migrate-only".to_string()]).unwrap(),
            Invocation::MigrateOnly
        );
        for args in [
            vec!["--build-identity".to_string()],
            vec!["suggest".to_string(), "show status".to_string()],
            vec![
                "--sandbox-exec".to_string(),
                "--".to_string(),
                "git status".to_string(),
            ],
        ] {
            assert_eq!(parse_invocation(args).unwrap(), Invocation::ServerCommand);
        }
    }

    #[test]
    fn observation_requires_complete_explicit_intent() {
        for args in [
            vec!["--observe-fixture"],
            vec!["--observe-fixture", "relative.db", "--target", "target"],
            vec!["--observe-fixture", "/fixture.db", "--target", ""],
            vec!["--observe-fixture", "/fixture.db", "--target", "--port"],
            vec![
                "--observe-fixture",
                "/fixture.db",
                "--target",
                "target",
                "--port",
                "65536",
            ],
            vec![
                "--observe-fixture",
                "/fixture.db",
                "--target",
                "target",
                "--port",
                "-1",
            ],
            vec![
                "--observe-fixture",
                "/fixture.db",
                "--target",
                "target",
                "--migrate-only",
            ],
        ] {
            assert!(parse_invocation(args.into_iter().map(str::to_owned)).is_err());
        }
        for (extra, port) in [
            (vec![], 8043),
            (vec!["--port", "0"], 0),
            (vec!["--port", "65535"], 65535),
        ] {
            let args = [
                vec!["--observe-fixture", "/fixture.db", "--target", "target"],
                extra,
            ]
            .concat();
            assert_eq!(
                parse_invocation(args.into_iter().map(str::to_owned)).unwrap(),
                Invocation::Observation {
                    fixture: "/fixture.db".into(),
                    target: "target".into(),
                    port,
                }
            );
        }
    }

    #[test]
    fn fatal_local_authority_exit_bypasses_synchronous_diagnostic() {
        assert_eq!(
            server_failure_disposition(&phoenix_ide::FatalLocalAuthorityExit),
            ServerFailureDisposition::ExitFatalLocalAuthority
        );
        assert_eq!(
            server_failure_disposition(&std::io::Error::other("ordinary failure")),
            ServerFailureDisposition::RecordDiagnostic
        );
    }

    #[test]
    fn rejects_unknown_and_extra_arguments() {
        for args in [
            vec!["--unknown".to_string()],
            vec!["--migrate-only".to_string(), "extra".to_string()],
        ] {
            assert_eq!(parse_invocation(args).unwrap_err(), super::USAGE);
        }
    }
}
