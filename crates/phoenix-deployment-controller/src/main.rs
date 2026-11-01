//! Safe store inspection and explicit administrative closeout; no native runtime.
use std::{ffi::OsString, io::Write, path::PathBuf, process::ExitCode};

use phoenix_deployment_controller::{
    AdministrativeRiskAcceptance, AttemptId, CleanupOutcome, ControllerSqliteStore, LoadedAttempt,
    Phase, RecoveryDisposition,
};
use serde::Serialize;

#[derive(Clone, Copy)]
enum Command {
    Status,
    Closeout,
}

struct Request {
    command: Command,
    store: PathBuf,
    attempt: AttemptId,
}

#[derive(Serialize)]
struct StatusSummary {
    attempt_id: String,
    persistence_revision: u64,
    phase: Phase,
    cleanup_outcome: CleanupOutcome,
}

impl StatusSummary {
    fn from_loaded(loaded: &LoadedAttempt) -> Self {
        Self {
            attempt_id: loaded.attempt_id().get().to_string(),
            persistence_revision: loaded.persistence_revision().get(),
            phase: loaded.phase(),
            cleanup_outcome: loaded.cleanup_outcome(),
        }
    }
}

// Exit codes and categories are the CLI contract in the feature requirements.
#[derive(Clone, Copy)]
enum Failure {
    Usage,
    Store,
    NotFound,
    WrongPhase,
    Internal,
}

impl Failure {
    const fn code(self) -> u8 {
        match self {
            Self::Usage => 2,
            Self::Store => 3,
            Self::NotFound => 4,
            Self::WrongPhase => 5,
            Self::Internal => 6,
        }
    }

    const fn category(self) -> &'static str {
        match self {
            Self::Usage => "usage_error",
            Self::Store => "store_error",
            Self::NotFound => "attempt_not_found",
            Self::WrongPhase => "wrong_phase",
            Self::Internal => "internal_error",
        }
    }
}

fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Request, Failure> {
    let command = match args.next().as_deref().and_then(std::ffi::OsStr::to_str) {
        Some("status") => Command::Status,
        Some("closeout") => Command::Closeout,
        _ => return Err(Failure::Usage),
    };
    let mut store = None;
    let mut attempt = None;
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--store") if store.is_none() => {
                let path = args
                    .next()
                    .filter(|path| !path.is_empty())
                    .ok_or(Failure::Usage)?;
                store = Some(PathBuf::from(path));
            }
            Some("--attempt") if attempt.is_none() => {
                let value = args.next().ok_or(Failure::Usage)?;
                let text = value.to_str().ok_or(Failure::Usage)?;
                if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(Failure::Usage);
                }
                attempt = Some(AttemptId::new(text.parse().map_err(|_| Failure::Usage)?));
            }
            _ => return Err(Failure::Usage),
        }
    }
    Ok(Request {
        command,
        store: store.ok_or(Failure::Usage)?,
        attempt: attempt.ok_or(Failure::Usage)?,
    })
}

fn run(request: Request) -> Result<StatusSummary, Failure> {
    let mut store =
        ControllerSqliteStore::open_existing(request.store).map_err(|_| Failure::Store)?;
    let loaded = store
        .load_attempt(request.attempt)
        .map_err(|_| Failure::Store)?
        .ok_or(Failure::NotFound)?;
    match request.command {
        Command::Status => Ok(StatusSummary::from_loaded(&loaded)),
        Command::Closeout => {
            // Check before recovery preparation: rejected phases cannot advance a revision.
            if loaded.phase() != Phase::AdministrativeCloseoutRequired {
                return Err(Failure::WrongPhase);
            }
            let RecoveryDisposition::Stable(stable) =
                loaded.prepare_recovery().map_err(|_| Failure::Internal)?
            else {
                return Err(Failure::Internal);
            };
            let acceptance = AdministrativeRiskAcceptance::for_attempt(stable.attempt_id());
            let committed = stable
                .accept_administrative_risk(acceptance)
                .map_err(|_| Failure::Internal)?
                .persist(&mut store)
                .map_err(|_| Failure::Store)?;
            let (checkpoint, batch) = committed.into_dispatch();
            if !batch.is_empty() || checkpoint.phase() != Phase::AdministrativelyClosed {
                return Err(Failure::Internal);
            }
            Ok(StatusSummary {
                attempt_id: checkpoint.attempt_id().get().to_string(),
                persistence_revision: checkpoint
                    .persistence_revision()
                    .ok_or(Failure::Internal)?
                    .get(),
                phase: checkpoint.phase(),
                cleanup_outcome: checkpoint.cleanup_outcome(),
            })
        }
    }
}

fn execute() -> Result<(), Failure> {
    let request = parse(std::env::args_os().skip(1))?;
    let summary = run(request)?;
    let mut json = serde_json::to_vec(&summary).map_err(|_| Failure::Internal)?;
    json.push(b'\n');
    std::io::stdout()
        .lock()
        .write_all(&json)
        .map_err(|_| Failure::Internal)
}

fn main() -> ExitCode {
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("{}", failure.category());
            ExitCode::from(failure.code())
        }
    }
}
