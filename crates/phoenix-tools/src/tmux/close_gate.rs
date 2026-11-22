//! Bounded same-server approval gate for exact tmux retirement.
//!
//! The helper is deliberately a private protocol entry point: it never starts
//! the application server and fails closed on every malformed/unsupported input.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use super::backend::{TokenBoundKill, SERVER_TOKEN_VAR};
use super::probe::command_output;
use tokio::time::Instant;

use phoenix_core::process_identity::{
    current_process_identity, process_identity_matches, ProcessIdentity,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

const HELPER_ARG: &str = "--phoenix-tmux-close-gate-v1";
const MAX_FRAME: usize = 256;
const PROTOCOL_MARKER: &[u8] = b"PHOENIX_TMUX_CLOSE_GATE_V1";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerReport {
    nonce: String,
    pid: u32,
    token: String,
}

/// Parse exactly `nonce pid token`, bounded and canonical. Tokens are UUID-shaped so
/// they cannot become tmux format-language fragments when used in a condition.
fn parse_server_report(bytes: &[u8]) -> Option<ServerReport> {
    if bytes.len() > MAX_FRAME
        || bytes.contains(&b'\r')
        || bytes[..bytes.len().saturating_sub(1)].contains(&b'\n')
    {
        return None;
    }
    let raw = std::str::from_utf8(bytes).ok()?;
    let text = raw.strip_suffix('\n').unwrap_or(raw);
    let mut fields = text.split(' ');
    let nonce = fields.next()?;
    let pid_text = fields.next()?;
    let token = fields.next()?;
    if fields.next().is_some()
        || !canonical_nonce(nonce)
        || pid_text.is_empty()
        || !pid_text.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let pid = pid_text.parse::<u32>().ok().filter(|pid| *pid > 0)?;
    if !canonical_uuid(token) {
        return None;
    }
    Some(ServerReport {
        nonce: nonce.to_owned(),
        pid,
        token: token.to_owned(),
    })
}

fn canonical_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
            }
        })
}

/// `CLOCK_MONOTONIC_RAW` is shared across Phoenix and its exec'd helper on
/// Linux/macOS. No wall-clock fallback is permitted for destructive approval.
fn monotonic_raw_ns() -> Option<u64> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    unsafe {
        let mut ts = std::mem::MaybeUninit::<libc::timespec>::uninit();
        if libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, ts.as_mut_ptr()) != 0 {
            return None;
        }
        let ts = ts.assume_init();
        if ts.tv_sec < 0 || !(0..1_000_000_000).contains(&ts.tv_nsec) {
            return None;
        }
        let n = (ts.tv_sec as u128)
            .checked_mul(1_000_000_000)?
            .checked_add(ts.tv_nsec as u128)?;
        u64::try_from(n).ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Recognized helper argv are consumed before normal application startup.
/// Returning `None` means this is not the helper command; malformed recognized
/// invocations return `Some(Err)` so they can never fall through to the server.
pub fn dispatch_if_helper(args: &[std::ffi::OsString]) -> Option<Result<(), String>> {
    if args.first().map(std::ffi::OsString::as_os_str) != Some(std::ffi::OsStr::new(HELPER_ARG)) {
        return None;
    }
    Some(run_helper_args(args).map_err(str::to_owned))
}

