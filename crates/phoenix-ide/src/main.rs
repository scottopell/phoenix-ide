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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match parse_invocation(std::env::args().skip(1))? {
        Invocation::Server | Invocation::ServerCommand => phoenix_ide::run_server().await,
        Invocation::MigrateOnly => phoenix_ide::migrate_database().await,
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
