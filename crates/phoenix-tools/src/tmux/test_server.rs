use std::ffi::CString;
use std::fs;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
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
import itertools
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
root_stat = root.stat()
root_identity = (root_stat.st_dev, root_stat.st_ino)
root_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
control_root_fd = os.open(control_root, os.O_RDONLY | os.O_DIRECTORY)
control_root_stat = control_root.stat()
control_root_identity = (control_root_stat.st_dev, control_root_stat.st_ino)
owned = []
retained_controls = {}
unconfirmed_obligations = []
cleanup_deadline = None
cleanup_failed = False
identity_timeout = float(os.environ.get("PHOENIX_TMUX_IDENTITY_TIMEOUT", "6.0"))
adoption_timeout = float(os.environ.get("PHOENIX_TMUX_ADOPTION_TIMEOUT", "1.0"))
publication_timeout = float(os.environ.get("PHOENIX_TMUX_PUBLICATION_TIMEOUT", "1.0"))
cleanup_timeout = float(os.environ.get("PHOENIX_TMUX_CLEANUP_TIMEOUT", "6.5"))
heartbeat_stale = 0.5
quarantine_hook = os.environ.get("PHOENIX_TMUX_QUARANTINE_HOOK")
adoption_hook = os.environ.get("PHOENIX_TMUX_ADOPTION_HOOK")
publication_hook = os.environ.get("PHOENIX_TMUX_PUBLICATION_HOOK")
record_hook = os.environ.get("PHOENIX_TMUX_RECORD_HOOK")
retirement_hook = os.environ.get("PHOENIX_TMUX_RETIREMENT_HOOK")
root_quarantine_hook = os.environ.get("PHOENIX_TMUX_ROOT_QUARANTINE_HOOK")
completion_hook = os.environ.get("PHOENIX_TMUX_COMPLETION_HOOK")
provisional = []
adopted_pending_publication = []
preserved_visible_paths = set()
cleanup_record_status = {}

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

def process(pid, timeout=0.5):
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
            timeout=timeout,
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
    except OverflowError:
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

class DeadlineExpired(RuntimeError):
    pass

class IdentityOutputError(RuntimeError):
    pass

