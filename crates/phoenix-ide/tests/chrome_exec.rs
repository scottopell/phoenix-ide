#![cfg(any(target_os = "linux", target_os = "macos"))]

use base64::Engine as _;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::process::Command;

#[test]
fn server_chrome_exec_precedes_argument_parsing_and_preserves_native_arguments() {
    let target = std::fs::canonicalize("/bin/sh").unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(target.as_os_str().as_bytes());
    let output = Command::new(env!("CARGO_BIN_EXE_phoenix-ide"))
        .env("PHOENIX_INTERNAL_CHROME_EXEC", encoded)
        .args(["-c", r#"printf '%s\n' "$$"; ps -o pgid= -p "$$"; printf '%s\000' "${PHOENIX_INTERNAL_CHROME_EXEC-unset}" "$@""#, "shim", "space value", "--chrome-argument"])
        .arg(std::ffi::OsString::from_vec(vec![b'x', 0xff, b'y']))
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut lines = output.stdout.splitn(3, |byte| *byte == b'\n');
    let pid = std::str::from_utf8(lines.next().unwrap()).unwrap().trim();
    let group = std::str::from_utf8(lines.next().unwrap()).unwrap().trim();
    assert_eq!(pid, group);
    assert_eq!(
        lines.next().unwrap(),
        b"unset\0space value\0--chrome-argument\0x\xffy\0"
    );
}

#[test]
fn server_malformed_chrome_exec_cannot_start_the_application() {
    let output = Command::new(env!("CARGO_BIN_EXE_phoenix-ide"))
        .env("PHOENIX_INTERNAL_CHROME_EXEC", "%%%")
        .arg("--help")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}
