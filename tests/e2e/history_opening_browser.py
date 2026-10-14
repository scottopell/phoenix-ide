#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx", "httpx-sse"]
# ///
"""INSTRUMENTED runtime qualification, never shipping proof; never builds/installs.

Inputs are a clean independent d4c2772 checkout, independently built ignored
libtest executable and its CI receipt. Receipt extends opening-instrumented-
receipt.json with ui_files and ui_lock_sha256 measured AFTER the UI build.
Source/toolchain provenance is the build owner's attestation, not hash proof.
Only compiler-free selftests may be run locally for this change.
"""
from __future__ import annotations

import argparse
import contextlib
import json
import os
from pathlib import Path
import signal
import tempfile
import time

import history_browser as shipping

PIN = "d4c2772a95aa46da608b356c8fe7be074de1b7aa"
DRIVER = "opening_qualifier::actual_server_opening_qualifier"
CLASS = "instrumented_cfg_test_library_not_shipping_runtime"
ROOT = shipping.ROOT
SCRIPT = ROOT / "ui/scripts/qualify-history-opening-realserver.mjs"


def identity(app, exe, receipt_path):
    app, exe = app.resolve(strict=True), exe.absolute()
    if app == ROOT.resolve() or shipping.git(app, "rev-parse", "--show-toplevel") != str(app):
        raise ValueError("independent app checkout required")
    if shipping.git(app, "rev-parse", "HEAD") != PIN:
        raise ValueError("app full source SHA mismatch")
    if shipping.git(app, "status", "--porcelain", "--untracked-files=no"):
        raise ValueError("app tracked source dirty")
    if not exe.is_file() or exe.resolve() != exe or not os.access(exe, os.X_OK):
        raise ValueError("standalone non-symlink executable required")
    receipt = json.loads(receipt_path.read_text())
    measured = {"app_source": PIN, "app_tree": shipping.git(app, "rev-parse", "HEAD^{tree}"),
        "harness_source": shipping.git(ROOT, "rev-parse", "HEAD"),
        "executable_sha256": shipping.digest(exe),
        "cargo_lock_sha256": shipping.digest(app / "Cargo.lock"),
        "ui_lock_sha256": shipping.digest(app / "ui/pnpm-lock.yaml"),
        "ui_files": shipping.ui_hashes(app), "evidence_class": CLASS, "ignored_driver": DRIVER}
    if any(receipt.get(k) != v for k, v in measured.items()):
        raise ValueError("instrumented receipt source/tree/harness/hash/lock/UI mismatch")
    if not isinstance(receipt.get("rustc"), str) or not receipt["rustc"].startswith("rustc "):
        raise ValueError("build rustc attestation required")
    if receipt.get("focused_test_exit") != 0 or receipt.get("compiler_artifact_selector") != [
            "cargo", "test", "--no-run", "--message-format=json"]:
        raise ValueError("instrumented build selector/focused tests missing")
    return measured | {"rustc": receipt["rustc"], "receipt_sha256": shipping.digest(receipt_path),
        "harness_files": {str(p.relative_to(ROOT)): shipping.digest(p) for p in
            [Path(__file__), SCRIPT, Path(shipping.__file__), ROOT / "ui/scripts/qualify-history-realserver.mjs"]},
        "source_provenance": "independent build-owner attestation"}


def startup_identity(log, exe, temp, port):
    lines = [line.split("OPENING_QUALIFIER ", 1)[1] for line in log.read_text().splitlines()
             if "OPENING_QUALIFIER " in line]
    if len(lines) != 1:
        raise ValueError("one compiled startup identity required")
    data = json.loads(lines[0])
    home, db = Path(data["home"]).resolve(strict=True), Path(data["db"]).resolve()
    if (data.get("git_sha") != PIN or Path(data["exe"]).resolve() != exe.resolve()
            or data.get("bind") != "127.0.0.1" or data.get("port") != port
            or data.get("instrumented_test_binary") is not True
            or not home.is_relative_to(temp.resolve()) or home == temp.resolve()
            or db != home / ".phoenix-ide/phoenix.db"):
        raise ValueError("compiled startup app/exe/owned HOME/DB/loopback identity mismatch")
    return data | {"executable_sha256": shipping.digest(exe)}


def readiness(proc, url, expected_hash=None, timeout=30):
    import httpx
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if proc.poll() is not None:
            raise RuntimeError("owned child exited before HTTP readiness; see retained logs")
        try:
            response = httpx.get(url, timeout=1, trust_env=False)
            if response.status_code == 200:
                if expected_hash is not None:
                    import hashlib
                    assert hashlib.sha256(response.content).hexdigest() == expected_hash, "preview hash mismatch"
                return
        except httpx.TransportError:
            pass
        time.sleep(.1)
    raise TimeoutError("actual HTTP readiness deadline")