fn run_helper_args(args: &[std::ffi::OsString]) -> Result<(), &'static str> {
    let args = args
        .iter()
        .map(|arg| arg.to_str().ok_or("non-Unicode close-gate helper argument"))
        .collect::<Result<Vec<_>, _>>()?;
    if args.len() == 2 && args[1] == "--probe" {
        println!("PHOENIX_TMUX_CLOSE_GATE_PROBE_V1");
        return Ok(());
    }
    if args.len() != 5 {
        return Err("invalid close-gate helper arguments");
    }
    let endpoint = args[1];
    let nonce = args[2];
    let deadline_ns = args[3]
        .parse::<u64>()
        .map_err(|_| "invalid close-gate deadline")?;
    let pid = args[4]
        .parse::<u32>()
        .ok()
        .filter(|p| *p > 0)
        .ok_or("invalid close-gate pid")?;
    if endpoint.len() > 96 || !canonical_nonce(nonce) {
        return Err("invalid close-gate endpoint or nonce");
    }

    // Sample the shared raw clock before creating Tokio. Keep this absolute
    // expiry: runtime initialization must not renew time consumed by setup.
    let raw_now = monotonic_raw_ns().ok_or("monotonic clock unavailable")?;
    let remaining = deadline_ns
        .checked_sub(raw_now)
        .filter(|n| *n > 0)
        .ok_or("close-gate deadline elapsed")?;
    let expires = tokio::time::Instant::now()
        .checked_add(Duration::from_nanos(remaining))
        .ok_or("invalid close-gate deadline")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| "cannot create bounded helper runtime")?;
    if monotonic_raw_ns().is_none_or(|now| now >= deadline_ns)
        || tokio::time::Instant::now() >= expires
    {
        return Err("close-gate deadline elapsed");
    }
    runtime.block_on(helper_protocol(endpoint, nonce, pid, deadline_ns, expires))
}

async fn helper_protocol(
    endpoint: &str,
    nonce: &str,
    pid: u32,
    raw_deadline_ns: u64,
    expires: tokio::time::Instant,
) -> Result<(), &'static str> {
    tokio::time::timeout_at(expires, async {
        let mut stream = UnixStream::connect(endpoint)
            .await
            .map_err(|_| "close-gate unavailable")?;
        stream
            .write_all(PROTOCOL_MARKER)
            .await
            .map_err(|_| "close-gate write failed")?;
        stream
            .write_all(b" ")
            .await
            .map_err(|_| "close-gate write failed")?;
        stream
            .write_all(nonce.as_bytes())
            .await
            .map_err(|_| "close-gate write failed")?;
        stream
            .write_all(b" ")
            .await
            .map_err(|_| "close-gate write failed")?;
        stream
            .write_all(pid.to_string().as_bytes())
            .await
            .map_err(|_| "close-gate write failed")?;
        stream
            .write_all(b"\n")
            .await
            .map_err(|_| "close-gate write failed")?;
        let mut reply = [0u8; 8];
        stream
            .read_exact(&mut reply)
            .await
            .map_err(|_| "close-gate reply failed")?;
        if &reply != b"APPROVE\n" {
            return Err("close-gate approval rejected");
        }
        if monotonic_raw_ns().is_none_or(|now| now >= raw_deadline_ns) {
            return Err("close-gate deadline elapsed");
        }
        Ok(())
    })
    .await
    .map_err(|_| "close-gate deadline elapsed")?
}

/// Verify the report still names the captured exact incarnation at the final
/// affirmative boundary. Both PID and process birth are required.
fn report_matches_incarnation(report: &ServerReport, expected: ProcessIdentity) -> bool {
    report.pid == expected.pid
        && current_process_identity(report.pid)
            .is_some_and(|observed| observed == expected && process_identity_matches(expected))
}

/// Bind one private listener and approve only a nonce/PID-matched helper request.
/// The caller must verify the tmux report and process incarnation before calling this.
async fn approve_helper(
    listener: &UnixListener,
    nonce: &str,
    pid: u32,
    expires: tokio::time::Instant,
    raw_deadline_ns: u64,
    expected_process: ProcessIdentity,
    report: &ServerReport,
) -> Result<(), &'static str> {
    approve_helper_with_observer(
        listener,
        nonce,
        pid,
        expires,
        raw_deadline_ns,
        expected_process,
        report,
        |expected| report_matches_incarnation(report, expected),
    )
    .await
}

