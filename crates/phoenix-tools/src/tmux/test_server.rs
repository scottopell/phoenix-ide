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
root_stat = root.stat()
root_identity = (root_stat.st_dev, root_stat.st_ino)
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
        raise RuntimeError("tmux identity output was malformed")
    return int(server_pid), tuple(dict.fromkeys(int(pid) for pid in pane_pids))

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

def retire_record(record, deadline):
    _, device, inode, control, recorded_processes = record
    processes = list(recorded_processes)
    expected_token = tmux_format_literal(processes[0][2])
    states = [identity_state(identity) for identity in processes]
    if all(state == "absent" for state in states):
        return record
    if states[0] != "owned" or any(state not in ("owned", "absent") for state in states[1:]):
        return None
    try:
        control_stat = control.stat()
        if control_stat.st_dev != device or control_stat.st_ino != inode:
            return None
        expected_server = processes[0][0]
        if retirement_hook:
            subprocess.run(
                [retirement_hook, str(control)],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, check=False,
                timeout=remaining_timeout(deadline),
            )
        observed_server, pane_pids = query_control_processes(control, deadline)
        if observed_server != expected_server:
            return None
        known_pids = {identity[0] for identity in processes}
        for pane_pid in pane_pids:
            if pane_pid not in known_pids:
                started = birth(pane_pid)
                if started is None:
                    return None
                processes.append((pane_pid, started, None))
                known_pids.add(pane_pid)
        record = persist_record_processes(record, processes)
        subprocess.run(
            ["tmux", "-S", str(control), "if-shell", "-F",
             f"#{{&&:#{{==:#{{pid}},{expected_server}}},#{{==:#{{PHOENIX_TMUX_SERVER_TOKEN}},{expected_token}}}}}",
             "kill-server", ""],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, check=False,
            timeout=remaining_timeout(deadline),
        )
    except (OSError, RuntimeError, subprocess.TimeoutExpired):
        return None
    while time.monotonic() < deadline:
        if all(identity_state(identity) == "absent" for identity in processes):
            return record
        time.sleep(min(0.05, max(0, deadline - time.monotonic())))
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
                    raise RuntimeError("late pane birth identity was unavailable")
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

quiet = 0
while time.monotonic() < cleanup_deadline:
    root_replaced = not original_root_exists()
    unconfirmed = root_replaced or cleanup_failed
    states = [
        identity_state(identity)
        for _, _, _, _, processes in owned
        for identity in processes
    ]
    unconfirmed_state = (any(state != "absent" for state in states)
                         or bool(unconfirmed_obligations))
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
                sys.exit(1)
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
                sys.exit(1)
            shutil.rmtree(root_quarantine)
        except OSError:
            try:
                if root_quarantine.exists() and not root.exists():
                    os.replace(root_quarantine, root)
            except OSError:
                pass
            sys.exit(1)
        if completion_hook:
            subprocess.run(
                [completion_hook, str(control_root)],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, check=False,
                timeout=remaining_timeout(cleanup_deadline),
            )
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

fn publication_paths(control_root: &Path, nonce: uuid::Uuid) -> (PathBuf, PathBuf, PathBuf) {
    (
        control_root.join(format!(".published-{nonce}")),
        control_root.join(format!(".publication-cancelled-{nonce}")),
        control_root.join(format!(".publication-acknowledged-{nonce}")),
    )
}

fn atomic_ack_matches(path: &Path, expected: &str) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
        && fs::read_to_string(path).is_ok_and(|value| value == expected)
}

