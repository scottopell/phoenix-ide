use phoenix_deployment_controller::MonotonicTick;
use std::{path::Path, process::Command};

pub(super) fn sampling_runner_command(
    dir: &Path,
    path: &Path,
    deadline: MonotonicTick,
    pause: bool,
) -> Command {
    sampling_runner_command_with_root(
        dir,
        path,
        deadline,
        phoenix_deployment_controller::ProcessIdentity::new(
            phoenix_deployment_controller::ProcessId::new(std::num::NonZeroU32::new(42).unwrap()),
            phoenix_deployment_controller::BirthToken::new(1, 0).unwrap(),
        ),
        pause,
    )
}

pub(super) fn sampling_runner_command_with_root(
    dir: &Path,
    path: &Path,
    deadline: MonotonicTick,
    root: phoenix_deployment_controller::ProcessIdentity,
    pause: bool,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"));
    let deadline_arg = deadline.get().to_string();
    let root_pid = root.pid().get().to_string();
    let root_birth_seconds = root.birth().seconds().to_string();
    let root_birth_nanos = root.birth().nanos().to_string();
    command.args([
        "--mode",
        "initial-sample",
        "--socket-dir",
        dir.to_str().unwrap(),
        "--store",
        path.to_str().unwrap(),
        "--attempt",
        "64",
        "--deadline-nanoseconds",
        &deadline_arg,
        "--root-pid",
        &root_pid,
        "--root-birth-seconds",
        &root_birth_seconds,
        "--root-birth-nanos",
        &root_birth_nanos,
    ]);
    if pause {
        command.arg("--fixture-pause-after-apply");
    }
    command
}

pub(super) fn sampling_rows(path: &Path) -> (i64, i64, i64, u64, u64) {
    use rusqlite::Connection;
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let count = |table: &str| -> i64 {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    };
    let (revision, sequence): (Vec<u8>, Vec<u8>) = db.query_row(
        "SELECT applying_revision, i.sequence FROM native_sampling_applications a JOIN native_sampling_issues i USING(attempt_id,dispatch_revision,ordinal) ORDER BY i.dispatch_revision DESC, i.ordinal DESC LIMIT 1",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    (
        count("native_sampling_issues"),
        count("native_sampling_applications"),
        count("native_sampling_ack_confirmations"),
        u64::from_be_bytes(revision.try_into().unwrap()),
        u64::from_be_bytes(sequence.try_into().unwrap()),
    )
}

pub(super) fn settled_sampling_keys(path: &Path) -> Vec<(u64, i64, u64, u64, u64)> {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut query = db.prepare(
        "SELECT i.dispatch_revision, i.ordinal, i.sequence, i.session_generation, c.confirming_session_generation FROM native_sampling_issues i JOIN native_sampling_applications a USING(attempt_id,dispatch_revision,ordinal) JOIN native_sampling_ack_confirmations c USING(attempt_id,dispatch_revision,ordinal) ORDER BY i.dispatch_revision,i.ordinal"
    ).unwrap();
    query
        .query_map([], |row| {
            let word = |index| -> rusqlite::Result<u64> {
                let bytes: [u8; 8] = row.get::<_, Vec<u8>>(index)?.try_into().unwrap();
                Ok(u64::from_be_bytes(bytes))
            };
            Ok((word(0)?, row.get(1)?, word(2)?, word(3)?, word(4)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
