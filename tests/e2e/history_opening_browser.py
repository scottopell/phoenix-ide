#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx", "httpx-sse"]
# ///
"""INSTRUMENTED runtime qualification, never shipping proof; never builds/installs.

Inputs are a clean independent 2f57bd1 checkout, independently built ignored
libtest executable and its CI receipt. Receipt extends opening-instrumented-
receipt.json with UI and telemetry-module hashes measured AFTER the UI build.
Opt-in hosted joint mode runtime-imports three hash-attested telemetry harness
modules from immutable APP_ROOT; history-only behavior remains the default.
Source/toolchain provenance is the build owner's attestation, not hash proof.
Only compiler-free selftests may be run locally for this change.
"""
from __future__ import annotations

import argparse
import contextlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import sys
import tempfile
import time
from urllib.parse import urlparse

import history_browser as shipping

PIN = "2f57bd1ba22569a01381d63117480cd946b6bd8d"
TREE_PIN = "61b490ca88624f159a0eb51e35abc200c99220bd"
BASE_PIN = "7af06a4004b97b534d468b6ca2f42c145c8d4fb9"
DRIVER = "opening_qualifier::actual_server_opening_qualifier"
CLASS = "instrumented_cfg_test_library_not_shipping_runtime"
ROOT = shipping.ROOT
SCRIPT = ROOT / "ui/scripts/qualify-history-opening-realserver.mjs"
TELEMETRY_MODULES = {
    "ui/scripts/qualify-view-telemetry.mjs": "d0a52f49d583ae55ede9320d6f2ef1bd581fdd2f17dff9f6f989e9a6f75adb83",
    "tests/e2e/view_telemetry_sink.py": "aa3e704f4cf1ac356f130e8e64ed2733c7b253b9d927dd6544b301065b5790fd",
    "tests/e2e/assert_view_telemetry.py": "18bb8465f046d7473b95a49157df5824019ae62e068d34fa98d24c1af656ecdb",
}
TELEMETRY_KEYS = frozenset({"PHOENIX_TRACE_EXPORTER", "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
                            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "OTEL_SERVICE_NAME"})


