use std::process::Command;

#[test]
fn logging_init_failure_reaches_bounded_fatal_file() {
    let directory = tempfile::tempdir().unwrap();
    let blocker = directory.path().join("not-a-directory");
    std::fs::write(&blocker, "block").unwrap();
    let fatal = directory.path().join("fatal.log");

    let output = Command::new(env!("CARGO_BIN_EXE_phoenix_ide"))
        .env("PHOENIX_LOG_FILE", blocker.join("prod.log"))
        .env("PHOENIX_FATAL_LOG_FILE", &fatal)
        .env("PHOENIX_LOG_STDOUT", "false")
        .env("PHOENIX_TRACE_EXPORTER", "none")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let diagnostic = std::fs::read_to_string(fatal).unwrap();
    assert!(diagnostic.contains("PHOENIX_LOG_FILE"), "{diagnostic}");
    assert!(diagnostic.len() <= 64 * 1024);
}