async fn approve_helper_with_observer<F>(
    listener: &UnixListener,
    nonce: &str,
    pid: u32,
    expires: tokio::time::Instant,
    raw_deadline_ns: u64,
    expected_process: ProcessIdentity,
    report: &ServerReport,
    observe: F,
) -> Result<(), &'static str>
where
    F: FnOnce(ProcessIdentity) -> bool,
{
    tokio::time::timeout_at(expires, async {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|_| "close-gate accept failed")?;
        approve_request_on_stream(
            stream,
            nonce,
            pid,
            expires,
            raw_deadline_ns,
            expected_process,
            report,
            observe,
        )
        .await
    })
    .await
    .map_err(|_| "close-gate deadline elapsed")?
}

async fn approve_request_on_stream<F>(
    mut stream: UnixStream,
    nonce: &str,
    pid: u32,
    expires: tokio::time::Instant,
    raw_deadline_ns: u64,
    expected_process: ProcessIdentity,
    report: &ServerReport,
    observe: F,
) -> Result<(), &'static str>
where
    F: FnOnce(ProcessIdentity) -> bool,
{
    let mut request = [0u8; MAX_FRAME];
    let mut used = 0usize;
    loop {
        if used == request.len() {
            return Err("close-gate request too large");
        }
        let n = stream
            .read(&mut request[used..])
            .await
            .map_err(|_| "close-gate request failed")?;
        if n == 0 {
            return Err("close-gate request truncated");
        }
        used += n;
        if request[..used].contains(&b'\n') {
            break;
        }
    }
    let expected_request = format!(
        "{} {} {}\n",
        std::str::from_utf8(PROTOCOL_MARKER).unwrap(),
        nonce,
        pid
    );
    if &request[..used] != expected_request.as_bytes() {
        return Err("close-gate request rejected");
    }
    if tokio::time::Instant::now() >= expires
        || monotonic_raw_ns().is_none_or(|now| now >= raw_deadline_ns)
    {
        return Err("close-gate deadline elapsed");
    }
    if report.nonce != nonce || report.pid != pid || !observe(expected_process) {
        return Err("close-gate process incarnation changed");
    }
    if tokio::time::Instant::now() >= expires
        || monotonic_raw_ns().is_none_or(|now| now >= raw_deadline_ns)
    {
        return Err("close-gate deadline elapsed");
    }
    stream
        .write_all(b"APPROVE\n")
        .await
        .map_err(|_| "close-gate approval write failed")?;
    Ok(())
}