#[derive(Debug)]
pub(crate) struct AdoptedTestServer {
    pub(crate) processes: TestServerProcesses,
    published: PathBuf,
    publication_acknowledged: PathBuf,
    publication_cancelled: PathBuf,
    publication_committed: PathBuf,
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
        publication_committed: PathBuf,
        adoption_rejected: PathBuf,
    ) -> Self {
        Self {
            processes,
            published,
            publication_acknowledged,
            publication_cancelled,
            adoption_rejected,
            publication_committed,
            committed: false,
        }
    }

    pub(crate) async fn commit_publication(mut self) -> io::Result<TestServerProcesses> {
        fs::write(&self.published, [])?;
        let deadline = tokio::time::Instant::now() + CLEANUP_TIMEOUT;
        loop {
            if let Ok(reason) = fs::read_to_string(&self.adoption_rejected) {
                self.committed = true;
                return Err(io::Error::other(format!(
                    "tmux watchdog rejected publication: {reason}"
                )));
            }
            if atomic_ack_matches(&self.publication_acknowledged, "published") {
                fs::write(&self.publication_committed, b"committed")?;
                self.committed = true;
                return Ok(self.processes.clone());
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
        retire_owned_server(
            socket,
            control_socket,
            self.processes.clone(),
            expected_token,
        )
        .await?;
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
    let (published, publication_cancelled, publication_acknowledged) =
        publication_paths(control_root, nonce);
    let publication_committed = control_root.join(format!(".publication-committed-{nonce}"));
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
        if atomic_ack_matches(&adoption_acknowledged, "adopted") {
            let processes = registered.ok_or_else(|| {
                io::Error::other("tmux watchdog acknowledged adoption before registration")
            })?;
            return Ok(AdoptedTestServer::new(
                processes,
                published,
                publication_acknowledged,
                publication_cancelled,
                publication_committed,
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
        std::iter::once(socket_name)
            .chain(std::iter::once(control_name))
            .chain([
                processes.server.pid.to_string(),
                processes.server.start_time.to_string(),
                expected_token.to_owned(),
            ])
            .chain(
                std::iter::once(processes.pane)
                    .chain(processes.additional_panes.iter().copied())
                    .flat_map(|identity| {
                        [identity.pid.to_string(), identity.start_time.to_string()]
                    }),
            )
            .collect::<Vec<_>>()
            .join("\t"),
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
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

    fn stop_heartbeat(owner: &mut TestTmuxServerOwner) {
        owner.heartbeat_stop.store(true, Ordering::Release);
        if let Some(heartbeat) = owner.heartbeat.take() {
            heartbeat.join().unwrap();
        }
    }

    fn await_watchdog_exit(owner: &mut TestTmuxServerOwner) -> ExitStatus {
        wait_for_watchdog(owner.watchdog.as_mut().expect("watchdog is live"))
            .expect("watchdog must exit")
    }

    fn stop_heartbeat_and_request_cleanup(owner: &mut TestTmuxServerOwner) -> ExitStatus {
        stop_heartbeat(owner);
        request_cleanup(owner.path(), true).unwrap();
        await_watchdog_exit(owner)
    }

    fn disarm_owner(owner: &mut TestTmuxServerOwner) {
        owner.watchdog.take();
        owner.root.take();
        owner.control_root.take();
    }

    fn write_raw_spawn_request(
        root: &Path,
        control_root: &Path,
        nonce: &str,
        environment_name: &str,
    ) {
        fs::write(
            control_root.join(format!(".spawn-{nonce}")),
            format!(
                "{nonce}.sock\t{nonce}.sock\t{}\t{}\t{nonce}-token\t{environment_name}",
                root.join("config").display(),
                root.display()
            ),
        )
        .unwrap();
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
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
        assert!(
            WATCHDOG_PROGRAM.contains("for record in conflicts:\n            owned.remove(record)")
        );
        assert!(WATCHDOG_PROGRAM.contains("provisional[:] ="));
        assert!(WATCHDOG_PROGRAM.contains("adopted_pending_publication[:] ="));
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
        assert!(WATCHDOG_PROGRAM.contains(
            "while time.monotonic() < cleanup_deadline:\n    root_replaced = not original_root_exists()\n    unconfirmed = root_replaced or cleanup_failed"
        ));
        assert!(WATCHDOG_PROGRAM.contains(
            "not unconfirmed_state and not unconfirmed and not sockets and not creators"
        ));
    }

    #[test]
    fn spawn_path_is_reserved_before_tmux_starts() {
        let reservation = WATCHDOG_PROGRAM
            .find("obligation = reserve_spawn(socket, control, token)")
            .unwrap();
        let spawn = WATCHDOG_PROGRAM.find("spawned = subprocess.run(").unwrap();
        assert!(reservation < spawn);
        assert!(WATCHDOG_PROGRAM
            .contains("for existing_socket, existing_control, _ in unconfirmed_obligations"));
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
        assert!(root.exists(), "failed cleanup must preserve its exact root");

        drop(replacement);
        assert_ne!(probe_sync(&control), ProbeResult::Live);
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&first_processes);
        assert!(
            phoenix_core::process_identity::process_identity_matches(second_processes.server),
            "cleanup must not signal a server outside its exact owner"
        );
        assert!(
            phoenix_core::process_identity::process_identity_matches(second_processes.pane),
            "cleanup must not signal a pane outside its exact owner"
        );
        second.shutdown();
        assert_exact_processes_gone(&second_processes);
    }

    fn write_tmux_wrapper(fake_bin: &Path, program: &str) {
        let fake_tmux = fake_bin.join("tmux");
        fs::write(&fake_tmux, program).unwrap();
        let mut permissions = fs::metadata(&fake_tmux).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(fake_tmux, permissions).unwrap();
    }

    #[test]
    fn daemonized_spawn_timeout_is_recovered_by_reserved_token() {
        let Ok(real_tmux) = which::which("tmux") else {
            return;
        };
        let fake_bin = TempDir::new().unwrap();
        write_tmux_wrapper(
            fake_bin.path(),
            &format!(
                "#!/bin/sh\ncase \" $* \" in *\" new-session \"*) '{}' \"$@\"; sleep 2; exit 0;; esac\nexec '{}' \"$@\"\n",
                real_tmux.display(),
                real_tmux.display()
            ),
        );
        let owner = TestTmuxServerOwner::new_with_watchdog_path(Some(fake_bin.path()));
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        write_adoption_environment(&control_root, "timeout-env.json", "timeout-token");
        write_raw_spawn_request(&root, &control_root, "timeout", "timeout-env.json");
        wait_until(
            || control_root.join("timeout.sock").exists(),
            "daemonized spawn control endpoint",
        );

        owner.shutdown();

        assert!(!root.exists());
        assert!(!control_root.exists());
    }

    #[test]
    fn registration_admission_binds_original_control_endpoint() {
        let bind = WATCHDOG_PROGRAM
            .find("os.link(control, registration_control)")
            .unwrap();
        let authenticate = WATCHDOG_PROGRAM
            .match_indices("if not original_control_root_exists()")
            .map(|(offset, _)| offset)
            .find(|offset| *offset > bind)
            .unwrap();
        let observe = WATCHDOG_PROGRAM
            .find("identities = observe_control(\n                registration_control")
            .unwrap();
        let record = WATCHDOG_PROGRAM
            .find("control_stat = registration_control.stat()")
            .unwrap();
        assert!(bind < authenticate && authenticate < observe && observe < record);
    }

    #[test]
    fn replacement_control_root_registration_is_not_dequeued() {
        assert!(WATCHDOG_PROGRAM.contains(
            "for request in control_root.glob(\".register-*\"):\n        if not owner_alive() or not register(request):"
        ));
        assert!(WATCHDOG_PROGRAM.contains("os.link(control, registration_control)"));
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

        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
    }

    #[test]
    fn retirement_wire_and_lookup_include_expected_token() {
        assert!(WATCHDOG_PROGRAM.contains("expected_token = tmux_format_literal(fields[4])"));
        assert!(
            WATCHDOG_PROGRAM.contains("identities = [(int(fields[2]), fields[3], expected_token)]")
        );
        assert!(WATCHDOG_PROGRAM.contains("for index in range(5, len(fields), 2)"));
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
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

    #[test]
    fn absent_control_with_foreign_replacement_anchor_fails_closed() {
        if which::which("tmux").is_err() {
            return;
        }
        let mut owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let control = control_root.join("foreign-anchor.sock");
        let (socket, processes) = spawn_server_with_processes(&owner, "foreign-anchor");
        assert!(Command::new("tmux")
            .args(["-S", &control.to_string_lossy(), "kill-server"])
            .status()
            .unwrap()
            .success());
        assert_exact_processes_gone(&processes);
        fs::remove_file(socket).unwrap();
        fs::remove_file(control).ok();
        let anchor = fs::read_dir(&control_root)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".control-anchor-")
            })
            .unwrap();
        fs::remove_file(&anchor).unwrap();
        std::os::unix::fs::symlink("foreign-target", &anchor).unwrap();

        let status = stop_heartbeat_and_request_cleanup(&mut owner);

        assert!(!status.success());
        assert!(
            anchor.is_symlink(),
            "foreign anchor replacement was removed"
        );
        disarm_owner(&mut owner);
        if root.exists() {
            fs::remove_dir_all(root).unwrap();
        }
        if control_root.exists() {
            fs::remove_dir_all(control_root).unwrap();
        }
    }

    #[test]
    fn replacement_control_endpoint_survives_after_subject_and_original_exit() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let control = control_root.join("control-replacement.sock");
        let (socket, processes) = spawn_server_with_processes(&owner, "control-replacement");
        fs::remove_file(&socket).unwrap();
        assert!(Command::new("tmux")
            .args(["-S", &control.to_string_lossy(), "kill-server"])
            .status()
            .unwrap()
            .success());
        assert_exact_processes_gone(&processes);
        fs::remove_file(&control).ok();
        let replacement = std::os::unix::net::UnixListener::bind(&control).unwrap();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(
            panic.is_err(),
            "replacement control must fail cleanup closed"
        );
        assert!(control.exists(), "replacement control endpoint was removed");
        assert_eq!(
            replacement.local_addr().unwrap().as_pathname(),
            Some(control.as_path())
        );
        drop(replacement);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn one_field_retirement_request_cannot_abandon_other_owned_records() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let control_root = owner.control_root_path().to_path_buf();
        let (_, first) = spawn_server_with_processes(&owner, "malformed-first");
        let (_, second) = spawn_server_with_processes(&owner, "malformed-second");
        fs::write(
            control_root.join(".retire-request-one-field"),
            "only-one-field",
        )
        .unwrap();
        wait_until(
            || control_root.join(".retire-rejected-one-field").exists(),
            "malformed retirement rejection",
        );

        owner.shutdown();

        assert_exact_processes_gone(&first);
        assert_exact_processes_gone(&second);
    }

    #[test]
    fn heartbeat_disappearance_enters_cleanup_and_retires_exact_processes() {
        if which::which("tmux").is_err() {
            return;
        }
        let mut owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let (_, processes) = spawn_server_with_processes(&owner, "heartbeat-loss");
        stop_heartbeat(&mut owner);
        fs::remove_file(root.join(".parent-heartbeat")).unwrap();

        let status = await_watchdog_exit(&mut owner);

        assert!(status.success());
        assert_exact_processes_gone(&processes);
        assert!(!root.exists());
        assert!(!control_root.exists());
        disarm_owner(&mut owner);
    }

    #[test]
    fn captured_identities_survive_fallible_anchor_creation() {
        let Ok(real_tmux) = which::which("tmux") else {
            return;
        };
        let hook_dir = TempDir::new().unwrap();
        let observed = hook_dir.path().join("observed");
        let hook = hook_dir.path().join("fail-anchor");
        fs::write(
            &hook,
            format!(
                "#!/bin/sh\nserver=$('{}' -S \"$1\" display-message -p '#{{pid}}')\npane=$('{}' -S \"$1\" list-panes -a -F '#{{pane_pid}}' | head -1)\nprintf '%s\\n%s\\n' \"$server\" \"$pane\" > '{}'\nexit 1\n",
                real_tmux.display(),
                real_tmux.display(),
                observed.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            (None, None),
            None,
            None,
            None,
            (None, None),
            (Some(&hook), None, None, None),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        write_adoption_environment(&control_root, "anchor-env.json", "anchor-token");
        write_raw_spawn_request(&root, &control_root, "anchor", "anchor-env.json");
        wait_until(
            || observed.exists(),
            "captured identities before anchor failure",
        );
        let identities = fs::read_to_string(&observed)
            .unwrap()
            .lines()
            .map(|pid| {
                let pid = pid.parse().unwrap();
                phoenix_core::process_identity::current_process_identity(pid).unwrap()
            })
            .collect::<Vec<_>>();
        wait_until(
            || control_root.join(".rejected-anchor").exists(),
            "anchor failure rejection",
        );

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(
            panic.is_err(),
            "missing durable anchor must fail cleanup closed"
        );
        for identity in identities {
            wait_until(
                || !phoenix_core::process_identity::process_identity_matches(identity),
                "captured process retirement after anchor failure",
            );
        }
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn retirement_discovered_exited_pane_uses_updated_owned_record() {
        if which::which("tmux").is_err() {
            return;
        }
        let hook_dir = TempDir::new().unwrap();
        let once = hook_dir.path().join("once");
        let hook = hook_dir.path().join("add-short-pane");
        fs::write(
            &hook,
            format!(
                "#!/bin/sh\n[ -e '{}' ] && exit 0\ntouch '{}'\ntmux -S \"$1\" new-window -d -t main sleep 0.05\n",
                once.display(),
                once.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            (None, Some(Duration::from_secs(2))),
            None,
            None,
            None,
            (None, None),
            (None, Some(&hook), None, None),
        );
        let (_, processes) = spawn_server_with_processes(&owner, "late-exited-pane");

        owner.shutdown();

        assert_exact_processes_gone(&processes);
    }

    #[test]
    fn replacement_identity_is_retained_before_previous_anchor_cleanup() {
        let append = WATCHDOG_PROGRAM
            .find("owned.append(record)")
            .expect("replacement identity is retained");
        let retain = WATCHDOG_PROGRAM
            .find("retained_controls[control.name] = (device, inode, processes, None)")
            .expect("replacement control identity is retained");
        let previous_anchor = WATCHDOG_PROGRAM
            .find("previous_anchor.unlink()")
            .expect("previous anchor is removed");
        assert!(append < previous_anchor && retain < previous_anchor);
    }

    #[test]
    fn retirement_discovered_hup_resistant_pane_blocks_false_cleanup_success() {
        if which::which("tmux").is_err() {
            return;
        }
        let hook_dir = TempDir::new().unwrap();
        let pane_pid_file = hook_dir.path().join("pane-pid");
        let hook = hook_dir.path().join("add-surviving-pane");
        fs::write(
            &hook,
            format!(
                "#!/bin/sh\n[ -e '{}' ] && exit 0\ntmux -S \"$1\" new-window -d -t main sh -c 'trap \"\" HUP; echo $$ > \"{}\"; exec sleep 30'\n",
                pane_pid_file.display(),
                pane_pid_file.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            (None, Some(Duration::from_secs(2))),
            None,
            None,
            None,
            (None, None),
            (None, Some(&hook), None, None),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let (_, original) = spawn_server_with_processes(&owner, "late-pane");

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(
            panic.is_err(),
            "surviving late pane must prevent cleanup success"
        );
        assert_exact_processes_gone(&original);
        let pane_pid = fs::read_to_string(&pane_pid_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        let late_pane = phoenix_core::process_identity::current_process_identity(pane_pid)
            .expect("late pane remains alive and inspectable");
        assert!(phoenix_core::process_identity::process_identity_matches(
            late_pane
        ));
        unsafe { libc::kill(pane_pid.cast_signed(), libc::SIGTERM) };
        wait_until(
            || !phoenix_core::process_identity::process_identity_matches(late_pane),
            "late pane test cleanup",
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn replacement_control_root_cannot_process_spawn_requests() {
        let mut owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let moved_control_root = control_root.with_extension("original");
        stop_heartbeat(&mut owner);
        fs::rename(&control_root, &moved_control_root).unwrap();
        fs::create_dir(&control_root).unwrap();
        fs::write(control_root.join("replacement-env.json"), "[]").unwrap();
        write_raw_spawn_request(
            &root,
            &control_root,
            "replacement-root",
            "replacement-env.json",
        );

        let status = await_watchdog_exit(&mut owner);

        assert!(!status.success());
        assert!(control_root.join(".spawn-replacement-root").exists());
        assert!(!control_root.join("replacement-root.sock").exists());
        assert!(!root.join("replacement-root.sock").exists());
        disarm_owner(&mut owner);
        assert!(!root.exists());
        assert!(!control_root.exists());
        fs::remove_dir_all(moved_control_root).unwrap();
    }

    #[test]
    fn successful_cleanup_disarms_tempdir_before_foreign_path_reuse() {
        let hook_dir = TempDir::new().unwrap();
        let hook = hook_dir.path().join("replace-control-root");
        fs::write(
            &hook,
            "#!/bin/sh\nmkdir \"$1\"\nprintf keep > \"$1/foreign\"\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            (None, None),
            None,
            None,
            None,
            (None, None),
            (None, None, None, Some(&hook)),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();

        owner.shutdown();

        assert_eq!(
            fs::read_to_string(control_root.join("foreign")).unwrap(),
            "keep"
        );
        assert!(!root.exists());
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn malformed_spawn_token_is_rejected_before_ownership_admission() {
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        fs::write(control_root.join("invalid-token-env.json"), "[]").unwrap();
        write_raw_spawn_request(
            &root,
            &control_root,
            "invalid_token",
            "invalid-token-env.json",
        );
        wait_until(
            || control_root.join(".rejected-invalid_token").exists(),
            "invalid token rejection",
        );
        assert!(!control_root.join("invalid_token.sock").exists());
        assert!(!root.join("invalid_token.sock").exists());

        owner.shutdown();
    }

    #[test]
    fn final_root_quarantine_classifies_every_moved_socket() {
        let hook_dir = TempDir::new().unwrap();
        let hook = hook_dir.path().join("publish-late-socket");
        fs::write(
            &hook,
            "#!/bin/sh\npython3 - \"$1/late.sock\" <<'PY'\nimport socket,sys\ns=socket.socket(socket.AF_UNIX)\ns.bind(sys.argv[1])\nPY\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            (None, None),
            None,
            None,
            None,
            (None, None),
            (None, None, Some(&hook), None),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(
            panic.is_err(),
            "unknown moved socket must fail cleanup closed"
        );
        assert!(root.join("late.sock").exists());
        fs::remove_dir_all(root).unwrap();
        if control_root.exists() {
            fs::remove_dir_all(control_root).unwrap();
        }
    }

    #[test]
    fn final_root_quarantine_allows_late_non_socket_artifact() {
        let hook_dir = TempDir::new().unwrap();
        let hook = hook_dir.path().join("publish-late-entry");
        fs::write(
            &hook,
            "#!/bin/sh\nmktemp \"$1/late-entry.XXXXXX\" >/dev/null\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            (None, None),
            None,
            None,
            None,
            (None, None),
            (None, None, Some(&hook), None),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();

        owner.shutdown();

        assert!(!root.exists());
        assert!(!control_root.exists());
    }

    #[test]
    fn retired_path_ownership_blocks_later_unregistered_kill() {
        assert!(WATCHDOG_PROGRAM.contains("if retired:\n        record = retired_record"));
        assert!(WATCHDOG_PROGRAM.contains("preserved_visible_paths.add(socket)"));
        assert!(WATCHDOG_PROGRAM.contains(
            "if socket in preserved_visible_paths:\n            unconfirmed = True\n            continue"
        ));
    }

    #[test]
    fn malformed_environment_shape_retires_all_owned_processes_and_fails_closed() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let (_, processes) = spawn_server_with_processes(&owner, "malformed-environment");
        fs::write(control_root.join("wrong-shape.json"), "null").unwrap();
        write_raw_spawn_request(&root, &control_root, "wrong-shape", "wrong-shape.json");
        wait_until(
            || control_root.join(".rejected-wrong-shape").exists(),
            "wrong-shape environment rejection",
        );

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(panic.is_err(), "malformed environment must remain visible");
        assert_exact_processes_gone(&processes);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn spawn_batch_rechecks_parent_liveness_before_each_dequeue() {
        assert!(WATCHDOG_PROGRAM.contains(
            "for request in control_root.glob(\".spawn-*\"):\n        if not owner_alive() or not spawn_owned(request):"
        ));
    }

    #[test]
    fn final_control_cleanup_authenticates_control_root_incarnation() {
        let capture = WATCHDOG_PROGRAM.find("control_root_identity = (").unwrap();
        let quarantine = WATCHDOG_PROGRAM
            .find("os.replace(control_root, quarantine)")
            .unwrap();
        let authenticate = WATCHDOG_PROGRAM
            .match_indices("!= control_root_identity")
            .map(|(offset, _)| offset)
            .find(|offset| *offset > quarantine)
            .unwrap();
        assert!(capture < quarantine && quarantine < authenticate);
    }

    #[test]
    fn unregistered_sweep_binds_probe_and_kill_to_quarantined_endpoint() {
        let quarantine = WATCHDOG_PROGRAM
            .find("os.replace(socket, quarantine)")
            .unwrap();
        let probe = WATCHDOG_PROGRAM
            .find("[\"tmux\", \"-S\", str(quarantine), \"list-sessions\"]")
            .unwrap();
        let kill = WATCHDOG_PROGRAM
            .find("[\"tmux\", \"-S\", str(quarantine), \"kill-server\"]")
            .unwrap();
        assert!(quarantine < probe && probe < kill);
    }

    #[test]
    fn final_socket_root_deletion_authenticates_atomic_quarantine() {
        let quarantine = WATCHDOG_PROGRAM
            .find("os.replace(root, root_quarantine)")
            .unwrap();
        let authenticate = WATCHDOG_PROGRAM.find("!= root_identity").unwrap();
        let remove = WATCHDOG_PROGRAM
            .find("shutil.rmtree(root_quarantine)")
            .unwrap();
        assert!(quarantine < authenticate && authenticate < remove);
    }

    #[test]
    fn forged_acknowledgments_do_not_commit_waiters() {
        let dir = TempDir::new().unwrap();
        let forged = dir.path().join("forged");
        fs::write(&forged, "forged").unwrap();
        assert!(!atomic_ack_matches(&forged, "published"));
        fs::remove_file(&forged).unwrap();
        fs::create_dir(&forged).unwrap();
        assert!(!atomic_ack_matches(&forged, "adopted"));
        fs::remove_dir(&forged).unwrap();
        fs::write(&forged, "published").unwrap();
        assert!(atomic_ack_matches(&forged, "published"));
    }

    #[test]
    fn absent_record_replacement_reconciles_all_matching_lifecycle_state() {
        let conflict = WATCHDOG_PROGRAM.find("if conflicts:").unwrap();
        let lifecycle = WATCHDOG_PROGRAM.get(conflict..).unwrap();
        let provisional = lifecycle.find("provisional[:] =").unwrap();
        let publication = lifecycle.find("adopted_pending_publication[:] =").unwrap();
        let owned = lifecycle.find("owned.remove(record)").unwrap();
        assert!(provisional < publication && publication < owned);
    }

    #[test]
    fn final_control_cleanup_requires_a_durable_hard_link_anchor() {
        let record = WATCHDOG_PROGRAM.find("os.link(control, anchor)").unwrap();
        let authenticate = WATCHDOG_PROGRAM
            .find("anchor_stat = anchor.stat()")
            .unwrap();
        let remove = WATCHDOG_PROGRAM.find("shutil.rmtree(quarantine)").unwrap();
        assert!(record < authenticate && authenticate < remove);
    }

    #[test]
    fn directory_retirement_marker_enters_global_cleanup_without_abandoning_records() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let (_, first) = spawn_server_with_processes(&owner, "directory-marker-first");
        let (_, second) = spawn_server_with_processes(&owner, "directory-marker-second");
        fs::create_dir(control_root.join(".retire-request-directory")).unwrap();
        wait_until(
            || control_root.join(".retire-rejected-directory").exists(),
            "directory retirement marker rejection",
        );

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(
            panic.is_err(),
            "marker removal failure must report cleanup failure"
        );
        assert_exact_processes_gone(&first);
        assert_exact_processes_gone(&second);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn replacement_socket_root_is_preserved_without_touching_its_socket() {
        if which::which("tmux").is_err() {
            return;
        }
        let mut owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let moved_root = root.with_file_name(format!(
            "{}-original",
            root.file_name().unwrap().to_string_lossy()
        ));
        fs::rename(&root, &moved_root).unwrap();
        fs::create_dir(&root).unwrap();
        let replacement_socket = root.join("replacement.sock");
        let replacement = std::os::unix::net::UnixListener::bind(&replacement_socket).unwrap();

        let error = owner
            .finish(true)
            .expect_err("replacement root must fail closed");

        assert!(error
            .to_string()
            .contains("watchdog reported cleanup failure"));
        assert!(
            replacement_socket.exists(),
            "replacement socket was removed"
        );
        assert_eq!(
            replacement.local_addr().unwrap().as_pathname(),
            Some(replacement_socket.as_path())
        );
        drop(replacement);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(moved_root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn dead_identity_requires_a_retained_inode_anchor_before_visible_cleanup() {
        let anchor = WATCHDOG_PROGRAM
            .find("os.link(control, control_anchor)")
            .expect("retirement retains the independently named control inode");
        let authenticate = WATCHDOG_PROGRAM
            .find("anchor_stat.st_dev != record[1] or anchor_stat.st_ino != record[2]")
            .expect("retirement authenticates the retained inode");
        let retire = WATCHDOG_PROGRAM
            .find("retired_record = retire_record(record")
            .expect("retirement follows anchor authentication");
        let classify = WATCHDOG_PROGRAM
            .find("visible_owned = socket_stat.st_dev == record[1]")
            .expect("retirement classifies the visible endpoint");
        let remove_control = WATCHDOG_PROGRAM
            .find("retired = remove_retired_control(record)")
            .expect("control cleanup follows process retirement");
        let unlink_anchor = WATCHDOG_PROGRAM
            .find("control_anchor.unlink()")
            .expect("anchor removal follows endpoint decisions");
        assert!(anchor < authenticate && authenticate < retire);
        assert!(retire < classify && classify < remove_control);
        assert!(remove_control < unlink_anchor);
    }

    #[test]
    fn live_unrelated_tmux_replacement_survives_retirement_and_final_sweep() {
        if which::which("tmux").is_err() {
            return;
        }
        let owner = TestTmuxServerOwner::new();
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let control = control_root.join("live-replacement.sock");
        let (socket, processes) = spawn_server_with_processes(&owner, "live-replacement");
        fs::remove_file(&socket).unwrap();
        assert!(Command::new("tmux")
            .args([
                "-S",
                &socket.to_string_lossy(),
                "new-session",
                "-d",
                "-s",
                "unrelated",
                "sleep 300",
            ])
            .env_remove("TMUX")
            .status()
            .unwrap()
            .success());

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert!(
            panic.is_err(),
            "protected replacement must fail cleanup closed"
        );
        assert_exact_processes_gone(&processes);
        assert_eq!(probe_sync(&socket), ProbeResult::Live);
        assert!(Command::new("tmux")
            .args(["-S", &socket.to_string_lossy(), "kill-server"])
            .status()
            .unwrap()
            .success());
        assert_ne!(probe_sync(&control), ProbeResult::Live);
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
            (None, None),
            None,
            Some(Duration::from_secs(1)),
            Some(&hook),
            (None, None),
            (None, None, None, None),
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
        assert_exact_processes_gone(&processes);
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
        let cancelled_processes = adopted.processes.clone();

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

        let replacement_token = "cancel-before-publish-replacement";
        let replacement_environment = vec![
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            (
                "PHOENIX_TMUX_SERVER_TOKEN".to_owned(),
                replacement_token.to_owned(),
            ),
        ];
        let replacement = spawn_owned_server(
            &socket,
            &control,
            &root.join("config"),
            &root,
            replacement_token,
            &replacement_environment,
        )
        .await
        .expect("same path must be re-admitted after cancelled publication");
        fs::hard_link(&control, &socket).unwrap();
        let replacement_processes = replacement.commit_publication().await.unwrap();

        owner.shutdown();

        assert_exact_processes_gone(&replacement_processes);
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
        let processes = adopted.processes.clone();
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
            (None, None),
            None,
            Some(Duration::from_secs(1)),
            Some(&hook),
            (None, None),
            (None, None, None, None),
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
            || {
                !phoenix_core::process_identity::process_identity_matches(processes.server)
                    && !root.join("io-failure.sock").exists()
                    && !control_root.join("io-failure.sock").exists()
            },
            "I/O failure exact retirement and endpoint removal",
        );
        fs::remove_dir_all(control_root.join(".adopted-io-failure")).unwrap();
        owner.shutdown();
    }

    #[tokio::test]
    async fn publication_marker_cleanup_failure_never_acknowledges_success() {
        if which::which("tmux").is_err() {
            return;
        }
        let hook_dir = TempDir::new().unwrap();
        let hook = hook_dir.path().join("replace-publication-marker");
        fs::write(&hook, "#!/bin/sh\nrm -f \"$1\"\nmkdir \"$1\"\n").unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            (None, None),
            None,
            None,
            None,
            (None, Some(&hook)),
            (None, None, None, None),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let socket = root.join("publication-cleanup-failure.sock");
        let control = control_root.join("publication-cleanup-failure.sock");
        let token = "publication-cleanup-failure-token";
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
        let processes = adopted.processes.clone();
        fs::hard_link(&control, &socket).unwrap();

        adopted
            .commit_publication()
            .await
            .expect_err("marker cleanup failure must not acknowledge publication");

        assert!(!control_root
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".publication-acknowledged-")));
        assert_exact_processes_gone(&processes);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));
        assert!(
            panic.is_err(),
            "marker failure evidence must prevent cleanup success"
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn malformed_publication_commit_marker_is_not_accepted() {
        assert!(WATCHDOG_PROGRAM.contains("publication_committed.is_file()"));
        assert!(WATCHDOG_PROGRAM.contains("not publication_committed.is_symlink()"));
        assert!(WATCHDOG_PROGRAM.contains("publication_committed.read_text() == \"committed\""));
    }

    #[test]
    fn publication_waits_for_deadline_before_missing_marker_retirement() {
        assert!(WATCHDOG_PROGRAM.contains("elif now >= deadline or published.exists():"));
    }

    #[tokio::test]
    async fn abort_after_publication_ack_before_commit_retires_visible_server() {
        if which::which("tmux").is_err() {
            return;
        }
        let hook_dir = TempDir::new().unwrap();
        let acknowledged = hook_dir.path().join("acknowledged");
        let hook = hook_dir.path().join("ack-publication");
        fs::write(
            &hook,
            format!(
                "#!/bin/sh\n: > \"$2\"\n: > '{}'\n/bin/sleep 0.4\n",
                acknowledged.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        let owner = TestTmuxServerOwner::new_with_watchdog_test_options(
            None,
            (None, None),
            None,
            None,
            None,
            (Some(Duration::from_secs(2)), Some(&hook)),
            (None, None, None, None),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let socket = root.join("abort-after-ack.sock");
        let control = control_root.join("abort-after-ack.sock");
        let token = "abort-after-ack-token";
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
        let processes = adopted.processes.clone();
        fs::hard_link(&control, &socket).unwrap();
        let task = tokio::spawn(adopted.commit_publication());
        if tokio::time::timeout(Duration::from_secs(120), async {
            while !acknowledged.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err()
        {
            task.abort();
            let _ = task.await;
            owner.shutdown();
            panic!("publication hook was not acknowledged");
        }
        task.abort();
        let _ = task.await;
        wait_until(
            || !phoenix_core::process_identity::process_identity_matches(processes.server),
            "post-ack publication cancellation retirement",
        );

        let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert_exact_processes_gone(&processes);
        assert!(!socket.exists());
        if cleanup.is_err() {
            if root.exists() {
                fs::remove_dir_all(&root).unwrap();
            }
            if control_root.exists() {
                fs::remove_dir_all(&control_root).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn cancellation_during_publication_hook_retires_visible_server() {
        if which::which("tmux").is_err() {
            return;
        }
        let hook_dir = TempDir::new().unwrap();
        let entered = hook_dir.path().join("entered");
        let hook = hook_dir.path().join("block-publication");
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
            (None, None),
            None,
            None,
            None,
            (Some(Duration::from_secs(2)), Some(&hook)),
            (None, None, None, None),
        );
        let root = owner.path().to_path_buf();
        let control_root = owner.control_root_path().to_path_buf();
        let socket = root.join("cancel-publication.sock");
        let control = control_root.join("cancel-publication.sock");
        let token = "cancel-publication-token";
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
        let processes = adopted.processes.clone();
        fs::hard_link(&control, &socket).unwrap();
        let task = tokio::spawn(adopted.commit_publication());
        while !entered.exists() {
            tokio::task::yield_now().await;
        }
        task.abort();
        let _ = task.await;
        wait_until(
            || !phoenix_core::process_identity::process_identity_matches(processes.server),
            "publication cancellation retirement",
        );

        let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));

        assert_exact_processes_gone(&processes);
        assert!(!socket.exists());
        if cleanup.is_err() {
            if root.exists() {
                fs::remove_dir_all(&root).unwrap();
            }
            if control_root.exists() {
                fs::remove_dir_all(&control_root).unwrap();
            }
        }
    }

    #[test]
    fn publication_hook_timeout_routes_through_exact_retirement() {
        assert!(WATCHDOG_PROGRAM.contains(
            "except (OSError, subprocess.TimeoutExpired):\n                retire_registered(socket, control, identities)\n                retain_obligation(socket, control)"
        ));
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
            (None, None),
            None,
            Some(Duration::from_secs(2)),
            Some(&hook),
            (None, None),
            (None, None, None, None),
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
        let cleanup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.shutdown()));
        if cleanup.is_err() {
            if root.exists() {
                fs::remove_dir_all(&root).unwrap();
            }
            if control_root.exists() {
                fs::remove_dir_all(&control_root).unwrap();
            }
        }
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
            (None, None),
            None,
            Some(Duration::ZERO),
            Some(&hook),
            (None, None),
            (None, None, None, None),
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
            (None, None),
            Some(&hook),
            None,
            None,
            (None, None),
            (None, None, None, None),
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
        assert_exact_processes_gone(&processes);

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
            (None, Some(Duration::from_millis(1))),
            None,
            None,
            None,
            (None, None),
            (None, None, None, None),
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
            assert_exact_processes_gone(&captured);
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
    fn pane_identities_are_bound_to_authenticated_server_without_requiring_late_tokens() {
        assert!(WATCHDOG_PROGRAM.contains(
            "identities.extend((pane_pid, birth(pane_pid), None) for pane_pid in pane_pids)"
        ));
        assert!(WATCHDOG_PROGRAM.contains("if token is None:\n        return \"owned\""));
    }

    #[test]
    fn retirement_captures_every_late_pane_before_killing_the_server() {
        let enumerate = WATCHDOG_PROGRAM
            .find("observed_server, pane_pids = query_control_processes(control, deadline)")
            .unwrap();
        let capture = WATCHDOG_PROGRAM
            .find("processes.append((pane_pid, started, None))")
            .unwrap();
        let kill = WATCHDOG_PROGRAM.find("\"kill-server\", \"\"],").unwrap();
        assert!(enumerate < capture && capture < kill);
    }

    #[test]
    fn lease_expiry_response_failure_retains_cleanup_obligation() {
        let lease_branch = WATCHDOG_PROGRAM.find("elif now >= deadline:").unwrap();
        let lease_program = WATCHDOG_PROGRAM.get(lease_branch..).unwrap();
        let response = lease_program
            .find("publish_response(\n                    adoption_rejected,")
            .unwrap();
        let guard = lease_program
            .find("except OSError:\n                retain_obligation(socket, control)")
            .unwrap();
        assert!(response < guard);
    }

    #[test]
    fn pre_cleanup_retirements_share_the_owner_cleanup_deadline() {
        assert!(WATCHDOG_PROGRAM
            .contains("if (root / \".cleanup-request\").exists() and cleanup_deadline is None:"));
        assert!(WATCHDOG_PROGRAM.contains(
            "return min(now + identity_timeout, cleanup_deadline) if cleanup_deadline else now + identity_timeout"
        ));
        assert!(WATCHDOG_PROGRAM.contains("retire_record(record, retirement_deadline())"));
    }

    #[test]
    fn final_cleanup_reenumerates_late_panes_before_retirement() {
        let cleanup = WATCHDOG_PROGRAM
            .find("for index, record in enumerate(owned):")
            .unwrap();
        let enumerate = WATCHDOG_PROGRAM
            .get(cleanup..)
            .unwrap()
            .find("observed_server, pane_pids = query_control_processes(control, cleanup_deadline)")
            .unwrap();
        let expand = WATCHDOG_PROGRAM
            .get(cleanup..)
            .unwrap()
            .find("expanded.append((pane_pid, started, None))")
            .unwrap();
        let retire = WATCHDOG_PROGRAM
            .get(cleanup..)
            .unwrap()
            .find("retire_record(record, cleanup_deadline)")
            .unwrap();
        assert!(enumerate < expand && expand < retire);
    }

    #[test]
    fn cleanup_request_stops_spawn_batch_before_next_spawn() {
        assert!(WATCHDOG_PROGRAM.contains(
            "for request in control_root.glob(\".spawn-*\"):\n        if not owner_alive() or not spawn_owned(request):"
        ));
    }

    #[test]
    fn adoption_hook_failure_retains_obligation_and_enters_cleanup() {
        assert!(WATCHDOG_PROGRAM.contains(
            "except (OSError, subprocess.TimeoutExpired):\n                retain_obligation(socket, control)\n                break"
        ));
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
        assert!(socket.exists(), "unrelated visible replacement was removed");
        drop(replacement);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(control_root).unwrap();
    }

    #[test]
    fn cleanup_accepts_only_jointly_owned_or_jointly_absent_processes() {
        assert!(WATCHDOG_PROGRAM
            .contains("if all(state == \"absent\" for state in states):\n        return record"));
        assert!(WATCHDOG_PROGRAM.contains(
            "if states[0] != \"owned\" or any(state not in (\"owned\", \"absent\") for state in states[1:]):\n        return None"
        ));
        assert!(WATCHDOG_PROGRAM.contains("retire_record(record, cleanup_deadline)"));
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
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
        assert_exact_processes_gone(&processes);
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
            additional_panes: Vec::new(),
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
        assert_exact_processes_gone(&processes);
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

    #[cfg(unix)]
    fn assert_killed(status: ExitStatus) {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }
}