def identity(app, exe, receipt_path):
    app, exe = app.resolve(strict=True), exe.absolute()
    if app == ROOT.resolve() or shipping.git(app, "rev-parse", "--show-toplevel") != str(app):
        raise ValueError("independent app checkout required")
    if shipping.git(app, "rev-parse", "HEAD") != PIN:
        raise ValueError("app full source SHA mismatch")
    if shipping.git(app, "rev-parse", "HEAD^{tree}") != TREE_PIN:
        raise ValueError("app source tree mismatch")
    if shipping.git(app, "merge-base", PIN, BASE_PIN) != BASE_PIN:
        raise ValueError("integrated app base mismatch")
    if shipping.git(app, "status", "--porcelain", "--untracked-files=no"):
        raise ValueError("app tracked source dirty")
    if not exe.is_file() or exe.resolve() != exe or not os.access(exe, os.X_OK):
        raise ValueError("standalone non-symlink executable required")
    receipt = json.loads(receipt_path.read_text())
    modules = {relative: shipping.digest(app / relative) for relative in TELEMETRY_MODULES}
    if modules != TELEMETRY_MODULES:
        raise ValueError("telemetry runtime module hash mismatch")
    measured = {"app_source": PIN, "app_tree": TREE_PIN, "app_base": BASE_PIN,
        "harness_source": shipping.git(ROOT, "rev-parse", "HEAD"),
        "executable_sha256": shipping.digest(exe),
        "cargo_lock_sha256": shipping.digest(app / "Cargo.lock"),
        "ui_lock_sha256": shipping.digest(app / "ui/pnpm-lock.yaml"),
        "ui_files": shipping.ui_hashes(app), "telemetry_modules": modules,
        "evidence_class": CLASS, "ignored_driver": DRIVER}
    if any(receipt.get(k) != v for k, v in measured.items()):
        raise ValueError("instrumented receipt source/tree/harness/hash/lock/UI mismatch")
    if not isinstance(receipt.get("rustc"), str) or not receipt["rustc"].startswith("rustc "):
        raise ValueError("build rustc attestation required")
    if receipt.get("focused_test_exit") != 0 or receipt.get("compiler_artifact_selector") != [
            "cargo", "test", "--no-run", "--message-format=json"]:
        raise ValueError("instrumented build selector/focused tests missing")
    return measured | {"rustc": receipt["rustc"], "receipt_sha256": shipping.digest(receipt_path),
        "harness_files": {str(p.relative_to(ROOT)): shipping.digest(p) for p in
            [Path(__file__), SCRIPT, Path(shipping.__file__),
             ROOT / "ui/scripts/qualify-history-realserver.mjs",
             ROOT / "ui/scripts/history-postfault-baseline.mjs",
             ROOT / "ui/scripts/history-joint-telemetry.mjs"]},
        "runtime_imports": {str((app / relative).resolve()): digest for relative, digest in modules.items()},
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


def load_telemetry_modules(app):
    e2e = app / "tests/e2e"
    sink_path = e2e / "view_telemetry_sink.py"
    assertion_path = e2e / "assert_view_telemetry.py"
    if {relative: shipping.digest(app / relative) for relative in TELEMETRY_MODULES} != TELEMETRY_MODULES:
        raise ValueError("telemetry module identity changed before runtime import")
    previous = sys.modules.get("view_telemetry_sink")
    old_path = list(sys.path)
    try:
        sys.path.insert(0, str(e2e))
        sink_spec = importlib.util.spec_from_file_location("view_telemetry_sink", sink_path)
        if sink_spec is None or sink_spec.loader is None:
            raise ImportError("telemetry sink import spec unavailable")
        sink = importlib.util.module_from_spec(sink_spec)
        sys.modules["view_telemetry_sink"] = sink
        sink_spec.loader.exec_module(sink)
        assertion_spec = importlib.util.spec_from_file_location(
            "joint_assert_view_telemetry", assertion_path)
        if assertion_spec is None or assertion_spec.loader is None:
            raise ImportError("telemetry assertion import spec unavailable")
        assertion = importlib.util.module_from_spec(assertion_spec)
        assertion_spec.loader.exec_module(assertion)
        return sink, assertion
    except Exception:
        if previous is None:
            sys.modules.pop("view_telemetry_sink", None)
        else:
            sys.modules["view_telemetry_sink"] = previous
        raise
    finally:
        sys.path[:] = old_path


def _telemetry_sensitive(key):
    upper = key.upper()
    return (key == "PHOENIX_TRACE_EXPORTER" or upper.startswith("OTEL_")
            or upper.startswith("DD_") or "OTLP_HEADER" in upper)


def telemetry_environment(base, receiver):
    clean = {key: value for key, value in base.items() if not _telemetry_sensitive(key)}
    supplied = dict(receiver.env)
    if set(supplied) != TELEMETRY_KEYS:
        raise ValueError("receiver must own exactly four telemetry environment keys")
    endpoint = urlparse(supplied["OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"])
    if (endpoint.scheme != "http" or endpoint.hostname != "127.0.0.1" or endpoint.username
            or endpoint.password or endpoint.query or endpoint.fragment or endpoint.path != "/v1/traces"
            or endpoint.port is None):
        raise ValueError("receiver supplied a foreign telemetry endpoint")
    expected = {"PHOENIX_TRACE_EXPORTER": "otlp",
                "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL": "http/protobuf",
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": receiver.endpoint,
                "OTEL_SERVICE_NAME": receiver.service}
    if supplied != expected:
        raise ValueError("receiver telemetry environment values mismatch")
    result = clean | supplied
    if {key for key in result if _telemetry_sensitive(key)} != TELEMETRY_KEYS:
        raise ValueError("unexpected inherited telemetry environment")
    return result


def receiver_serving_check(receiver):
    if receiver.expired or not receiver.is_serving():
        raise RuntimeError("owned receiver expired or stopped before app drain completed")


def receiver_startup_check(receiver, run_id, services):
    receiver_serving_check(receiver)
    endpoint = urlparse(receiver.endpoint)
    expected_service = f"phoenix-view-qualification-{run_id}"
    if (endpoint.scheme != "http" or endpoint.hostname != "127.0.0.1"
            or endpoint.path != "/v1/traces" or endpoint.port is None
            or receiver.service != expected_service or receiver.service in services):
        raise ValueError("owned unique loopback receiver required")
    with socket.create_connection((endpoint.hostname, endpoint.port), timeout=2):
        pass
    services.add(receiver.service)
    return {"endpoint": receiver.endpoint, "service_name": receiver.service,
            "startup_checked_before_app": True}


def receiver_is_stopped(receiver):
    endpoint = urlparse(receiver.endpoint)
    try:
        connection = socket.create_connection((endpoint.hostname, endpoint.port), timeout=.25)
    except OSError:
        return True
    connection.close()
    return False


def actual_server_identity(startup, fixture):
    deployment = fixture.request("GET", "/api/deployment")
    source = deployment.get("build", {}).get("git_sha")
    if source != PIN or startup.get("git_sha") != source:
        raise ValueError("actual deployment source identity mismatch")
    db = Path(startup["db"]).resolve(strict=True)
    with sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=2) as connection:
        rows = connection.execute(
            "SELECT singleton_key, instance_id FROM instance_identity").fetchall()
    if len(rows) != 1 or rows[0][0] != 1 or not isinstance(rows[0][1], str):
        raise ValueError("one authoritative isolated server identity required")
    try:
        parsed = shipping.uuid.UUID(rows[0][1])
    except (ValueError, AttributeError) as error:
        raise ValueError("invalid authoritative server identity") from error
    if parsed.version != 4 or str(parsed) != rows[0][1]:
        raise ValueError("invalid authoritative server identity")
    return {"appSource": source, "frontendSource": source,
            "serverInstanceId": rows[0][1]}


