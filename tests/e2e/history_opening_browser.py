#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx", "httpx-sse"]
# ///
"""INSTRUMENTED runtime qualification, never shipping proof; never builds/installs.

Inputs are a clean independent checkout matching PIN/TREE_PIN, independently built ignored
libtest executable and its CI receipt. Receipt extends opening-instrumented-
receipt.json with UI and telemetry-module hashes measured AFTER the UI build.
Opt-in hosted joint mode runtime-imports four hash-attested telemetry harness
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
import uuid
from urllib.parse import urlparse, urlencode, quote

import history_browser as shipping
import history_controlled_artifact as artifact_admission

PIN = "a57b23bcd53163efa4782e21b1214a3963006dd2"
TREE_PIN = "718cbba72ca615733902a4d0c6166a7cb9ac06d2"
BASE_PIN = "92986f0481df5b1ae9e29bde382670e60f369588"
DRIVER = "opening_qualifier::actual_server_opening_qualifier"
COLLECTOR_TEST = "api::view_journey::tests::collector_witness_requires_real_auth_and_validation_and_counts_each_acceptance"
SDK_METADATA_TEST = "api::view_journey::tests::pinned_default_layer_emits_typed_sdk_metadata_separately_from_journey_fields"
PRODUCTION_CAPACITY_TESTS = [
    "api::view_journey::tests::production_attribute_capacity_preserves_complete_ready_collector_schema",
    "logging::tests::otel_limits_are_conservative_and_event_free",
    "logging::tests::otel_filter_is_a_spans_only_allowlist",
]
CLASS = "instrumented_cfg_test_library_not_shipping_runtime"
ROOT = shipping.ROOT
SCRIPT = ROOT / "ui/scripts/qualify-history-opening-realserver.mjs"
TELEMETRY_MODULES = {
    "ui/scripts/qualify-view-telemetry.mjs": "1c702c78bb06f74f3dae4c8f97d1932fc308959596f3602375bc65d7d99fb9e1",
    "ui/scripts/view-telemetry-diagnostics.mjs": "e06232a39200d88e0a98aedb3d9e6183537f97a6e9498f27fe57a7e2a9a4696b",
    "tests/e2e/view_telemetry_sink.py": "cdb1744c69eee7db301c990e53c8e636eee00ff3d5550d9097ba0cb96b0bd695",
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
    artifact_admission.require_build_owner_binding(app, receipt)
    modules = {relative: shipping.digest(app / relative) for relative in TELEMETRY_MODULES}
    if modules != TELEMETRY_MODULES:
        raise ValueError("telemetry runtime module hash mismatch")
    measured = {"app_source": PIN, "app_tree": TREE_PIN, "app_base": BASE_PIN,
        "harness_source": shipping.git(ROOT, "rev-parse", "HEAD"),
        "executable_sha256": shipping.digest(exe),
        "cargo_lock_sha256": shipping.digest(app / "Cargo.lock"),
        "ui_lock_sha256": shipping.digest(app / "ui/pnpm-lock.yaml"),
        "ui_files": shipping.ui_hashes(app), "telemetry_modules": modules,
        "ui_artifacts": artifact_admission.build_owner_binding(app),
        "evidence_class": CLASS, "ignored_driver": DRIVER}
    if any(receipt.get(k) != v for k, v in measured.items()):
        raise ValueError("instrumented receipt source/tree/harness/hash/lock/UI mismatch")
    if not isinstance(receipt.get("rustc"), str) or not receipt["rustc"].startswith("rustc "):
        raise ValueError("build rustc attestation required")
    if receipt.get("focused_test_exit") != 0 or receipt.get("compiler_artifact_selector") != [
            "cargo", "test", "--no-run", "--message-format=json"]:
        raise ValueError("instrumented build selector/focused tests missing")
    if (type(receipt.get("collector_witness_exit")) is not int or receipt["collector_witness_exit"] != 0
            or receipt.get("collector_witness_command") != [COLLECTOR_TEST, "--exact", "--nocapture", "--test-threads=1"]):
        raise ValueError("real collector witness test was not successfully executed")
    if (type(receipt.get("sdk_metadata_exit")) is not int or receipt["sdk_metadata_exit"] != 0
            or type(receipt.get("sdk_metadata_passed")) is not int or receipt["sdk_metadata_passed"] != 1
            or receipt.get("sdk_metadata_command") != [SDK_METADATA_TEST, "--exact", "--nocapture", "--test-threads=1"]):
        raise ValueError("real default SDK layer test was not successfully executed exactly once")
    capacity = receipt.get("production_capacity_tests")
    if not isinstance(capacity, list) or len(capacity) != len(PRODUCTION_CAPACITY_TESTS):
        raise ValueError("production collector capacity and privacy guards missing")
    for test, result in zip(PRODUCTION_CAPACITY_TESTS, capacity, strict=True):
        if (not isinstance(result, dict) or set(result) != {"command", "exit", "passed"}
                or result["command"] != [test, "--exact", "--nocapture", "--test-threads=1"]
                or type(result["exit"]) is not int or result["exit"] != 0
                or type(result["passed"]) is not int or result["passed"] != 1):
            raise ValueError("production collector capacity and privacy guards did not execute exactly once")
    return measured | {"rustc": receipt["rustc"], "receipt_sha256": shipping.digest(receipt_path),
        "harness_files": {str(p.relative_to(ROOT)): shipping.digest(p) for p in
            [Path(__file__), SCRIPT, Path(shipping.__file__), Path(artifact_admission.__file__),
             ROOT/'ui/scripts/history-controlled-functional.mjs',
             ROOT/'ui/scripts/qualify-history-controlled-functional.mjs',
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


def owned_qualification_password(startup, evidence_path):
    import stat
    home = Path(startup["home"])
    path = Path(startup["credential_file"])
    try:
        info, directory = path.lstat(), home.lstat()
        checks = {
            "owned_child": path == home / "qualification-password",
            "canonical": path.resolve(strict=True) == path,
            "file_regular": stat.S_ISREG(info.st_mode),
            "file_mode_0600": stat.S_IMODE(info.st_mode) == 0o600,
            "file_current_uid": info.st_uid == os.getuid(),
            "file_size_36": info.st_size == 36,
            "home_directory": stat.S_ISDIR(directory.st_mode),
            "home_mode_0700": stat.S_IMODE(directory.st_mode) == 0o700,
            "home_current_uid": directory.st_uid == os.getuid(),
        }
        facts = {"checks": checks, "file_mode": oct(stat.S_IMODE(info.st_mode)),
                 "home_mode": oct(stat.S_IMODE(directory.st_mode)), "file_size": info.st_size}
    except OSError as error:
        shipping.write_json(evidence_path, {"status": "stat_failed", "error_type": type(error).__name__})
        raise ValueError("owned synthetic credential metadata unavailable") from None
    shipping.write_json(evidence_path, {"status": "valid" if all(checks.values()) else "invalid", **facts})
    if not all(checks.values()):
        raise ValueError("owned synthetic credential file identity mismatch")
    try:
        password = path.read_text()
        valid = str(uuid.UUID(password)) == password
    except (OSError, ValueError):
        raise ValueError("owned synthetic credential format unavailable or invalid") from None
    if not valid:
        raise ValueError("owned synthetic credential format mismatch")
    return password


def authenticate_fixture(client, password):
    def require_status(authenticated):
        response = client.get("/api/auth/status")
        try:
            value = response.json()
        except ValueError:
            raise ValueError("owned fixture authentication status invalid JSON") from None
        if (response.status_code != 200 or not isinstance(value, dict)
                or value.get("auth_required") is not True
                or value.get("authenticated") is not authenticated):
            raise ValueError("owned fixture authentication prerequisite not established")
    require_status(False)
    try:
        response = client.post("/api/auth/login", json={"password": password})
    except Exception:
        raise RuntimeError("owned fixture session login transport failed") from None
    if response.status_code != 200 or not client.cookies:
        raise ValueError("owned fixture session login failed")
    require_status(True)


class AuthenticatedHarness:
    def __init__(self, harness, client):
        self.harness, self.client = harness, client

    def __getattr__(self, name):
        return getattr(self.harness, name)

    def _default_model(self, base_url):
        response = self.client.get("/api/models")
        response.raise_for_status()
        model = response.json().get("default")
        if not isinstance(model, str) or not model:
            raise AssertionError("authenticated fixture default model required")
        return model

    def _wait_for_sse_signal(self, base_url, row, predicate, timeout):
        import httpx
        from httpx_sse import connect_sse
        started = time.monotonic()
        with connect_sse(self.client, "GET", f"/api/conversations/{row}/stream",
                         timeout=httpx.Timeout(connect=5, read=timeout, write=5, pool=5)) as source:
            source.response.raise_for_status()
            for event in source.iter_sse():
                if time.monotonic() - started > timeout:
                    raise TimeoutError("authenticated fixture SSE signal deadline")
                if predicate(event.event, event.data):
                    return
        raise RuntimeError("authenticated fixture SSE closed before expected signal")


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


class OwnedFixtureCompletion:
    def __init__(self, startup, fixture):
        self.fixture = fixture
        self.identity = actual_server_identity(startup, fixture)
        self.db = Path(startup["db"]).resolve(strict=True)
        self.file_identity = (self.db.stat().st_dev, self.db.stat().st_ino)

    def __call__(self, row, exhausted, message_id, data):
        correlation = message_id or next((m["message_id"] for m in reversed(data["messages"])
                                         if m["message_type"] == "user"), None)
        if correlation is None:
            return False
        stat = self.db.stat()
        if (stat.st_dev, stat.st_ino) != self.file_identity:
            raise ValueError("owned fixture database identity changed")
        with contextlib.closing(sqlite3.connect(
                self.db.as_uri() + "?mode=ro", uri=True, timeout=2)) as connection:
            connection.execute("BEGIN")
            identity = connection.execute(
                "SELECT singleton_key, instance_id FROM instance_identity").fetchall()
            if identity != [(1, self.identity["serverInstanceId"])]:
                raise ValueError("owned fixture server identity changed")
            active = connection.execute(
                "SELECT count(*) FROM durable_turns WHERE conversation_id = ? "
                "AND disposition = 'Runtime' AND terminal_kind IS NULL AND owns_conversation = 1",
                (row,)).fetchone()[0]
            queued = connection.execute(
                "SELECT count(*) FROM steering_messages WHERE conversation_id = ?", (row,)).fetchone()[0]
            terminal = connection.execute(
                "SELECT terminal_kind, owns_conversation FROM durable_turns "
                "WHERE conversation_id = ? AND canonical_message_id = ? AND disposition = 'Runtime'",
                (row, correlation)).fetchall()
            creation = connection.execute(
                "SELECT status, claim_worker_id, claim_token, claim_lease_until_unix_micros "
                "FROM product_creation_jobs WHERE published_conversation_id = ? AND request_id = ?",
                (row, correlation)).fetchall() if message_id is None and not terminal else []
        facts = {"conversation_id": row, "message_id": correlation,
                 "active_runtime_owner": bool(active), "steering_backlog": bool(queued),
                 "runtime_terminal": terminal[0][0] if len(terminal) == 1 else None,
                 "runtime_owner_released": terminal == [("Failed" if exhausted else "Completed", 0)],
                 "product_opening_published": creation == [("published", None, None, None)],
                 "live_idle": None}
        shipping.write_json(self.fixture.out / "fixture-completion.json", facts)
        if queued:
            raise AssertionError("unexpected steering backlog during isolated fixture completion")
        settled = terminal == [("Failed" if exhausted else "Completed", 0)]
        if message_id is None and not exhausted and not terminal:
            settled = creation == [("published", None, None, None)]
        if active or not settled:
            return False
        if exhausted:
            return True
        result = self.fixture.request("POST", f"/api/conversations/{row}/messages/reconcile",
                                      {"message_ids": [correlation]})
        entries = result.get("entries")
        if (not isinstance(entries, list) or len(entries) != 1
                or entries[0].get("message_id") != correlation
                or entries[0].get("status") != "persisted"):
            raise ValueError("fixture completion reconciliation identity mismatch")
        if not isinstance(result.get("conversation_idle"), bool):
            raise ValueError("fixture completion requires authoritative runtime idle status")
        facts["live_idle"] = result["conversation_idle"]
        shipping.write_json(self.fixture.out / "fixture-completion.json", facts)
        return result["conversation_idle"]


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
                           "spaControl": {"kind": "sidebar_home"}},
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


COLLECTOR_TARGET = "phoenix_ide::view_journey_acceptance"
COLLECTOR_MESSAGE = "conversation_view_accepted"
COLLECTOR_MAX_BYTES = 16 * 1024 * 1024
COLLECTOR_MAX_LINE = 64 * 1024
COLLECTOR_MAX_EVENTS = 64


def canonical_uuid(value):
    try:
        return isinstance(value, str) and str(shipping.uuid.UUID(value)) == value
    except ValueError:
        return False


def collector_acceptances(path, identity):
    """Project only validated handler-completion facts from the owned app log."""
    expected_keys = {"message", "schema_version", "open_id", "server_instance_id", "backend_source", "status"}
    records, seen, size = [], set(), 0
    with Path(path).open("rb") as source:
        while line := source.readline(COLLECTOR_MAX_LINE + 1):
            size += len(line)
            if size > COLLECTOR_MAX_BYTES or len(line) > COLLECTOR_MAX_LINE:
                raise ValueError("collector log input bound exceeded")
            claimed = COLLECTOR_TARGET.encode() in line or COLLECTOR_MESSAGE.encode() in line
            try:
                def unique_keys(pairs):
                    result = {}
                    for key, value in pairs:
                        if key in result:
                            raise ValueError("duplicate collector JSON key")
                        result[key] = value
                    return result
                event = json.loads(line, object_pairs_hook=unique_keys)
            except (ValueError, UnicodeDecodeError):
                if claimed:
                    raise ValueError("malformed collector success record") from None
                continue
            if not isinstance(event, dict):
                if claimed:
                    raise ValueError("invalid collector success envelope")
                continue
            fields = event.get("fields")
            matches = event.get("target") == COLLECTOR_TARGET or (
                isinstance(fields, dict) and fields.get("message") == COLLECTOR_MESSAGE)
            if not matches:
                continue
            if (event.get("target") != COLLECTOR_TARGET or event.get("level") != "INFO"
                    or not isinstance(fields, dict) or set(fields) != expected_keys
                    or fields["message"] != COLLECTOR_MESSAGE):
                raise ValueError("invalid collector success schema")
            if (type(fields["schema_version"]) is not int or fields["schema_version"] != 1
                    or type(fields["status"]) is not int or fields["status"] != 204
                    or not canonical_uuid(fields["open_id"])
                    or not canonical_uuid(fields["server_instance_id"])
                    or not isinstance(fields["backend_source"], str)
                    or len(fields["backend_source"]) != 40
                    or any(c not in "0123456789abcdef" for c in fields["backend_source"])):
                raise ValueError("invalid collector success values")
            if (fields["backend_source"] != identity["appSource"]
                    or fields["server_instance_id"] != identity["serverInstanceId"]):
                raise ValueError("foreign collector success identity")
            if fields["open_id"] in seen:
                raise ValueError("duplicate collector acceptance")
            seen.add(fields["open_id"])
            if len(records) >= COLLECTOR_MAX_EVENTS:
                raise ValueError("collector acceptance count bound exceeded")
            records.append({key: value for key, value in fields.items() if key != "message"})
    return records


def validate_controlled_receipt(receipt):
    if (receipt.get('evidence_class') != 'controlled_functional'
            or receipt.get('status') != 'passed_assertions'
            or receipt.get('full_acceptance') is not False
            or receipt.get('overhead_clearance') is not False
            or receipt.get('resume', {}).get('status') != 'unsupported'
            or receipt.get('comparison', {}).get('overhead_clearance') is not False
            or receipt.get('acceptance') != 'partial_controlled_functional_collector_export_only'
            or receipt.get('auth_negative') != {'attempts': 1, 'response_status': 401, 'expected_acceptances': 0}):
        raise ValueError('controlled evidence class or scope mismatch')
    if any(type(value) is not int for value in receipt['auth_negative'].values()):
        raise ValueError('controlled auth-negative counts must be integers')
    trials, journeys = receipt.get('trials'), receipt.get('journeys')
    if not isinstance(trials, list) or len(trials) != 6 or not isinstance(journeys, list) or len(journeys) != 6:
        raise ValueError('controlled exact six-scenario inventory required')
    seen = set()
    for route in ('ordinary', 'global'):
        for kind, outcome in (('history', 'ready'), ('pagehide', 'incomplete'), ('cancellation', 'canceled')):
            entries = [t for t in trials if t.get('route') == route and t.get('kind') == kind]
            if len(entries) != 1:
                raise ValueError('controlled missing or duplicate scenario')
            trial = entries[0]
            reports = trial.get('reports')
            if (trial.get('status') != 'passed_assertions' or trial.get('errors') != []
                    or not isinstance(reports, list) or len(reports) != 1):
                raise ValueError('controlled report or action failure')
            report = reports[0]
            identifier = report.get('open_id')
            if (not isinstance(identifier, str) or identifier in seen or report.get('outcome') != outcome
                    or type(report.get('sample_rate')) not in (int, float) or report['sample_rate'] != 1
                    or trial.get('admission', {}).get('sampled') is not True
                    or trial['admission'].get('open_id') != identifier
                    or report.get('route') != route or trial['admission'].get('route') != route):
                raise ValueError('controlled report ownership, outcome or selection mismatch')
            if kind == 'history':
                history = trial.get('additional_history', {})
                if (history.get('report_count') != 1 or history.get('original_owner') != identifier
                        or type(history.get('request_count')) is not int or history['request_count'] < 1
                        or trial.get('physical_history', {}).get('original_page') is not True
                        or trial['physical_history'].get('viewport') != {'width': 1360, 'height': 900}
                        or type(trial['physical_history'].get('stability_frames')) is not int
                        or trial['physical_history']['stability_frames'] != 4):
                    raise ValueError('controlled positive original-owner historical evidence missing')
            if kind == 'pagehide' and trial.get('transport_settled_before_context_close') is not True:
                raise ValueError('controlled departing context closed before transport settled')
            if kind != 'pagehide' and trial.get('statuses') != [204]:
                raise ValueError('controlled nondeparting response missing')
            if kind == 'pagehide' and trial.get('delivery_proof') != 'provisional_until_real_collector_and_export_join':
                raise ValueError('controlled departing proof cannot substitute collector/export')
            seen.add(identifier)
    ids = [j.get('open_id') for j in journeys]
    if len(set(ids)) != 6 or set(ids) != seen:
        raise ValueError('controlled report/journey exact inventory mismatch')
    return receipt


def seal_collector_export_delivery(path, identity, browser, final, drain, run_id, service_name, evidence_mode='stock_natural'):
    if evidence_mode == 'controlled_functional':
        validate_controlled_receipt(browser)
    elif evidence_mode != 'stock_natural' or browser.get('evidence_class') == 'controlled_functional':
        raise ValueError('controlled evidence cannot enter stock collector/export seal')
    if drain != {"run_id": run_id, "service_name": service_name, "app_stopped": True,
                 "receiver_stopped": True, "errors": []}:
        raise ValueError("collector/export join requires normal owned drain")
    if (final.get("run_id") != run_id or final.get("service_name") != service_name
            or not service_name.endswith(run_id)):
        raise ValueError("collector/export run identity mismatch")
    if (browser.get("runId") != run_id or browser.get("serviceName") != service_name
            or browser.get("identity") != identity):
        raise ValueError("collector/browser identity mismatch")
    trials = list(browser.get("trials", []))
    for stage in ("terminal", "lifecycle"):
        trials.extend(browser.get(stage, {}).get("trials", []))
    if "failure" in browser or any("failure" in trial for trial in trials):
        raise ValueError("collector/export join cannot accept browser failure")
    reports = [report for trial in trials for report in trial.get("reports", [])]
    ids = [report["open_id"] for report in reports]
    journeys = browser.get("journeys", [])
    if (not 0 < len(ids) <= COLLECTOR_MAX_EVENTS or len(set(ids)) != len(ids)
            or any(not canonical_uuid(value) for value in ids)
            or len(journeys) != len(ids) or {case["open_id"] for case in journeys} != set(ids)):
        raise ValueError("collector/browser report inventory mismatch")
    accepted = collector_acceptances(path, identity)
    if len(accepted) != len(ids) or {entry["open_id"] for entry in accepted} != set(ids):
        raise ValueError("real collector acceptance inventory mismatch")
    retrieved = final.get("retrieved", [])
    if (len(retrieved) != len(ids) or {entry["open_id"] for entry in retrieved} != set(ids)):
        raise ValueError("actual exported inventory mismatch")
    outcomes = {report["open_id"]: report["outcome"] for report in reports}
    for entry in retrieved:
        trace = entry.get("trace_id")
        if (entry.get("outcome") != outcomes[entry["open_id"]]
                or not isinstance(trace, str) or len(trace) != 32
                or any(c not in "0123456789abcdef" for c in trace) or set(trace) == {"0"}):
            raise ValueError("actual exported journey mismatch")
    return {"schema_version": 1, "run_id": run_id, "service_name": service_name,
            "evidence_class": evidence_mode,
            "full_acceptance": False, "overhead_clearance": False,
            "scope": "real_collector_and_semantic_export_not_full_joint_or_overhead_acceptance",
            "collector_acceptances": accepted, "exported": retrieved,
            "normal_drain": True}


def exhausted_summary(state):
    """ConvState is internally tagged by type; ContextExhausted carries summary: String."""
    if (not isinstance(state, dict) or state.get("type") != "context_exhausted"
            or not isinstance(state.get("summary"), str) or not state["summary"].strip()):
        raise ValueError("persisted ContextExhausted state with nonempty summary required")
    return state["summary"]


def settle_equal_text_successor(fixture, successor, opening_id, writable):
    data = fixture.wait(successor, message_id=opening_id)
    if writable:
        assert fixture.h._state_str(data['conversation']['state']) == 'idle', 'controlled latest owner must be idle'
    else:
        fixture.send(successor, '[[scenario:context_window_exceeded]] [[ttft:0]]', True)


def equal_text_fixture(fixture, aggregate, *, writable=False):
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
    settle_equal_text_successor(fixture, successor, opening_id, writable)

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


def request_normal_server_stop(control):
    deadline = time.monotonic() + 2
    with socket.socket(socket.AF_UNIX) as client:
        def budget():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError('normal control stop deadline')
            client.settimeout(remaining)
        budget()
        client.connect(str(control))
        budget()
        client.sendall(b'{"command":"stop"}\n')
        response = bytearray()
        while not response.endswith(b'\n'):
            budget()
            chunk = client.recv(4096 - len(response))
            if not chunk:
                raise RuntimeError('normal control stop acknowledgement missing')
            response.extend(chunk)
            if len(response) >= 4096:
                raise RuntimeError('normal control stop acknowledgement overflow')
        if json.loads(response) != {'ok': True, 'result': {'state': 'stopped'}}:
            raise RuntimeError('normal control stop acknowledgement rejected')


def normal_server_completion(server, case, startup, control, failures, *, request_stop=False):
    """Called before owned_process's forced process-group cleanup."""
    primary_error = None
    receipt = {"status": "failed", "normal_wait_timeout_seconds": SERVER_COMPLETION_TIMEOUT,
               "owned_home": startup["home"], "control": str(control), "returncode": None,
               "stop": "already_exited" if server.poll() is not None else "browser_owned"}
    try:
        if request_stop and server.poll() is None:
            receipt['status'] = 'stop_request_failed'
            receipt['stop'] = 'failed'
            request_normal_server_stop(control)
            receipt['stop'] = 'acknowledged'
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
    except BaseException as error:
        primary_error = error
        failures.remember_primary("normal_completion", error)
        raise
    finally:
        try:
            shipping.write_json(case / "server-completion.json", receipt)
        except Exception as publication_error:
            failures.remember_cleanup("normal_completion_receipt", publication_error)
            if primary_error is not None:
                raise primary_error from publication_error
            raise
    return receipt


