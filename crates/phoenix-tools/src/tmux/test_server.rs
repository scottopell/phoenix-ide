use std::fs;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use phoenix_core::process_identity::ProcessIdentity;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::probe::{probe_sync, ProbeResult};
use super::registry::TmuxRegistry;

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(8);

const WATCHDOG_PROGRAM: &str = r##"
import ctypes
import fcntl
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

root = Path(sys.argv[1])
parent = int(sys.argv[2])
control_root = Path(sys.argv[3])
owned = []
unconfirmed_obligations = []

class ProcBsdInfo(ctypes.Structure):
    _fields_ = [
        ("flags", ctypes.c_uint32), ("status", ctypes.c_uint32),
        ("xstatus", ctypes.c_uint32), ("pid", ctypes.c_uint32),
        ("ppid", ctypes.c_uint32), ("uid", ctypes.c_uint32),
        ("gid", ctypes.c_uint32), ("ruid", ctypes.c_uint32),
        ("rgid", ctypes.c_uint32), ("svuid", ctypes.c_uint32),
        ("svgid", ctypes.c_uint32), ("rfu_1", ctypes.c_uint32),
        ("comm", ctypes.c_char * 16), ("name", ctypes.c_char * 32),
        ("nfiles", ctypes.c_uint32), ("pgid", ctypes.c_uint32),
        ("pjobc", ctypes.c_uint32), ("e_tdev", ctypes.c_uint32),
        ("e_tpgid", ctypes.c_uint32), ("nice", ctypes.c_int32),
        ("start_tvsec", ctypes.c_uint64), ("start_tvusec", ctypes.c_uint64),
    ]

def process(pid):
    try:
        if sys.platform.startswith("linux"):
            stat = Path(f"/proc/{pid}/stat").read_text()
            fields = stat[stat.rfind(")") + 1:].split()
            environment = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
            return fields[19], fields[0] == "Z", environment
        info = ProcBsdInfo()
        size = ctypes.sizeof(info)
        if ctypes.CDLL("/usr/lib/libproc.dylib").proc_pidinfo(
            pid, 3, 0, ctypes.byref(info), size
        ) != size:
            return None
        result = subprocess.run(
            ["ps", "eww", "-p", str(pid), "-o", "command="],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            check=False,
            text=True,
            timeout=0.5,
        )
        birth = info.start_tvsec * 1_000_000 + info.start_tvusec
        return str(birth), info.status == 5, result.stdout.split()
    except (FileNotFoundError, IndexError, OSError, subprocess.TimeoutExpired):
        return None

