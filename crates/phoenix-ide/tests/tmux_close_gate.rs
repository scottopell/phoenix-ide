#![cfg(unix)]

use std::os::unix::ffi::OsStringExt as _;

const HELPER: &str = "--phoenix-tmux-close-gate-v1";

#[tokio::test]
async fn helper_probe_emits_exact_protocol_marker_and_exits_before_startup() {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_phoenix_ide"));
    command.args([HELPER, "--probe"]).kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .expect("helper probe deadline")
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"PHOENIX_TMUX_CLOSE_GATE_PROBE_V1\n");
    assert!(output.stderr.is_empty());
}

#[tokio::test]
async fn malformed_recognized_helper_arguments_exit_125_without_falling_through() {
    for args in [
        vec![HELPER.into(), "--probe".into(), "extra".into()],
        vec![
            HELPER.into(),
            "endpoint".into(),
            "nonce".into(),
            "1".into(),
            "42".into(),
            "extra".into(),
        ],
        vec![
            HELPER.into(),
            "--probe".into(),
            std::ffi::OsString::from_vec(vec![b'x', 0xff]),
        ],
    ] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_phoenix_ide"));
        command
            .env("PHOENIX_INTERNAL_CHROME_EXEC", "%%%")
            .args(args)
            .kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
            .await
            .expect("malformed helper deadline")
            .unwrap();
        assert_eq!(output.status.code(), Some(125));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("tmux close gate rejected"));
    }
}

// These tests create only unique disposable servers. Missing tmux is a failure,
// not a silent skip; the exact-source hosted job installs and records its version.
mod disposable {
    use super::*;
    use phoenix_core::process_identity::{current_process_identity, ProcessIdentity};
    use phoenix_tools::tmux::{backend::TokenBoundKill, close_gate::retire_exact_on_server};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    const TOKEN_VAR: &str = "PHOENIX_TMUX_SERVER_TOKEN";

    async fn command(socket: &Path, config: &Path, args: &[&str]) -> std::process::Output {
        let mut command = tokio::process::Command::new("tmux");
        command
            .arg("-f")
            .arg(config)
            .arg("-S")
            .arg(socket)
            .args(args)
            .env_remove("TMUX")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .expect("disposable tmux command deadline")
            .expect("hosted tmux must be installed");
        assert!(
            output.status.success(),
            "tmux: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        socket: PathBuf,
        config: PathBuf,
        token: String,
        process: ProcessIdentity,
    }

    impl Fixture {
        async fn new() -> Self {
            Self::with_token(uuid::Uuid::new_v4().to_string()).await
        }

        async fn with_token(token: String) -> Self {
            let directory = tempfile::Builder::new()
                .prefix("phx-gate-test-")
                .tempdir_in("/tmp")
                .unwrap();
            let socket = directory.path().join("s");
            let config = directory.path().join("c");
            std::fs::write(&config, b"").unwrap();
            let output = command(
                &socket,
                &config,
                &[
                    "new-session",
                    "-d",
                    "-P",
                    "-F",
                    "#{pid}",
                    "-s",
                    "gate",
                    ";",
                    "set-environment",
                    "-g",
                    TOKEN_VAR,
                    &token,
                ],
            )
            .await;
            let pid = String::from_utf8(output.stdout)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let process = current_process_identity(pid).expect("fresh disposable server identity");
            Self {
                _directory: directory,
                socket,
                config,
                token,
                process,
            }
        }

        async fn retire(
            &self,
            expected: ProcessIdentity,
            token: &str,
            helper: &Path,
        ) -> TokenBoundKill {
            retire_exact_on_server(
                &self.socket,
                &self.config,
                token,
                expected,
                tokio::time::Instant::now() + Duration::from_secs(10),
                helper,
            )
            .await
        }

        fn assert_original_running(&self) {
            assert_eq!(
                current_process_identity(self.process.pid),
                Some(self.process)
            );
        }

        async fn cleanup(&self) {
            let outcome = self
                .retire(
                    self.process,
                    &self.token,
                    Path::new(env!("CARGO_BIN_EXE_phoenix_ide")),
                )
                .await;
            assert_eq!(
                outcome,
                TokenBoundKill::Killed,
                "explicit fixture cleanup outcome"
            );
            tokio::time::timeout(Duration::from_secs(5), async {
                while current_process_identity(self.process.pid) == Some(self.process) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("explicit fixture cleanup process exit");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if current_process_identity(self.process.pid) != Some(self.process) {
                return;
            }
            // Failure-path cleanup uses only the production exact gate. Report
            // failures without panicking or falling back to PID/path authority.
            let socket = self.socket.clone();
            let config = self.config.clone();
            let token = self.token.clone();
            let process = self.process;
            let worker = std::thread::Builder::new().spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => return Err(format!("runtime creation failed: {error}")),
                };
                let outcome = runtime.block_on(retire_exact_on_server(
                    &socket,
                    &config,
                    &token,
                    process,
                    tokio::time::Instant::now() + Duration::from_secs(3),
                    Path::new(env!("CARGO_BIN_EXE_phoenix_ide")),
                ));
                if outcome == TokenBoundKill::Killed {
                    Ok(())
                } else {
                    Err(format!("exact cleanup returned {outcome:?}"))
                }
            });
            match worker {
                Ok(worker) => match worker.join() {
                    Ok(Ok(())) => (),
                    Ok(Err(error)) => eprintln!("disposable tmux fixture cleanup failed: {error}"),
                    Err(_) => eprintln!("disposable tmux fixture cleanup worker panicked"),
                },
                Err(error) => eprintln!("could not start disposable tmux cleanup worker: {error}"),
            }
        }
    }