def remaining_timeout(deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise DeadlineExpired("tmux identity registration deadline expired")
    return min(0.5, remaining)

def query_control_processes(control, deadline):
    server = subprocess.run(
        ["tmux", "-S", str(control), "display-message", "-p", "#{pid}"],
        stdin=subprocess.DEVNULL, capture_output=True, check=False, text=True,
        timeout=remaining_timeout(deadline),
    )
    panes = subprocess.run(
        ["tmux", "-S", str(control), "list-panes", "-a", "-F", "#{pane_pid}"],
        stdin=subprocess.DEVNULL, capture_output=True, check=False, text=True,
        timeout=remaining_timeout(deadline),
    )
    server_pid = server.stdout.removesuffix("\n")
    pane_pids = panes.stdout.splitlines()
    if (server.returncode != 0 or panes.returncode != 0 or not pane_pids
            or not server_pid.isascii() or not server_pid.isdecimal()
            or any(not pid.isascii() or not pid.isdecimal() for pid in pane_pids)):
        raise IdentityOutputError("tmux identity output was malformed")
    server_pid = int(server_pid)
    pane_pids = tuple(dict.fromkeys(int(pid) for pid in pane_pids))
    if server_pid > 2_147_483_647 or any(pid > 2_147_483_647 for pid in pane_pids):
        raise IdentityOutputError("tmux identity PID was out of range")
    return server_pid, pane_pids

def observe_control(control, expected_token, deadline):
    last_error = RuntimeError("tmux identity query did not run")
    while time.monotonic() < deadline:
        try:
            server_pid, pane_pids = query_control_processes(control, deadline)
            token_result = subprocess.run(
                ["tmux", "-S", str(control), "show-environment", "-g", "PHOENIX_TMUX_SERVER_TOKEN"],
                stdin=subprocess.DEVNULL, capture_output=True, check=False, text=True,
                timeout=remaining_timeout(deadline),
            )
            token = token_result.stdout.strip().partition("=")[2]
            if token_result.returncode != 0 or token != expected_token:
                raise RuntimeError("tmux server token did not match registration")
            identities = [(server_pid, birth(server_pid), token)]
            identities.extend((pane_pid, birth(pane_pid), None) for pane_pid in pane_pids)
            if any(started is None for _, started, _ in identities):
                raise RuntimeError("process birth identity was unavailable")
            return identities
        except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
            last_error = error
            remaining = deadline - time.monotonic()
            if remaining > 0:
                time.sleep(min(0.1, remaining))
    raise RuntimeError(f"tmux processes never became ready: {last_error}")

def original_root_exists():
    try:
        current = root.stat()
        return (current.st_dev, current.st_ino) == root_identity
    except OSError:
        return False

def original_control_root_exists():
    try:
        current = control_root.stat()
        return (current.st_dev, current.st_ino) == control_root_identity
    except OSError:
        return False

def anchored_identity(directory_fd, name):
    try:
        current = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
        return [current.st_dev, current.st_ino]
    except OSError:
        return None

def capped(values, size, limit):
    return list(itertools.islice(values, limit)), max(0, size - limit)

def record_cleanup_failure(reason, terminal):
    records = []
    bounded_owned, omitted_records = capped(iter(owned), len(owned), 16)
    omitted_processes = 0
    probe_deadline = time.monotonic() + 0.25
    for socket, device, inode, control, processes in bounded_owned:
        bounded_processes, omitted = capped(iter(processes), len(processes), 32)
        omitted_processes += omitted
        process_records = []
        for identity in bounded_processes:
            remaining = probe_deadline - time.monotonic()
            if remaining <= 0:
                state = "probe-budget-exhausted"
            else:
                observed = process(identity[0], min(0.05, remaining))
                if observed is None:
                    state = "unverifiable" if pid_exists(identity[0]) else "absent"
                elif observed[0] != identity[1]:
                    state = "mismatched"
                elif observed[1]:
                    state = "absent"
                elif identity[2] is None:
                    state = "owned"
                else:
                    token_bytes = f"PHOENIX_TMUX_SERVER_TOKEN={identity[2]}".encode()
                    state = "owned" if any(
                        value == token_bytes or value == token_bytes.decode()
                        for value in observed[2]
                    ) else "mismatched"
            process_records.append({
                "pid": identity[0], "birth": identity[1], "state": state
            })
        records.append({
            "socket": socket.name,
            "control": control.name,
            "socket_identity": anchored_identity(root_fd, socket.name),
            "control_identity": anchored_identity(control_root_fd, control.name),
            "expected_identity": [device, inode],
            "processes": process_records,
        })
    visible_paths, omitted_visible_paths = capped(
        (path.name for path in preserved_visible_paths), len(preserved_visible_paths), 32
    )
    controls, omitted_controls = capped(
        iter(retained_controls), len(retained_controls), 32
    )
    statuses, omitted_statuses = capped(
        iter(cleanup_record_status.items()), len(cleanup_record_status), 32
    )
    receipt = {
        "version": 1,
        "reason": reason,
        "root_identity_matches": original_root_exists(),
        "control_root_identity_matches": original_control_root_exists(),
        "cleanup_failed": cleanup_failed,
        "quiet": terminal.get("quiet", 0),
        "unconfirmed": terminal.get("unconfirmed", True),
        "unconfirmed_state": terminal.get("unconfirmed_state", True),
        "sockets": terminal.get("sockets", True),
        "creators": terminal.get("creators", True),
        "unconfirmed_obligations": len(unconfirmed_obligations),
        "preserved_visible_paths": visible_paths,
        "retained_controls": controls,
        "record_status": dict(statuses),
        "records": records,
        "omitted": {
            "records": omitted_records,
            "processes": omitted_processes,
            "preserved_visible_paths": omitted_visible_paths,
            "retained_controls": omitted_controls,
            "record_status": omitted_statuses,
        },
    }
    serialized = json.dumps(receipt, sort_keys=True)
    if len(serialized.encode()) > 16384:
        serialized = json.dumps({
            "version": 1,
            "reason": reason,
            "receipt_too_large": True,
            "owned_records": len(owned),
            "unconfirmed_obligations": len(unconfirmed_obligations),
        }, sort_keys=True)
    pending = f".pending-cleanup-failure-{uuid.uuid4().hex}.json"
    try:
        descriptor = os.open(
            pending,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
            0o600,
            dir_fd=root_fd,
        )
        try:
            payload = serialized.encode()
            written = 0
            while written < len(payload):
                count = os.write(descriptor, payload[written:])
                if count == 0:
                    raise OSError("cleanup receipt write made no progress")
                written += count
        finally:
            os.close(descriptor)
        os.replace(pending, ".cleanup-failure.json", src_dir_fd=root_fd, dst_dir_fd=root_fd)
    except OSError:
        try:
            os.unlink(pending, dir_fd=root_fd)
        except OSError:
            pass

def fail_cleanup(reason, terminal):
    record_cleanup_failure(reason, terminal)
    sys.exit(1)

def terminal_state(quiet, unconfirmed, unconfirmed_state, sockets, creators, states):
    return {
        "quiet": quiet,
        "unconfirmed": unconfirmed,
        "unconfirmed_state": unconfirmed_state,
        "sockets": sockets,
        "creators": creators,
        "states": states,
    }

def record_uncaught_cleanup_failure(error_type, error, traceback):
    if issubclass(error_type, SystemExit):
        return sys.__excepthook__(error_type, error, traceback)
    if issubclass(error_type, subprocess.TimeoutExpired):
        reason = "cleanup-subprocess-timeout"
    elif issubclass(error_type, DeadlineExpired):
        reason = "cleanup-deadline-expired"
    elif issubclass(error_type, IdentityOutputError):
        reason = "cleanup-identity-output-error"
    elif issubclass(error_type, OSError):
        reason = "cleanup-os-error"
    else:
        reason = "cleanup-runtime-error"
    record_cleanup_failure(reason, globals().get("terminal", {}))
    return sys.__excepthook__(error_type, error, traceback)

sys.excepthook = record_uncaught_cleanup_failure

def owner_alive():
    if (not original_root_exists() or not original_control_root_exists()
            or (root / ".cleanup-request").exists()):
        return False
    try:
        return time.time() - heartbeat.stat().st_mtime <= heartbeat_stale
    except FileNotFoundError:
        return False
    except OSError:
        return False

def reserve_spawn(socket, control, token):
    if any(existing_socket == socket or existing_control == control
           for existing_socket, existing_control, _ in unconfirmed_obligations):
        raise RuntimeError("tmux spawn path already has an unresolved obligation")
    conflicts = [
        record for record in owned
        if record[0] == socket or record[3] == control
    ]
    if any(identity_state(identity) != "absent"
           for _, _, _, _, processes in conflicts for identity in processes):
        raise RuntimeError("live tmux ownership record already exists")
    obligation = (socket, control, token)
    unconfirmed_obligations.append(obligation)
    return obligation

def record_owned(socket, device, inode, control, identities):
    tmux_format_literal(identities[0][2])
    conflicts = [
        record for record in owned
        if record[0] == socket or record[3] == control
    ]
    if any(identity_state(identity) != "absent"
           for _, _, _, _, processes in conflicts for identity in processes):
        raise RuntimeError("live tmux ownership record already exists")
    if conflicts:
        provisional[:] = [item for item in provisional if item[0] != socket and item[1] != control]
        adopted_pending_publication[:] = [
            item for item in adopted_pending_publication
            if item[0] != socket and item[1] != control
        ]
        for record in conflicts:
            owned.remove(record)
    processes = tuple(identities)
    previous = retained_controls.get(control.name)
    record = (socket, device, inode, control, processes)
    owned.append(record)
    retained_controls[control.name] = (device, inode, processes, None)
    if previous is not None and previous[3] is not None:
        (control_root / previous[3]).unlink(missing_ok=True)
    if record_hook:
        injected = subprocess.run(
            [record_hook, str(control)],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, check=False, timeout=1.0,
        )
        if injected.returncode != 0:
            raise OSError("injected control anchor failure")
    anchor = control.parent / f".control-anchor-{uuid.uuid4()}"
    os.link(control, anchor)
    anchor_stat = anchor.stat()
    if anchor_stat.st_dev != device or anchor_stat.st_ino != inode:
        anchor.unlink(missing_ok=True)
        raise RuntimeError("tmux control anchor identity did not match")
    retained_controls[control.name] = (device, inode, processes, anchor.name)
    preserved_visible_paths.discard(socket)

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

def persist_record_processes(record, processes):
    updated = (record[0], record[1], record[2], record[3], tuple(processes))
    for index, candidate in enumerate(owned):
        if candidate is record or candidate == record:
            owned[index] = updated
            break
    retained = retained_controls.get(record[3].name)
    if retained is not None:
        retained_controls[record[3].name] = (
            retained[0], retained[1], tuple(processes), retained[3]
        )
    return updated

def set_cleanup_record_status(control, stage, detail):
    cleanup_record_status[control.name] = {"stage": stage, "detail": detail}

def retire_record(record, deadline):
    _, device, inode, control, recorded_processes = record
    processes = list(recorded_processes)
    expected_token = tmux_format_literal(processes[0][2])
    states = [identity_state(identity) for identity in processes]
    if all(state == "absent" for state in states):
        set_cleanup_record_status(control, "preflight", "already-absent")
        return record
    if states[0] != "owned" or any(state not in ("owned", "absent") for state in states[1:]):
        set_cleanup_record_status(control, "preflight", "identity-not-owned")
        return None
    try:
        control_stat = control.stat()
        if control_stat.st_dev != device or control_stat.st_ino != inode:
            set_cleanup_record_status(control, "preflight", "control-identity-mismatch")
            return None
        expected_server = processes[0][0]
        if retirement_hook:
            subprocess.run(
                [retirement_hook, str(control)],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, check=False,
                timeout=remaining_timeout(deadline),
            )
        set_cleanup_record_status(control, "query", "started")
        observed_server, pane_pids = query_control_processes(control, deadline)
        if observed_server != expected_server:
            set_cleanup_record_status(control, "query", "server-pid-mismatch")
            return None
        known_pids = {identity[0] for identity in processes}
        for pane_pid in pane_pids:
            if pane_pid not in known_pids:
                started = birth(pane_pid)
                if started is None:
                    if not pid_exists(pane_pid):
                        continue
                    set_cleanup_record_status(control, "query", "live-late-pane-birth-unavailable")
                    return None
                processes.append((pane_pid, started, None))
                known_pids.add(pane_pid)
        record = persist_record_processes(record, processes)
        set_cleanup_record_status(control, "atomic-kill", "started")
        subprocess.run(
            ["tmux", "-S", str(control), "if-shell", "-F",
             f"#{{&&:#{{==:#{{pid}},{expected_server}}},#{{==:#{{PHOENIX_TMUX_SERVER_TOKEN}},{expected_token}}}}}",
             "kill-server", ""],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, check=False,
            timeout=remaining_timeout(deadline),
        )
    except subprocess.TimeoutExpired:
        set_cleanup_record_status(control, "subprocess", "timeout")
        return None
    except DeadlineExpired:
        set_cleanup_record_status(control, "subprocess", "deadline-expired")
        return None
    except IdentityOutputError as error:
        set_cleanup_record_status(control, "query", str(error))
        return None
    except RuntimeError as error:
        set_cleanup_record_status(control, "runtime", str(error))
        return None
    except OSError:
        set_cleanup_record_status(control, "subprocess", "os-error")
        return None
    set_cleanup_record_status(control, "final-absence", "waiting")
    while time.monotonic() < deadline:
        if all(identity_state(identity) == "absent" for identity in processes):
            set_cleanup_record_status(control, "final-absence", "verified")
            return record
        time.sleep(min(0.05, max(0, deadline - time.monotonic())))
    set_cleanup_record_status(control, "final-absence", "deadline-expired")
    return None

class EnvironmentError(RuntimeError):
    pass

def load_environment(path):
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise EnvironmentError(str(error)) from error
    if (not isinstance(value, list)
            or any(not isinstance(pair, list) or len(pair) != 2
                   or not isinstance(pair[0], str) or not isinstance(pair[1], str)
                   for pair in value)):
        raise EnvironmentError("spawn environment must be string pairs")
    return dict(value)

def spawn_owned(request):
    global cleanup_failed
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
        token = tmux_format_literal(token)
        socket = root / socket_name
        control = control_root / control_name
        if socket.parent != root or control.parent != control_root or control.exists():
            raise RuntimeError("spawn paths are not unused exact children of owned roots")
        environment = load_environment(control_root / env_path)
        obligation = reserve_spawn(socket, control, token)
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
    except (OSError, subprocess.TimeoutExpired) as error:
        if control is not None and control.exists():
            try:
                identities = observe_control(control, token, time.monotonic() + identity_timeout)
                control_stat = control.stat()
                record_owned(socket, control_stat.st_dev, control_stat.st_ino, control, identities)
                unconfirmed_obligations.remove(obligation)
            except (OSError, RuntimeError, subprocess.TimeoutExpired, ValueError):
                pass
        try:
            publish_response(rejected, str(error))
        except OSError:
            return False
    except EnvironmentError as error:
        cleanup_failed = True
        try:
            publish_response(rejected, str(error))
        except OSError:
            pass
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

def retain_obligation(socket, control):
    if not any(existing_socket == socket and existing_control == control
               for existing_socket, existing_control, _ in unconfirmed_obligations):
        unconfirmed_obligations.append((socket, control, None))

def retirement_deadline():
    global cleanup_deadline
    now = time.monotonic()
    if (root / ".cleanup-request").exists() and cleanup_deadline is None:
        cleanup_deadline = now + cleanup_timeout
    return min(now + identity_timeout, cleanup_deadline) if cleanup_deadline else now + identity_timeout

def retire_registered(socket, control, identities):
    expected = tuple(identities)
    record = exact_record(socket, control, expected)
    if record is None:
        return False
    control_anchor = control_root / f".retiring-control-{uuid.uuid4()}"
    try:
        os.link(control, control_anchor)
        anchor_stat = control_anchor.stat()
        if anchor_stat.st_dev != record[1] or anchor_stat.st_ino != record[2]:
            control_anchor.unlink(missing_ok=True)
            return False
    except OSError:
        return False
    retired_record = retire_record(record, retirement_deadline())
    retired = retired_record is not None
    if retired:
        record = retired_record
    try:
        socket_stat = socket.stat()
        visible_owned = socket_stat.st_dev == record[1] and socket_stat.st_ino == record[2]
        visible_replacement = not visible_owned
    except FileNotFoundError:
        visible_owned = False
        visible_replacement = False
    except OSError:
        retired = False
        visible_owned = False
        visible_replacement = False
    if retired and visible_owned:
        retired = remove_exact_visible(record)
    if retired:
        retired = remove_retired_control(record)
    try:
        control_anchor.unlink()
    except FileNotFoundError:
        pass
    except OSError:
        retired = False
    if not retired:
        return False
    retained_controls.pop(control.name, None)
    control_anchor.unlink(missing_ok=True)
    for anchor in control_root.glob(".control-anchor-*"):
        try:
            anchor_stat = anchor.stat()
            if anchor_stat.st_dev == record[1] and anchor_stat.st_ino == record[2]:
                anchor.unlink()
        except FileNotFoundError:
            pass
    owned.remove(record)
    try:
        final_socket_stat = socket.stat()
        if (final_socket_stat.st_dev, final_socket_stat.st_ino) != (record[1], record[2]):
            preserved_visible_paths.add(socket)
    except FileNotFoundError:
        pass
    provisional[:] = [
        item for item in provisional
        if not (item[0] == socket and item[1] == control and tuple(item[2]) == expected)
    ]
    matching_publications = [
        item for item in adopted_pending_publication
        if item[0] == socket and item[1] == control and tuple(item[2]) == expected
    ]
    adopted_pending_publication[:] = [
        item for item in adopted_pending_publication if item not in matching_publications
    ]
    for item in matching_publications:
        nonce = item[4].name.removeprefix(".publication-cancelled-")
        for path in (*item[3:7], control_root / f".registered-{nonce}",
                     control_root / f".adopted-{nonce}",
                     control_root / f".publication-committed-{nonce}"):
            path.unlink(missing_ok=True)

    unconfirmed_obligations[:] = [
        obligation for obligation in unconfirmed_obligations
        if obligation[0] != socket or obligation[1] != control
    ]
    return True

def retire(request):
    global cleanup_failed
    rejected = request.with_name(
        request.name.replace(".retire-request-", ".retire-rejected-", 1)
    )
    acknowledged = request.with_name(
        request.name.replace(".retire-request-", ".retired-", 1)
    )
    try:
        fields = request.read_text().split("\t")
        if len(fields) < 7 or len(fields[5:]) % 2 != 0:
            raise RuntimeError("retirement request was malformed")
        socket = root / fields[0]
        control = control_root / fields[1]
        expected_token = tmux_format_literal(fields[4])
        identities = [(int(fields[2]), fields[3], expected_token)]
        identities.extend(
            (int(fields[index]), fields[index + 1], None)
            for index in range(5, len(fields), 2)
        )
        retired = retire_registered(socket, control, identities)
        if not retired and all(identity_state(identity) == "absent" for identity in identities):
            retired = True
        if not retired:
            raise RuntimeError("exact owned spawn retirement could not be proven")
        publish_response(acknowledged, "retired")
    except (OSError, RuntimeError, subprocess.TimeoutExpired, ValueError) as error:
        try:
            publish_response(rejected, str(error))
        except OSError:
            return False
    finally:
        try:
            request.unlink(missing_ok=True)
        except OSError:
            cleanup_failed = True
            return False
    return True

def register(request):
    rejected = request.with_name(request.name.replace(".register-", ".rejected-", 1))
    acknowledged = request.with_name(request.name.replace(".register-", ".registered-", 1))
    try:
        fields = request.read_text().split("\t")
        socket_name, control_name, expected_token = fields
        expected_token = tmux_format_literal(expected_token)
        socket = root / socket_name
        control = control_root / control_name
        if (socket.parent != root or control.parent != control_root
                or control.is_symlink() or not control.is_socket()):
            raise RuntimeError("registration control is not an exact child of the owned control root")
        registration_control = control_root.with_name(
            f"{control_root.name}.registration-{uuid.uuid4()}"
        )
        previous = retained_controls.get(control.name)
        os.link(control, registration_control)
        try:
            if not original_control_root_exists():
                raise RuntimeError("registration control root incarnation changed")
            identities = observe_control(
                registration_control, expected_token, time.monotonic() + adoption_timeout
            )
            control_stat = registration_control.stat()
            record_owned(
                socket, control_stat.st_dev, control_stat.st_ino,
                registration_control, identities
            )
            record = owned[-1]
            retained = retained_controls.pop(registration_control.name)
            external_anchor = registration_control.parent / retained[3]
            durable_anchor = control_root / retained[3]
            os.link(external_anchor, durable_anchor)
            external_anchor.unlink()
            record = (record[0], record[1], record[2], control, record[4])
            owned[-1] = record
            retained_controls[control.name] = retained
            if previous is not None and previous[3] is not None:
                previous_anchor = control_root / previous[3]
                try:
                    previous_stat = previous_anchor.stat()
                    if previous_stat.st_dev != previous[0] or previous_stat.st_ino != previous[1]:
                        raise RuntimeError("previous retained control anchor changed identity")
                    previous_anchor.unlink()
                except FileNotFoundError:
                    pass
        finally:
            registration_control.unlink(missing_ok=True)
        try:
            publish_response(acknowledged, "\t".join(
                str(value) for identity in identities for value in identity[:2]
            ))
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
while owner_alive():
    for request in control_root.glob(".spawn-*"):
        if not owner_alive() or not spawn_owned(request):
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
        publication_committed = publication_acknowledged.with_name(
            publication_acknowledged.name.replace(
                ".publication-acknowledged-", ".publication-committed-", 1
            )
        )
        try:
            commit_valid = (publication_committed.is_file()
                            and not publication_committed.is_symlink()
                            and publication_committed.read_text() == "committed")
        except OSError:
            commit_valid = False
        if commit_valid:
            publication_committed.unlink()
            publication_acknowledged.unlink(missing_ok=True)
            publication_cancelled.unlink(missing_ok=True)
            adopted_pending_publication.remove(item)
            continue
        if publication_cancelled.exists():
            retired = retire_registered(socket, control, identities)
            if not retired and all(identity_state(identity) == "absent" for identity in identities):
                retired = True
            if not retired:
                retain_obligation(socket, control)
                continue
            try:
                publish_response(adoption_rejected, "publication cancelled; exact retirement completed")
            except OSError:
                pass
            publication_cancelled.unlink(missing_ok=True)
            publication_acknowledged.unlink(missing_ok=True)
            if item in adopted_pending_publication:
                adopted_pending_publication.remove(item)
            continue
        if publication_acknowledged.exists() and not published.exists():
            continue
        if publish_valid:
            try:
                if publication_hook:
                    subprocess.run(
                        [publication_hook, str(published), str(publication_acknowledged)],
                        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL, check=False, timeout=1.0,
                    )
                published.unlink(missing_ok=True)
                if publication_cancelled.exists():
                    retired = retire_registered(socket, control, identities)
                    if not retired and all(identity_state(identity) == "absent" for identity in identities):
                        retired = True
                    if not retired:
                        retain_obligation(socket, control)
                        continue
                    try:
                        publish_response(adoption_rejected, "publication cancelled; exact retirement completed")
                    except OSError:
                        pass
                    publication_cancelled.unlink(missing_ok=True)
                    publication_acknowledged.unlink(missing_ok=True)
                    if item in adopted_pending_publication:
                        adopted_pending_publication.remove(item)
                    continue
                publish_response(publication_acknowledged, "published")
            except (OSError, subprocess.TimeoutExpired):
                retire_registered(socket, control, identities)
                retain_obligation(socket, control)
                try:
                    publish_response(
                        adoption_rejected,
                        "publication marker cleanup failed; exact retirement attempted",
                    )
                except OSError:
                    pass
                adopted_pending_publication.remove(item)
        elif now >= deadline or published.exists():
            retired = retire_registered(socket, control, identities)
            if not retired and all(identity_state(identity) == "absent" for identity in identities):
                retired = True
            if not retired:
                retain_obligation(socket, control)
            try:
                publish_response(adoption_rejected, "publication failed; exact retirement completed")
            except OSError:
                pass
            if item in adopted_pending_publication:
                adopted_pending_publication.remove(item)
    for item in list(provisional):
        (socket, control, identities, adopt, adopted, adoption_rejected,
         published, publication_cancelled, publication_acknowledged,
         acknowledged, deadline) = item
        if adoption_hook:
            try:
                subprocess.run(
                    [adoption_hook, str(adopt), "expired" if now >= deadline else "pending"],
                    stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL, check=False, timeout=1.0,
                )
            except (OSError, subprocess.TimeoutExpired):
                retain_obligation(socket, control)
                break
        if now < deadline and adopt.exists():
            try:
                adopt.unlink()
                publish_response(adopted, "adopted")
                provisional.remove(item)
                adopted_pending_publication.append((
                    socket, control, identities, published, publication_cancelled,
                    publication_acknowledged, adoption_rejected,
                    time.monotonic() + publication_timeout
                ))
            except OSError:
                if not retire_registered(socket, control, identities):
                    retain_obligation(socket, control)
        elif now >= deadline:
            retired = retire_registered(socket, control, identities)
            if not retired:
                retain_obligation(socket, control)
            try:
                publish_response(
                    adoption_rejected,
                    "lease expired; exact server retired" if retired else
                    "lease expired; exact provisional retirement could not be proven",
                )
            except OSError:
                retain_obligation(socket, control)
    for request in control_root.glob(".retire-request-*"):
        if not retire(request):
            break
    else:
        request = None
    if request is not None:
        break
    for request in control_root.glob(".register-*"):
        if not owner_alive() or not register(request):
            break
    else:
        request = None
    if request is not None:
        break
    if not root.exists():
        break
    if parent == 1:
        try:
            if time.time() - heartbeat.stat().st_mtime > heartbeat_stale:
                break
        except FileNotFoundError:
            break
    elif os.getppid() != parent:
        break
    time.sleep(0.05)
if original_root_exists():
    try:
        (root / ".cleanup-ack").touch()
    except FileNotFoundError:
        pass

if cleanup_deadline is None:
    cleanup_deadline = time.monotonic() + cleanup_timeout
for socket, control, token in list(unconfirmed_obligations):
    if token is None or time.monotonic() >= cleanup_deadline:
        continue
    try:
        identities = observe_control(control, token, cleanup_deadline)
        control_stat = control.stat()
        record_owned(socket, control_stat.st_dev, control_stat.st_ino, control, identities)
        unconfirmed_obligations.remove((socket, control, token))
    except (OSError, RuntimeError, subprocess.TimeoutExpired, ValueError):
        pass

for index, record in enumerate(owned):
    if time.monotonic() >= cleanup_deadline:
        break
    socket, device, inode, control, processes = record
    try:
        observed_server, pane_pids = query_control_processes(control, cleanup_deadline)
        if observed_server != processes[0][0]:
            continue
        known_pids = {identity[0] for identity in processes}
        expanded = list(processes)
        for pane_pid in pane_pids:
            if pane_pid not in known_pids:
                started = birth(pane_pid)
                if started is None:
                    if not pid_exists(pane_pid):
                        continue
                    raise RuntimeError("live late pane birth identity was unavailable")
                expanded.append((pane_pid, started, None))
                known_pids.add(pane_pid)
        record = (socket, device, inode, control, tuple(expanded))
        owned[index] = record
        retire_record(record, cleanup_deadline)
    except (OSError, RuntimeError, subprocess.TimeoutExpired):
        pass

def remove_authenticated_control_root():
    if not control_root.exists():
        return True
    quarantine = control_root.with_name(f"{control_root.name}.retired-{uuid.uuid4()}")
    if not original_control_root_exists():
        return False
    try:
        os.replace(control_root, quarantine)
        moved_root_stat = quarantine.stat()
        if (moved_root_stat.st_dev, moved_root_stat.st_ino) != control_root_identity:
            if not control_root.exists():
                os.replace(quarantine, control_root)
            return False
    except OSError:
        return False
    expected = dict(retained_controls)
    anchor_names = {record[3] for record in expected.values()}
    authenticated = True
    try:
        for control_name, (device, inode, processes, anchor_name) in expected.items():
            if anchor_name is None:
                authenticated = False
                break
            control = quarantine / control_name
            anchor = quarantine / anchor_name
            try:
                anchor_lstat = anchor.lstat()
                anchor_stat = anchor.stat()
                if anchor.is_symlink():
                    raise OSError("retained control anchor became a symlink")
                if control.exists():
                    control_lstat = control.lstat()
                    control_stat = control.stat()
                    if control.is_symlink() or not control.is_socket():
                        raise OSError("retained control path changed type")
                    if (control_lstat.st_dev != device or control_lstat.st_ino != inode
                            or control_stat.st_dev != device or control_stat.st_ino != inode):
                        raise OSError("retained control identity changed")
            except OSError:
                authenticated = False
                break
            if (anchor_lstat.st_dev != device or anchor_lstat.st_ino != inode
                    or anchor_stat.st_dev != device or anchor_stat.st_ino != inode
                    or any(identity_state(identity) != "absent" for identity in processes)):
                authenticated = False
                break
        for entry in quarantine.iterdir():
            if not authenticated:
                break
            if entry.name in anchor_names:
                continue
            registered = expected.get(entry.name)
            if registered is None:
                if entry.is_socket() or entry.is_symlink():
                    authenticated = False
                    break
                continue
        if authenticated:
            shutil.rmtree(quarantine)
            return not control_root.exists()
    except OSError:
        authenticated = False
    try:
        if not control_root.exists():
            os.replace(quarantine, control_root)
    except OSError:
        pass
    return False

terminal = terminal_state(0, True, True, True, True, {})
quiet = 0
while time.monotonic() < cleanup_deadline:
    root_replaced = not original_root_exists()
    unconfirmed = root_replaced or cleanup_failed
    process_states = {
        control.name: [identity_state(identity) for identity in processes]
        for _, _, _, control, processes in owned
    }
    unconfirmed_state = (any(
        state != "absent" for states in process_states.values() for state in states
    ) or bool(unconfirmed_obligations))
    registered_sockets = {
        socket: (device, inode, control, processes)
        for socket, device, inode, control, processes in owned
    }
    for socket in (() if root_replaced else root.glob("*.sock")):
        if socket.is_symlink():
            socket.unlink(missing_ok=True)
            continue
        if not socket.is_socket():
            continue
        registered = registered_sockets.get(socket)
        if registered is not None:
            if socket in preserved_visible_paths:
                unconfirmed = True
                continue
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
                    preserved_visible_paths.add(socket)
            except FileNotFoundError:
                pass
            except OSError:
                unconfirmed = True
            continue
        if socket in preserved_visible_paths:
            unconfirmed = True
            continue
        quarantine = root / f".unregistered-{uuid.uuid4()}"
        try:
            socket_stat = socket.stat()
            os.replace(socket, quarantine)
            quarantined_stat = quarantine.stat()
            if (quarantined_stat.st_dev, quarantined_stat.st_ino) != (socket_stat.st_dev, socket_stat.st_ino):
                raise RuntimeError("unregistered socket identity changed during quarantine")
            probe = subprocess.run(
                ["tmux", "-S", str(quarantine), "list-sessions"],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, check=False,
                timeout=remaining_timeout(cleanup_deadline),
            )
            if probe.returncode != 0:
                raise RuntimeError("unregistered socket probe was unverifiable")
            killed = subprocess.run(
                ["tmux", "-S", str(quarantine), "kill-server"],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, check=False,
                timeout=remaining_timeout(cleanup_deadline),
            )
            if killed.returncode != 0:
                raise RuntimeError("unregistered server kill failed")
            quarantine.unlink(missing_ok=True)
        except (OSError, RuntimeError, subprocess.TimeoutExpired):
            try:
                if quarantine.exists() and not socket.exists():
                    os.replace(quarantine, socket)
            except OSError:
                pass
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
    terminal = terminal_state(
        quiet, unconfirmed, unconfirmed_state, sockets, creators, process_states
    )
    if quiet >= 5:
        if not remove_authenticated_control_root():
            quiet = 0
            time.sleep(min(0.1, max(0, cleanup_deadline - time.monotonic())))
            continue
        root_quarantine = root.with_name(f"{root.name}.retired-{uuid.uuid4()}")
        try:
            os.replace(root, root_quarantine)
            moved_root_stat = root_quarantine.stat()
            if (moved_root_stat.st_dev, moved_root_stat.st_ino) != root_identity:
                if not root.exists():
                    os.replace(root_quarantine, root)
                fail_cleanup("root-quarantine-identity-mismatch", terminal)
            if root_quarantine_hook:
                subprocess.run(
                    [root_quarantine_hook, str(root_quarantine)],
                    stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL, check=False,
                    timeout=remaining_timeout(cleanup_deadline),
                )
            reconciled = True
            for entry in root_quarantine.iterdir():
                if entry.is_symlink():
                    reconciled = False
                    break
                if not entry.is_socket():
                    continue
                record = next((candidate for candidate in owned if candidate[0].name == entry.name), None)
                entry_stat = entry.stat()
                if (record is None or (entry_stat.st_dev, entry_stat.st_ino) != (record[1], record[2])
                        or any(identity_state(process) != "absent" for process in record[4])):
                    reconciled = False
                    break
                entry.unlink()
            if not reconciled:
                if not root.exists():
                    os.replace(root_quarantine, root)
                fail_cleanup("root-quarantine-reconciliation-failed", terminal)
            if completion_hook:
                subprocess.run(
                    [completion_hook, str(control_root)],
                    stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL, check=False,
                    timeout=remaining_timeout(cleanup_deadline),
                )
            shutil.rmtree(root_quarantine)
        except (RuntimeError, subprocess.TimeoutExpired):
            try:
                if root_quarantine.exists() and not root.exists():
                    os.replace(root_quarantine, root)
            except OSError:
                pass
            raise
        except OSError:
            try:
                if root_quarantine.exists() and not root.exists():
                    os.replace(root_quarantine, root)
            except OSError:
                pass
            fail_cleanup("root-quarantine-operation-failed", terminal)
        sys.exit(0)
    time.sleep(min(0.1, max(0, cleanup_deadline - time.monotonic())))
fail_cleanup("cleanup-deadline-expired", terminal)
"##;

/// Owns real tmux servers created by tests, including after abrupt runner death.
pub struct TestTmuxServerOwner {
    root: Option<TempDir>,
    root_anchor: Option<fs::File>,
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

fn set_duration_env(command: &mut Command, name: &str, value: Option<Duration>) {
    if let Some(value) = value {
        command.env(name, value.as_secs_f64().to_string());
    }
}

fn set_path_env(command: &mut Command, name: &str, value: Option<&Path>) {
    if let Some(value) = value {
        command.env(name, value);
    }
}

fn configure_watchdog_env(
    command: &mut Command,
    deadlines: (Option<Duration>, Option<Duration>),
    quarantine_hook: Option<&Path>,
    adoption: (Option<Duration>, Option<&Path>),
    publication: (Option<Duration>, Option<&Path>),
    lifecycle_hooks: (Option<&Path>, Option<&Path>, Option<&Path>, Option<&Path>),
) {
    set_duration_env(command, "PHOENIX_TMUX_IDENTITY_TIMEOUT", deadlines.0);
    set_duration_env(command, "PHOENIX_TMUX_CLEANUP_TIMEOUT", deadlines.1);
    set_duration_env(command, "PHOENIX_TMUX_ADOPTION_TIMEOUT", adoption.0);
    set_duration_env(command, "PHOENIX_TMUX_PUBLICATION_TIMEOUT", publication.0);
    set_path_env(command, "PHOENIX_TMUX_ADOPTION_HOOK", adoption.1);
    set_path_env(command, "PHOENIX_TMUX_QUARANTINE_HOOK", quarantine_hook);
    set_path_env(command, "PHOENIX_TMUX_PUBLICATION_HOOK", publication.1);
    set_path_env(command, "PHOENIX_TMUX_RECORD_HOOK", lifecycle_hooks.0);
    set_path_env(command, "PHOENIX_TMUX_RETIREMENT_HOOK", lifecycle_hooks.1);
    set_path_env(
        command,
        "PHOENIX_TMUX_ROOT_QUARANTINE_HOOK",
        lifecycle_hooks.2,
    );
    set_path_env(command, "PHOENIX_TMUX_COMPLETION_HOOK", lifecycle_hooks.3);
}

fn read_cleanup_receipt(root_anchor: &fs::File) -> io::Result<String> {
    let name = CString::new(".cleanup-failure.json").expect("static receipt name has no NUL");
    let descriptor = unsafe {
        libc::openat(
            root_anchor.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if descriptor == -1 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { fs::File::from_raw_fd(descriptor) };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("cleanup receipt is not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(16_385).read_to_end(&mut bytes)?;
    if bytes.len() > 16_384 {
        bytes.truncate(16_384);
        bytes.extend_from_slice(b"...[truncated]");
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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
            (identity_timeout, None),
            None,
            None,
            None,
            (None, None),
            (None, None, None, None),
        )
    }

    pub(crate) fn new_with_watchdog_test_options(
        watchdog_path: Option<&Path>,
        deadlines: (Option<Duration>, Option<Duration>),
        quarantine_hook: Option<&Path>,
        adoption_timeout: Option<Duration>,
        adoption_hook: Option<&Path>,
        publication: (Option<Duration>, Option<&Path>),
        lifecycle_hooks: (Option<&Path>, Option<&Path>, Option<&Path>, Option<&Path>),
    ) -> Self {
        let root = tempfile::Builder::new()
            .prefix("ptt-")
            .tempdir_in("/private/tmp")
            .or_else(|_| tempfile::Builder::new().prefix("ptt-").tempdir_in("/tmp"))
            .expect("create short isolated tmux test root");
        let canonical_root = root.path().canonicalize().expect("canonicalize test root");
        let root_anchor = fs::File::open(&canonical_root).expect("open anchored tmux test root");
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
        configure_watchdog_env(
            &mut command,
            deadlines,
            quarantine_hook,
            (adoption_timeout, adoption_hook),
            publication,
            lifecycle_hooks,
        );
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
            root_anchor: Some(root_anchor),
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
        let root_anchor = self.root_anchor.take().expect("owner root anchor is live");
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
                let receipt = read_cleanup_receipt(&root_anchor)
                    .unwrap_or_else(|error| format!("unavailable ({error})"));
                return Err(io::Error::other(format!(
                    "tmux test watchdog reported cleanup failure: {status}; receipt: {receipt}"
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
        let _ = root.keep();
        let _ = control_root.keep();
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
#[derive(Clone, Debug)]
pub(crate) struct TestServerProcesses {
    pub(crate) server: ProcessIdentity,
    pub(crate) pane: ProcessIdentity,
    additional_panes: Vec<ProcessIdentity>,
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

fn parse_acknowledged_processes(value: &str) -> io::Result<TestServerProcesses> {
    let fields = value.split('\t').collect::<Vec<_>>();
    if fields.len() < 4 || fields.len() % 2 != 0 {
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
        additional_panes: fields[4..]
            .chunks_exact(2)
            .map(|fields| {
                Ok(ProcessIdentity {
                    pid: u32::try_from(parse(fields[0])?).map_err(io::Error::other)?,
                    start_time: parse(fields[1])?,
                })
            })
            .collect::<io::Result<Vec<_>>>()?,
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
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;
    use std::process::ExitStatus;

    use super::*;

    #[test]
    fn process_identity_parser_and_receipt_reader_reject_ambiguous_inputs() {
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

        let root = TempDir::new().unwrap();
        let anchor = fs::File::open(root.path()).unwrap();
        fs::write(
            root.path().join(".cleanup-failure.json"),
            vec![b'x'; 20_000],
        )
        .unwrap();
        let replacement = root.path().with_extension("replacement");
        fs::rename(root.path(), &replacement).unwrap();
        fs::create_dir(root.path()).unwrap();
        fs::write(root.path().join(".cleanup-failure.json"), b"forged").unwrap();

        let receipt = read_cleanup_receipt(&anchor).unwrap();
        assert!(!receipt.contains("forged"));
        assert!(receipt.ends_with("...[truncated]"));
        fs::remove_dir_all(root.path()).unwrap();
        fs::remove_dir_all(replacement).unwrap();
        std::mem::forget(root);
    }

    #[test]
    fn forced_parent_process_group_death_kills_exact_owned_processes() {
        if which::which("tmux").is_err() {
            return;
        }
        let marker_dir = TempDir::new().unwrap();
        let marker = marker_dir.path().join("ready");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "tmux::test_server::tests::forced_parent_death_fixture",
                "--nocapture",
                "--ignored",
            ])
            .env("PHOENIX_TMUX_TEST_DEATH_MARKER", &marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
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
            additional_panes: Vec::new(),
        };
        assert_eq!(probe_sync(&socket), ProbeResult::Live);

        let child_pid = i32::try_from(child.id()).expect("child pid fits pid_t");
        assert_eq!(unsafe { libc::killpg(child_pid, libc::SIGKILL) }, 0);
        assert_killed(child.wait().unwrap());
        wait_until(|| !root.exists(), "watchdog cleanup after forced death");
        assert_ne!(probe_sync(&socket), ProbeResult::Live);
        assert_exact_processes_gone(&processes);
    }

    #[test]
    #[ignore = "subprocess fixture; parent test terminates it"]
    fn forced_parent_death_fixture() {
        let Some(marker) = std::env::var_os("PHOENIX_TMUX_TEST_DEATH_MARKER") else {
            return;
        };
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let (socket, processes) = spawn_server_with_processes(&owner, "forced-death");
        let marker = PathBuf::from(marker);
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

    fn wait_until(mut condition: impl FnMut() -> bool, description: &str) {
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {description}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn assert_exact_processes_gone(processes: &TestServerProcesses) {
        wait_until(
            || {
                !phoenix_core::process_identity::process_identity_matches(processes.server)
                    && !phoenix_core::process_identity::process_identity_matches(processes.pane)
                    && processes.additional_panes.iter().all(|identity| {
                        !phoenix_core::process_identity::process_identity_matches(*identity)
                    })
            },
            "exact tmux server and pane-shell exit",
        );
    }

    fn assert_killed(status: ExitStatus) {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }
}