def pid_exists(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True

def birth(pid):
    observed = process(pid)
    return None if observed is None else observed[0]

def identity_state(identity):
    pid, started, token = identity
    observed = process(pid)
    if observed is None:
        return "unverifiable" if pid_exists(pid) else "absent"
    if observed[0] != started:
        return "mismatched"
    if observed[1]:
        return "absent"
    if token is None:
        return "owned"
    token_bytes = f"PHOENIX_TMUX_SERVER_TOKEN={token}".encode()
    token_present = any(
        value == token_bytes or value == token_bytes.decode()
        for value in observed[2]
    )
    return "owned" if token_present else "mismatched"

def publish_response(path, value):
    pending = path.with_name(f".pending-{path.name}")
    pending.write_text(value)
    os.replace(pending, path)

def query_control_processes(control):
    observed = subprocess.run(
        ["tmux", "-S", str(control), "display-message", "-p", "#{pid}|#{pane_pid}"],
        stdin=subprocess.DEVNULL, capture_output=True, check=False, text=True, timeout=0.5,
    )
    if observed.returncode != 0:
        raise RuntimeError("tmux identity query failed")
    process_fields = observed.stdout.removesuffix("\n").split("|")
    if len(process_fields) != 2:
        raise RuntimeError("tmux identity output was malformed")
    server_pid, pane_pid = process_fields
    if (not server_pid.isascii() or not server_pid.isdecimal()
            or not pane_pid.isascii() or not pane_pid.isdecimal()):
        raise RuntimeError("tmux identity output was malformed")
    return int(server_pid), int(pane_pid)

def observe_control(control, expected_token):
    last_error = RuntimeError("tmux identity query did not run")
    for attempt in range(50):
        try:
            server_pid, pane_pid = query_control_processes(control)
            token_result = subprocess.run(
                ["tmux", "-S", str(control), "show-environment", "-g", "PHOENIX_TMUX_SERVER_TOKEN"],
                stdin=subprocess.DEVNULL, capture_output=True, check=False, text=True, timeout=0.5,
            )
            token = token_result.stdout.strip().partition("=")[2]
            if token_result.returncode != 0 or token != expected_token:
                raise RuntimeError("tmux server token did not match registration")
            identities = [
                (server_pid, birth(server_pid), token),
                (pane_pid, birth(pane_pid), None),
            ]
            if any(started is None for _, started, _ in identities):
                raise RuntimeError("process birth identity was unavailable")
            return identities
        except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
            last_error = error
            if attempt + 1 < 50:
                time.sleep(0.1)
    raise RuntimeError(f"tmux processes never became ready: {last_error}")

def spawn_owned(request):
    rejected = request.with_name(request.name.replace(".spawn-", ".rejected-", 1))
    acknowledged = request.with_name(request.name.replace(".spawn-", ".registered-", 1))
    control = None
    try:
        socket_name, control_name, config, cwd, token, env_path = request.read_text().split("\t")
        socket = root / socket_name
        control = control_root / control_name
        if socket.parent != root or control.parent != control_root or control.exists():
            raise RuntimeError("spawn paths are not unused exact children of owned roots")
        environment = dict(json.loads((control_root / env_path).read_text()))
        obligation = (socket, control)
        unconfirmed_obligations.append(obligation)
        spawned = subprocess.run(
            ["tmux", "-f", config, "-S", str(control), "new-session", "-d", "-c", cwd,
             "-s", "main", ";", "set-environment", "-g", "PHOENIX_TMUX_SERVER_TOKEN", token],
            stdin=subprocess.DEVNULL, capture_output=True, check=False, text=True, timeout=0.5,
            env=environment,
        )
        if spawned.returncode != 0:
            raise RuntimeError(f"tmux spawn failed: {spawned.stderr}")
        identities = observe_control(control, token)
        control_stat = control.stat()
        owned.append((socket, control_stat.st_dev, control_stat.st_ino, control, tuple(identities)))
        unconfirmed_obligations.remove(obligation)
        publish_response(acknowledged, "\t".join(
            str(value) for identity in identities for value in identity[:2]
        ))
    except (OSError, RuntimeError, subprocess.TimeoutExpired, ValueError, json.JSONDecodeError) as error:
        if control is not None and control.exists():
            try:
                subprocess.run(["tmux", "-S", str(control), "kill-server"], timeout=0.5)
            except (OSError, subprocess.TimeoutExpired):
                pass
        try:
            publish_response(rejected, str(error))
        except OSError:
            return False
    finally:
        try:
            request.unlink(missing_ok=True)
        except OSError:
            pass
    return True

def register(request):
    rejected = request.with_name(request.name.replace(".register-", ".rejected-", 1))
    acknowledged = request.with_name(request.name.replace(".register-", ".registered-", 1))
    try:
        fields = request.read_text().split("\t")
        socket_name, control_name, expected_token = fields
        socket = root / socket_name
        control = control_root / control_name
        if (socket.parent != root or control.parent != control_root
                or control.is_symlink() or not control.is_socket()):
            raise RuntimeError("registration control is not an exact child of the owned control root")
        identities = observe_control(control, expected_token)
        server_pid, pane_pid = str(identities[0][0]), str(identities[1][0])
        control_stat = control.stat()
        owned.append((socket, control_stat.st_dev, control_stat.st_ino, control, tuple(identities)))
        try:
            publish_response(acknowledged, "\t".join([
                server_pid, identities[0][1], pane_pid, identities[1][1]
            ]))
        except OSError:
            return False
    except (OSError, RuntimeError, subprocess.TimeoutExpired, ValueError) as error:
        try:
            publish_response(rejected, str(error))
        except OSError:
            return False
    finally:
        try:
            request.unlink(missing_ok=True)
        except OSError:
            pass
    return True

(root / ".armed").touch()
heartbeat = root / ".parent-heartbeat"
while not (root / ".cleanup-request").exists():
    for request in control_root.glob(".spawn-*"):
        if not spawn_owned(request):
            break
    else:
        request = None
    if request is not None:
        break
    for request in control_root.glob(".register-*"):
        if not register(request):
            break
    else:
        request = None
    if request is not None:
        break
    if not root.exists():
        break
    if parent == 1:
        try:
            if time.time() - heartbeat.stat().st_mtime > 0.5:
                break
        except FileNotFoundError:
            break
    elif os.getppid() != parent:
        break
    time.sleep(0.05)
try:
    (root / ".cleanup-ack").touch()
except FileNotFoundError:
    pass

for _, device, inode, control, processes in owned:
    states = [identity_state(identity) for identity in processes]
    server_state, pane_state = states
    if server_state == "absent" and pane_state == "absent":
        continue
    if server_state != "owned" or pane_state not in ("owned", "absent"):
        continue
    try:
        control_stat = control.stat()
        if control_stat.st_dev != device or control_stat.st_ino != inode:
            continue
        expected_server = processes[0][0]
        killed = subprocess.run(
            ["tmux", "-S", str(control), "if-shell", "-F",
             f"#{{==:#{{pid}},{expected_server}}}", "kill-server", ""],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=0.5,
        )
    except (OSError, subprocess.TimeoutExpired):
        pass

quiet = 0
for _ in range(50):
    states = [
        identity_state(identity)
        for _, _, _, _, processes in owned
        for identity in processes
    ]
    unconfirmed_state = any(state != "absent" for state in states) or bool(unconfirmed_obligations)
    registered_sockets = {
        socket: (device, inode, processes)
        for socket, device, inode, _, processes in owned
    }
    for socket in root.glob("*.sock"):
        if socket.is_symlink():
            socket.unlink(missing_ok=True)
            continue
        if not socket.is_socket():
            continue
        registered = registered_sockets.get(socket)
        if registered is not None:
            device, inode, processes = registered
            states = [identity_state(identity) for identity in processes]
            try:
                socket_stat = socket.stat()
            except OSError:
                unconfirmed = True
                continue
            if socket_stat.st_dev != device or socket_stat.st_ino != inode:
                unconfirmed = True
                continue
            if all(state == "absent" for state in states):
                socket.unlink()
                if socket.exists():
                    unconfirmed = True
            else:
                unconfirmed = True
            continue
        try:
            probe = subprocess.run(
                ["tmux", "-S", str(socket), "list-sessions"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
                timeout=0.5,
            )
            if probe.returncode == 0:
                killed = subprocess.run(
                    ["tmux", "-S", str(socket), "kill-server"],
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=False,
                    timeout=0.5,
                )
                if killed.returncode == 0:
                    socket.unlink(missing_ok=True)
                else:
                    unconfirmed = True
            else:
                unconfirmed = True
        except (OSError, subprocess.TimeoutExpired):
            unconfirmed = True
    sockets = any(path.is_socket() for path in root.glob("*.sock"))
    creators = False
    for marker in root.glob(".creating-*"):
        try:
            with marker.open("r+") as marker_file:
                try:
                    fcntl.flock(marker_file, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    marker.unlink(missing_ok=True)
                    marker.with_suffix(".locked").unlink(missing_ok=True)
                except BlockingIOError:
                    creators = True
        except OSError:
            creators = True
    quiet = quiet + 1 if not unconfirmed_state and not sockets and not creators else 0
    if quiet >= 5:
        if root.exists():
            shutil.rmtree(root)
        if control_root.exists():
            shutil.rmtree(control_root)
        sys.exit(0)
    time.sleep(0.1)
print(f"tmux test watchdog retained failed control root: {control_root}", file=sys.stderr)
sys.exit(1)
"##;

/// Owns real tmux servers created by tests, including after abrupt runner death.
pub struct TestTmuxServerOwner {
    root: Option<TempDir>,
    control_root: Option<TempDir>,
    watchdog: Option<Child>,
    heartbeat_stop: Arc<AtomicBool>,
    heartbeat: Option<thread::JoinHandle<()>>,
}

impl Default for TestTmuxServerOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl TestTmuxServerOwner {
    /// Creates an isolated short socket root and its detached cleanup watchdog.
    ///
    /// # Panics
    ///
    /// Panics when the temporary root or watchdog process cannot be created or
    /// armed. Tests cannot safely continue without containment.
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_watchdog_path(None)
    }

    fn new_with_watchdog_path(watchdog_path: Option<&Path>) -> Self {
        let root = tempfile::Builder::new()
            .prefix("ptt-")
            .tempdir_in("/private/tmp")
            .or_else(|_| tempfile::Builder::new().prefix("ptt-").tempdir_in("/tmp"))
            .expect("create short isolated tmux test root");
        let canonical_root = root.path().canonicalize().expect("canonicalize test root");
        let control_root = tempfile::Builder::new()
            .prefix("ptw-")
            .tempdir_in(canonical_root.parent().expect("test root has parent"))
            .expect("create tmux test control root");
        let canonical_control_root = control_root
            .path()
            .canonicalize()
            .expect("canonicalize test control root");
        let private_tmp = Path::new("/private/tmp")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from("/private/tmp"));
        let tmp = Path::new("/tmp")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        assert!(
            canonical_root.starts_with(private_tmp) || canonical_root.starts_with(tmp),
            "tmux test root must be under a short system temporary directory"
        );

        fs::write(canonical_root.join(".parent-heartbeat"), [])
            .expect("initialize tmux test owner heartbeat");
        let parent_pid = std::process::id().to_string();
        let mut command = Command::new("python3");
        command
            .args(["-c", WATCHDOG_PROGRAM])
            .arg(&canonical_root)
            .arg(parent_pid)
            .arg(&canonical_control_root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(path) = watchdog_path {
            let inherited_path = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = vec![path.to_path_buf()];
            paths.extend(std::env::split_paths(&inherited_path));
            command.env(
                "PATH",
                std::env::join_paths(paths).expect("join watchdog PATH"),
            );
        }
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut watchdog = command.spawn().expect("spawn tmux test watchdog");
        wait_for_watchdog_arm(&mut watchdog, &canonical_root)
            .expect("tmux test watchdog must arm before owner is exposed");
        let heartbeat_stop = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&heartbeat_stop);
        let heartbeat_path = canonical_root.join(".parent-heartbeat");
        let heartbeat = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let _ = fs::write(&heartbeat_path, []);
                // test-timing-allow: heartbeat cadence detects PID-1 runner death; cleanup uses explicit markers and watchdog exit
                thread::sleep(Duration::from_millis(100));
            }
        });

        Self {
            root: Some(root),
            control_root: Some(control_root),
            watchdog: Some(watchdog),
            heartbeat_stop,
            heartbeat: Some(heartbeat),
        }
    }

    pub(crate) fn socket_dir(&self) -> &Path {
        self.root.as_ref().expect("owner is live").path()
    }

    /// Returns the unique socket root owned by this test fixture.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.socket_dir()
    }

    /// Creates a registry whose servers are confined to this owner's root.
    #[must_use]
    pub fn registry(&self) -> TmuxRegistry {
        TmuxRegistry::with_socket_dir(self.socket_dir().to_path_buf())
            .with_test_spawn_containment(self.control_root_path().to_path_buf())
    }

    #[cfg(test)]
    pub(crate) fn registry_with_sink(
        &self,
        sink: Option<super::registry::TmuxLifecycleSink>,
    ) -> TmuxRegistry {
        TmuxRegistry::with_socket_dir_binary_and_sink(self.socket_dir().to_path_buf(), true, sink)
            .with_test_spawn_containment(self.control_root_path().to_path_buf())
    }

    pub(crate) fn control_root_path(&self) -> &Path {
        self.control_root
            .as_ref()
            .expect("owner control root is live")
            .path()
    }

    /// Kills and verifies all exact servers under the owned root.
    ///
    /// # Panics
    ///
    /// Panics when the watchdog cannot complete cleanup or any exact server
    /// remains live. The root is preserved for recovery in that case.
    pub fn shutdown(mut self) {
        self.finish(true).expect("tmux test cleanup must succeed");
    }

    fn finish(&mut self, graceful: bool) -> io::Result<()> {
        let root = self.root.take().expect("owner root is live");
        let control_root = self
            .control_root
            .take()
            .expect("owner control root is live");
        let mut watchdog = self.watchdog.take().expect("watchdog is live");
        let root_path = root.path().to_path_buf();
        self.heartbeat_stop.store(true, Ordering::Release);
        if let Some(heartbeat) = self.heartbeat.take() {
            let _ = heartbeat.join();
        }
        let handoff_error = request_cleanup(&root_path, graceful).err();

        let result = wait_for_watchdog(&mut watchdog).and_then(|status| {
            if !status.success() {
                return Err(io::Error::other(format!(
                    "tmux test watchdog reported cleanup failure: {status}"
                )));
            }
            if root_path.exists() {
                verify_no_live_servers(&root_path)?;
            }
            if let Some(error) = handoff_error {
                return Err(error);
            }
            Ok(())
        });
        if result.is_err() {
            let retained_root = root.keep();
            let retained_control = control_root.keep();
            return Err(io::Error::other(format!(
                "{}; retained root: {}; retained control root: {}",
                result.expect_err("result is error"),
                retained_root.display(),
                retained_control.display()
            )));
        }
        result
    }
}