def exhausted_summary(state):
    """ConvState is internally tagged by type; ContextExhausted carries summary: String."""
    if (not isinstance(state, dict) or state.get("type") != "context_exhausted"
            or not isinstance(state.get("summary"), str) or not state["summary"].strip()):
        raise ValueError("persisted ContextExhausted state with nonempty summary required")
    return state["summary"]


def equal_text_fixture(fixture, aggregate):
    predecessor = aggregate["members"][-1]
    fixture.send(predecessor, "[[scenario:context_window_exceeded]] [[ttft:0]]", True)
    persisted = fixture.request("GET", f"/api/conversations/{predecessor}")
    summary = exhausted_summary(persisted["conversation"]["state"])
    key = str(shipping.uuid.uuid4())
    response = fixture.request("POST", f"/api/conversations/{predecessor}/continue",
                               {"handoff": summary, "message_id": key})
    successor = response["conversation_id"]
    assert response["status"] == "accepted" and successor not in aggregate["members"], response
    opening_id = f"{successor}:{key}"
    fixture.wait(successor, message_id=opening_id)
    fixture.send(successor, "[[scenario:context_window_exceeded]] [[ttft:0]]", True)

    members = aggregate["members"] + [successor]
    member_messages = [fixture.request("GET", f"/api/conversations/{row}")["messages"]
                       for row in members]
    pages, cursor, cursors = [], None, set()
    for _ in range(8):
        query = {"message_limit": 100, "boundary_representation": "original_messages"}
        if cursor:
            query["before"] = cursor
        page = fixture.request("GET", aggregate["endpoint"] + "?" + shipping.urlencode(query))
        pages.append(page)
        if not page["has_older"]:
            break
        cursor = page["before"]
        assert cursor and cursor not in cursors, "cursor did not advance"
        cursors.add(cursor)
    else:
        raise AssertionError("equal-text fixture paging exceeded eight pages")
    keys = shipping.complete_occurrences(pages, member_messages)
    assert len(keys) > 201 and len(pages) >= 3
    assert any(key[2] == aggregate["old_message"] for key in keys), "older anchor lost"
    latest = pages[0]
    assert latest["latest_transcript_row_id"] == successor
    assert len(latest["boundaries"]) == len(aggregate["boundaries"]) + 1
    opening = latest["latest_opening"]
    assert (opening["status"], opening["opening_authority"], opening["presentation"]) == (
        "accepted", "user_authorized_instruction", "separate_message")
    assert opening["accepted_message_id"] == opening_id
    edge = opening["edge"]
    assert edge["predecessor_transcript_row_id"] == predecessor
    assert edge["successor_transcript_row_id"] == successor
    boundary = next(b for b in latest["boundaries"] if b["message_id"] == edge["continuation_message_id"])
    assert boundary["represented_successor_opening_message_id"] is None
    boundary_message = next(m for m in member_messages[-2] if m["message_id"] == boundary["message_id"])
    opening_message = next(m for m in member_messages[-1] if m["message_id"] == opening_id)
    assert opening_message["message_type"] == "user"
    assert boundary_message["content"]["summary"] == opening_message["content"]["text"] == summary
    result = aggregate | {"members": members, "latest_transcript_row_id": successor,
        "persisted_rows": sum(map(len, member_messages)), "page_count": len(pages),
        "occurrences": keys, "boundaries": latest["boundaries"], "manual_opening": opening}
    if aggregate["kind"] == "global":
        result["route"] = f"/global/{successor}"
    return result


