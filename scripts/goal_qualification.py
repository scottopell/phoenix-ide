#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Same-job, full-workspace nextest harnesses for opt-in goal qualification.

Capture replaces the check compile phase. Its caller owns the existing timeout
and process group. Reuse never invokes a Cargo build or repairs missing evidence.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = ("phoenix-core", "phoenix-db", "phoenix_ide")
CAPTURE_COMMAND = ["cargo", "nextest", "list", "--locked", "--workspace",
                   "--list-type", "binaries-only", "--message-format", "json"]
METADATA_COMMAND = ["cargo", "metadata", "--locked", "--format-version", "1"]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read_command(command):
    return subprocess.run(command, cwd=ROOT, check=True, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE).stdout


def job_directory():
    required = ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_JOB", "RUNNER_TEMP")
    if os.environ.get("GITHUB_ACTIONS") != "true" or any(not os.environ.get(k) for k in required):
        raise ValueError("goal harness reuse requires a GitHub Actions job")
    if os.environ.get("NEXTEST_PROFILE", "default") != "default":
        raise ValueError("goal harness reuse requires the default nextest profile")
    expected = Path(os.environ["RUNNER_TEMP"]).resolve() / (
        f"goal-qualification-{os.environ['GITHUB_RUN_ID']}-"
        f"{os.environ['GITHUB_RUN_ATTEMPT']}-{os.environ['GITHUB_JOB']}"
    )
    actual = Path(os.environ["PHOENIX_GOAL_REUSE_DIR"])
    if actual != expected or actual.is_symlink():
        raise ValueError("goal harness directory must be private to this job attempt")
    return actual


def identity():
    status = read_command(["git", "status", "--porcelain=v1", "--untracked-files=normal"])
    if status:
        raise ValueError("goal harness qualification requires clean source")
    paths = read_command(["git", "ls-files", "-z"]).decode().split("\0")
    inputs = {ROOT / p for p in paths if p and (
        Path(p).name in {"Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml"}
        or p.startswith((".cargo/", ".config/nextest"))
    )}
    for ancestor in (ROOT, *ROOT.parents):
        inputs.update(ancestor / ".cargo" / name for name in ("config", "config.toml"))
    cargo_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    inputs.update(cargo_home / name for name in ("config", "config.toml"))
    files = {str(p): digest(p.read_bytes()) if p.is_file() else None for p in sorted(inputs)}
    # Worker counts and color are execution limits, not compiled-harness inputs.
    excluded = {"CARGO_BUILD_JOBS", "CARGO_TERM_COLOR", "RUST_TEST_THREADS"}
    build_env = {k: v for k, v in os.environ.items() if k not in excluded and (
        k.startswith(("CARGO_", "RUSTC", "RUSTUP_", "NEXTEST_")) or k in {"RUSTFLAGS", "RUSTDOCFLAGS"}
    )}
    if build_env.get("RUSTC_WRAPPER"):
        build_env["RUSTC_WRAPPER"] = shutil.which(build_env["RUSTC_WRAPPER"])
    return {
        "head": read_command(["git", "rev-parse", "HEAD"]).decode().strip(),
        "dirty": False, "root": str(ROOT), "inputs": files,
        "build_env_sha256": digest(json.dumps(build_env, sort_keys=True).encode()),
        "tools": {name: {"path": shutil.which(name), "version": read_command(cmd).decode().strip()}
                  for name, cmd in (("rustc", ["rustc", "-vV"]),
                                    ("cargo", ["cargo", "-Vv"]),
                                    ("cargo-nextest", ["cargo", "nextest", "--version"]))},
        "job": {k: os.environ[k] for k in (
            "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_JOB", "GITHUB_SHA", "RUNNER_TEMP")},
        "capture_command": CAPTURE_COMMAND, "metadata_command": METADATA_COMMAND,
        "profile": "default", "scope": "full-workspace",
    }


def binary_identity(path):
    path = Path(path)
    if not path.is_absolute() or not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError("captured test binary is missing or not executable")
    stat = path.stat()
    return [stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns]