    #[cfg(target_os = "linux")]
    struct PausedHelper {
        _directory: tempfile::TempDir,
        executable: PathBuf,
        ready: PathBuf,
        fifo: PathBuf,
        status: PathBuf,
    }

    #[cfg(target_os = "linux")]
    impl PausedHelper {
        fn new() -> Self {
            use std::os::unix::fs::PermissionsExt;
            let directory = tempfile::Builder::new()
                .prefix("phx-gate-pause-")
                .tempdir_in("/tmp")
                .unwrap();
            let executable = directory.path().join("helper's name");
            let ready = directory.path().join("ready");
            let fifo = directory.path().join("release");
            let status = directory.path().join("status");
            let fifo_c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
            let quote =
                |path: &Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"));
            let status_pending = directory.path().join("status-pending");
            let source = format!("#!/bin/sh\nif [ \"$2\" = --probe ]; then printf 'PHOENIX_TMUX_CLOSE_GATE_PROBE_V1\\n'; exit 0; fi\n: > {}\n/usr/bin/timeout 15 /bin/sh -c 'read line < \"$1\"' sh {} || exit 124\n{} \"$@\"\nstatus=$?\nprintf '%s\\n' \"$status\" > {}\nmv {} {}\nexit \"$status\"\n",
                quote(&ready), quote(&fifo), quote(Path::new(env!("CARGO_BIN_EXE_phoenix_ide"))), quote(&status_pending), quote(&status_pending), quote(&status));
            std::fs::write(&executable, source).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                _directory: directory,
                executable,
                ready,
                fifo,
                status,
            }
        }

        async fn wait_ready(&self) {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !self.ready.exists() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("same-server queue reached the paused helper");
        }