def validate_joint_aggregates(aggregates):
    if not isinstance(aggregates, list) or len(aggregates) != 2:
        raise ValueError("two telemetry aggregates required")
    by_kind = {aggregate.get("kind"): aggregate for aggregate in aggregates}
    if set(by_kind) != {"ordinary", "global"} or len(by_kind) != len(aggregates):
        raise ValueError("one ordinary and one Global telemetry aggregate required")
    ordinary, global_ = by_kind["ordinary"], by_kind["global"]
    for aggregate in aggregates:
        if (not isinstance(aggregate.get("product_conversation_id"), str)
                or not aggregate["product_conversation_id"]
                or not isinstance(aggregate.get("route"), str)
                or not isinstance(aggregate.get("members"), list)
                or len(aggregate["members"]) < 2
                or aggregate.get("latest_transcript_row_id") != aggregate["members"][-1]):
            raise ValueError("telemetry aggregate identity is incomplete")
    root = global_["members"][0]
    latest = global_["latest_transcript_row_id"]
    if (len({ordinary["product_conversation_id"], global_["product_conversation_id"]}) != 2
            or len({global_["product_conversation_id"], root, latest}) != 3
            or global_["route"] != f"/global/{latest}"
            or global_["endpoint"] != f"/api/global/coordinator/history/{root}"):
        raise ValueError("Global telemetry product/root/latest identity mismatch")
    try:
        json.dumps(aggregates, allow_nan=False)
    except (TypeError, ValueError) as error:
        raise ValueError("telemetry fixtures must be finite JSON") from error
    return aggregates


def joint_fixture(identity_value, aggregates):
    aggregates = validate_joint_aggregates(aggregates)
    ordinary = next(aggregate for aggregate in aggregates if aggregate["kind"] == "ordinary")
    value = {"identity": identity_value, "aggregates": aggregates,
             "lifecycle": {"route": ordinary["route"],
                           "productConversationId": ordinary["product_conversation_id"],
                           "spaSelector": 'a.statebar-slug[href="/"][title="Back to conversations"]:visible'},
             "forbidden": []}
    json.dumps(value, allow_nan=False)
    return value


