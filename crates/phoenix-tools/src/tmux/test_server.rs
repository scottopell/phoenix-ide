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
import uuid

root = Path(sys.argv[1])
parent = int(sys.argv[2])
control_root = Path(sys.argv[3])
owned = []
unconfirmed_obligations = []
identity_timeout = float(os.environ.get("PHOENIX_TMUX_IDENTITY_TIMEOUT", "6.0"))
adoption_timeout = float(os.environ.get("PHOENIX_TMUX_ADOPTION_TIMEOUT", "1.0"))
cleanup_timeout = float(os.environ.get("PHOENIX_TMUX_CLEANUP_TIMEOUT", "6.5"))
quarantine_hook = os.environ.get("PHOENIX_TMUX_QUARANTINE_HOOK")
adoption_hook = os.environ.get("PHOENIX_TMUX_ADOPTION_HOOK")
provisional = []
adopted_pending_publication = []

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

def remaining_timeout(deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise RuntimeError("tmux identity registration deadline expired")
    return min(0.5, remaining)

def query_control_processes(control, deadline):
    observed = subprocess.run(
        ["tmux", "-S", str(control), "display-message", "-p", "#{pid}|#{pane_pid}"],
        stdin=subprocess.DEVNULL, capture_output=True, check=False, text=True,
        timeout=remaining_timeout(deadline),
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

def observe_control(control, expected_token, deadline):
    last_error = RuntimeError("tmux identity query did not run")
    while time.monotonic() < deadline:
        try:
            server_pid, pane_pid = query_control_processes(control, deadline)
            token_result = subprocess.run(
                ["tmux", "-S", str(control), "show-environment", "-g", "PHOENIX_TMUX_SERVER_TOKEN"],
                stdin=subprocess.DEVNULL, capture_output=True, check=False, text=True,
                timeout=remaining_timeout(deadline),
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
            remaining = deadline - time.monotonic()
            if remaining > 0:
                time.sleep(min(0.1, remaining))
    raise RuntimeError(f"tmux processes never became ready: {last_error}")

def reserve_spawn(socket, control):
    if any(existing_socket == socket or existing_control == control
           for existing_socket, existing_control in unconfirmed_obligations):
        raise RuntimeError("tmux spawn path already has an unresolved obligation")
    conflicts = [
        record for record in owned
        if record[0] == socket or record[3] == control
    ]
    if any(identity_state(identity) != "absent"
           for _, _, _, _, processes in conflicts for identity in processes):
        raise RuntimeError("live tmux ownership record already exists")
    obligation = (socket, control)
    unconfirmed_obligations.append(obligation)
    return obligation

def record_owned(socket, device, inode, control, identities):
    conflicts = [
        record for record in owned
        if record[0] == socket or record[3] == control
    ]
    if any(identity_state(identity) != "absent"
           for _, _, _, _, processes in conflicts for identity in processes):
        raise RuntimeError("live tmux ownership record already exists")
    for record in conflicts:
        owned.remove(record)
    owned.append((socket, device, inode, control, tuple(identities)))

def exact_record(socket, control, identities):
    expected = tuple(identities)
    for record in owned:
        if record[0] == socket and record[3] == control:
            if tuple(record[4]) == expected:
                return record
    return None

def tmux_format_literal(value):
    if not value or any(not (character.isascii() and (character.isalnum() or character == "-"))
                        for character in value):
        raise RuntimeError("tmux ownership token has invalid protocol characters")
    return value

def retire_record(record, deadline):
    _, device, inode, control, processes = record
    expected_token = tmux_format_literal(processes[0][2])
    states = [identity_state(identity) for identity in processes]
    if all(state == "absent" for state in states):
        return True
    if states[0] != "owned" or states[1] not in ("owned", "absent"):
        return False
    try:
        control_stat = control.stat()
        if control_stat.st_dev != device or control_stat.st_ino != inode:
            return False
        expected_server = processes[0][0]
        subprocess.run(
            ["tmux", "-S", str(control), "if-shell", "-F",
             f"#{{&&:#{{==:#{{pid}},{expected_server}}},#{{==:#{{PHOENIX_TMUX_SERVER_TOKEN}},{expected_token}}}}}",
             "kill-server", ""],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, check=False,
            timeout=remaining_timeout(deadline),
        )
    except (OSError, RuntimeError, subprocess.TimeoutExpired):
        return False
    while time.monotonic() < deadline:
        if all(identity_state(identity) == "absent" for identity in processes):
            return True
        time.sleep(min(0.05, max(0, deadline - time.monotonic())))
    return False

def spawn_owned(request):
    rejected = request.with_name(request.name.replace(".spawn-", ".rejected-", 1))
    acknowledged = request.with_name(request.name.replace(".spawn-", ".registered-", 1))
    control = None
    try:
        fields = request.read_text().split("\t")
        if len(fields) == 7:
            socket_name, control_name, config, cwd, token, env_path, adopt_name = fields
        elif len(fields) == 6:
            socket_name, control_name, config, cwd, token, env_path = fields
            adopt_name = request.name.replace(".spawn-", ".adopt-", 1)
        else:
            raise RuntimeError("spawn request was malformed")
        socket = root / socket_name
        control = control_root / control_name
        if socket.parent != root or control.parent != control_root or control.exists():
            raise RuntimeError("spawn paths are not unused exact children of owned roots")
        environment = dict(json.loads((control_root / env_path).read_text()))
        obligation = reserve_spawn(socket, control)
        spawned = subprocess.run(
            ["tmux", "-f", config, "-S", str(control), "new-session", "-d", "-c", cwd,
             "-s", "main", ";", "set-environment", "-g", "PHOENIX_TMUX_SERVER_TOKEN", token],
            stdin=subprocess.DEVNULL, capture_output=True, check=False, text=True, timeout=0.5,
            env=environment,
        )
        if spawned.returncode != 0:
            raise RuntimeError(f"tmux spawn failed: {spawned.stderr}")
        identities = observe_control(control, token, time.monotonic() + identity_timeout)
        control_stat = control.stat()
        record_owned(socket, control_stat.st_dev, control_stat.st_ino, control, identities)
        unconfirmed_obligations.remove(obligation)
        adopt = control_root / adopt_name
        if adopt.parent != control_root:
            raise RuntimeError("adoption marker is not an exact child of control root")
        adopted = request.with_name(request.name.replace(".spawn-", ".adopted-", 1))
        adoption_rejected = request.with_name(
            request.name.replace(".spawn-", ".adoption-rejected-", 1)
        )
        published = request.with_name(request.name.replace(".spawn-", ".published-", 1))
        publication_cancelled = request.with_name(
            request.name.replace(".spawn-", ".publication-cancelled-", 1)
        )
        publication_acknowledged = request.with_name(
            request.name.replace(".spawn-", ".publication-acknowledged-", 1)
        )
        provisional.append((socket, control, tuple(identities), adopt, adopted,
                            adoption_rejected, published, publication_cancelled,
                            publication_acknowledged, acknowledged,
                            time.monotonic() + adoption_timeout))
        publish_response(acknowledged, "\t".join(
            str(value) for identity in identities for value in identity[:2]
        ))
    except (OSError, RuntimeError, subprocess.TimeoutExpired, ValueError, json.JSONDecodeError) as error:
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

def remove_exact_visible(record):
    socket, device, inode, _, _ = record
    if not socket.exists():
        return True
    quarantine = root / f".retired-visible-{uuid.uuid4()}"
    try:
        os.replace(socket, quarantine)
        moved_stat = quarantine.stat()
        if moved_stat.st_dev != device or moved_stat.st_ino != inode:
            try:
                if not socket.exists():
                    os.replace(quarantine, socket)
            except OSError:
                pass
            return False
        quarantine.unlink()
        return True
    except FileNotFoundError:
        return True
    except OSError:
        return False

def remove_retired_control(record):
    _, device, inode, control, _ = record
    quarantine = control_root / f".retired-control-{uuid.uuid4()}"
    try:
        os.replace(control, quarantine)
        moved_stat = quarantine.stat()
        if moved_stat.st_dev != device or moved_stat.st_ino != inode:
            try:
                if not control.exists():
                    os.replace(quarantine, control)
            except OSError:
                pass
            return False
        quarantine.unlink()
        return True
    except FileNotFoundError:
        return True
    except OSError:
        return False

def retire(request):
    rejected = request.with_name(
        request.name.replace(".retire-request-", ".retire-rejected-", 1)
    )
    acknowledged = request.with_name(
        request.name.replace(".retire-request-", ".retired-", 1)
    )
    try:
        fields = request.read_text().split("\t")
        if len(fields) != 7:
            raise RuntimeError("retirement request was malformed")
        socket = root / fields[0]
        control = control_root / fields[1]
        expected_token = tmux_format_literal(fields[4])
        identities = ((int(fields[2]), fields[3], expected_token), (int(fields[5]), fields[6], None))
        record = exact_record(socket, control, identities)
        if record is None:
            raise RuntimeError("retirement identity did not match an owned record")
        if not retire_record(record, time.monotonic() + identity_timeout):
            raise RuntimeError("exact owned record retirement could not be proven")
        if not remove_retired_control(record):
            raise RuntimeError("retired control endpoint could not be removed exactly")
        owned.remove(record)
        publish_response(acknowledged, "retired")
    except (OSError, RuntimeError, subprocess.TimeoutExpired, ValueError) as error:
        try:
            publish_response(rejected, str(error))
        except OSError:
            return False
    finally:
        request.unlink(missing_ok=True)
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
        identities = observe_control(control, expected_token, time.monotonic() + identity_timeout)
        server_pid, pane_pid = str(identities[0][0]), str(identities[1][0])
        control_stat = control.stat()
        record_owned(socket, control_stat.st_dev, control_stat.st_ino, control, identities)
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
    now = time.monotonic()
    for item in list(adopted_pending_publication):
        (socket, control, identities, published, publication_cancelled,
         publication_acknowledged, adoption_rejected, deadline) = item
        record = exact_record(socket, control, identities)
        publish_valid = False
        if published.exists() and record is not None:
            try:
                socket_stat = socket.stat()
                control_stat = control.stat()
                publish_valid = (
                    socket_stat.st_dev == record[1] and socket_stat.st_ino == record[2]
                    and control_stat.st_dev == record[1] and control_stat.st_ino == record[2]
                )
            except OSError:
                publish_valid = False
        if publication_cancelled.exists():
            adopted_pending_publication.remove(item)
            retired = record is not None and retire_record(
                record, time.monotonic() + identity_timeout
            )
            cleaned = (retired and remove_exact_visible(record)
                       and remove_retired_control(record))
            if cleaned:
                owned.remove(record)
            else:
                unconfirmed_obligations.append((socket, control))
        elif publish_valid:
            try:
                publish_response(publication_acknowledged, "published")
                published.unlink(missing_ok=True)
                adopted_pending_publication.remove(item)
            except OSError:
                adopted_pending_publication.remove(item)
                unconfirmed_obligations.append((socket, control))
        elif published.exists() or now >= deadline:
            adopted_pending_publication.remove(item)
            retired = record is not None and retire_record(
                record, time.monotonic() + identity_timeout
            )
            cleaned = (retired and remove_exact_visible(record)
                       and remove_retired_control(record))
            if cleaned:
                owned.remove(record)
                try:
                    publish_response(adoption_rejected, "publication failed; exact server retired")
                except OSError:
                    pass
            else:
                unconfirmed_obligations.append((socket, control))
    for item in list(provisional):
        (socket, control, identities, adopt, adopted, adoption_rejected,
         published, publication_cancelled, publication_acknowledged,
         acknowledged, deadline) = item
        if adoption_hook:
            subprocess.run(
                [adoption_hook, str(adopt), "expired" if now >= deadline else "pending"],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, check=False, timeout=1.0,
            )
        if now < deadline and adopt.exists():
            provisional.remove(item)
            try:
                adopt.unlink()
                publish_response(adopted, "adopted")
                adopted_pending_publication.append((
                    socket, control, identities, published, publication_cancelled,
                    publication_acknowledged, adoption_rejected,
                    time.monotonic() + adoption_timeout
                ))
            except OSError:
                record = exact_record(socket, control, identities)
                retired = record is not None and retire_record(
                    record, time.monotonic() + identity_timeout
                )
                cleaned = (retired and remove_exact_visible(record)
                           and remove_retired_control(record))
                if cleaned:
                    owned.remove(record)
                else:
                    unconfirmed_obligations.append((socket, control))
        elif now >= deadline:
            provisional.remove(item)
            record = exact_record(socket, control, identities)
            retired = record is not None and retire_record(
                record, time.monotonic() + identity_timeout
            )
            if retired and remove_retired_control(record):
                owned.remove(record)
                publish_response(adoption_rejected, "lease expired; exact server retired")
            else:
                publish_response(
                    adoption_rejected,
                    "lease expired; exact provisional retirement could not be proven",
                )
                unconfirmed_obligations.append((socket, control))
    for request in control_root.glob(".retire-request-*"):
        if not retire(request):
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

cleanup_deadline = time.monotonic() + cleanup_timeout
for _, device, inode, control, processes in owned:
    if time.monotonic() >= cleanup_deadline:
        break
    try:
        expected_token = tmux_format_literal(processes[0][2])
    except RuntimeError:
        continue
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
             f"#{{&&:#{{==:#{{pid}},{expected_server}}},#{{==:#{{PHOENIX_TMUX_SERVER_TOKEN}},{expected_token}}}}}",
             "kill-server", ""],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=remaining_timeout(cleanup_deadline),
        )
    except (OSError, RuntimeError, subprocess.TimeoutExpired):
        pass

quiet = 0
while time.monotonic() < cleanup_deadline:
    unconfirmed = False
    states = [
        identity_state(identity)
        for _, _, _, _, processes in owned
        for identity in processes
    ]
    unconfirmed_state = any(state != "absent" for state in states) or bool(unconfirmed_obligations)
    registered_sockets = {
        socket: (device, inode, control, processes)
        for socket, device, inode, control, processes in owned
    }
    for socket in root.glob("*.sock"):
        if socket.is_symlink():
            socket.unlink(missing_ok=True)
            continue
        if not socket.is_socket():
            continue
        registered = registered_sockets.get(socket)
        if registered is not None:
            device, inode, control, processes = registered
            states = [identity_state(identity) for identity in processes]
            try:
                control_stat = control.stat()
            except OSError:
                unconfirmed = True
                continue
            if (control_stat.st_dev != device or control_stat.st_ino != inode
                    or any(state != "absent" for state in states)):
                unconfirmed = True
                continue
            if quarantine_hook:
                subprocess.run(
                    [quarantine_hook, str(socket)],
                    stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL, check=False,
                    timeout=remaining_timeout(cleanup_deadline),
                )
            quarantine = root / f".quarantine-{uuid.uuid4()}"
            try:
                os.replace(socket, quarantine)
                moved_stat = quarantine.stat()
                if moved_stat.st_dev == device and moved_stat.st_ino == inode:
                    quarantine.unlink()
                else:
                    try:
                        if not socket.exists():
                            os.replace(quarantine, socket)
                    except OSError:
                        pass
                    unconfirmed = True
            except FileNotFoundError:
                pass
            except OSError:
                unconfirmed = True
            continue
        try:
            probe = subprocess.run(
                ["tmux", "-S", str(socket), "list-sessions"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
                timeout=remaining_timeout(cleanup_deadline),
            )
            if probe.returncode == 0:
                killed = subprocess.run(
                    ["tmux", "-S", str(socket), "kill-server"],
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=False,
                    timeout=remaining_timeout(cleanup_deadline),
                )
                if killed.returncode == 0:
                    socket.unlink(missing_ok=True)
                else:
                    unconfirmed = True
            else:
                unconfirmed = True
        except (OSError, RuntimeError, subprocess.TimeoutExpired):
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
    quiet = quiet + 1 if not unconfirmed_state and not unconfirmed and not sockets and not creators else 0
    if quiet >= 5:
        if root.exists():
            shutil.rmtree(root)
        if control_root.exists():
            shutil.rmtree(control_root)
        sys.exit(0)
    time.sleep(min(0.1, max(0, cleanup_deadline - time.monotonic())))
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
        Self::new_with_watchdog_options(watchdog_path, None)
    }

    fn new_with_watchdog_options(
        watchdog_path: Option<&Path>,
        identity_timeout: Option<Duration>,
    ) -> Self {
        Self::new_with_watchdog_test_options(
            watchdog_path,
            identity_timeout,
            None,
            None,
            None,
            None,
        )
    }

    fn new_with_watchdog_test_options(
        watchdog_path: Option<&Path>,
        identity_timeout: Option<Duration>,
        cleanup_timeout: Option<Duration>,
        quarantine_hook: Option<&Path>,
        adoption_timeout: Option<Duration>,
        adoption_hook: Option<&Path>,
    ) -> Self {
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
        if let Some(timeout) = identity_timeout {
            command.env(
                "PHOENIX_TMUX_IDENTITY_TIMEOUT",
                timeout.as_secs_f64().to_string(),
            );
        }
        if let Some(timeout) = cleanup_timeout {
            command.env(
                "PHOENIX_TMUX_CLEANUP_TIMEOUT",
                timeout.as_secs_f64().to_string(),
            );
        }
        if let Some(timeout) = adoption_timeout {
            command.env(
                "PHOENIX_TMUX_ADOPTION_TIMEOUT",
                timeout.as_secs_f64().to_string(),
            );
        }
        if let Some(hook) = adoption_hook {
            command.env("PHOENIX_TMUX_ADOPTION_HOOK", hook);
        }
        if let Some(hook) = quarantine_hook {
            command.env("PHOENIX_TMUX_QUARANTINE_HOOK", hook);
        }
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

fn write_server_environment(path: &Path, server_env: &[(String, String)]) -> io::Result<()> {
    fs::write(
        path,
        serde_json::to_vec(server_env).map_err(io::Error::other)?,
    )
}

#[derive(Debug)]
pub(crate) struct AdoptedTestServer {
    pub(crate) processes: TestServerProcesses,
    published: PathBuf,
    publication_acknowledged: PathBuf,
    publication_cancelled: PathBuf,
    adoption_rejected: PathBuf,
    committed: bool,
}

impl Drop for AdoptedTestServer {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::write(&self.publication_cancelled, []);
        }
    }
}