impl Drop for TestTmuxServerOwner {
    fn drop(&mut self) {
        if self.watchdog.is_none() {
            return;
        }
        if let Err(error) = self.finish(false) {
            if thread::panicking() {
                eprintln!("tmux test cleanup failed while unwinding: {error}");
            } else {
                panic!("tmux test cleanup failed: {error}");
            }
        }
    }
}

fn verify_no_live_servers(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_socket() && probe_sync(&entry.path()) == ProbeResult::Live {
            return Err(io::Error::other(format!(
                "tmux server remains live at {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct TestServerProcesses {
    pub(crate) server: ProcessIdentity,
    pub(crate) pane: ProcessIdentity,
}

#[cfg(test)]
fn parse_process_ids(output: &str) -> io::Result<(&str, &str)> {
    let output = output.strip_suffix('\n').ok_or_else(|| {
        io::Error::other("tmux test process identity output lacked its line terminator")
    })?;
    let (server_pid, pane_pid) = output.split_once('|').ok_or_else(|| {
        io::Error::other("tmux test process identity output lacked its delimiter")
    })?;
    if server_pid.is_empty()
        || pane_pid.is_empty()
        || !server_pid.bytes().all(|byte| byte.is_ascii_digit())
        || !pane_pid.bytes().all(|byte| byte.is_ascii_digit())
        || pane_pid.contains('|')
    {
        return Err(io::Error::other(
            "tmux test process identity output was malformed",
        ));
    }
    Ok((server_pid, pane_pid))
}

struct RegistrationArtifacts {
    paths: Vec<PathBuf>,
}

impl Drop for RegistrationArtifacts {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

fn protocol_field(path: &Path, label: &str) -> io::Result<String> {
    path.to_str()
        .filter(|value| !value.contains('\t'))
        .map(ToOwned::to_owned)
        .ok_or_else(|| io::Error::other(format!("{label} is not protocol-safe UTF-8")))
}

pub(crate) async fn spawn_owned_server(
    socket: &Path,
    control_socket: &Path,
    config_path: &Path,
    cwd: &Path,
    token: &str,
    server_env: &[(String, String)],
) -> io::Result<TestServerProcesses> {
    let control_root = control_socket
        .parent()
        .ok_or_else(|| io::Error::other("tmux test control socket has no root"))?;
    let socket_name = protocol_field(
        Path::new(
            socket
                .file_name()
                .ok_or_else(|| io::Error::other("socket has no name"))?,
        ),
        "socket name",
    )?;
    let control_name = protocol_field(
        Path::new(
            control_socket
                .file_name()
                .ok_or_else(|| io::Error::other("control has no name"))?,
        ),
        "control name",
    )?;
    let nonce = uuid::Uuid::new_v4();
    let request = control_root.join(format!(".spawn-{nonce}"));
    let pending = control_root.join(format!(".pending-spawn-{nonce}"));
    let acknowledged = control_root.join(format!(".registered-{nonce}"));
    let rejected = control_root.join(format!(".rejected-{nonce}"));
    let env_file = control_root.join(format!(".environment-{nonce}.json"));
    fs::write(
        &env_file,
        serde_json::to_vec(server_env).map_err(io::Error::other)?,
    )?;
    let _artifacts = RegistrationArtifacts {
        paths: vec![
            pending.clone(),
            request.clone(),
            acknowledged.clone(),
            rejected.clone(),
            control_root.join(format!(".pending-registered-{nonce}")),
            control_root.join(format!(".pending-rejected-{nonce}")),
            env_file.clone(),
        ],
    };
    fs::write(
        &pending,
        format!(
            "{socket_name}\t{control_name}\t{}\t{}\t{token}\t{}",
            protocol_field(config_path, "config path")?,
            protocol_field(cwd, "cwd")?,
            protocol_field(
                Path::new(env_file.file_name().expect("env file has name")),
                "env file",
            )?
        ),
    )?;
    fs::rename(pending, request)?;
    let deadline = tokio::time::Instant::now() + CLEANUP_TIMEOUT;
    loop {
        if let Ok(value) = fs::read_to_string(&acknowledged) {
            return parse_acknowledged_processes(&value);
        }
        if let Ok(reason) = fs::read_to_string(&rejected) {
            return Err(io::Error::other(format!(
                "tmux watchdog rejected spawn: {reason}"
            )));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux watchdog did not acknowledge spawn",
            ));
        }
        tokio::task::yield_now().await;
    }
}

fn parse_acknowledged_processes(value: &str) -> io::Result<TestServerProcesses> {
    let fields = value.split('\t').collect::<Vec<_>>();
    if fields.len() != 4 {
        return Err(io::Error::other(
            "tmux watchdog returned malformed process identities",
        ));
    }
    let parse = |value: &str| {
        value
            .parse::<u128>()
            .map_err(|error| io::Error::other(format!("invalid tmux process identity: {error}")))
    };
    Ok(TestServerProcesses {
        server: ProcessIdentity {
            pid: u32::try_from(parse(fields[0])?).map_err(io::Error::other)?,
            start_time: parse(fields[1])?,
        },
        pane: ProcessIdentity {
            pid: u32::try_from(parse(fields[2])?).map_err(io::Error::other)?,
            start_time: parse(fields[3])?,
        },
    })
}

pub(crate) fn register_owned_server(
    socket: &Path,
    control_socket: &Path,
    expected_token: &str,
) -> io::Result<TestServerProcesses> {
    let socket_name = socket
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("tmux test socket name is not UTF-8"))?;
    let control_root = control_socket
        .parent()
        .ok_or_else(|| io::Error::other("tmux test control socket has no root"))?;
    let control_name = control_socket
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("tmux test control socket name is not UTF-8"))?;
    let nonce = uuid::Uuid::new_v4();
    let request = control_root.join(format!(".register-{nonce}"));
    let pending = control_root.join(format!(".pending-registration-{nonce}"));
    let acknowledged = control_root.join(format!(".registered-{nonce}"));
    let rejected = control_root.join(format!(".rejected-{nonce}"));
    let pending_acknowledged = control_root.join(format!(".pending-registered-{nonce}"));
    let pending_rejected = control_root.join(format!(".pending-rejected-{nonce}"));
    let _artifacts = RegistrationArtifacts {
        paths: vec![
            pending.clone(),
            request.clone(),
            acknowledged.clone(),
            rejected.clone(),
            pending_acknowledged,
            pending_rejected,
        ],
    };
    fs::write(
        &pending,
        format!("{socket_name}\t{control_name}\t{expected_token}"),
    )?;
    fs::rename(pending, &request)?;

    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    loop {
        if let Ok(value) = fs::read_to_string(&acknowledged) {
            return parse_acknowledged_processes(&value);
        }
        if let Ok(reason) = fs::read_to_string(&rejected) {
            return Err(io::Error::other(format!(
                "tmux watchdog rejected process ownership: {reason}"
            )));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux watchdog did not acknowledge process ownership",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_watchdog_arm(watchdog: &mut Child, root: &Path) -> io::Result<()> {
    let armed = root.join(".armed");
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    while !armed.exists() {
        if let Some(status) = watchdog.try_wait()? {
            return Err(io::Error::other(format!(
                "tmux test watchdog exited before arming: {status}"
            )));
        }
        if Instant::now() >= deadline {
            let _ = watchdog.kill();
            let _ = watchdog.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux test watchdog did not arm",
            ));
        }
        // test-timing-allow: the armed marker is the completion signal; the deadline only bounds failed startup
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn request_cleanup(root: &Path, graceful: bool) -> io::Result<()> {
    let request = root.join(".cleanup-request");
    let pending = root.join(".cleanup-request.pending");
    let reason: &[u8] = if graceful { b"graceful" } else { b"drop" };
    fs::write(&pending, reason)?;
    fs::rename(pending, request)?;
    let ack = root.join(".cleanup-ack");
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    while !ack.exists() && root.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux test watchdog did not acknowledge cleanup request",
            ));
        }
        // test-timing-allow: acknowledgment or completed root removal is the signal; the deadline only bounds a failed handoff
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn wait_for_watchdog(watchdog: &mut Child) -> io::Result<std::process::ExitStatus> {
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    loop {
        if let Some(status) = watchdog.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            if let Ok(pid) = i32::try_from(watchdog.id()) {
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
            let _ = watchdog.kill();
            let _ = watchdog.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux test watchdog did not finish cleanup",
            ));
        }
        // test-timing-allow: watchdog exit is the completion signal; the deadline only bounds failed cleanup
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::ExitStatus;

    use super::*;

    #[test]
    fn process_identity_parser_accepts_exact_tmux_form_and_rejects_malformed_output() {
        assert_eq!(
            parse_process_ids("74173|74177\n").unwrap(),
            ("74173", "74177")
        );
        for malformed in [
            "74173\\t74177\n",
            "74173|74177",
            "|74177\n",
            "74173|\n",
            "74173|74177|1\n",
            " 74173|74177\n",
            "74173|74177 \n",
        ] {
            assert!(
                parse_process_ids(malformed).is_err(),
                "accepted malformed tmux output: {malformed:?}"
            );
        }
    }

    fn spawn_server_with_processes(
        owner: &TestTmuxServerOwner,
        name: &str,
    ) -> (PathBuf, TestServerProcesses) {
        let socket = owner.path().join(format!("{name}.sock"));
        let control = owner.control_root_path().join(format!("{name}.sock"));
        let server_token = uuid::Uuid::new_v4().to_string();
        let status = Command::new("tmux")
            .args([
                "-S",
                &control.to_string_lossy(),
                "new-session",
                "-d",
                "-s",
                "main",
                "sleep 300",
                ";",
                "set-environment",
                "-g",
                "PHOENIX_TMUX_SERVER_TOKEN",
                &server_token,
            ])
            .env_remove("TMUX")
            .env("PHOENIX_TMUX_SERVER_TOKEN", &server_token)
            .status()
            .expect("launch disposable tmux test server");
        assert!(status.success());
        fs::hard_link(&control, &socket).expect("publish disposable tmux socket");
        assert_eq!(probe_sync(&control), ProbeResult::Live);
        let processes = register_owned_server(&socket, &control, &server_token)
            .expect("register exact disposable server");
        (socket, processes)
    }

    #[test]
    fn tmux_emits_strict_process_identity_wire_form() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let (socket, processes) = spawn_server_with_processes(&owner, "wire-form");
        let output = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(["display-message", "-p", "#{pid}|#{pane_pid}"])
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(output.status.success());
        let output = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            output,
            format!("{}|{}\n", processes.server.pid, processes.pane.pid)
        );
        let (server_pid, pane_pid) = parse_process_ids(&output).unwrap();
        assert_eq!(server_pid.parse::<u32>().unwrap(), processes.server.pid);
        assert_eq!(pane_pid.parse::<u32>().unwrap(), processes.pane.pid);
        owner.shutdown();
        assert_exact_processes_gone(processes);
    }

    #[test]
    fn registration_consumes_only_its_exact_handshake_artifacts() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let sentinel = owner.path().join("unrelated-registration-note");
        fs::write(&sentinel, "keep").unwrap();
        let (_, processes) = spawn_server_with_processes(&owner, "handshake");
        assert_no_registration_artifacts(owner.control_root_path());

        let socket = owner.path().join("handshake.sock");
        let control = owner.control_root_path().join("handshake.sock");
        let error = register_owned_server(&socket, &control, "deliberately-wrong-token")
            .expect_err("wrong authenticated token must be rejected");
        assert!(error.to_string().contains("rejected process ownership"));
        assert_no_registration_artifacts(owner.control_root_path());
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "keep");

        owner.shutdown();
        assert_exact_processes_gone(processes);
    }

    fn assert_no_registration_artifacts(root: &Path) {
        let residue = fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| {
                name.starts_with(".register-")
                    || name.starts_with(".registered-")
                    || name.starts_with(".rejected-")
                    || name.starts_with(".pending-registration-")
                    || name.starts_with(".pending-registered-")
                    || name.starts_with(".pending-rejected-")
            })
            .collect::<Vec<_>>();
        assert!(
            residue.is_empty(),
            "registration residue remained: {residue:?}"
        );
    }

    fn wait_until(mut condition: impl FnMut() -> bool, description: &str) {
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {description}"
            );
            // test-timing-allow: cross-process marker/root state is the completion signal; the deadline only bounds a wedged fixture
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn authenticated_process_exit_unlinks_exact_stale_socket_and_removes_root() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let (socket, processes) = spawn_server_with_processes(&owner, "normal");
        owner.shutdown();
        assert!(!socket.exists(), "exact stale socket must be unlinked");
        assert_exact_processes_gone(processes);
        assert!(!root.exists());
    }

    #[test]
    fn cleanup_uses_immutable_process_token_after_global_token_mutation() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let (socket, processes) = spawn_server_with_processes(&owner, "mutated-global-token");
        let status = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(["set-environment", "-gu", "PHOENIX_TMUX_SERVER_TOKEN"])
            .env_remove("TMUX")
            .status()
            .unwrap();
        assert!(status.success());
        owner.shutdown();
        assert_exact_processes_gone(processes);
    }

    #[test]
    fn contained_spawn_retries_until_process_identity_is_ready() {
        assert!(WATCHDOG_PROGRAM.contains("for attempt in range(50):"));
        assert!(WATCHDOG_PROGRAM.contains("time.sleep(0.1)"));
        assert!(WATCHDOG_PROGRAM.contains("tmux processes never became ready"));
    }

    #[test]
    fn successful_identity_reconciliation_removes_the_spawn_obligation() {
        assert!(WATCHDOG_PROGRAM.contains(
            "identities = observe_control(control, token)\n        control_stat = control.stat()\n        owned.append((socket, control_stat.st_dev, control_stat.st_ino, control, tuple(identities)))\n        unconfirmed_obligations.remove(obligation)"
        ));
    }

    #[test]
    fn unlinked_socket_cleanup_kills_registered_server_and_pane() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let (socket, processes) = spawn_server_with_processes(&owner, "missing-socket");
        fs::remove_file(socket).unwrap();
        owner.shutdown();
        assert_exact_processes_gone(processes);
        assert!(!root.exists());
    }

    #[test]
    fn replacement_at_registered_socket_is_not_unlinked() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let control = control_root.join("replacement.sock");
        let (socket, processes) = spawn_server_with_processes(&owner, "replacement");
        fs::remove_file(&socket).unwrap();
        let replacement = std::os::unix::net::UnixListener::bind(&socket).unwrap();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));
        assert!(
            panic.is_err(),
            "replacement socket must fail cleanup closed"
        );
        assert!(socket.exists(), "replacement socket must not be unlinked");
        assert_exact_processes_gone(processes);
        assert!(root.exists(), "failed cleanup must preserve its exact root");

        drop(replacement);
        assert_ne!(probe_sync(&control), ProbeResult::Live);
        assert_exact_processes_gone(processes);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn unconfirmed_spawn_obligation_preserves_roots_and_prevents_false_success() {
        let fake_bin = TempDir::new().unwrap();
        let fake_tmux = fake_bin.path().join("tmux");
        fs::write(
            &fake_tmux,
            "#!/bin/sh\ncase \"$*\" in *new-session*) : > \"$4\"; /bin/sleep 2;; *kill-server*) /bin/sleep 2;; esac\nexit 1\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake_tmux).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_tmux, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_path(Some(fake_bin.path()));
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let control = control_root.join("unconfirmed.sock");
        let env_file = control_root.join("env.json");
        fs::write(
            &env_file,
            serde_json::to_vec(&vec![(
                "PATH".to_owned(),
                fake_bin.path().to_string_lossy().into_owned(),
            )])
            .unwrap(),
        )
        .unwrap();
        fs::write(
            control_root.join(".spawn-unconfirmed"),
            format!(
                "unconfirmed.sock\tunconfirmed.sock\t{}\t{}\ttoken\tenv.json",
                root.join("config").display(),
                root.display()
            ),
        )
        .unwrap();
        wait_until(|| control.exists(), "unconfirmed control endpoint");
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));
        assert!(panic.is_err());
        assert!(root.exists());
        assert!(control_root.exists());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    fn process_absent_or_zombie(identity: ProcessIdentity) -> bool {
        if !phoenix_core::process_identity::process_identity_matches(identity) {
            return true;
        }
        Command::new("ps")
            .args(["-o", "state=", "-p", &identity.pid.to_string()])
            .output()
            .ok()
            .is_some_and(|output| output.status.success() && output.stdout.contains(&b'Z'))
    }

    #[test]
    fn missing_unconfirmed_control_preserves_roots_and_prevents_false_success() {
        let fake_bin = TempDir::new().unwrap();
        let fake_tmux = fake_bin.path().join("tmux");
        let daemon = fake_bin.path().join("daemon");
        fs::write(
            &fake_tmux,
            format!(
                "#!/bin/sh\ncontrol=\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = -S ]; then control=$2; break; fi\n  shift\ndone\ncase \"$*\" in *new-session*)\n  : > \"$control\"\n  /usr/bin/nohup /bin/sleep 10 >/dev/null 2>&1 &\n  printf '%s' \"$!\" > '{}'\n  /bin/rm \"$control\"\n  ;;\nesac\nexit 1\n",
                daemon.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake_tmux).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_tmux, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_path(Some(fake_bin.path()));
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let env_file = control_root.join("missing-env.json");
        fs::write(
            &env_file,
            serde_json::to_vec(&vec![(
                "PATH".to_owned(),
                fake_bin.path().to_string_lossy().into_owned(),
            )])
            .unwrap(),
        )
        .unwrap();
        fs::write(
            control_root.join(".spawn-missing-unconfirmed"),
            format!(
                "missing.sock\tmissing.sock\t{}\t{}\ttoken\tmissing-env.json",
                root.join("config").display(),
                root.display()
            ),
        )
        .unwrap();
        wait_until(|| daemon.exists(), "detached fake daemon");
        let daemon_pid = fs::read_to_string(&daemon).unwrap().parse::<u32>().unwrap();
        let daemon_identity = phoenix_core::process_identity::current_process_identity(daemon_pid)
            .expect("detached fake daemon has exact birth identity");
        wait_until(
            || control_root.join(".rejected-missing-unconfirmed").exists(),
            "missing-control spawn rejection",
        );
        assert!(!control_root.join("missing.sock").exists());

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(panic.is_err());
        assert!(root.exists());
        assert!(control_root.exists());
        assert!(phoenix_core::process_identity::process_identity_matches(
            daemon_identity
        ));
        wait_until(
            || process_absent_or_zombie(daemon_identity),
            "detached fake daemon natural exit",
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn cleanup_failure_returns_inspectable_control_root_for_reclamation() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let (socket, processes) = spawn_server_with_processes(&owner, "retained-control");
        fs::remove_file(&socket).unwrap();
        let replacement = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()))
            .expect_err("replacement must preserve failure evidence");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("cleanup panic has text");
        assert!(message.contains(&format!(
            "retained control root: {}",
            control_root.display()
        )));
        assert!(control_root.exists());
        assert_exact_processes_gone(processes);
        drop(replacement);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn cleanup_does_not_kill_another_test_owners_server() {
        if which::which("tmux").is_err() {
            return;
        }
        let first = TestTmuxServerOwner::new();
        let second = TestTmuxServerOwner::new();
        let (_, first_processes) = spawn_server_with_processes(&first, "first");
        let (_, second_processes) = spawn_server_with_processes(&second, "second");
        first.shutdown();
        assert_exact_processes_gone(first_processes);
        assert!(
            phoenix_core::process_identity::process_identity_matches(second_processes.server),
            "cleanup must not signal a server outside its exact owner"
        );
        assert!(
            phoenix_core::process_identity::process_identity_matches(second_processes.pane),
            "cleanup must not signal a pane outside its exact owner"
        );
        second.shutdown();
        assert_exact_processes_gone(second_processes);
    }

    fn write_tmux_wrapper(fake_bin: &Path, program: &str) {
        let fake_tmux = fake_bin.join("tmux");
        fs::write(&fake_tmux, program).unwrap();
        let mut permissions = fs::metadata(&fake_tmux).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(fake_tmux, permissions).unwrap();
    }

    #[test]
    fn matching_control_endpoint_accepts_one_incarnation_bound_cleanup_request() {
        let Ok(real_tmux) = which::which("tmux") else {
            return;
        };
        let fake_bin = TempDir::new().unwrap();
        let accepted = fake_bin.path().join("accepted");
        write_tmux_wrapper(
            fake_bin.path(),
            &format!(
                "#!/bin/sh\nif [ \"$3\" = if-shell ]; then printf '%s\\n' \"$*\" >> '{}'; fi\nexec '{}' \"$@\"\n",
                accepted.display(),
                real_tmux.display()
            ),
        );
        let owner = TestTmuxServerOwner::new_with_watchdog_path(Some(fake_bin.path()));
        let (_, processes) = spawn_server_with_processes(&owner, "accepted-cleanup");

        owner.shutdown();

        assert_exact_processes_gone(processes);
        let requests = fs::read_to_string(accepted).unwrap();
        assert_eq!(requests.lines().count(), 1);
        assert!(requests.contains(&format!(
            "if-shell -F #{{==:#{{pid}},{}}} kill-server",
            processes.server.pid
        )));
    }

    #[test]
    fn replacement_control_endpoint_rejects_incarnation_bound_cleanup_request() {
        let Ok(real_tmux) = which::which("tmux") else {
            return;
        };
        let fake_bin = TempDir::new().unwrap();
        let replaced = fake_bin.path().join("replaced");
        write_tmux_wrapper(
            fake_bin.path(),
            &format!(
                "#!/bin/sh\nif [ \"$3\" = if-shell ] && [ ! -e '{}' ]; then\n  rm -f \"$2\"\n  '{}' -S \"$2\" new-session -d -s replacement 'sleep 300'\n  : > '{}'\nfi\nexec '{}' \"$@\"\n",
                replaced.display(),
                real_tmux.display(),
                replaced.display(),
                real_tmux.display()
            ),
        );
        let owner = TestTmuxServerOwner::new_with_watchdog_path(Some(fake_bin.path()));
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let socket = root.join("replacement-after-check.sock");
        let control = control_root.join("replacement-after-check.sock");
        let (_, processes) = spawn_server_with_processes(&owner, "replacement-after-check");

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(
            panic.is_err(),
            "surviving owned server must fail cleanup closed"
        );
        assert!(
            replaced.exists(),
            "wrapper did not replace the accepted endpoint"
        );
        assert_eq!(probe_sync(&control), ProbeResult::Live);
        assert!(
            phoenix_core::process_identity::process_identity_matches(processes.server),
            "owned server should remain available at its other hard link"
        );
        assert!(Command::new(&real_tmux)
            .args(["-S", &control.to_string_lossy(), "kill-server"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new(real_tmux)
            .args(["-S", &socket.to_string_lossy(), "kill-server"])
            .status()
            .unwrap()
            .success());
        assert_exact_processes_gone(processes);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn cleanup_uses_incarnation_bound_socket_without_numeric_pid_signals() {
        assert!(!WATCHDOG_PROGRAM.contains("os.kill(identity[0]"));
        assert!(!WATCHDOG_PROGRAM.contains("os.link(socket, control)"));
        assert!(WATCHDOG_PROGRAM.contains(
            "[\"tmux\", \"-S\", str(control), \"if-shell\", \"-F\",\n             f\"#{{==:#{{pid}},{expected_server}}}\", \"kill-server\", \"\"]"
        ));
    }

    #[test]
    fn cleanup_revalidates_control_inode_before_incarnation_bound_kill() {
        assert!(WATCHDOG_PROGRAM.contains("control_stat.st_dev"));
        assert!(WATCHDOG_PROGRAM.contains("control_stat.st_ino"));
        assert!(!WATCHDOG_PROGRAM.contains("observed_server, _ = query_control_processes(control)"));
    }

    #[test]
    fn pane_identity_is_bound_to_authenticated_server_without_requiring_late_token() {
        assert!(WATCHDOG_PROGRAM.contains(
            "(server_pid, birth(server_pid), token),\n                (pane_pid, birth(pane_pid), None),"
        ));
        assert!(WATCHDOG_PROGRAM.contains("if token is None:\n        return \"owned\""));
    }

    #[test]
    fn authenticated_server_with_absent_original_pane_is_retired() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let (socket, processes) = spawn_server_with_processes(&owner, "pane-absent");
        assert!(Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args([
                "new-window",
                "-d",
                "-t",
                "main",
                "-n",
                "survivor",
                "sleep 300"
            ])
            .status()
            .unwrap()
            .success());
        let panes = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(["list-panes", "-a", "-F", "#{pane_pid}|#{pane_id}"])
            .output()
            .unwrap();
        assert!(panes.status.success());
        let panes = String::from_utf8(panes.stdout).unwrap();
        let pane_id = panes
            .lines()
            .find_map(|line| {
                let (pid, pane_id) = line.split_once('|')?;
                (pid.parse::<u32>().ok()? == processes.pane.pid).then_some(pane_id)
            })
            .expect("registered pane remains addressable");
        assert!(Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(["kill-pane", "-t", pane_id])
            .status()
            .unwrap()
            .success());
        wait_until(
            || !phoenix_core::process_identity::process_identity_matches(processes.pane),
            "original pane exit",
        );
        owner.shutdown();
        assert_exact_processes_gone(processes);
    }

    #[test]
    fn natural_exit_racing_kill_server_uses_final_identity_state() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let (socket, processes) = spawn_server_with_processes(&owner, "natural-exit");
        assert!(Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .arg("kill-server")
            .status()
            .unwrap()
            .success());
        wait_until(
            || {
                !phoenix_core::process_identity::process_identity_matches(processes.server)
                    && !phoenix_core::process_identity::process_identity_matches(processes.pane)
            },
            "natural server and pane exit",
        );
        owner.shutdown();
        assert_exact_processes_gone(processes);
    }

    #[test]
    fn control_endpoint_authenticates_identity_after_visible_socket_replacement() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let socket = owner.path().join("pre-registration-replacement.sock");
        let control = owner
            .control_root_path()
            .join("pre-registration-replacement.sock");
        let token = uuid::Uuid::new_v4().to_string();
        assert!(Command::new("tmux")
            .args([
                "-S",
                &control.to_string_lossy(),
                "new-session",
                "-d",
                "-s",
                "main",
                "sleep 300",
                ";",
                "set-environment",
                "-g",
                "PHOENIX_TMUX_SERVER_TOKEN",
                &token,
            ])
            .env_remove("TMUX")
            .env("PHOENIX_TMUX_SERVER_TOKEN", &token)
            .status()
            .unwrap()
            .success());
        let replacement = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let processes = register_owned_server(&socket, &control, &token).unwrap();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));
        assert!(
            panic.is_err(),
            "visible replacement must remain fail-closed"
        );
        assert_exact_processes_gone(processes);
        assert!(socket.exists(), "unrelated visible replacement was removed");
        drop(replacement);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn cleanup_accepts_only_jointly_owned_or_jointly_absent_processes() {
        assert!(WATCHDOG_PROGRAM.contains(
            "if server_state == \"absent\" and pane_state == \"absent\":\n        continue\n    if server_state != \"owned\" or pane_state not in (\"owned\", \"absent\"):\n        continue"
        ));
    }

    #[test]
    fn missing_root_cleanup_kills_registered_server_and_pane() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let (_, processes) = spawn_server_with_processes(&owner, "missing-root");
        fs::remove_dir_all(&root).unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));
        assert!(panic.is_err(), "lost cleanup handoff must remain visible");
        assert_exact_processes_gone(processes);
        assert!(!root.exists());
    }

    #[test]
    fn root_loss_during_acknowledgment_retains_owned_processes_for_cleanup() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let (socket, processes) = spawn_server_with_processes(&owner, "ack-root-loss");
        fs::remove_file(&socket).unwrap();
        fs::remove_dir_all(owner.path()).unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));
        assert!(panic.is_err(), "lost cleanup handoff must remain visible");
        assert_exact_processes_gone(processes);
    }

    #[test]
    fn idle_owner_does_not_require_tmux() {
        let fake_bin = TempDir::new().unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_path(Some(fake_bin.path()));
        let root = owner.path().to_path_buf();
        owner.shutdown();
        assert!(!root.exists());
    }

    #[test]
    fn socket_symlink_is_unlinked_without_invoking_tmux() {
        let fake_bin = TempDir::new().unwrap();
        let fake_tmux = fake_bin.path().join("tmux");
        let invoked = fake_bin.path().join("invoked");
        fs::write(
            &fake_tmux,
            format!("#!/bin/sh\ntouch '{}'\nexit 0\n", invoked.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake_tmux).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_tmux, permissions).unwrap();

        let owner = TestTmuxServerOwner::new_with_watchdog_path(Some(fake_bin.path()));
        let root = owner.path().to_path_buf();
        std::os::unix::fs::symlink("/tmp/not-a-test-socket", root.join("escape.sock")).unwrap();
        owner.shutdown();
        assert!(!invoked.exists(), "watchdog must not pass symlinks to tmux");
        assert!(!root.exists());
    }

    #[test]
    fn unverifiable_failed_probe_preserves_socket_and_root() {
        let fake_bin = TempDir::new().unwrap();
        let fake_tmux = fake_bin.path().join("tmux");
        fs::write(&fake_tmux, "#!/bin/sh\nexit 1\n").unwrap();
        let mut permissions = fs::metadata(&fake_tmux).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_tmux, permissions).unwrap();

        let owner = TestTmuxServerOwner::new_with_watchdog_path(Some(fake_bin.path()));
        let root = owner.path().to_path_buf();
        let socket = root.join("unconfirmed.sock");
        std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));
        assert!(panic.is_err(), "cleanup failure must fail the test");
        assert!(root.exists(), "failed cleanup must preserve its exact root");
        assert!(
            socket.exists(),
            "unconfirmed socket must remain recoverable"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn panic_unwind_kills_exact_server_and_removes_root() {
        if which::which("tmux").is_err() {
            return;
        }
        let (root, socket, processes) = std::panic::catch_unwind(|| {
            let owner = TestTmuxServerOwner::new();
            let root = owner.path().to_path_buf();
            let (socket, processes) = spawn_server_with_processes(&owner, "panic");
            std::panic::panic_any((root, socket, processes));
        })
        .expect_err("fixture must panic")
        .downcast::<(PathBuf, PathBuf, TestServerProcesses)>()
        .map(|paths| *paths)
        .expect("panic payload");
        assert_ne!(probe_sync(&socket), ProbeResult::Live);
        assert_exact_processes_gone(processes);
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn task_cancellation_kills_exact_server_and_removes_root() {
        if which::which("tmux").is_err() {
            return;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let owner = TestTmuxServerOwner::new();
            let root = owner.path().to_path_buf();
            let (socket, processes) = spawn_server_with_processes(&owner, "cancel");
            tx.send((root, socket, processes)).unwrap();
            std::future::pending::<()>().await;
        });
        let (root, socket, processes) = rx.await.unwrap();
        task.abort();
        let _ = task.await;
        assert_ne!(probe_sync(&socket), ProbeResult::Live);
        assert_exact_processes_gone(processes);
        assert!(!root.exists());
    }

    #[test]
    fn cleanup_waits_for_in_flight_late_server_creation() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let socket = root.join("late.sock");
        let marker = root.join(".creating-late");
        let script = r#"
import os
from pathlib import Path
import subprocess
import sys
import time

marker = Path(sys.argv[1])
marker.write_text(str(os.getpid()))
time.sleep(0.25)
try:
    subprocess.run(
        ["tmux", "-S", sys.argv[2], "new-session", "-d", "-s", "main", "sleep 300"],
        check=True,
    )
finally:
    marker.unlink(missing_ok=True)
"#;
        let mut creator = Command::new("python3")
            .args(["-c", script])
            .arg(&marker)
            .arg(&socket)
            .spawn()
            .unwrap();
        wait_until(|| marker.exists(), "in-flight creator marker");
        owner.shutdown();
        assert!(creator.wait().unwrap().success());
        assert_ne!(probe_sync(&socket), ProbeResult::Live);
        assert!(!root.exists());
    }

    #[test]
    fn forced_creator_sigkill_before_publication_is_reclaimed_by_watchdog() {
        if which::which("tmux").is_err() {
            return;
        }
        let marker_dir = TempDir::new().unwrap();
        let marker = marker_dir.path().join("creator-ready");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tmux::test_server::tests::forced_termination_fixture",
                "--nocapture",
                "--ignored",
            ])
            .env("PHOENIX_TMUX_TEST_DEATH_MARKER", &marker)
            .env("PHOENIX_TMUX_TEST_DEATH_BEFORE_PUBLICATION", "1")
            .spawn()
            .unwrap();
        wait_until(|| marker.exists(), "hidden watchdog-owned server readiness");
        unsafe { libc::kill(child.id().cast_signed(), libc::SIGKILL) };
        assert_killed(child.wait().unwrap());
        let values = fs::read_to_string(&marker).unwrap();
        let mut values = values.lines();
        let root = PathBuf::from(values.next().unwrap());
        let control_root = PathBuf::from(values.next().unwrap());
        wait_until(
            || !root.exists() && !control_root.exists(),
            "watchdog cleanup after creator SIGKILL",
        );
    }

    #[test]
    fn forced_test_runner_termination_kills_exact_server() {
        run_forced_termination_case(false);
    }

    #[test]
    fn forced_test_runner_process_group_termination_kills_exact_server() {
        run_forced_termination_case(true);
    }

    #[test]
    #[ignore = "subprocess fixture; parent test terminates it"]
    fn forced_termination_fixture() {
        let Some(marker) = std::env::var_os("PHOENIX_TMUX_TEST_DEATH_MARKER") else {
            return;
        };
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        if std::env::var_os("PHOENIX_TMUX_TEST_DEATH_BEFORE_PUBLICATION").is_some() {
            let control_root = owner.control_root_path().to_path_buf();
            let control = control_root.join("creator-death.sock");
            let token = uuid::Uuid::new_v4().to_string();
            let env_file = control_root.join("creator-death-env.json");
            fs::write(
                &env_file,
                serde_json::to_vec(&vec![
                    ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
                    ("HOME".to_owned(), std::env::var("HOME").unwrap_or_default()),
                    (
                        "SHELL".to_owned(),
                        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned()),
                    ),
                    ("PHOENIX_TMUX_SERVER_TOKEN".to_owned(), token.clone()),
                ])
                .unwrap(),
            )
            .unwrap();
            let pending = control_root.join(".pending-spawn-creator-death");
            let request = control_root.join(".spawn-creator-death");
            fs::write(
                &pending,
                format!(
                    "creator-death.sock\tcreator-death.sock\t{}\t{}\t{token}\tcreator-death-env.json",
                    owner.path().join("_phoenix.tmux.conf").display(),
                    owner.path().display()
                ),
            )
            .unwrap();
            fs::rename(pending, request).unwrap();
            wait_until(
                || crate::tmux::probe::probe_sync(&control) == ProbeResult::Live,
                "hidden server before publication",
            );
            let marker = PathBuf::from(marker);
            let pending_marker = marker.with_extension("pending");
            fs::write(
                &pending_marker,
                format!("{}\n{}\n", owner.path().display(), control_root.display()),
            )
            .unwrap();
            fs::rename(pending_marker, marker).unwrap();
            std::mem::forget(owner);
            loop {
                thread::park();
            }
        }
        let marker = PathBuf::from(marker);
        let (socket, processes) = spawn_server_with_processes(&owner, "forced-death");
        let pending_marker = marker.with_extension("pending");
        let mut file = fs::File::create(&pending_marker).unwrap();
        writeln!(file, "{}", root.display()).unwrap();
        writeln!(file, "{}", socket.display()).unwrap();
        writeln!(file, "{}", processes.server.pid).unwrap();
        writeln!(file, "{}", processes.server.start_time).unwrap();
        writeln!(file, "{}", processes.pane.pid).unwrap();
        writeln!(file, "{}", processes.pane.start_time).unwrap();
        file.sync_all().unwrap();
        fs::rename(pending_marker, marker).unwrap();
        let mut parent_pipe = String::new();
        std::io::stdin().read_line(&mut parent_pipe).unwrap();
        drop(owner);
    }

    fn run_forced_termination_case(kill_group: bool) {
        if which::which("tmux").is_err() {
            return;
        }
        let marker_dir = TempDir::new().unwrap();
        let marker = marker_dir.path().join("ready");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "tmux::test_server::tests::forced_termination_fixture",
                "--nocapture",
                "--ignored",
            ])
            .env("PHOENIX_TMUX_TEST_DEATH_MARKER", &marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if kill_group {
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = command.spawn().unwrap();
        wait_until(|| marker.exists(), "forced-death fixture readiness");
        let paths = fs::read_to_string(&marker).unwrap();
        let mut paths = paths.lines();
        let root = PathBuf::from(paths.next().unwrap());
        let socket = PathBuf::from(paths.next().unwrap());
        let processes = TestServerProcesses {
            server: ProcessIdentity {
                pid: paths.next().unwrap().parse().unwrap(),
                start_time: paths.next().unwrap().parse().unwrap(),
            },
            pane: ProcessIdentity {
                pid: paths.next().unwrap().parse().unwrap(),
                start_time: paths.next().unwrap().parse().unwrap(),
            },
        };
        assert_eq!(probe_sync(&socket), ProbeResult::Live);

        let child_pid = i32::try_from(child.id()).expect("child pid fits pid_t");
        let result = if kill_group {
            unsafe { libc::killpg(child_pid, libc::SIGKILL) }
        } else {
            unsafe { libc::kill(child_pid, libc::SIGKILL) }
        };
        assert_eq!(result, 0, "failed to kill disposable test runner");
        let status = child.wait().unwrap();
        assert_killed(status);
        wait_until(|| !root.exists(), "watchdog cleanup after forced death");
        assert_ne!(probe_sync(&socket), ProbeResult::Live);
        assert_exact_processes_gone(processes);
    }

    fn assert_exact_processes_gone(processes: TestServerProcesses) {
        wait_until(
            || {
                !phoenix_core::process_identity::process_identity_matches(processes.server)
                    && !phoenix_core::process_identity::process_identity_matches(processes.pane)
            },
            "exact tmux server and pane-shell exit",
        );
    }

    #[cfg(unix)]
    fn assert_killed(status: ExitStatus) {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }
}