def controlled_history_targets(aggregates):
    targets = []
    for owned_aggregate in aggregates:
        source, message = owned_aggregate['members'][0], owned_aggregate['old_message']
        source_route = ('/global/' + quote(source, safe='')
                        if owned_aggregate['kind'] == 'global' else owned_aggregate['route'])
        route = source_route + '?' + urlencode({'source_transcript': source, 'qualification': 'retained'}) + '#message-' + quote(message, safe='')
        targets.append({'kind': owned_aggregate['kind'], 'route': route,
            'anchor': {'persistedMessageID': message, 'sourceRow': source},
            'composerOwner': owned_aggregate['members'][-1]})
    return targets


def qualify(app, exe, output, joint_telemetry=False, controlled=False):
    if controlled and not joint_telemetry:
        raise ValueError('controlled functional capture requires real collector/export joining')
    mode = 'controlled_functional' if controlled else 'stock_natural'
    manifest_name = 'controlled-functional-build.json' if controlled else 'stock-natural-build.json'
    artifact = artifact_admission.admit_artifact(app, app/'ui'/manifest_name, PIN, TREE_PIN, mode)
    (artifact_admission.require_controlled if controlled else artifact_admission.require_stock)(artifact)
    h = shipping.load_app_harness(app)
    telemetry = load_telemetry_modules(app) if joint_telemetry else None
    safe = {"PATH": os.environ["PATH"]}
    services = set()
    with tempfile.TemporaryDirectory(prefix="ho-", dir="/tmp") as short:
        temp = Path(short).resolve()
        os.chmod(temp, 0o700)
        for kind in (("ordinary",) if controlled else ("ordinary", "global")):
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
                                readiness(server, base + "/api/auth/status")
                                startup = startup_identity(
                                    case / "instrumented-runtime.log", exe, work, port)
                                shipping.write_json(case / "startup.json", startup)
                                fixture = shipping.Fixture(h, base, case)
                                try:
                                    authenticate_fixture(fixture.client, owned_qualification_password(
                                        startup, case / "credential-identity.json"))
                                    fixture.h = AuthenticatedHarness(h, fixture.client)
                                    fixture.completion = OwnedFixtureCompletion(startup, fixture)
                                    shipping.write_json(case / "authentication.json", {
                                        "auth_required": True, "parent_authenticated": True,
                                        "credential_source": "owned_cfg_test_home", "session": "existing_login_cookie"})
                                    cwd = work / "cwd"
                                    cwd.mkdir(mode=0o700)
                                    aggregate = equal_text_fixture(
                                        fixture, fixture.build(kind, cwd), writable=controlled)
                                    if receiver:
                                        other_kind = ("global" if kind == "ordinary"
                                                      else "ordinary")
                                        other_cwd = work / f"telemetry-{other_kind}-cwd"
                                        other_cwd.mkdir(mode=0o700)
                                        other = fixture.build(other_kind, other_cwd)
                                        if controlled:
                                            other = equal_text_fixture(fixture, other, writable=True)
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
                                    "build": {"outDir": str(artifact.root)},
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
                                    "HISTORY_AUTH_HOME": startup["home"],
                                    "HISTORY_AUTH_CREDENTIAL_FILE": startup["credential_file"],
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
                                if controlled:
                                    if artifact_admission.digest(app/'ui'/manifest_name) != artifact.manifest_sha256:
                                        raise ValueError('controlled manifest changed after admission')
                                    targets = controlled_history_targets((aggregate, other))
                                    admission_path = case/'controlled-artifact-admission.json'
                                    shipping.write_json(admission_path, {'evidence_class': mode, 'sample_rate': 1,
                                        'source_sha': PIN, 'source_tree': TREE_PIN,
                                        'manifest_sha256': artifact.manifest_sha256,
                                        'ui_files': dict(artifact.ui_files), 'history_targets': targets})
                                    node_env['HISTORY_CONTROLLED_ADMISSION'] = str(admission_path)
                                with failures.own(shipping.owned_process(
                                        ["node", str(app /
                                            "ui/node_modules/vite/bin/vite.js"),
                                         "preview", "--config", str(config)],
                                        work, node_env, case / "preview.log"), "preview") as preview:
                                    readiness(preview, ui, shipping.digest(
                                        artifact.root / "index.html"))
                                    with failures.own(shipping.owned_process(
                                            ["node", str(ROOT/'ui/scripts/qualify-history-controlled-functional.mjs' if controlled else SCRIPT)], work, node_env,
                                            case / "browser.log"), "browser") as driver:
                                        driver.wait(timeout=230 if controlled else 500 if receiver else 180)
                                        assert driver.returncode == 0, (
                                            "opening browser assertions failed; see case logs")
                                result = json.loads(
                                    (case / "browser-result.json").read_text())
                                if controlled:
                                    validate_controlled_receipt(result)
                                else:
                                    assert result["status"] == "passed_assertions" and result["checks"]
                                if receiver:
                                    telemetry_result = (case / "browser-result.json" if controlled
                                                        else case / "telemetry/telemetry-browser-result.json")
                                    telemetry_browser_result(telemetry_result)
                                    receiver_serving_check(receiver)
                                    receiver.assert_receipt(telemetry_result)
                                completion = normal_server_completion(
                                    server, case, startup, control, failures, request_stop=controlled)
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
                        telemetry_result,
                        drain_receipt=drain)
                    final['scope']['evidence_class'] = mode
                    shipping.write_json(case / "telemetry-final.json", final)
                    delivery = seal_collector_export_delivery(
                        case / "instrumented-runtime.log", fixtures["identity"],
                        telemetry_browser_result(telemetry_result),
                        final, drain, run_id, receiver.service, evidence_mode=mode)
                    shipping.write_json(case / "telemetry-delivery.json", delivery)
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


    after_artifact = artifact_admission.admit_artifact(app, app/'ui'/manifest_name, PIN, TREE_PIN, mode)
    if after_artifact != artifact:
        raise ValueError('UI artifact identity changed during capture')