impl AdoptedTestServer {
    fn new(
        processes: TestServerProcesses,
        published: PathBuf,
        publication_acknowledged: PathBuf,
        publication_cancelled: PathBuf,
        adoption_rejected: PathBuf,
    ) -> Self {
        Self {
            processes,
            published,
            publication_acknowledged,
            publication_cancelled,
            adoption_rejected,
            committed: false,
        }
    }

    pub(crate) async fn commit_publication(mut self) -> io::Result<TestServerProcesses> {
        fs::write(&self.published, [])?;
        let deadline = tokio::time::Instant::now() + CLEANUP_TIMEOUT;
        loop {
            if self.publication_acknowledged.exists() {
                self.committed = true;
                return Ok(self.processes);
            }
            if let Ok(reason) = fs::read_to_string(&self.adoption_rejected) {
                return Err(io::Error::other(format!(
                    "tmux watchdog rejected publication: {reason}"
                )));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "tmux watchdog did not acknowledge visible publication",
                ));
            }
            tokio::task::yield_now().await;
        }
    }

    pub(crate) async fn retire(
        mut self,
        socket: &Path,
        control_socket: &Path,
        expected_token: &str,
    ) -> io::Result<()> {
        retire_owned_server(socket, control_socket, self.processes, expected_token).await?;
        self.committed = true;
        Ok(())
    }
}