/// Retire only the same tmux server incarnation after its own helper approves.
pub async fn retire_exact_on_server(
    socket_path: &Path,
    config_path: &Path,
    expected_token: &str,
    expected_process: ProcessIdentity,
    expires: Instant,
    helper_executable: &Path,
) -> TokenBoundKill {
    if Instant::now() >= expires {
        return TokenBoundKill::DeadlineExceeded;
    }
    if expected_token.len() != 36
        || !expected_token.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
            }
        })
        || !valid_embed_path(socket_path)
        || !valid_embed_path(config_path)
        || !valid_embed_path(helper_executable)
    {
        return TokenBoundKill::Failed {
            reason: "invalid exact-retirement token or path".into(),
        };
    }
    let exe = helper_executable;
    let mut probe = tokio::process::Command::new(exe);
    probe
        .args(["--phoenix-tmux-close-gate-v1", "--probe"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    match command_output(probe, Some(expires)).await {
        Ok(Some(o))
            if o.status.success()
                && o.stdout == b"PHOENIX_TMUX_CLOSE_GATE_PROBE_V1\n"
                && o.stderr.is_empty() =>
        {
            ()
        }
        Ok(None) => return TokenBoundKill::DeadlineExceeded,
        _ => {
            return TokenBoundKill::Failed {
                reason: "current executable does not support close-gate helper".into(),
            }
        }
    }
    use std::os::unix::fs::PermissionsExt;
    let dir = match tempfile::Builder::new()
        .prefix("phx-close-")
        .tempdir_in("/tmp")
    {
        Ok(d) => d,
        Err(e) => {
            return TokenBoundKill::Failed {
                reason: format!("cannot create close-gate directory: {e}"),
            }
        }
    };
    if let Err(e) = std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)) {
        return TokenBoundKill::Failed {
            reason: e.to_string(),
        };
    }
    let endpoint = dir.path().join("g.sock");
    let Some(endpoint_text) = endpoint.to_str().filter(|_| valid_embed_path(&endpoint)) else {
        return TokenBoundKill::Failed {
            reason: "invalid close-gate endpoint".into(),
        };
    };
    let Some(exe_text) = exe.to_str() else {
        return TokenBoundKill::Failed {
            reason: "invalid helper executable path".into(),
        };
    };
    let listener = match tokio::net::UnixListener::bind(&endpoint) {
        Ok(l) => l,
        Err(e) => {
            return TokenBoundKill::Failed {
                reason: e.to_string(),
            }
        }
    };
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let raw_now = match monotonic_raw_ns() {
        Some(n) => n,
        None => {
            return TokenBoundKill::Failed {
                reason: "close-gate deadline unavailable".into(),
            }
        }
    };
    let remain: u64 = match expires
        .saturating_duration_since(Instant::now())
        .as_nanos()
        .try_into()
    {
        Ok(n) if n > 0 => n,
        _ => return TokenBoundKill::DeadlineExceeded,
    };
    let deadline_ns = match raw_now.checked_add(remain) {
        Some(n) => n,
        None => {
            return TokenBoundKill::Failed {
                reason: "close-gate deadline unavailable".into(),
            }
        }
    };
    let pid = expected_process.pid;
    let pid_condition = format!("#{{==:#{{pid}},{pid}}}");
    let token_condition = format!("#{{==:#{{E:{SERVER_TOKEN_VAR}}},{expected_token}}}");
    let condition = format!("#{{&&:{pid_condition},{token_condition}}}");
    let helper = format!(
        "{} --phoenix-tmux-close-gate-v1 {} {} {} {}",
        shell_quote(exe_text),
        shell_quote(endpoint_text),
        nonce,
        deadline_ns,
        pid
    );
    let marker = "display-message -p PHOENIX_TOKEN_MISMATCH";
    let queue = close_gate_queue(&helper, &condition, marker, &shell_quote);
    let report_cmd = format!("{nonce} #{{pid}} #{{E:{SERVER_TOKEN_VAR}}}");
    let mut command = tokio::process::Command::new("tmux");
    command
        .args(["-f"])
        .arg(config_path)
        .args(["-S"])
        .arg(socket_path)
        .args([
            "display-message",
            "-p",
            &report_cmd,
            ";",
            "if-shell",
            "-F",
            &condition,
            &queue,
            marker,
        ])
        .env_remove("TMUX")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return TokenBoundKill::Failed {
                reason: format!("cannot launch close queue: {e}"),
            }
        }
    };
    let Some(stdout) = child.stdout.take() else {
        return TokenBoundKill::Failed {
            reason: "tmux stdout unavailable".into(),
        };
    };
    let Some(stderr) = child.stderr.take() else {
        return TokenBoundKill::Failed {
            reason: "tmux stderr unavailable".into(),
        };
    };
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Vec<u8>, String>>();
    let approval = async {
        let bytes = rx
            .await
            .map_err(|_| "tmux report collector stopped".to_string())?
            .map_err(|e| e)?;
        let report = parse_server_report(&bytes)
            .ok_or_else(|| "malformed same-server report".to_string())?;
        if report.nonce != nonce || report.pid != pid || report.token != expected_token {
            return Err("same-server report mismatch".into());
        }
        approve_helper(
            &listener,
            &nonce,
            pid,
            expires,
            deadline_ns,
            expected_process,
            &report,
        )
        .await
        .map_err(str::to_string)?;
        Ok::<_, String>(report)
    };
    let child_wait = async { child.wait().await.map_err(|e| e.to_string()) };
    let stdout_drain = async {
        drain_stdout(stdout, Some(tx))
            .await
            .map_err(|e| e.to_string())
    };
    let stderr_drain = async { drain_capped(stderr, 8192).await.map_err(|e| e.to_string()) };
    let operation = async { tokio::try_join!(approval, child_wait, stdout_drain, stderr_drain) };
    let result = match tokio::time::timeout_at(expires, operation).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            return if Instant::now() >= expires {
                TokenBoundKill::DeadlineExceeded
            } else {
                TokenBoundKill::Failed {
                    reason: format!("close queue failed: {e}"),
                }
            }
        }
        Err(_) => return TokenBoundKill::DeadlineExceeded,
    };
    let (_report, status, marker_found, stderr) = result;
    if marker_found {
        return TokenBoundKill::TokenMismatch;
    }
    if !status.success() {
        return TokenBoundKill::Failed {
            reason: format!(
                "same-server close queue failed: {}",
                String::from_utf8_lossy(&stderr)
            ),
        };
    }
    TokenBoundKill::Killed
}