def telemetry_browser_result(path):
    if not path.is_file():
        raise RuntimeError("joint telemetry producer receipt is missing")
    result = json.loads(path.read_text())
    if not isinstance(result, dict) or not isinstance(result.get("journeys"), list):
        raise ValueError("joint telemetry producer receipt is invalid")
    return result


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
    product = shipping.validate_wire_identity(pages, members, aggregate["endpoint"], aggregate["kind"], aggregate.get("product_conversation_id"))
    keys = shipping.complete_occurrences(pages, member_messages)
    boundaries = shipping.member_boundaries(members, member_messages)
    assert all(page["boundaries"] == boundaries for page in pages), "original boundary metadata differs from persisted member edges"
    assert len(keys) > 201 and len(pages) >= 3
    assert any(key[2] == aggregate["old_message"] for key in keys), "older anchor lost"
    latest = pages[0]
    assert latest["latest_transcript_row_id"] == successor
    assert len(boundaries) == len(aggregate["boundaries"]) + 1
    opening = latest["latest_opening"]
    assert (opening["status"], opening["opening_authority"], opening["presentation"]) == (
        "accepted", "user_authorized_instruction", "separate_message")
    assert opening["accepted_message_id"] == opening_id
    assert opening["edge"]["product_conversation_id"] == product
    edge = opening["edge"]
    assert edge["predecessor_transcript_row_id"] == predecessor
    assert edge["successor_transcript_row_id"] == successor
    boundary = next(b for b in latest["boundaries"] if b["message_id"] == edge["continuation_message_id"])
    assert boundary["predecessor_transcript_row_id"] == predecessor
    assert boundary["successor_transcript_row_id"] == successor
    assert boundary["represented_successor_opening_message_id"] is None
    boundary_message = next(m for m in member_messages[-2] if m["message_id"] == boundary["message_id"])
    opening_message = next(m for m in member_messages[-1] if m["message_id"] == opening_id)
    assert opening_message["message_type"] == "user"
    assert boundary_message["content"]["summary"] == opening_message["content"]["text"] == summary
    result = aggregate | {"members": members, "latest_transcript_row_id": successor,
        "persisted_rows": sum(map(len, member_messages)), "page_count": len(pages),
        "occurrences": keys, "boundaries": boundaries, "manual_opening": opening, "product_conversation_id": product}
    if aggregate["kind"] == "global":
        result["route"] = f"/global/{successor}"
    return result


SERVER_COMPLETION_TIMEOUT = 65


class LifecycleFailures:
    def __init__(self):
        self.primary = None
        self.cleanup = []
        self.cleanup_overflow = False

    def remember_primary(self, stage, error):
        if self.primary is None:
            self.primary = (stage, error)

    def remember_cleanup(self, stage, error):
        if any(existing is error for _, existing in self.cleanup):
            return
        if len(self.cleanup) < 8:
            self.cleanup.append((stage, error))
        else:
            self.cleanup_overflow = True

    @contextlib.contextmanager
    def own(self, manager, stage):
        if stage not in {"receiver", "server", "preview", "browser"}:
            raise ValueError("unknown lifecycle evidence stage")
        body_error = None
        try:
            with manager as resource:
                try:
                    yield resource
                except BaseException as error:
                    self.remember_primary(stage, error)
                    body_error = self.primary[1]
                    if error is not body_error:
                        self.remember_cleanup(stage, error)
                        raise body_error from error
                    raise
        except BaseException as error:
            if body_error is not None and error is not body_error:
                self.remember_cleanup(stage, error)
                raise body_error from error
            self.remember_primary(stage, error)
            raise
        if body_error is not None:
            raise body_error

    def write_receipt(self, path):
        try:
            shipping.write_json(path, self.receipt())
        except Exception as error:
            self.remember_cleanup("failure_receipt", error)
            if self.primary is not None:
                raise self.primary[1] from error
            raise

    def receipt(self):
        def fact(item):
            stage, error = item
            message = str(error)
            return {"stage": stage, "type": type(error).__name__,
                    "message": message[:1024], "message_truncated": len(message) > 1024}
        return {"primary": fact(self.primary) if self.primary else None,
                "cleanup": [fact(item) for item in self.cleanup],
                "cleanup_overflow": self.cleanup_overflow}


def forced_server_shutdown(server, case, control, failures):
    receipt = {"status": "forced_shutdown_attempt", "control_present": control.exists(),
               "stop_response_received": False, "returncode_before": server.poll()}
    try:
        if receipt["control_present"]:
            with socket.socket(socket.AF_UNIX) as client:
                client.settimeout(2)
                client.connect(str(control))
                client.sendall(b'{"command":"stop"}\n')
                receipt["stop_response_received"] = bool(client.recv(4096))
    except (OSError, ValueError) as error:
        failures.remember_cleanup("server_control_stop", error)
        receipt["error_type"] = type(error).__name__
    finally:
        receipt["returncode_after"] = server.poll()
        shipping.write_json(case / "forced-server-shutdown.json", receipt)