pub(crate) async fn spawn_owned_server(
    socket: &Path,
    control_socket: &Path,
    config_path: &Path,
    cwd: &Path,
    token: &str,
    server_env: &[(String, String)],
) -> io::Result<AdoptedTestServer> {
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
    let adopted = control_root.join(format!(".adopt-{nonce}"));
    let adoption_acknowledged = control_root.join(format!(".adopted-{nonce}"));
    let adoption_rejected = control_root.join(format!(".adoption-rejected-{nonce}"));
    let published = control_root.join(format!(".published-{nonce}"));
    let publication_cancelled = control_root.join(format!(".publication-cancelled-{nonce}"));
    let publication_acknowledged = control_root.join(format!(".publication-acknowledged-{nonce}"));
    let env_file = control_root.join(format!(".environment-{nonce}.json"));
    write_server_environment(&env_file, server_env)?;
    let _artifacts = RegistrationArtifacts {
        paths: vec![
            pending.clone(),
            request.clone(),
            rejected.clone(),
            control_root.join(format!(".pending-rejected-{nonce}")),
            env_file.clone(),
        ],
    };
    fs::write(
        &pending,
        format!(
            "{socket_name}\t{control_name}\t{}\t{}\t{token}\t{}\t{}",
            protocol_field(config_path, "config path")?,
            protocol_field(cwd, "cwd")?,
            protocol_field(
                Path::new(env_file.file_name().expect("env file has name")),
                "env file",
            )?,
            protocol_field(
                Path::new(adopted.file_name().expect("adoption marker has name")),
                "adoption marker",
            )?
        ),
    )?;
    fs::rename(pending, request)?;
    let deadline = tokio::time::Instant::now() + CLEANUP_TIMEOUT;
    let mut registered = None;
    loop {
        if registered.is_none() {
            if let Ok(value) = fs::read_to_string(&acknowledged) {
                registered = Some(parse_acknowledged_processes(&value)?);
                fs::write(&adopted, [])?;
            }
        }
        if adoption_acknowledged.exists() {
            let processes = registered.ok_or_else(|| {
                io::Error::other("tmux watchdog acknowledged adoption before registration")
            })?;
            return Ok(AdoptedTestServer::new(
                processes,
                published,
                publication_acknowledged,
                publication_cancelled,
                adoption_rejected,
            ));
        }
        if let Ok(reason) = fs::read_to_string(&adoption_rejected) {
            return Err(io::Error::other(format!(
                "tmux watchdog rejected adoption: {reason}"
            )));
        }
        if let Ok(reason) = fs::read_to_string(&rejected) {
            return Err(io::Error::other(format!(
                "tmux watchdog rejected spawn: {reason}"
            )));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux watchdog did not acknowledge spawn adoption",
            ));
        }
        tokio::task::yield_now().await;
    }
}