def qualify(app, exe, output):
    h = shipping.load_app_harness(app)
    safe = {k: os.environ[k] for k in ("PATH", "SYSTEMROOT") if k in os.environ}
    # /tmp keeps Unix paths short and is the Rust driver's system temp root on Linux/macOS.
    with tempfile.TemporaryDirectory(prefix="ho-", dir="/tmp") as short:
        temp = Path(short).resolve()
        os.chmod(temp, 0o700)
        for kind in ("ordinary", "global"):
            case = output / kind
            case.mkdir()
            with tempfile.TemporaryDirectory(prefix=kind + "-", dir=temp) as owned:
                work = Path(owned)
                os.chmod(work, 0o700)
                control = work / "c.sock"
                port = h._free_port()
                base = f"http://127.0.0.1:{port}"
                env = safe | {"HOME": str(work), "TMPDIR": str(temp), "PHOENIX_PORT": str(port),
                              "PHOENIX_TEST_OPENING_CONTROL": str(control)}
                command = [str(exe), DRIVER, "--exact", "--ignored", "--test-threads=1", "--nocapture"]
                with shipping.owned_process(command, work, env, case / "instrumented-runtime.log") as server:
                    try:
                        readiness(server, base + "/api/models")
                        startup = startup_identity(case / "instrumented-runtime.log", exe, work, port)
                        shipping.write_json(case / "startup.json", startup)
                        fixture = shipping.Fixture(h, base, case)
                        try:
                            cwd = work / "cwd"
                            cwd.mkdir(mode=0o700)
                            aggregate = fixture.build(kind, cwd)
                            aggregate = equal_text_fixture(fixture, aggregate)
                        finally:
                            fixture.client.close()
                        shipping.write_json(case / "fixture.json", aggregate)
                        preview_port = h._free_port()
                        ui = f"http://127.0.0.1:{preview_port}"
                        fault = work / "fault.json"
                        shipping.write_json(fault, {"aggregatePath": aggregate["endpoint"], "failRoster": False,
                            "streamPath": "", "drop": 0})
                        config = work / "preview.mjs"
                        options = {"root": str(app / "ui"), "build": {"outDir": str(app / "ui/dist")},
                            "preview": {"host": "127.0.0.1", "port": preview_port, "strictPort": True}}
                        config.write_text("import { previewTransport } from " + json.dumps(SCRIPT.as_uri()) + ";\n"
                            + "const config = " + json.dumps(options) + ";\n"
                            + "config.preview.proxy = previewTransport(" + json.dumps(base) + ","
                            + json.dumps(str(fault)) + "," + json.dumps(str(case / "preview-transport.jsonl"))
                            + "); export default config;\n")
                        node_env = safe | {"HOME": str(work), "APP_ROOT": str(app), "HISTORY_BASE_URL": ui,
                            "HISTORY_API_URL": base, "HISTORY_OUTPUT": str(case), "HISTORY_FIXTURE": str(case / "fixture.json"),
                            "HISTORY_CONTROL": str(control), "HISTORY_FAULT": str(fault),
                            "PLAYWRIGHT_BROWSERS_PATH": os.environ.get("PLAYWRIGHT_BROWSERS_PATH", str(Path.home() /
                                ("Library/Caches/ms-playwright" if os.sys.platform == "darwin" else ".cache/ms-playwright")))}
                        with shipping.owned_process(["node", str(app / "ui/node_modules/vite/bin/vite.js"),
                                "preview", "--config", str(config)], work, node_env, case / "preview.log") as preview:
                            readiness(preview, ui, shipping.digest(app / "ui/dist/index.html"))
                            with shipping.owned_process(["node", str(SCRIPT)], work, node_env, case / "browser.log") as driver:
                                driver.wait(timeout=180)
                                assert driver.returncode == 0, "opening browser assertions failed; see case logs"
                        result = json.loads((case / "browser-result.json").read_text())
                        assert result["status"] == "passed_assertions" and result["checks"]
                    finally:
                        # Node normally releases exactly once; Stop is a distinct failure cleanup command.
                        if control.exists():
                            import socket
                            with contextlib.suppress(OSError, ValueError):
                                with socket.socket(socket.AF_UNIX) as client:
                                    client.settimeout(2)
                                    client.connect(str(control))
                                    client.sendall(b'{"command":"stop"}\n')
                                    client.recv(4096)
                control.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app-root", required=True, type=Path)
    parser.add_argument("--instrumented-exe", required=True, type=Path)
    parser.add_argument("--instrumented-receipt", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--timeout", type=int, default=1100)
    args = parser.parse_args()
    if not 120 <= args.timeout <= 1200:
        parser.error("whole-run timeout must be 120..1200 seconds")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    result = {"status": "failed", "evidence_class": CLASS, "runtime_executed": False,
              "uncovered": ["Shipping binary, real providers, production, physical devices.",
                "Generated authority: pinned manual ContinueConversationRequest cannot select opening_authority."]}
    start = time.monotonic()
    def expired(*_):
        raise TimeoutError("opening qualification whole-run deadline")
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(args.timeout)
    try:
        app, exe = args.app_root.resolve(), args.instrumented_exe.absolute()
        before = identity(app, exe, args.instrumented_receipt)
        shipping.write_json(output / "identity.json", before)
        qualify(app, exe, output)
        assert identity(app, exe, args.instrumented_receipt) == before, "artifact identity changed"
        result["status"] = "passed_assertions"
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        signal.alarm(0)
        result["cases"] = {kind: {
            "verified_runtime_startup": (output / kind / "startup.json").is_file(),
            "browser": json.loads((output / kind / "browser-result.json").read_text())
                if (output / kind / "browser-result.json").is_file() else {"status": "not_completed"}}
            for kind in ("ordinary", "global")}
        result["runtime_executed"] = any(case["verified_runtime_startup"] for case in result["cases"].values())
        result["elapsed_seconds"] = time.monotonic() - start
        shipping.write_json(output / "result.json", result)


if __name__ == "__main__":
    main()