fn valid_embed_path(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|value| !value.chars().any(|c| matches!(c, '#' | '\0' | '\n' | '\r')))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn close_gate_queue(
    helper: &str,
    condition: &str,
    marker: &str,
    quote: &impl Fn(&str) -> String,
) -> String {
    let guarded_kill = format!(
        "if-shell -F {} kill-server {}",
        quote(condition),
        quote(marker)
    );
    format!(
        "if-shell {} {} {}",
        quote(helper),
        quote(&guarded_kill),
        quote(marker)
    )
}

async fn drain_stdout<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    mut report_tx: Option<tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>>,
) -> std::io::Result<bool> {
    use tokio::io::AsyncReadExt;
    const MARKER: &[u8] = b"PHOENIX_TOKEN_MISMATCH";
    let mut report = Vec::new();
    let mut tail = Vec::new();
    let mut marker_found = false;
    let mut chunk = [0u8; 1024];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            if let Some(tx) = report_tx.take() {
                let _ = tx.send(Err("tmux report ended before newline".into()));
            }
            return Ok(marker_found);
        }
        let bytes = &chunk[..n];
        if report_tx.is_some() {
            if let Some(pos) = bytes.iter().position(|b| *b == b'\n') {
                if report.len() + pos + 1 > 256 {
                    let _ = report_tx
                        .take()
                        .unwrap()
                        .send(Err("tmux report too large".into()));
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "tmux report too large",
                    ));
                }
                report.extend_from_slice(&bytes[..=pos]);
                let _ = report_tx
                    .take()
                    .unwrap()
                    .send(Ok(std::mem::take(&mut report)));
                tail.extend_from_slice(&bytes[pos + 1..]);
            } else {
                if report.len() + n > 256 {
                    let _ = report_tx
                        .take()
                        .unwrap()
                        .send(Err("tmux report too large".into()));
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "tmux report too large",
                    ));
                }
                report.extend_from_slice(bytes);
            }
        } else {
            tail.extend_from_slice(bytes);
        }
        if tail.windows(MARKER.len()).any(|w| w == MARKER) {
            marker_found = true;
        }
        if tail.len() > MARKER.len() {
            tail.drain(..tail.len() - MARKER.len());
        }
    }
}