def retained_evidence(path, missing_status, errors):
    if not path.is_file():
        return {"status": missing_status}
    try:
        value = json.loads(path.read_text())
        if not isinstance(value, dict):
            raise ValueError("evidence receipt must be an object")
        return value
    except (OSError, ValueError) as error:
        fact = {"file": path.name, "error_type": type(error).__name__}
        errors.append(fact)
        return {"status": "invalid_evidence", **fact}


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app-root", required=True, type=Path)
    parser.add_argument("--instrumented-exe", required=True, type=Path)
    parser.add_argument("--instrumented-receipt", required=True, type=Path)
    parser.add_argument("--joint-telemetry", action="store_true",
                        help="hosted-only joint history plus telemetry evidence")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument('--controlled-functional', action='store_true', help='separate controlled UI; never stock or overhead evidence')
    parser.add_argument("--timeout", type=int, default=1100)
    args = parser.parse_args(argv)
    if not 120 <= args.timeout <= 1200:
        parser.error("whole-run timeout must be 120..1200 seconds")
    if args.controlled_functional and (not args.joint_telemetry or args.timeout > 420):
        parser.error('controlled capture requires --joint-telemetry and --timeout <=420')
    return args


def main():
    args = parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    result = {"status": "failed", "evidence_class": CLASS, "runtime_executed": False,
              "joint_telemetry": args.joint_telemetry,
              "telemetry_evidence_class": 'controlled_functional' if args.controlled_functional else 'stock_natural',
              "full_acceptance": False, "overhead_clearance": False,
              "uncovered": ["Shipping binary, real providers, production, physical devices.",
                "Generated authority: pinned manual ContinueConversationRequest cannot select opening_authority."]}
    start = time.monotonic()
    primary_error = None
    def expired(*_):
        raise TimeoutError("opening qualification whole-run deadline")
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(args.timeout)
    try:
        app, exe = args.app_root.resolve(), args.instrumented_exe.absolute()
        before = identity(app, exe, args.instrumented_receipt)
        shipping.write_json(output / "identity.json", before)
        qualify(app, exe, output, args.joint_telemetry, args.controlled_functional)
        assert identity(app, exe, args.instrumented_receipt) == before, "artifact identity changed"
        result["status"] = "passed_assertions"
    except Exception as error:
        primary_error = error
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        signal.alarm(0)
        aggregation_errors = []
        result["cases"] = {kind: {
            "verified_runtime_startup": (output / kind / "startup.json").is_file(),
            "server_completion": retained_evidence(output / kind / "server-completion.json",
                                                    "not_completed", aggregation_errors),
            "lifecycle_failures": retained_evidence(output / kind / "lifecycle-failures.json",
                                                    "not_observed", aggregation_errors),
            "forced_shutdown": retained_evidence(output / kind / "forced-server-shutdown.json",
                                                  "not_observed", aggregation_errors),
            "browser": retained_evidence(output / kind / "browser-result.json",
                                         "not_completed", aggregation_errors),
            "telemetry": retained_evidence(output / kind / "telemetry-final.json",
                                           "not_enabled_or_completed", aggregation_errors)}
            for kind in (("ordinary",) if args.controlled_functional else ("ordinary", "global"))}
        result["runtime_executed"] = any(case["verified_runtime_startup"] for case in result["cases"].values())
        result["elapsed_seconds"] = time.monotonic() - start
        result["aggregation_errors"] = aggregation_errors
        if aggregation_errors:
            result["status"] = "failed"
        try:
            shipping.write_json(output / "result.json", result)
        except Exception as publication_error:
            if primary_error is not None:
                raise primary_error from publication_error
            raise
        if aggregation_errors and primary_error is None:
            raise RuntimeError("qualification receipt aggregation failed; see result.json")


if __name__ == "__main__":
    main()
