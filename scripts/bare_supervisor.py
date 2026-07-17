#!/usr/bin/env python3
"""Persistent same-user process owner for Phoenix on Linux without systemd."""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import socket
import struct
import subprocess
import sys
import time
import urllib.request
from pathlib import Path
from typing import Optional

PROTOCOL_VERSION = 1
MAX_REQUEST_BYTES = 16 * 1024


class SupervisorError(RuntimeError):
    pass


@dataclasses.dataclass(frozen=True)
class RuntimeIdentity:
    version: str
    git_sha: str


@dataclasses.dataclass(frozen=True)
class ChildIdentity:
    pid: int
    proc_start_time: int
    runtime: RuntimeIdentity


@dataclasses.dataclass(frozen=True)
class Layout:
    root: Path

    @property
    def run_dir(self) -> Path:
        return self.root / "run"

    @property
    def socket(self) -> Path:
        return self.run_dir / "supervisor.sock"

    @property
    def state(self) -> Path:
        return self.run_dir / "supervisor-state.json"

    @property
    def log(self) -> Path:
        return self.root / "prod.log"


def proc_start_time(pid: int, proc_root: Path = Path("/proc")) -> int:
    try:
        value = (proc_root / str(pid) / "stat").read_text()
        end = value.rfind(")")
        fields_after_comm = value[end + 2 :].split()
        return int(fields_after_comm[19])
    except (OSError, ValueError, IndexError) as exc:
        raise SupervisorError(f"cannot read process identity for PID {pid}") from exc


def direct_child_matches(child: subprocess.Popen[bytes], identity: ChildIdentity, proc_root: Path = Path("/proc")) -> bool:
    return (
        child.pid == identity.pid
        and child.poll() is None
        and proc_start_time(identity.pid, proc_root) == identity.proc_start_time
    )


def fetch_identity(url: str) -> RuntimeIdentity:
    with urllib.request.urlopen(url, timeout=2) as response:
        value = json.load(response)
    try:
        return RuntimeIdentity(version=str(value["version"]), git_sha=str(value["git_sha"]))
    except (KeyError, TypeError) as exc:
        raise SupervisorError("runtime returned no exact identity") from exc


def wait_for_identity(child: subprocess.Popen[bytes], expected: RuntimeIdentity, url: str, timeout: float) -> ChildIdentity:
    start_time = proc_start_time(child.pid)
    deadline = time.monotonic() + timeout
    observed = "not responding"
    while time.monotonic() < deadline:
        if child.poll() is not None:
            raise SupervisorError(f"managed child exited with status {child.returncode}")
        try:
            actual = fetch_identity(url)
            observed = f"{actual.version}/{actual.git_sha}"
            if actual == expected:
                identity = ChildIdentity(child.pid, start_time, actual)
                if direct_child_matches(child, identity):
                    return identity
        except Exception as exc:
            observed = type(exc).__name__
        time.sleep(0.1)
    raise SupervisorError(f"exact runtime identity did not become ready; observed {observed}")


def peer_uid(connection: socket.socket) -> int:
    if not sys.platform.startswith("linux") or not hasattr(socket, "SO_PEERCRED"):
        raise SupervisorError("Linux SO_PEERCRED is required")
    credentials = connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i"))
    _pid, uid, _gid = struct.unpack("3i", credentials)
    return uid


