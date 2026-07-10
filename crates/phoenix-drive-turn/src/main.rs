use phoenix_ide::drive_turn::{self, DatabaseMode, DriveTurnRequest};
use std::path::PathBuf;
use std::time::Duration;

const HELP: &str = "drive-turn — drive one user turn through the production Phoenix runtime

Usage:
  drive-turn --cwd PATH --model MODEL (--prompt TEXT | --prompt-file PATH)
             [--memory | --temp-db | --db PATH] [--timeout SECONDS]

Database modes:
  --memory       Transient in-memory SQLite database (default)
  --temp-db      Retained unique SQLite file in the OS temp directory
  --db PATH      Retained SQLite file at PATH

Output:
  One JSON object on stdout. Runtime logs are emitted on stderr.
";

#[derive(Debug)]
struct Args {
    cwd: PathBuf,
    model: String,
    prompt: String,
    database: DatabaseMode,
    timeout: Duration,
}

#[tokio::main]
async fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{HELP}");
            return;
        }
        Err(error) => {
            eprintln!("drive-turn: {error}\n\n{HELP}");
            std::process::exit(2);
        }
    };

    let request = DriveTurnRequest {
        cwd: args.cwd,
        model: args.model,
        prompt: args.prompt,
        database: args.database,
        timeout: args.timeout,
    };
    match drive_turn::run(request).await {
        Ok(result) => match serde_json::to_writer(std::io::stdout(), &result) {
            Ok(()) => println!(),
            Err(error) => {
                eprintln!("drive-turn: failed to serialize result: {error}");
                std::process::exit(1);
            }
        },
        Err(error) => {
            eprintln!("drive-turn: {error}");
            std::process::exit(1);
        }
    }
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let mut args = args.into_iter();
    let mut cwd = None;
    let mut model = None;
    let mut prompt = None;
    let mut prompt_file = None;
    let mut database = None;
    let mut timeout = Duration::from_secs(300);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--cwd" => cwd = Some(PathBuf::from(next_value(&mut args, "--cwd")?)),
            "--model" => model = Some(next_value(&mut args, "--model")?),
            "--prompt" => prompt = Some(next_value(&mut args, "--prompt")?),
            "--prompt-file" => {
                prompt_file = Some(PathBuf::from(next_value(&mut args, "--prompt-file")?));
            }
            "--memory" => set_database(&mut database, DatabaseMode::Memory)?,
            "--temp-db" => set_database(&mut database, DatabaseMode::TemporaryFile)?,
            "--db" => set_database(
                &mut database,
                DatabaseMode::File(PathBuf::from(next_value(&mut args, "--db")?)),
            )?,
            "--timeout" => {
                let value = next_value(&mut args, "--timeout")?;
                timeout = Duration::from_secs(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid --timeout value '{value}'"))?,
                );
            }
            _ => return Err(format!("unknown argument '{arg}'")),
        }
    }

    if prompt.is_some() && prompt_file.is_some() {
        return Err("--prompt and --prompt-file are mutually exclusive".into());
    }
    let prompt = match (prompt, prompt_file) {
        (Some(prompt), None) => prompt,
        (None, Some(path)) => std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
        (None, None) => return Err("one of --prompt or --prompt-file is required".into()),
        (Some(_), Some(_)) => unreachable!("mutual exclusion checked above"),
    };

    Ok(Some(Args {
        cwd: cwd.ok_or_else(|| "--cwd is required".to_string())?,
        model: model.ok_or_else(|| "--model is required".to_string())?,
        prompt,
        database: database.unwrap_or(DatabaseMode::Memory),
        timeout,
    }))
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn set_database(slot: &mut Option<DatabaseMode>, mode: DatabaseMode) -> Result<(), String> {
    if slot.replace(mode).is_some() {
        return Err("--memory, --temp-db, and --db are mutually exclusive".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_memory() {
        let args = parse_args([
            "--cwd".into(),
            "/tmp".into(),
            "--model".into(),
            "model".into(),
            "--prompt".into(),
            "hello".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(args.database, DatabaseMode::Memory);
    }

    #[test]
    fn rejects_parallel_database_representations() {
        let error = parse_args([
            "--cwd".into(),
            "/tmp".into(),
            "--model".into(),
            "model".into(),
            "--prompt".into(),
            "hello".into(),
            "--memory".into(),
            "--temp-db".into(),
        ])
        .unwrap_err();
        assert!(error.contains("mutually exclusive"));
    }
}
