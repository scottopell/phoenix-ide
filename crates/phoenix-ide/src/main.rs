#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invocation {
    Server,
    MigrateOnly,
}

fn parse_invocation(args: impl IntoIterator<Item = String>) -> Result<Invocation, String> {
    let args: Vec<String> = args.into_iter().collect();
    match args.as_slice() {
        [] => Ok(Invocation::Server),
        [arg] if arg == "--migrate-only" => Ok(Invocation::MigrateOnly),
        _ => Err("usage: phoenix_ide [--migrate-only]".to_string()),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match parse_invocation(std::env::args().skip(1))? {
        Invocation::Server => phoenix_ide::run_server().await,
        Invocation::MigrateOnly => phoenix_ide::migrate_database().await,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_invocation, Invocation};

    #[test]
    fn parses_server_and_migrate_only_invocations() {
        assert_eq!(parse_invocation([]).unwrap(), Invocation::Server);
        assert_eq!(
            parse_invocation(["--migrate-only".to_string()]).unwrap(),
            Invocation::MigrateOnly
        );
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