pub(crate) async fn retire_owned_server(
    socket: &Path,
    control_socket: &Path,
    processes: TestServerProcesses,
    expected_token: &str,
) -> io::Result<()> {
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
    let request = control_root.join(format!(".retire-request-{nonce}"));
    let pending = control_root.join(format!(".pending-retire-{nonce}"));
    let retired = control_root.join(format!(".retired-{nonce}"));
    let rejected = control_root.join(format!(".retire-rejected-{nonce}"));
    fs::write(
        &pending,
        format!(
            "{socket_name}\t{control_name}\t{}\t{}\t{expected_token}\t{}\t{}",
            processes.server.pid,
            processes.server.start_time,
            processes.pane.pid,
            processes.pane.start_time,
        ),
    )?;
    fs::rename(&pending, &request)?;
    let deadline = tokio::time::Instant::now() + CLEANUP_TIMEOUT;
    loop {
        if retired.exists() {
            return Ok(());
        }
        if let Ok(reason) = fs::read_to_string(&rejected) {
            return Err(io::Error::other(format!(
                "tmux watchdog rejected retirement: {reason}"
            )));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "tmux watchdog did not acknowledge retirement",
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
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
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
    fn cleanup_fails_closed_after_accepted_server_token_mutation() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let control = control_root.join("mutated-global-token.sock");
        let (socket, processes) = spawn_server_with_processes(&owner, "mutated-global-token");
        let status = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(["set-environment", "-gu", "PHOENIX_TMUX_SERVER_TOKEN"])
            .env_remove("TMUX")
            .status()
            .unwrap();
        assert!(status.success());

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(panic.is_err());
        assert_eq!(probe_sync(&control), ProbeResult::Live);
        assert!(phoenix_core::process_identity::process_identity_matches(
            processes.server
        ));
        assert!(Command::new("tmux")
            .args(["-S", &control.to_string_lossy(), "kill-server"])
            .status()
            .unwrap()
            .success());
        assert_exact_processes_gone(processes);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn contained_spawn_retries_until_process_identity_is_ready() {
        assert!(WATCHDOG_PROGRAM.contains("while time.monotonic() < deadline:"));
        assert!(WATCHDOG_PROGRAM.contains("time.sleep(min(0.1, remaining))"));
        assert!(WATCHDOG_PROGRAM.contains("tmux processes never became ready"));
    }

    #[test]
    fn identity_registration_uses_one_deadline_shorter_than_owner_handoff() {
        assert!(WATCHDOG_PROGRAM.contains("timeout=remaining_timeout(deadline)"));
        assert!(WATCHDOG_PROGRAM
            .contains("observe_control(control, token, time.monotonic() + identity_timeout)"));
        assert!(CLEANUP_TIMEOUT > Duration::from_secs(6));
    }

    #[test]
    fn slow_identity_probes_share_one_absolute_deadline() {
        let fake_bin = TempDir::new().unwrap();
        write_tmux_wrapper(
            fake_bin.path(),
            "#!/bin/sh\ncase \"$*\" in *new-session*) : > \"$2\"; exit 0;; esac\n/bin/sleep 0.18\nexit 1\n",
        );
        let owner = TestTmuxServerOwner::new_with_watchdog_options(
            Some(fake_bin.path()),
            Some(Duration::from_millis(250)),
        );
        let control_root = owner.control_root_path().to_path_buf();
        let root = owner.path().to_path_buf();
        let env_file = control_root.join("slow-env.json");
        fs::write(
            &env_file,
            serde_json::to_vec(&vec![(
                "PATH".to_owned(),
                fake_bin.path().to_string_lossy().into_owned(),
            )])
            .unwrap(),
        )
        .unwrap();
        let started = Instant::now();
        fs::write(
            control_root.join(".spawn-slow"),
            format!(
                "slow.sock\tslow.sock\t{}\t{}\ttoken\tslow-env.json",
                root.join("config").display(),
                root.display()
            ),
        )
        .unwrap();
        wait_until(
            || control_root.join(".rejected-slow").exists(),
            "deadline-bounded rejection",
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "identity registration exceeded one short absolute deadline: {:?}",
            started.elapsed()
        );
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));
        assert!(panic.is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn duplicate_owned_records_require_explicit_prior_absence_before_replacement() {
        assert!(WATCHDOG_PROGRAM.contains("if any(identity_state(identity) != \"absent\""));
        assert!(WATCHDOG_PROGRAM.contains("for record in conflicts:\n        owned.remove(record)"));
        assert_eq!(WATCHDOG_PROGRAM.matches("owned.append(").count(), 1);
    }

    #[test]
    fn successful_identity_reconciliation_removes_the_spawn_obligation() {
        assert!(WATCHDOG_PROGRAM.contains(
            "identities = observe_control(control, token, time.monotonic() + identity_timeout)\n        control_stat = control.stat()\n        record_owned(socket, control_stat.st_dev, control_stat.st_ino, control, identities)\n        unconfirmed_obligations.remove(obligation)"
        ));
    }

    #[test]
    fn failed_spawn_never_kills_through_an_unauthenticated_control_path() {
        assert!(!WATCHDOG_PROGRAM
            .contains("subprocess.run([\"tmux\", \"-S\", str(control), \"kill-server\"]"));
    }

    #[test]
    fn per_iteration_probe_uncertainty_blocks_quiet_success() {
        assert!(WATCHDOG_PROGRAM
            .contains("while time.monotonic() < cleanup_deadline:\n    unconfirmed = False"));
        assert!(WATCHDOG_PROGRAM.contains(
            "not unconfirmed_state and not unconfirmed and not sockets and not creators"
        ));
    }

    #[test]
    fn spawn_path_is_reserved_before_tmux_starts() {
        let reservation = WATCHDOG_PROGRAM
            .find("obligation = reserve_spawn(socket, control)")
            .unwrap();
        let spawn = WATCHDOG_PROGRAM.find("spawned = subprocess.run(").unwrap();
        assert!(reservation < spawn);
        assert!(WATCHDOG_PROGRAM
            .contains("for existing_socket, existing_control in unconfirmed_obligations"));
    }

    #[tokio::test]
    async fn duplicate_registry_spawns_leave_no_unresolved_obligation() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let scope = phoenix_core::work_scope::ResourceScopeKey::Work(
            phoenix_core::work_scope::WorkScopeId::parse("duplicate-spawn").unwrap(),
        );
        let first = owner.registry();
        let second = owner.registry();

        let (first_result, second_result) = tokio::join!(
            first.ensure_live(&scope, owner.path(), None, None),
            second.ensure_live(&scope, owner.path(), None, None),
        );

        assert_eq!(
            usize::from(first_result.is_ok()) + usize::from(second_result.is_ok()),
            1
        );
        owner.shutdown();
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
            "if-shell -F #{{&&:#{{==:#{{pid}},{}}},#{{==:#{{PHOENIX_TMUX_SERVER_TOKEN}},",
            processes.server.pid
        )));
        assert!(requests.contains("}} kill-server"));
        assert!(
            !requests.contains("#{==:#{PHOENIX_TMUX_SERVER_TOKEN},#{PHOENIX_TMUX_SERVER_TOKEN}}")
        );
    }

    #[test]
    fn matching_pid_with_mismatched_token_does_not_kill_server() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let socket = owner.path().join("token-mismatch.sock");
        let control = owner.control_root_path().join("token-mismatch.sock");
        let (_, processes) = spawn_server_with_processes(&owner, "token-mismatch");
        let predicate = format!(
            "#{{&&:#{{==:#{{pid}},{}}},#{{==:#{{PHOENIX_TMUX_SERVER_TOKEN}},wrong-token}}}}",
            processes.server.pid
        );

        assert!(Command::new("tmux")
            .args([
                "-S",
                &control.to_string_lossy(),
                "if-shell",
                "-F",
                &predicate,
                "kill-server",
                "",
            ])
            .status()
            .unwrap()
            .success());
        assert_eq!(probe_sync(&control), ProbeResult::Live);
        assert!(socket.exists());
        owner.shutdown();
        assert_exact_processes_gone(processes);
    }

    #[test]
    fn malformed_retirement_token_is_rejected_and_server_survives() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let socket = owner.path().join("malformed-token.sock");
        let control = owner.control_root_path().join("malformed-token.sock");
        let (_, processes) = spawn_server_with_processes(&owner, "malformed-token");
        fs::write(
            owner.control_root_path().join(".retire-request-malformed"),
            format!(
                "malformed-token.sock\tmalformed-token.sock\t{}\t{}\tbad,token\t{}\t{}",
                processes.server.pid,
                processes.server.start_time,
                processes.pane.pid,
                processes.pane.start_time
            ),
        )
        .unwrap();
        wait_until(
            || {
                owner
                    .control_root_path()
                    .join(".retire-rejected-malformed")
                    .exists()
            },
            "malformed retirement rejection",
        );

        assert_eq!(probe_sync(&control), ProbeResult::Live);
        assert!(socket.exists());
        owner.shutdown();
        assert_exact_processes_gone(processes);
    }

    #[test]
    fn retirement_wire_and_lookup_include_expected_token() {
        assert!(WATCHDOG_PROGRAM.contains("expected_token = tmux_format_literal(fields[4])"));
        assert!(
            WATCHDOG_PROGRAM.contains("identities = ((int(fields[2]), fields[3], expected_token),")
        );
        assert!(WATCHDOG_PROGRAM.contains("if tuple(record[4]) == expected:"));
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
    fn intact_control_anchor_permits_exact_dead_visible_socket_unlink() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let socket = owner.path().join("dead-visible.sock");
        let control = owner.control_root_path().join("dead-visible.sock");
        let (_, processes) = spawn_server_with_processes(&owner, "dead-visible");
        assert!(Command::new("tmux")
            .args(["-S", &control.to_string_lossy(), "kill-server"])
            .status()
            .unwrap()
            .success());
        assert_exact_processes_gone(processes);
        assert!(socket.exists());
        assert!(control.exists());

        owner.shutdown();

        assert!(!socket.exists());
    }

    #[test]
    fn missing_original_control_anchor_preserves_visible_replacement() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let control = control_root.join("missing-anchor.sock");
        let (socket, processes) = spawn_server_with_processes(&owner, "missing-anchor");
        assert!(Command::new("tmux")
            .args(["-S", &control.to_string_lossy(), "kill-server"])
            .status()
            .unwrap()
            .success());
        assert_exact_processes_gone(processes);
        fs::remove_file(&control).ok();
        fs::remove_file(&socket).unwrap();
        let replacement = std::os::unix::net::UnixListener::bind(&socket).unwrap();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(panic.is_err(), "missing control anchor must fail closed");
        assert!(socket.exists(), "visible replacement must remain untouched");
        drop(replacement);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    fn write_adoption_environment(control_root: &Path, name: &str, token: &str) {
        fs::write(
            control_root.join(name),
            serde_json::to_vec(&vec![
                ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
                ("PHOENIX_TMUX_SERVER_TOKEN".to_owned(), token.to_owned()),
            ])
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn adoption_wins_at_serialized_boundary_and_server_remains_stable() {
        if which::which("tmux").is_err() {
            return;
        }
        let hook_dir = TempDir::new().unwrap();
        let hook = hook_dir.path().join("adopt-pending");
        fs::write(
            &hook,
            "#!/bin/sh\nif [ \"$2\" = pending ]; then : > \"$1\"; fi\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            None,
            None,
            None,
            Some(Duration::from_secs(1)),
            Some(&hook),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        write_adoption_environment(&control_root, "accept-env.json", "accept-token");
        fs::write(
            control_root.join(".spawn-accept"),
            format!(
                "accept.sock\taccept.sock\t{}\t{}\taccept-token\taccept-env.json\t.adopt-accept",
                root.join("config").display(),
                root.display()
            ),
        )
        .unwrap();
        wait_until(
            || control_root.join(".registered-accept").exists(),
            "provisional acceptance registration",
        );
        let processes = parse_acknowledged_processes(
            &fs::read_to_string(control_root.join(".registered-accept")).unwrap(),
        )
        .unwrap();
        wait_until(
            || control_root.join(".adopted-accept").exists(),
            "watchdog adoption acknowledgement",
        );

        assert!(!control_root.join(".adoption-rejected-accept").exists());
        assert!(phoenix_core::process_identity::process_identity_matches(
            processes.server
        ));
        assert_eq!(
            probe_sync(&control_root.join("accept.sock")),
            ProbeResult::Live
        );
        owner.shutdown();
        assert_exact_processes_gone(processes);
    }

    #[tokio::test]
    async fn cancellation_after_adoption_before_publication_retires_and_allows_respawn() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let socket = root.join("cancel-before-publish.sock");
        let control = control_root.join("cancel-before-publish.sock");
        let token = "cancel-before-publish-token";
        let environment = vec![
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            ("PHOENIX_TMUX_SERVER_TOKEN".to_owned(), token.to_owned()),
        ];
        let adopted = spawn_owned_server(
            &socket,
            &control,
            &root.join("config"),
            &root,
            token,
            &environment,
        )
        .await
        .unwrap();
        let cancelled_processes = adopted.processes;

        drop(adopted);
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        while phoenix_core::process_identity::process_identity_matches(cancelled_processes.server)
            || socket.exists()
            || control.exists()
        {
            assert!(
                Instant::now() < deadline,
                "cancelled adopted server cleanup did not complete"
            );
            tokio::task::yield_now().await;
        }

        let registry = owner.registry();
        let scope = phoenix_core::work_scope::ResourceScopeKey::Work(
            phoenix_core::work_scope::WorkScopeId::parse("cancel-before-publish").unwrap(),
        );
        let replacement = registry
            .ensure_live(&scope, &root, None, None)
            .await
            .expect("subsequent ensure_live must recover cancelled adoption");
        assert_ne!(replacement.read().await.server_token, token);
        owner.shutdown();
    }

    #[tokio::test]
    async fn cancellation_after_visible_link_before_confirmation_removes_link_and_server() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let socket = root.join("cancel-after-link.sock");
        let control = control_root.join("cancel-after-link.sock");
        let token = "cancel-after-link-token";
        let environment = vec![
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            ("PHOENIX_TMUX_SERVER_TOKEN".to_owned(), token.to_owned()),
        ];
        let adopted = spawn_owned_server(
            &socket,
            &control,
            &root.join("config"),
            &root,
            token,
            &environment,
        )
        .await
        .unwrap();
        let processes = adopted.processes;
        fs::hard_link(&control, &socket).unwrap();

        drop(adopted);
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        while phoenix_core::process_identity::process_identity_matches(processes.server)
            || socket.exists()
            || control.exists()
        {
            assert!(
                Instant::now() < deadline,
                "linked cancelled server cleanup did not complete"
            );
            tokio::task::yield_now().await;
        }
        owner.shutdown();
    }

    #[test]
    fn adoption_ack_io_failure_routes_exact_record_through_cleanup() {
        if which::which("tmux").is_err() {
            return;
        }
        let hook_dir = TempDir::new().unwrap();
        let hook = hook_dir.path().join("block-adoption-ack");
        fs::write(
            &hook,
            "#!/bin/sh\nadopted=$(printf '%s' \"$1\" | sed 's/.adopt-/.adopted-/')\nmkdir -p \"$adopted\"\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            None,
            None,
            None,
            Some(Duration::from_secs(1)),
            Some(&hook),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        write_adoption_environment(&control_root, "io-failure-env.json", "io-failure-token");
        fs::write(
            control_root.join(".spawn-io-failure"),
            format!(
                "io-failure.sock\tio-failure.sock\t{}\t{}\tio-failure-token\tio-failure-env.json\t.adopt-io-failure",
                root.join("config").display(),
                root.display()
            ),
        )
        .unwrap();
        wait_until(
            || control_root.join(".registered-io-failure").exists(),
            "I/O failure provisional registration",
        );
        let processes = parse_acknowledged_processes(
            &fs::read_to_string(control_root.join(".registered-io-failure")).unwrap(),
        )
        .unwrap();
        fs::write(control_root.join(".adopt-io-failure"), []).unwrap();
        wait_until(
            || !phoenix_core::process_identity::process_identity_matches(processes.server),
            "I/O failure exact retirement",
        );
        assert!(!control_root.join("io-failure.sock").exists());
        fs::remove_dir_all(control_root.join(".adopted-io-failure")).unwrap();
        owner.shutdown();
    }

    #[tokio::test]
    async fn cancellation_while_waiting_for_adoption_ack_never_publishes() {
        if which::which("tmux").is_err() {
            return;
        }
        let hook_dir = TempDir::new().unwrap();
        let entered = hook_dir.path().join("entered");
        let hook = hook_dir.path().join("block-adoption");
        fs::write(
            &hook,
            format!("#!/bin/sh\n: > '{}'\n/bin/sleep 0.4\n", entered.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            None,
            None,
            None,
            Some(Duration::from_secs(2)),
            Some(&hook),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let socket = root.join("cancel-adoption.sock");
        let control = control_root.join("cancel-adoption.sock");
        let config = root.join("config");
        let environment = vec![
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            (
                "PHOENIX_TMUX_SERVER_TOKEN".to_owned(),
                "cancel-adoption-token".to_owned(),
            ),
        ];
        let task = tokio::spawn({
            let socket = socket.clone();
            let control = control.clone();
            let root = root.clone();
            async move {
                spawn_owned_server(
                    &socket,
                    &control,
                    &config,
                    &root,
                    "cancel-adoption-token",
                    &environment,
                )
                .await
            }
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while !entered.exists() && !task.is_finished() {
            assert!(
                Instant::now() < deadline,
                "watchdog adoption hook was not reached"
            );
            tokio::task::yield_now().await;
        }
        assert!(
            !task.is_finished(),
            "spawn waiter completed before cancellation"
        );

        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(
            !socket.exists(),
            "cancelled waiter must not publish visible socket"
        );
        wait_until(
            || {
                fs::read_dir(&control_root).unwrap().any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".adopted-")
                })
            },
            "watchdog ownership transition after waiter cancellation",
        );
        owner.shutdown();
        assert!(!root.exists());
        assert!(!control_root.exists());
    }

    #[tokio::test]
    async fn acknowledged_but_unadopted_spawn_expires_and_is_retired() {
        if which::which("tmux").is_err() {
            return;
        }
        let hook_dir = TempDir::new().unwrap();
        let hook = hook_dir.path().join("late-adopt");
        fs::write(
            &hook,
            "#!/bin/sh\nif [ \"$2\" = expired ]; then : > \"$1\"; fi\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            None,
            None,
            None,
            Some(Duration::ZERO),
            Some(&hook),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        write_adoption_environment(&control_root, "lease-env.json", "lease-token");
        fs::write(
            control_root.join(".spawn-lease"),
            format!(
                "lease.sock\tlease.sock\t{}\t{}\tlease-token\tlease-env.json\t.adopt-lease",
                root.join("config").display(),
                root.display()
            ),
        )
        .unwrap();
        wait_until(
            || control_root.join(".registered-lease").exists(),
            "provisional spawn acknowledgement",
        );
        let processes = parse_acknowledged_processes(
            &fs::read_to_string(control_root.join(".registered-lease")).unwrap(),
        )
        .unwrap();
        wait_until(
            || control_root.join(".adoption-rejected-lease").exists(),
            "lease-expiry retirement decision",
        );
        assert!(!control_root.join(".adopted-lease").exists());
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        while phoenix_core::process_identity::process_identity_matches(processes.server)
            || phoenix_core::process_identity::process_identity_matches(processes.pane)
        {
            assert!(
                Instant::now() < deadline,
                "provisional server did not retire"
            );
            tokio::task::yield_now().await;
        }
        owner.shutdown();
        assert!(!root.exists());
        assert!(!control_root.exists());
    }

    #[test]
    fn cleanup_deadline_bounds_every_cleanup_subprocess() {
        assert!(WATCHDOG_PROGRAM.contains("cleanup_deadline = time.monotonic() + cleanup_timeout"));
        assert!(WATCHDOG_PROGRAM.contains("timeout=remaining_timeout(cleanup_deadline)"));
        assert!(!WATCHDOG_PROGRAM.contains("for _ in range(50):"));
        assert!(CLEANUP_TIMEOUT > Duration::from_secs_f64(6.5));
    }

    #[test]
    fn final_visible_unlink_moves_and_authenticates_quarantine_entry() {
        assert!(WATCHDOG_PROGRAM.contains("os.replace(socket, quarantine)"));
        assert!(WATCHDOG_PROGRAM.contains("moved_stat = quarantine.stat()"));
        assert!(
            WATCHDOG_PROGRAM.contains("moved_stat.st_dev == device and moved_stat.st_ino == inode")
        );
        assert!(WATCHDOG_PROGRAM.contains("os.replace(quarantine, socket)"));
    }

    #[test]
    fn replacement_at_final_quarantine_boundary_is_restored_and_preserved() {
        if which::which("tmux").is_err() {
            return;
        }
        let hook_dir = TempDir::new().unwrap();
        let hook = hook_dir.path().join("replace-socket");
        write_tmux_wrapper(
            hook_dir.path(),
            "#!/bin/sh\nrm -f \"$1\"\npython3 - \"$1\" <<'PY'\nimport socket, sys\ns = socket.socket(socket.AF_UNIX)\ns.bind(sys.argv[1])\ns.close()\nPY\n",
        );
        fs::rename(hook_dir.path().join("tmux"), &hook).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            None,
            None,
            Some(&hook),
            None,
            None,
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let socket = root.join("final-swap.sock");
        let control = control_root.join("final-swap.sock");
        let (_, processes) = spawn_server_with_processes(&owner, "final-swap");
        assert!(Command::new("tmux")
            .args(["-S", &control.to_string_lossy(), "kill-server"])
            .status()
            .unwrap()
            .success());
        assert_exact_processes_gone(processes);

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(panic.is_err());
        assert!(socket.exists(), "replacement must be restored or preserved");
        assert_ne!(
            fs::metadata(&socket).unwrap().ino(),
            fs::metadata(&control).unwrap().ino()
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn short_shared_cleanup_deadline_retains_unvisited_owned_records() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            None,
            Some(Duration::from_millis(1)),
            None,
            None,
            None,
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let mut processes = Vec::new();
        for index in 0..4 {
            let (_, captured) = spawn_server_with_processes(&owner, &format!("bounded-{index}"));
            processes.push(captured);
        }
        let started = Instant::now();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(panic.is_err());
        assert!(started.elapsed() < CLEANUP_TIMEOUT);
        assert!(processes.iter().any(|captured| {
            phoenix_core::process_identity::process_identity_matches(captured.server)
        }));
        for index in 0..4 {
            let control = control_root.join(format!("bounded-{index}.sock"));
            let _ = Command::new("tmux")
                .args(["-S", &control.to_string_lossy(), "kill-server"])
                .status();
        }
        for captured in processes {
            assert_exact_processes_gone(captured);
        }
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn cleanup_uses_incarnation_bound_socket_without_numeric_pid_signals() {
        assert!(!WATCHDOG_PROGRAM.contains("os.kill(identity[0]"));
        assert!(!WATCHDOG_PROGRAM.contains("os.link(socket, control)"));
        assert!(WATCHDOG_PROGRAM.contains(
            "[\"tmux\", \"-S\", str(control), \"if-shell\", \"-F\",\n             f\"#{{&&:#{{==:#{{pid}},{expected_server}}},#{{==:#{{PHOENIX_TMUX_SERVER_TOKEN}},{expected_token}}}}}\",\n             \"kill-server\", \"\"]"
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