        async fn wait_exit(&self, expected: i32) {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Ok(value) = std::fs::read_to_string(&self.status) {
                        assert_eq!(value.trim().parse::<i32>().unwrap(), expected);
                        // Published by atomic rename only after the actual helper
                        // returned; timeout paths never publish this receipt.
                        return;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("paused helper actual exit status");
        }

        async fn release(&self) {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match std::fs::OpenOptions::new()
                        .write(true)
                        .custom_flags(libc::O_NONBLOCK)
                        .open(&self.fifo)
                    {
                        Ok(mut file) => {
                            file.write_all(b"release\n").unwrap();
                            break;
                        }
                        Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                            tokio::task::yield_now().await
                        }
                        Err(error) => panic!("private fixture release failed: {error}"),
                    }
                }
            })
            .await
            .expect("bounded private fixture release");
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn same_token_socket_rebinding_never_kills_the_replacement_server() {
        let original = Fixture::new().await;
        let replacement = Fixture::with_token(original.token.clone()).await;
        let alias = original._directory.path().join("alias");
        std::os::unix::fs::symlink(&original.socket, &alias).unwrap();
        let helper = PausedHelper::new();
        let operation = retire_exact_on_server(
            &alias,
            &original.config,
            &original.token,
            original.process,
            tokio::time::Instant::now() + Duration::from_secs(10),
            &helper.executable,
        );
        let witness = async {
            helper.wait_ready().await;
            std::fs::remove_file(&alias).unwrap();
            std::os::unix::fs::symlink(&replacement.socket, &alias).unwrap();
            helper.release().await;
        };
        let (outcome, ()) = tokio::join!(operation, witness);
        assert_eq!(outcome, TokenBoundKill::Killed);
        helper.wait_exit(0).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while current_process_identity(original.process.pid) == Some(original.process) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("original server exit after socket rebind");
        replacement.assert_original_running();
        replacement.cleanup().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn token_change_while_helper_waits_preserves_the_original_server() {
        let mut fixture = Fixture::new().await;
        let helper = PausedHelper::new();
        let original_token = fixture.token.clone();
        let changed_token = uuid::Uuid::new_v4().to_string();
        let operation_socket = fixture.socket.clone();
        let operation_config = fixture.config.clone();
        let expected_token = original_token.clone();
        let operation_process = fixture.process;
        let operation_helper = helper.executable.clone();
        let operation = async move {
            retire_exact_on_server(
                &operation_socket,
                &operation_config,
                &expected_token,
                operation_process,
                tokio::time::Instant::now() + Duration::from_secs(10),
                &operation_helper,
            )
            .await
        };
        let witness = async {
            helper.wait_ready().await;
            command(
                &fixture.socket,
                &fixture.config,
                &["set-environment", "-g", TOKEN_VAR, &changed_token],
            )
            .await;
            // Align Drop's exact-gate token immediately after server mutation.
            fixture.token = changed_token.clone();
            helper.release().await;
        };
        let (outcome, ()) = tokio::join!(operation, witness);
        // The helper approves the original incarnation; the final server-side
        // token guard (not helper rejection) must suppress the destructive branch.
        helper.wait_exit(0).await;
        // Restore server and Drop authority before assertions, so failure-path
        // cleanup still has exact-token authority.
        command(
            &fixture.socket,
            &fixture.config,
            &["set-environment", "-g", TOKEN_VAR, &original_token],
        )
        .await;
        fixture.token = original_token;
        assert_eq!(outcome, TokenBoundKill::TokenMismatch);
        fixture.assert_original_running();
        fixture.cleanup().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancellation_before_helper_approval_never_kills_the_server() {
        let fixture = Fixture::new().await;
        let helper = PausedHelper::new();
        let mut operation =
            Box::pin(fixture.retire(fixture.process, &fixture.token, &helper.executable));
        tokio::select! {
            outcome = &mut operation => panic!("gate completed before causal helper pause: {outcome:?}"),
            () = helper.wait_ready() => (),
        }
        drop(operation);
        fixture.assert_original_running();
        // Wake the bounded shell job after its parent listener is gone: the
        // real helper must fail, not turn a disconnected parent into approval.
        helper.release().await;
        helper.wait_exit(125).await;
        fixture.assert_original_running();
        fixture.cleanup().await;
    }

    #[tokio::test]
    async fn exact_gate_kills_only_the_original_disposable_server() {
        let fixture = Fixture::new().await;
        let outcome = fixture
            .retire(
                fixture.process,
                &fixture.token,
                Path::new(env!("CARGO_BIN_EXE_phoenix_ide")),
            )
            .await;
        assert_eq!(outcome, TokenBoundKill::Killed);
        tokio::time::timeout(Duration::from_secs(5), async {
            while current_process_identity(fixture.process.pid) == Some(fixture.process) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("original process exit observation");
    }

    #[tokio::test]
    async fn exact_gate_rejects_birth_and_token_mismatch_without_killing() {
        let fixture = Fixture::new().await;
        let wrong_birth = ProcessIdentity {
            pid: fixture.process.pid,
            start_time: fixture.process.start_time.wrapping_add(1),
        };
        assert!(matches!(
            fixture
                .retire(
                    wrong_birth,
                    &fixture.token,
                    Path::new(env!("CARGO_BIN_EXE_phoenix_ide"))
                )
                .await,
            TokenBoundKill::Failed { .. }
        ));
        fixture.assert_original_running();
        let wrong_token = uuid::Uuid::new_v4().to_string();
        assert!(matches!(
            fixture
                .retire(
                    fixture.process,
                    &wrong_token,
                    Path::new(env!("CARGO_BIN_EXE_phoenix_ide"))
                )
                .await,
            TokenBoundKill::Failed { .. }
        ));
        fixture.assert_original_running();
        fixture.cleanup().await;
    }

    #[tokio::test]
    async fn exact_gate_rejects_unsupported_or_format_embedded_helper() {
        let fixture = Fixture::new().await;
        assert!(
            matches!(fixture.retire(fixture.process, &fixture.token, Path::new("/bin/true")).await, TokenBoundKill::Failed { reason } if reason.contains("does not support close-gate helper"))
        );
        fixture.assert_original_running();
        let injected = fixture._directory.path().join("#(false)");
        assert!(
            matches!(fixture.retire(fixture.process, &fixture.token, &injected).await, TokenBoundKill::Failed { reason } if reason == "invalid exact-retirement token or path")
        );
        fixture.assert_original_running();
        fixture.cleanup().await;
    }
}