def write_json_atomic(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    with temporary.open("w") as stream:
        os.chmod(temporary, 0o600)
        json.dump(value, stream, sort_keys=True, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


class Supervisor:
    def __init__(self, layout: Layout, *, owner_uid: Optional[int] = None):
        self.layout = layout
        self.owner_uid = os.getuid() if owner_uid is None else owner_uid
        self.child: Optional[subprocess.Popen[bytes]] = None
        self.child_identity: Optional[ChildIdentity] = None
        self.running = True

    def prepare_layout(self) -> None:
        self.layout.root.mkdir(parents=True, exist_ok=True)
        os.chmod(self.layout.root, 0o700)
        self.layout.run_dir.mkdir(parents=True, exist_ok=True)
        os.chmod(self.layout.run_dir, 0o700)
        if self.layout.socket.is_symlink():
            raise SupervisorError("supervisor socket path must not be a symlink")
        self.layout.socket.unlink(missing_ok=True)

    def status(self) -> dict[str, object]:
        if self.child is not None and self.child_identity is not None and direct_child_matches(self.child, self.child_identity):
            child = dataclasses.asdict(self.child_identity)
            child["runtime"] = dataclasses.asdict(self.child_identity.runtime)
            return {"supervisor_pid": os.getpid(), "child": child}
        self.child_identity = None
        return {"supervisor_pid": os.getpid(), "child": None}

    def stop_child(self, timeout: float = 10) -> None:
        if self.child is None:
            self.child_identity = None
            return
        if self.child.poll() is None:
            self.child.terminate()
            try:
                self.child.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait(timeout=5)
        self.child = None
        self.child_identity = None
        write_json_atomic(self.layout.state, self.status())

    def start_child(self, command: list[str], env: dict[str, str], expected: RuntimeIdentity, health_url: str, timeout: float) -> ChildIdentity:
        self.stop_child()
        with self.layout.log.open("ab", buffering=0) as log:
            self.child = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            self.child_identity = wait_for_identity(self.child, expected, health_url, timeout)
        except BaseException:
            self.stop_child()
            raise
        write_json_atomic(self.layout.state, self.status())
        return self.child_identity

    def dispatch(self, request: dict[str, object]) -> dict[str, object]:
        if request.get("protocol_version") != PROTOCOL_VERSION:
            raise SupervisorError("unsupported supervisor protocol")
        action = request.get("action")
        if action == "status":
            return {"ok": True, **self.status()}
        if action == "stop":
            self.stop_child()
            return {"ok": True, **self.status()}
        if action == "shutdown-supervisor":
            self.stop_child()
            self.running = False
            return {"ok": True}
        raise SupervisorError("unsupported supervisor action")

    def serve(self) -> None:
        self.prepare_layout()
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            server.bind(str(self.layout.socket))
            os.chmod(self.layout.socket, 0o600)
            server.listen(8)
            server.settimeout(0.25)
            while self.running:
                if self.child is not None and self.child.poll() is not None:
                    self.child = None
                    self.child_identity = None
                    write_json_atomic(self.layout.state, self.status())
                try:
                    connection, _ = server.accept()
                except socket.timeout:
                    continue
                with connection:
                    try:
                        if peer_uid(connection) != self.owner_uid:
                            raise SupervisorError("peer UID does not own supervisor")
                        payload = connection.recv(MAX_REQUEST_BYTES + 1)
                        if len(payload) > MAX_REQUEST_BYTES:
                            raise SupervisorError("supervisor request is too large")
                        request = json.loads(payload)
                        if not isinstance(request, dict):
                            raise SupervisorError("supervisor request must be an object")
                        response = self.dispatch(request)
                    except Exception as exc:
                        response = {"ok": False, "error": str(exc)}
                    connection.sendall((json.dumps(response, sort_keys=True) + "\n").encode())
        finally:
            self.stop_child()
            server.close()
            self.layout.socket.unlink(missing_ok=True)


def request(socket_path: Path, payload: dict[str, object]) -> dict[str, object]:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        client.connect(str(socket_path))
        client.sendall(json.dumps(payload).encode())
        client.shutdown(socket.SHUT_WR)
        response = json.loads(client.makefile().readline())
        if not response.get("ok"):
            raise SupervisorError(str(response.get("error", "supervisor request failed")))
        return response
    finally:
        client.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--protocol-version", action="store_true")
    parser.add_argument("--root", type=Path, default=Path.home() / ".phoenix-ide")
    parser.add_argument("action", nargs="?", choices=("run", "status", "stop", "shutdown-supervisor"))
    args = parser.parse_args()
    if args.protocol_version:
        print(PROTOCOL_VERSION)
        return 0
    if args.action == "run":
        Supervisor(Layout(args.root)).serve()
        return 0
    if args.action is None:
        parser.error("an action is required")
    response = request(
        Layout(args.root).socket,
        {"protocol_version": PROTOCOL_VERSION, "action": args.action},
    )
    print(json.dumps(response, sort_keys=True, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
