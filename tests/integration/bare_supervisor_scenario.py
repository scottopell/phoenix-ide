#!/usr/bin/env python3
"""Exercise the bare-Linux supervisor ownership boundary on Linux."""

import argparse
import importlib.util
import json
import os
import sys
from pathlib import Path


def load(path):
    spec = importlib.util.spec_from_file_location("bare_supervisor_scenario", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def parent_pid(pid):
    value = (Path("/proc") / str(pid) / "stat").read_text()
    return int(value[value.rfind(")") + 2 :].split()[1])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--supervisor", required=True, type=Path)
    parser.add_argument("--fixture", required=True, type=Path)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--port", required=True, type=int)
    args = parser.parse_args()
    if not args.root.name.startswith("phoenix-qa-bare-") or args.port == 8031:
        raise SystemExit("refusing non-disposable bare supervisor scenario")

    module = load(args.supervisor)
    args.root.mkdir(mode=0o700)
    owner = module.Supervisor(module.Layout(args.root))
    child = owner.start_child(
        [
            "/usr/bin/python3", str(args.fixture),
            "--version", "1.0.0", "--git-sha", "aaaaaaaaaaaa",
            "--port", str(args.port),
        ],
        os.environ.copy(),
        module.RuntimeIdentity("1.0.0", "aaaaaaaaaaaa"),
        f"http://127.0.0.1:{args.port}/api/version",
        10,
    )
    try:
        if parent_pid(child.pid) != os.getpid():
            raise RuntimeError("supervisor is not the direct parent of Phoenix")
        if module.proc_start_time(child.pid) != child.proc_start_time:
            raise RuntimeError("child proc start time is not stable")
        if owner.status()["child"]["runtime"] != {"version": "1.0.0", "git_sha": "aaaaaaaaaaaa"}:
            raise RuntimeError("supervisor status lost exact runtime identity")
        owner.stop_child()
        if owner.status()["child"] is not None:
            raise RuntimeError("child stop did not clear managed identity")
        print(json.dumps({"direct_parent": True, "start_time": child.proc_start_time, "child_only_stop": True}))
    finally:
        owner.stop_child()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
