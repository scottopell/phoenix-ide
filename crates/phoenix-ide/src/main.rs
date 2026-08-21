#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invocation {
    Server,
    MigrateOnly,
    ServerCommand,
}

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
        _ => Err("usage: phoenix_ide [--migrate-only]".to_string()),
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
                phoenix_ide::record_fatal_diagnostic(error);
                if error
                    .downcast_ref::<phoenix_ide::FatalLocalAuthorityExit>()
                    .is_some()
                {
                    std::process::exit(phoenix_ide::FATAL_LOCAL_AUTHORITY_EXIT);
                }
            }
            result
        }
        Invocation::MigrateOnly => runtime.block_on(phoenix_ide::migrate_database()),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_invocation, Invocation};

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
    fn rejects_unknown_and_extra_arguments() {
        for args in [
            vec!["--unknown".to_string()],
            vec!["--migrate-only".to_string(), "extra".to_string()],
        ] {
            assert_eq!(
                parse_invocation(args).unwrap_err(),
                "usage: phoenix_ide [--migrate-only]"
            );
        }
    }
}