async fn drain_capped<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    cap: usize,
) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut result = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            return Ok(result);
        }
        let take = n.min(cap.saturating_sub(result.len()));
        result.extend_from_slice(&chunk[..take]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_paths_reject_tmux_format_syntax_and_controls() {
        for value in [
            "/tmp/#(command)",
            "/tmp/#S",
            "/tmp/##",
            "/tmp/#{}",
            "/tmp/line\nfeed",
            "/tmp/carriage\rreturn",
            "/tmp/null\0byte",
        ] {
            assert!(!valid_embed_path(Path::new(value)), "accepted {value:?}");
        }
        assert!(valid_embed_path(Path::new(
            "/tmp/path with spaces/it's okay"
        )));
    }

    #[test]
    fn shell_quote_preserves_spaces_and_apostrophes() {
        assert_eq!(
            shell_quote("/tmp/a path/it's okay"),
            "'/tmp/a path/it'\\''s okay'"
        );
    }

    #[test]
    fn close_gate_queue_only_kills_after_helper_and_exact_guard_succeed() {
        let queue = close_gate_queue(
            "/tmp/helper path",
            "#{==:#{pid},42}",
            "display-message PHOENIX_TOKEN_MISMATCH",
            &shell_quote,
        );
        assert!(queue.starts_with("if-shell '/tmp/helper path' 'if-shell -F "));
        assert!(queue.contains("kill-server"));
        assert!(queue.contains("PHOENIX_TOKEN_MISMATCH"));
        assert!(queue.ends_with(" 'display-message PHOENIX_TOKEN_MISMATCH'"));
        assert!(!queue.contains("run-shell"));
        assert!(!queue.contains("-b"));
        assert!(!queue.contains("; kill-server"));
    }

    #[tokio::test]
    async fn private_gate_approves_only_the_expected_request_and_fails_closed_on_eof() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let path = dir.path().join("gate.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let expires = tokio::time::Instant::now() + Duration::from_secs(2);
        let expected = ProcessIdentity {
            pid: 42,
            start_time: 9,
        };
        let report = ServerReport {
            nonce: "0123456789abcdef0123456789abcdef".into(),
            pid: 42,
            token: "01234567-89ab-cdef-0123-456789abcdef".into(),
        };
        let client_path = path.clone();
        let client = tokio::spawn(async move {
            let mut stream = UnixStream::connect(client_path).await.unwrap();
            stream
                .write_all(b"PHOENIX_TMUX_CLOSE_GATE_V1 0123456789abcdef0123456789abcdef 42\n")
                .await
                .unwrap();
            let mut reply = [0; 8];
            stream.read_exact(&mut reply).await.unwrap();
            reply
        });
        assert_eq!(
            approve_helper_with_observer(
                &listener,
                "0123456789abcdef0123456789abcdef",
                42,
                expires,
                u64::MAX,
                expected,
                &report,
                |_| true
            )
            .await,
            Ok(())
        );
        assert_eq!(client.await.unwrap(), *b"APPROVE\n");

        let path = dir.path().join("eof.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let client_path = path.clone();
        let client = tokio::spawn(async move {
            let stream = UnixStream::connect(client_path).await.unwrap();
            drop(stream);
        });
        let result = approve_helper_with_observer(
            &listener,
            "0123456789abcdef0123456789abcdef",
            42,
            expires,
            u64::MAX,
            expected,
            &report,
            |_| true,
        )
        .await;
        assert_eq!(result, Err("close-gate request truncated"));
        client.await.unwrap();

        let expired = tokio::time::Instant::now() - Duration::from_millis(1);
        assert_eq!(
            approve_helper_with_observer(
                &listener,
                "0123456789abcdef0123456789abcdef",
                42,
                expired,
                u64::MAX,
                expected,
                &report,
                |_| true
            )
            .await,
            Err("close-gate deadline elapsed")
        );
    }

    #[tokio::test]
    async fn canceled_request_drops_the_owned_writable_stream() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let path = dir.path().join("cancel.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let peer = UnixStream::connect(&path).await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let client = tokio::spawn(async move {
            let mut peer = peer;
            let mut reply = Vec::new();
            peer.read_to_end(&mut reply).await.unwrap();
            reply
        });
        let expected = ProcessIdentity {
            pid: 42,
            start_time: 9,
        };
        let report = ServerReport {
            nonce: "0123456789abcdef0123456789abcdef".into(),
            pid: 42,
            token: String::new(),
        };
        let expires = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut approval = Box::pin(approve_request_on_stream(
            stream,
            "0123456789abcdef0123456789abcdef",
            42,
            expires,
            u64::MAX,
            expected,
            &report,
            |_| true,
        ));
        assert!(futures::poll!(&mut approval).is_pending());
        drop(approval);
        assert!(tokio::time::timeout(Duration::from_secs(5), client)
            .await
            .unwrap()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn wrong_nonce_and_changed_process_are_denied_before_reply() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let nonce = "0123456789abcdef0123456789abcdef";
        let expected = ProcessIdentity {
            pid: 42,
            start_time: 9,
        };
        let report = ServerReport {
            nonce: nonce.into(),
            pid: 42,
            token: "01234567-89ab-cdef-0123-456789abcdef".into(),
        };
        let expires = tokio::time::Instant::now() + Duration::from_secs(2);
        for (name, request, observer_accepts, expected_error) in [
            (
                "nonce.sock",
                b"PHOENIX_TMUX_CLOSE_GATE_V1 ffffffffffffffffffffffffffffffff 42\n".as_slice(),
                true,
                "close-gate request rejected",
            ),
            (
                "identity.sock",
                b"PHOENIX_TMUX_CLOSE_GATE_V1 0123456789abcdef0123456789abcdef 42\n".as_slice(),
                false,
                "close-gate process incarnation changed",
            ),
        ] {
            let path = dir.path().join(name);
            let listener = UnixListener::bind(&path).unwrap();
            let client_path = path.clone();
            let client = tokio::spawn(async move {
                let mut stream = UnixStream::connect(client_path).await.unwrap();
                stream.write_all(request).await.unwrap();
                let mut reply = Vec::new();
                stream.read_to_end(&mut reply).await.unwrap();
                reply
            });
            let result = approve_helper_with_observer(
                &listener,
                nonce,
                42,
                expires,
                u64::MAX,
                expected,
                &report,
                move |_| observer_accepts,
            )
            .await;
            assert_eq!(result, Err(expected_error));
            assert!(client.await.unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn helper_protocol_rejects_wrong_approval_truncated_eof_and_expired_raw_deadline() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let nonce = "0123456789abcdef0123456789abcdef";
        for (name, reply, raw_deadline, expected) in [
            (
                "wrong.sock",
                Some(b"DENIED!\n".as_slice()),
                u64::MAX,
                Err("close-gate approval rejected"),
            ),
            ("eof.sock", None, u64::MAX, Err("close-gate reply failed")),
            (
                "expired.sock",
                Some(b"APPROVE\n".as_slice()),
                0,
                Err("close-gate deadline elapsed"),
            ),
        ] {
            let path = dir.path().join(name);
            let listener = UnixListener::bind(&path).unwrap();
            let peer_path = path.clone();
            let peer = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut byte = [0u8; 1];
                    if stream.read(&mut byte).await.unwrap() == 0 {
                        break;
                    }
                    request.push(byte[0]);
                    if byte[0] == b'\n' {
                        break;
                    }
                }
                if let Some(reply) = reply {
                    stream.write_all(reply).await.unwrap();
                }
            });
            let expires = tokio::time::Instant::now() + Duration::from_secs(2);
            let result = helper_protocol(
                &peer_path.to_string_lossy(),
                nonce,
                42,
                raw_deadline,
                expires,
            )
            .await;
            assert_eq!(result, expected);
            peer.await.unwrap();
        }
    }

    #[test]
    fn report_parser_is_bounded_and_canonical() {
        let token = "01234567-89ab-cdef-0123-456789abcdef";
        let nonce = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            parse_server_report(format!("{nonce} 42 {token}\n").as_bytes()),
            Some(ServerReport {
                nonce: nonce.into(),
                pid: 42,
                token: token.into()
            })
        );
        assert!(parse_server_report(format!("{nonce} 42 {token} extra\n").as_bytes()).is_none());
        assert!(parse_server_report(format!("{nonce} 42 {token}\n\n").as_bytes()).is_none());
        assert!(parse_server_report(&vec![b'x'; MAX_FRAME + 1]).is_none());
        assert!(parse_server_report(
            b"0123456789abcdef0123456789abcdef 0 01234567-89ab-cdef-0123-456789abcdef"
        )
        .is_none());
        assert!(parse_server_report(
            b"0123456789abcdef0123456789abcdef 42 #(){;}:,01234567-89ab-cdef-0123-456789abcd"
        )
        .is_none());
    }
}