def normal_server_completion(server, case, startup, control):
    """Called before owned_process's forced process-group cleanup."""
    receipt = {"status": "failed", "normal_wait_timeout_seconds": SERVER_COMPLETION_TIMEOUT,
               "owned_home": startup["home"], "control": str(control), "returncode": None}
    try:
        try:
            server.wait(timeout=SERVER_COMPLETION_TIMEOUT)
        except shipping.subprocess.TimeoutExpired:
            receipt["status"] = "timeout"
            raise TimeoutError("instrumented server normal drain exceeded 65 seconds")
        receipt["returncode"] = server.returncode
        receipt["status"] = ("success" if server.returncode == 0 else
                             "signal_exit" if server.returncode < 0 else "nonzero_exit")
        receipt["socket_removed"] = not control.exists()
        receipt["owned_home_removed"] = not Path(startup["home"]).exists()
        if not receipt["socket_removed"] or not receipt["owned_home_removed"]:
            receipt["exit_status"] = receipt["status"]
            receipt["status"] = "cleanup_failed"
        if receipt["status"] != "success":
            raise RuntimeError("instrumented server did not complete normally with owned resources removed")
    finally:
        shipping.write_json(case / "server-completion.json", receipt)
    return receipt


def qualify(app, exe, output, joint_telemetry=False):
    h = shipping.load_app_harness(app)
    telemetry = load_telemetry_modules(app) if joint_telemetry else None
    safe = {"PATH": os.environ["PATH"]}
    services = set()
    with tempfile.TemporaryDirectory(prefix="ho-", dir="/tmp") as short:
        temp = Path(short).resolve()
        os.chmod(temp, 0o700)
        for kind in ("ordinary", "global"):
            case = output / kind
            case.mkdir()
            run_id = f"history-{kind}-{shipping.uuid.uuid4().hex[:16]}"
            receiver_context = (telemetry[0].telemetry_sink(case, run_id, lifetime_seconds=650)
                                if telemetry else contextlib.nullcontext(None))
            receiver = server = None
            app_stopped = receiver_stopped = False
            lifecycle_errors = []
            failures = LifecycleFailures()
            try:
                with failures.own(receiver_context, "receiver") as receiver:
                    receiver_receipt = (receiver_startup_check(receiver, run_id, services)
                                        if receiver else None)
                    with tempfile.TemporaryDirectory(prefix=kind + "-", dir=temp) as owned:
                        work = Path(owned)
                        os.chmod(work, 0o700)
                        control = work / "c.sock"
                        port = h._free_port()
                        base = f"http://127.0.0.1:{port}"
                        env = safe | {"HOME": str(work), "TMPDIR": str(temp),
                                      "PHOENIX_PORT": str(port),
                                      "PHOENIX_TEST_OPENING_CONTROL": str(control)}
                        if receiver:
                            env = telemetry_environment(env, receiver)
                            shipping.write_json(case / "telemetry-receiver-startup.json",
                                                receiver_receipt)
                        command = [str(exe), DRIVER, "--exact", "--ignored",
                                   "--test-threads=1", "--nocapture"]
                        with failures.own(shipping.owned_process(
                                command, work, env,
                                case / "instrumented-runtime.log"), "server") as server:
                            try:
                                readiness(server, base + "/api/models")
                                startup = startup_identity(
                                    case / "instrumented-runtime.log", exe, work, port)
                                shipping.write_json(case / "startup.json", startup)
                                fixture = shipping.Fixture(h, base, case)
                                try:
                                    cwd = work / "cwd"
                                    cwd.mkdir(mode=0o700)
                                    aggregate = equal_text_fixture(
                                        fixture, fixture.build(kind, cwd))
                                    if receiver:
                                        other_kind = ("global" if kind == "ordinary"
                                                      else "ordinary")
                                        other_cwd = work / f"telemetry-{other_kind}-cwd"
                                        other_cwd.mkdir(mode=0o700)
                                        other = fixture.build(other_kind, other_cwd)
                                        other = other | {
                                            "latest_transcript_row_id": other["members"][-1]}
                                        fixtures = joint_fixture(
                                            actual_server_identity(startup, fixture),
                                            [aggregate, other])
                                        telemetry_input = {
                                            "runId": run_id,
                                            "serviceName": receiver.service,
                                            "fixtures": fixtures,
                                        }
                                        shipping.write_json(
                                            case / "telemetry-fixture.json", telemetry_input)
                                finally:
                                    fixture.client.close()
                                shipping.write_json(case / "fixture.json", aggregate)
                                preview_port = h._free_port()
                                ui = f"http://127.0.0.1:{preview_port}"
                                fault = work / "fault.json"
                                shipping.write_json(fault, {
                                    "aggregatePath": aggregate["endpoint"],
                                    "failRoster": False, "streamPath": "", "drop": 0})
                                config = work / "preview.mjs"
                                options = {
                                    "root": str(app / "ui"),
                                    "build": {"outDir": str(app / "ui/dist")},
                                    "preview": {"host": "127.0.0.1",
                                                "port": preview_port,
                                                "strictPort": True}}
                                config.write_text(
                                    "import { previewTransport } from "
                                    + json.dumps(SCRIPT.as_uri()) + ";\n"
                                    + "const config = " + json.dumps(options) + ";\n"
                                    + "config.preview.proxy = previewTransport("
                                    + json.dumps(base) + "," + json.dumps(str(fault))
                                    + "," + json.dumps(str(
                                        case / "preview-transport.jsonl"))
                                    + "); export default config;\n")
                                node_env = safe | {
                                    "HOME": str(work), "APP_ROOT": str(app),
                                    "HISTORY_BASE_URL": ui, "HISTORY_API_URL": base,
                                    "HISTORY_OUTPUT": str(case),
                                    "HISTORY_FIXTURE": str(case / "fixture.json"),
                                    "HISTORY_CONTROL": str(control),
                                    "HISTORY_FAULT": str(fault),
                                    "PLAYWRIGHT_BROWSERS_PATH": os.environ.get(
                                        "PLAYWRIGHT_BROWSERS_PATH",
                                        str(Path.home() / (
                                            "Library/Caches/ms-playwright"
                                            if os.sys.platform == "darwin"
                                            else ".cache/ms-playwright")))}
                                if receiver:
                                    node_env |= {
                                        "HISTORY_TELEMETRY_JOINT": "1",
                                        "HISTORY_TELEMETRY_FIXTURE": str(
                                            case / "telemetry-fixture.json"),
                                        "HISTORY_TELEMETRY_MODULE": str(
                                            app / "ui/scripts/qualify-view-telemetry.mjs"),
                                        "HISTORY_TELEMETRY_MODULE_SHA256":
                                            TELEMETRY_MODULES[
                                                "ui/scripts/qualify-view-telemetry.mjs"]}
                                with failures.own(shipping.owned_process(
                                        ["node", str(app /
                                            "ui/node_modules/vite/bin/vite.js"),
                                         "preview", "--config", str(config)],
                                        work, node_env, case / "preview.log"), "preview") as preview:
                                    readiness(preview, ui, shipping.digest(
                                        app / "ui/dist/index.html"))
                                    with failures.own(shipping.owned_process(
                                            ["node", str(SCRIPT)], work, node_env,
                                            case / "browser.log"), "browser") as driver:
                                        driver.wait(timeout=500 if receiver else 180)
                                        assert driver.returncode == 0, (
                                            "opening browser assertions failed; see case logs")
                                result = json.loads(
                                    (case / "browser-result.json").read_text())
                                assert result["status"] == "passed_assertions" and result["checks"]
                                if receiver:
                                    telemetry_result = case / "telemetry/telemetry-browser-result.json"
                                    telemetry_browser_result(telemetry_result)
                                    receiver_serving_check(receiver)
                                    receiver.assert_receipt(telemetry_result)
                                completion = normal_server_completion(
                                    server, case, startup, control)
                                app_stopped = (completion["status"] == "success"
                                               and server.poll() == 0)
                                if receiver:
                                    receiver_serving_check(receiver)
                                    shipping.write_json(case / "telemetry-app-stop-liveness.json", {
                                        "run_id": run_id, "service_name": receiver.service,
                                        "app_stopped": app_stopped, "receiver_serving": receiver.is_serving(),
                                        "receiver_expired": receiver.expired,
                                        "receiver_lifetime_seconds": 650,
                                        "scope": "observed after normal app completion before receiver exit"})
                            except BaseException as error:
                                failures.remember_primary("server_body", error)
                                raise
                            finally:
                                if not (case / "server-completion.json").exists():
                                    shipping.write_json(
                                        case / "server-completion.json", {
                                            "status": "force_cleanup_required",
                                            "returncode": server.poll()})
                                if json.loads((case / "server-completion.json").read_text())["status"] != "success":
                                    forced_server_shutdown(server, case, control, failures)
                        completion_path = case / "server-completion.json"
                        if completion_path.is_file():
                            app_stopped = (json.loads(completion_path.read_text()).get(
                                "status") == "success" and server.poll() == 0)
                        control.unlink(missing_ok=True)
                if receiver:
                    receiver_stopped = receiver_is_stopped(receiver)
                    if not app_stopped:
                        lifecycle_errors.append(
                            "app lacked normal completion before receiver shutdown")
                    if not receiver_stopped:
                        lifecycle_errors.append("receiver remained reachable after shutdown")
                    drain = {"run_id": run_id, "service_name": receiver.service,
                             "app_stopped": app_stopped,
                             "receiver_stopped": receiver_stopped,
                             "errors": lifecycle_errors}
                    shipping.write_json(case / "telemetry-drain.json", drain)
                    if lifecycle_errors:
                        raise RuntimeError("joint telemetry teardown failed")
                    final = telemetry[1].assertPhoenixTelemetry(
                        receiver.output, receiver.service, run_id,
                        case / "telemetry/telemetry-browser-result.json",
                        drain_receipt=drain)
                    shipping.write_json(case / "telemetry-final.json", final)
            except Exception as error:
                failures.remember_primary("case", error)
                lifecycle_errors.append(f"{type(error).__name__}: {error}")
                try:
                    if receiver is not None:
                        receiver_stopped = receiver_is_stopped(receiver)
                        drain_path = case / "telemetry-drain.json"
                        if not drain_path.exists():
                            shipping.write_json(drain_path, {
                                "run_id": run_id, "service_name": receiver.service,
                                "app_stopped": app_stopped,
                                "receiver_stopped": receiver_stopped,
                                "errors": lifecycle_errors})
                except Exception as cleanup_error:
                    failures.remember_cleanup("failure_drain", cleanup_error)
                    raise error from cleanup_error
                raise
            finally:
                failures.write_receipt(case / "lifecycle-failures.json")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app-root", required=True, type=Path)
    parser.add_argument("--instrumented-exe", required=True, type=Path)
    parser.add_argument("--instrumented-receipt", required=True, type=Path)
    parser.add_argument("--joint-telemetry", action="store_true",
                        help="hosted-only joint history plus telemetry evidence")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--timeout", type=int, default=1100)
    args = parser.parse_args()
    if not 120 <= args.timeout <= 1200:
        parser.error("whole-run timeout must be 120..1200 seconds")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    result = {"status": "failed", "evidence_class": CLASS, "runtime_executed": False,
              "joint_telemetry": args.joint_telemetry,
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
        qualify(app, exe, output, args.joint_telemetry)
        assert identity(app, exe, args.instrumented_receipt) == before, "artifact identity changed"
        result["status"] = "passed_assertions"
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        signal.alarm(0)
        result["cases"] = {kind: {
            "verified_runtime_startup": (output / kind / "startup.json").is_file(),
            "server_completion": json.loads((output / kind / "server-completion.json").read_text())
                if (output / kind / "server-completion.json").is_file() else {"status": "not_completed"},
            "lifecycle_failures": json.loads((output / kind / "lifecycle-failures.json").read_text())
                if (output / kind / "lifecycle-failures.json").is_file() else {"status": "not_observed"},
            "forced_shutdown": json.loads((output / kind / "forced-server-shutdown.json").read_text())
                if (output / kind / "forced-server-shutdown.json").is_file() else {"status": "not_observed"},
            "browser": json.loads((output / kind / "browser-result.json").read_text())
                if (output / kind / "browser-result.json").is_file() else {"status": "not_completed"},
            "telemetry": json.loads((output / kind / "telemetry-final.json").read_text())
                if (output / kind / "telemetry-final.json").is_file()
                else {"status": "not_enabled_or_completed"}}
            for kind in ("ordinary", "global")}
        result["runtime_executed"] = any(case["verified_runtime_startup"] for case in result["cases"].values())
        result["elapsed_seconds"] = time.monotonic() - start
        shipping.write_json(output / "result.json", result)


if __name__ == "__main__":
    main()