def validate_metadata(directory):
    cargo = json.loads((directory / "cargo.json").read_bytes())
    binaries = json.loads((directory / "binaries.json").read_bytes())
    if Path(cargo["workspace_root"]).resolve() != ROOT:
        raise ValueError("Cargo metadata belongs to another workspace")
    packages = {p["id"]: p["name"] for p in cargo["packages"]}
    nodes = cargo["resolve"]["nodes"]
    resolved = {n["id"] for n in nodes}
    if not resolved or not resolved.issubset(packages) or any(
        dep not in resolved for node in nodes for dep in node["dependencies"]
    ):
        raise ValueError("Cargo metadata must contain a complete resolve graph")
    workspace = set(cargo["workspace_members"])
    if not workspace.issubset(resolved):
        raise ValueError("Cargo resolve graph is missing workspace members")
    build = binaries["rust-build-meta"]
    target = Path(build["target-directory"]).resolve()
    if target != Path(cargo["target_directory"]).resolve():
        raise ValueError("Cargo and nextest target directories differ")
    if not build["base-output-directories"] or any(
        Path(p).is_absolute() or ".." in Path(p).parts or Path(p).parts[-1] != "debug"
        for p in build["base-output-directories"]
    ):
        raise ValueError("only the existing test/debug build profile is admitted")
    build_dir = Path(build.get("build-directory") or target).resolve()
    paths, covered = {}, set()
    for binary_id, binary in binaries["rust-binaries"].items():
        package = binary["package-id"]
        path = Path(binary["binary-path"])
        if binary_id != binary["binary-id"] or package not in workspace or not (
            path.resolve().is_relative_to(target) or path.resolve().is_relative_to(build_dir)
        ):
            raise ValueError("nextest binary is outside the captured workspace build")
        paths[str(path)] = binary_identity(path)
        covered.add(package)
    if covered != workspace or not set(PACKAGES).issubset({packages[p] for p in covered}):
        raise ValueError("captured binaries must cover the full workspace and all three goal packages")
    return {"files": {name: digest((directory / name).read_bytes())
                      for name in ("cargo.json", "binaries.json")}, "binaries": paths}


def capture_json(command, directory, name):
    # These children inherit run_step's process group and total compile deadline.
    with (directory / f"{name}.json").open("xb") as stdout, (
        directory / f"{name}.stderr.log"
    ).open("xb") as stderr:
        with subprocess.Popen(command, cwd=ROOT, stdout=stdout, stderr=subprocess.PIPE) as process:
            assert process.stderr is not None
            for line in process.stderr:
                stderr.write(line)
                stderr.flush()
                sys.stderr.buffer.write(line)
                sys.stderr.buffer.flush()
            returncode = process.wait()
    if returncode:
        raise ValueError(f"{name} capture failed ({returncode}); see retained stderr log")


def capture():
    directory = job_directory()
    directory.mkdir(mode=0o700, exist_ok=False)
    before = identity()
    capture_json(METADATA_COMMAND, directory, "cargo")
    capture_json(CAPTURE_COMMAND, directory, "binaries")
    artifacts = validate_metadata(directory)
    if identity() != before:
        raise ValueError("source, build configuration, or toolchain changed during capture")
    receipt = {"identity": before, "artifacts": artifacts}
    with (directory / "ready.tmp").open("x") as output:
        json.dump(receipt, output, sort_keys=True)
    os.replace(directory / "ready.tmp", directory / "ready.json")
    print("Captured same-job full-workspace harnesses; no isolated-three-package build equivalence claimed")


def reuse_command(mode, threads, expression=None):
    directory = job_directory()
    receipt = json.loads((directory / "ready.json").read_bytes())
    if receipt["identity"] != identity() or receipt["artifacts"] != validate_metadata(directory):
        raise ValueError("stale, changed, or partial goal harness evidence")
    if mode == "focused":
        if threads != 2 or not expression:
            raise ValueError("focused qualification requires its filter and two workers")
        packages = " | ".join(f"package(={name})" for name in PACKAGES)
        expression = f"({packages}) & ({expression})"
    elif expression is not None:
        raise ValueError("only focused qualification accepts an explicit filter")
    else:
        expression = "test(/export_bindings/)" if mode == "codegen" else "not test(/export_bindings/)"
    command = ["cargo", "nextest", "run", "--cargo-metadata", str(directory / "cargo.json"),
               "--binaries-metadata", str(directory / "binaries.json"),
               "-E", expression, "--test-threads", str(threads), "--no-tests", "fail"]
    if mode == "broad":
        command += ["--failure-output", "immediate"]
    if mode == "focused":
        command += ["--no-fail-fast", "--failure-output", "immediate",
                    "--status-level", "all", "--final-status-level", "all"]
    return command


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("capture", "codegen", "broad", "focused"))
    parser.add_argument("--test-threads", type=int)
    parser.add_argument("--filter")
    args = parser.parse_args()
    try:
        if args.mode == "capture":
            if args.test_threads is not None or args.filter is not None:
                raise ValueError("capture does not accept test execution flags")
            capture()
        else:
            if args.test_threads is None or args.test_threads < 1:
                raise ValueError("reuse requires a positive test worker count")
            command = reuse_command(args.mode, args.test_threads, args.filter)
            os.execvp(command[0], command)
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(f"Goal harness qualification failed closed: {error}; no rebuild attempted",
              file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
