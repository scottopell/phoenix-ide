use phoenix_deployment_controller::MonotonicTick;
use std::{path::Path, process::Command};

pub(super) fn sampling_runner_command(
    dir: &Path,
    path: &Path,
    deadline: MonotonicTick,
    pause: bool,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"));
    let deadline_arg = deadline.get().to_string();
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
        "42",
        "--root-birth-seconds",
        "1",
        "--root-birth-nanos",
        "0",
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
        "SELECT applying_revision, i.sequence FROM native_sampling_applications a JOIN native_sampling_issues i USING(attempt_id,dispatch_revision,ordinal)",
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
