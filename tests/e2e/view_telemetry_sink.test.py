#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["opentelemetry-proto==1.39.0"]
# ///
import importlib.util
import json
import tempfile
import subprocess
import sys
import time
import urllib.request
import urllib.error
import unittest
from unittest.mock import Mock, patch
from pathlib import Path
from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest

spec = importlib.util.spec_from_file_location("sink", Path(__file__).with_name("view_telemetry_sink.py"))
sink = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = sink
spec.loader.exec_module(sink)
OPEN = "a74e65c7-3bd0-43d5-87ed-1e2b3cfb7200"
SERVICE = "synthetic-view-qualification"


def wire(children=1):
    request = ExportTraceServiceRequest()
    resource = request.resource_spans.add()
    service = resource.resource.attributes.add(key="service.name")
    service.value.string_value = SERVICE
    spans = resource.scope_spans.add().spans
    view = spans.add(name="browser.conversation_view", trace_id=b"t" * 16, span_id=b"v" * 8)
    values = {"open.id": OPEN, "browser.outcome": "ready", "browser.presentation_evidence": "commit_frame_estimate",
              "server.instance_id": "candidate-instance", "server.backend_source": "a" * 40,
              "browser.meaningful_transcript_ms": 10.0, "browser.breadcrumb_presented_ms": 11.0,
              "browser.breadcrumb_enabled_ms": 12.0, "browser.anchor_settled_ms": 12.0,
              "browser.total_ms": 12.0, "browser.anchor_result": "tail_pinned",
              "browser.breadcrumb_ready_minus_meaningful_ms": 2.0}
    for key, value in values.items():
        attribute = view.attributes.add(key=key)
        if isinstance(value, str):
            attribute.value.string_value = value
        else:
            attribute.value.double_value = value
    for i in range(children):
        child = spans.add(name="browser.view_correlation", trace_id=b"t" * 16, span_id=(i + 1).to_bytes(8), parent_span_id=b"v" * 8)
        child.attributes.add(key="open.id").value.string_value = OPEN
    return request


class SinkTests(unittest.TestCase):
    def test_pinned_sdk_metadata_is_validated_consumed_not_semantic_or_private_projection(self):
        request = wire()
        child = request.resource_spans[0].scope_spans[0].spans[1]
        child.attributes.add(key="correlation.kind").value.string_value = "request"
        child.attributes.add(key="correlation.id").value.string_value = "request-1"
        for span in request.resource_spans[0].scope_spans[0].spans:
            for key in sink.SDK_METADATA_STRINGS:
                span.attributes.add(key=key).value.string_value = "PRIVATE_METADATA"
            for key in sink.SDK_METADATA_INTEGERS:
                span.attributes.add(key=key).value.int_value = 1
        data = request.SerializeToString()
        with tempfile.TemporaryDirectory() as directory:
            capture = sink.Capture(Path(directory) / "capture")
            capture.accept(data)
            self.assertEqual((capture.output / "batch-000.pb").read_bytes(), data)
            self.assertEqual(sink.final_index(capture.output)["records"], capture.records)
            index = sink.final_index(capture.output)
            span, children = sink.retrieve(index, SERVICE, OPEN)
            sink.validate_view(span, {"browser.outcome": "ready"}, ["PRIVATE_METADATA"])
            sink.validate_correlations(index, span, children, [{"kind": "request", "id": "request-1"}])
            projected = (capture.output / "capture.json").read_text()
            self.assertNotIn("PRIVATE_METADATA", projected)
            for key in sink.SDK_METADATA_STRINGS | sink.SDK_METADATA_INTEGERS:
                self.assertNotIn(key, projected)

    def test_sdk_metadata_rejects_types_size_duplicates_count_and_unknown_fields(self):
        cases = [
            ("thread.id", "bool_value", True, "sdk_metadata_type"),
            ("thread.id", "int_value", -1, "sdk_metadata_type"),
            ("busy_ns", "double_value", float("inf"), "attribute_non_finite"),
            ("idle_ns", "double_value", 1.0, "sdk_metadata_type"),
            ("target", "int_value", 1, "sdk_metadata_type"),
            ("thread.name", "string_value", "x" * 4097, "attribute_string_budget"),
            ("level", "string_value", "INFO", "attribute_unknown"),
            ("PRIVATE_APPLICATION", "string_value", "PRIVATE", "attribute_unknown"),
        ]
        for key, kind, value, code in cases:
            request = wire(0)
            setattr(request.resource_spans[0].scope_spans[0].spans[0].attributes.add(key=key).value, kind, value)
            with self.subTest(key=key), self.assertRaises(sink.CaptureRejected) as error:
                sink.decode(request.SerializeToString())
            self.assertEqual(error.exception.code, code)
        request = wire(0)
        span = request.resource_spans[0].scope_spans[0].spans[0]
        for _ in range(2): span.attributes.add(key="target").value.string_value = "PRIVATE"
        with self.assertRaises(sink.CaptureRejected) as error: sink.decode(request.SerializeToString())
        self.assertEqual(error.exception.code, "attribute_duplicate")
        request = wire(0)
        span = request.resource_spans[0].scope_spans[0].spans[0]
        for i in range(129): span.attributes.add(key=f"unknown-{i}").value.int_value = i
        with self.assertRaises(sink.CaptureRejected) as error: sink.decode(request.SerializeToString())
        self.assertEqual(error.exception.code, "attribute_count_budget")

    def test_capture_rejection_codes_are_finite_private_free_and_fail_closed_without_network(self):
        cases = [
            ("body_byte_budget", {"MAX_BODY": 1}, None),
            ("capture_byte_budget", {"MAX_BYTES": 1}, None),
            ("batch_count_budget", {"MAX_BATCHES": 0}, None),
            ("capture_span_budget", {"MAX_SPANS": 1}, [None, None]),
            ("decoded_byte_budget", {"MAX_BYTES": 10}, [{"private": "PRIVATE" * 10}]),
        ]
        for expected, limits, decoded in cases:
            with self.subTest(code=expected), tempfile.TemporaryDirectory() as directory:
                capture = sink.Capture(Path(directory) / "capture")
                data = b"x" if decoded is not None else wire().SerializeToString()
                with patch.multiple(sink, **limits):
                    if decoded is None:
                        with self.assertRaises(sink.CaptureRejected): capture.accept(data)
                    else:
                        with patch.object(sink, "decode", return_value=decoded):
                            with self.assertRaises(sink.CaptureRejected): capture.accept(data)
                receipt = json.loads((capture.output / "capture.json").read_text())
                self.assertEqual(receipt["errors"], [expected])
                self.assertEqual(receipt["records"], [])
                self.assertEqual(list(capture.output.glob("*.pb")), [])
                self.assertNotIn("PRIVATE", json.dumps(receipt))
                with self.assertRaises(ValueError): sink.final_index(capture.output)
                with self.assertRaises(ValueError): capture.accept(b"x")
                self.assertEqual(capture.errors, [expected])

    def test_decode_rejection_predicates_are_retained_without_values_or_attribute_names(self):
        mutations = {
            "attribute_unknown": lambda view: view.attributes.add(key="PRIVATE_UNKNOWN").value.__setattr__("string_value", "PRIVATE"),
            "attribute_duplicate": lambda view: view.attributes.add(key="open.id").value.__setattr__("string_value", "PRIVATE"),
            "attribute_string_budget": lambda view: view.attributes[0].value.__setattr__("string_value", "PRIVATE" * 1000),
            "attribute_non_scalar": lambda view: view.attributes[0].value.array_value.values.add().CopyFrom(view.attributes[1].value),
            "attribute_non_finite": lambda view: view.attributes[0].value.__setattr__("double_value", float("inf")),
            "trace_identity_invalid": lambda view: setattr(view, "span_id", b""),
        }
        for expected, mutate in mutations.items():
            request = wire(0)
            mutate(request.resource_spans[0].scope_spans[0].spans[0])
            with self.subTest(code=expected), tempfile.TemporaryDirectory() as directory:
                capture = sink.Capture(Path(directory) / "capture")
                with self.assertRaises(sink.CaptureRejected): capture.accept(request.SerializeToString())
                receipt = json.loads((capture.output / "capture.json").read_text())
                self.assertEqual(receipt["errors"], [expected])
                self.assertNotIn("PRIVATE", json.dumps(receipt))
        for data, expected in [(b"\xff", "protobuf_invalid")]:
            with tempfile.TemporaryDirectory() as directory:
                capture = sink.Capture(Path(directory) / "capture")
                with self.assertRaises(ValueError): capture.accept(data)
                self.assertEqual(capture.errors, [expected])

    def test_internal_capture_error_never_persists_exception_text(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = sink.Capture(Path(directory) / "capture")
            with patch.object(sink, "decode", side_effect=RuntimeError("PRIVATE")):
                with self.assertRaises(RuntimeError): capture.accept(b"x")
            self.assertEqual(capture.errors, ["capture_internal_failure"])
            self.assertNotIn("PRIVATE", (capture.output / "capture.json").read_text())

    def test_semantic_retrieval_not_http_acceptance(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = sink.Capture(Path(directory) / "capture")
            capture.accept(wire().SerializeToString())
            index = json.loads((capture.output / "capture.json").read_text())
            view, children = sink.retrieve(index, SERVICE, OPEN)
            self.assertEqual(view["trace_id"], (b"t" * 16).hex())
            self.assertEqual(len(children), 1)
            sink.validate_view(view, {"open.id": OPEN, "browser.outcome": "ready", "server.backend_source": "a" * 40}, ["private transcript"])
            with self.assertRaises(ValueError):
                sink.retrieve(index, "wrong-service", OPEN)
            with self.assertRaises(ValueError):
                sink.retrieve(index, SERVICE, "wrong-journey")
            capture.accept(wire().SerializeToString())
            with self.assertRaises(ValueError):
                sink.retrieve(json.loads((capture.output / "capture.json").read_text()), SERVICE, OPEN)

    def test_lifecycle_identity_timing_and_privacy_must_match(self):
        view = sink.decode(wire().SerializeToString())[0]
        for expected in [{"server.backend_source": "wrong-source"}, {"browser.outcome": "incomplete"}, {"server.instance_id": "different"}]:
            with self.subTest(expected=expected), self.assertRaises(ValueError):
                sink.validate_view(view, expected, [])
        view["attributes"]["browser.meaningful_transcript_ms"] = -1
        with self.assertRaises(ValueError):
            sink.validate_view(view, {}, [])
        view["attributes"]["browser.meaningful_transcript_ms"] = 10
        view["attributes"]["browser.presentation_evidence"] = "pixels"
        with self.assertRaises(ValueError):
            sink.validate_view(view, {}, [])
        view["attributes"]["browser.presentation_evidence"] = "commit_frame_estimate"
        view["attributes"]["private"] = "private transcript"
        with self.assertRaises(ValueError):
            sink.validate_view(view, {}, ["private transcript"])

    def test_exact_correlation_and_signed_frame_difference(self):
        records = sink.decode(wire().SerializeToString())
        view, child = records
        server_id = "a" * 32
        child["attributes"].update({"correlation.kind": "trace", "correlation.id": view["trace_id"]})
        with self.assertRaises(ValueError):
            sink.validate_correlations({"records": records}, view, [child], [{"kind": "trace", "id": view["trace_id"]}])
        child["attributes"]["correlation.id"] = server_id
        records.append({"service": SERVICE, "name": "coordinator.history_open", "trace_id": server_id})
        expected = [{"kind": "trace", "id": server_id}]
        sink.validate_correlations({"records": records}, view, [child], expected)
        with self.assertRaises(ValueError):
            sink.validate_correlations({"records": records}, view, [child], [])
        child["attributes"]["correlation.id"] = "b" * 32
        with self.assertRaises(ValueError):
            sink.validate_correlations({"records": records}, view, [child], [{"kind": "trace", "id": "b" * 32}])
        view["attributes"].update({"browser.breadcrumb_presented_ms": 4, "browser.breadcrumb_enabled_ms": 5,
                                   "browser.breadcrumb_ready_minus_meaningful_ms": -5})
        sink.validate_view(view, {}, [])
        view["attributes"]["browser.breadcrumb_ready_minus_meaningful_ms"] = 5
        with self.assertRaises(ValueError):
            sink.validate_view(view, {}, [])

    def test_missing_ready_private_projection_and_final_retention_failure(self):
        record = sink.decode(wire().SerializeToString())[0]
        del record["attributes"]["browser.anchor_settled_ms"]
        with self.assertRaises(ValueError):
            sink.validate_view(record, {}, [])
        request = wire()
        span = request.resource_spans[0].scope_spans[0].spans.add(name="coordinator.history_open", trace_id=b"s" * 16, span_id=b"s" * 8)
        span.attributes.add(key="private").value.string_value = "private transcript"
        with self.assertRaises(ValueError):
            sink.decode(request.SerializeToString())
        with tempfile.TemporaryDirectory() as directory:
            capture = sink.Capture(Path(directory) / "capture")
            capture.accept(wire().SerializeToString())
            sink.final_index(capture.output)
            (capture.output / "batch-001.pb").write_bytes(b"orphan")
            with self.assertRaises(ValueError):
                sink.final_index(capture.output)
            (capture.output / "batch-001.pb").unlink()
            capture.errors.append("late_failure")
            capture.finish()
            with self.assertRaises(ValueError):
                sink.final_index(capture.output)

    def test_explicit_budget_failure_is_retained_and_not_retrievable(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = sink.Capture(Path(directory) / "capture")
            with patch.object(sink, "MAX_BATCHES", 1):
                capture.accept(wire().SerializeToString())
                with self.assertRaises(ValueError):
                    capture.accept(wire().SerializeToString())
            index = json.loads((capture.output / "capture.json").read_text())
            self.assertTrue(index["errors"])
            with self.assertRaises(ValueError):
                sink.retrieve(index, SERVICE, OPEN)
            self.assertEqual(len(list(capture.output.glob("batch-*.pb"))), 1)

    def test_rejects_invalid_wire_ids_and_duplicate_attributes(self):
        for mutate in [lambda span: setattr(span, "trace_id", b"\0" * 16),
                       lambda span: span.attributes.add(key="open.id"),
                       lambda span: setattr(span.attributes[0].value, "double_value", float("nan"))]:
            request = wire()
            mutate(request.resource_spans[0].scope_spans[0].spans[0])
            with self.subTest(mutate=mutate), self.assertRaises(ValueError):
                sink.decode(request.SerializeToString())
        with self.assertRaises(Exception):
            sink.decode(b"\xff")

    def test_owned_loopback_http_capture_and_transport_rejection(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "capture"
            receipt = Path(directory) / "receipt.json"
            process = subprocess.Popen([sys.executable, str(Path(__file__).with_name("view_telemetry_sink.py")), "--output", str(output), "--receipt", str(receipt)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 3
                while not receipt.exists():
                    if process.poll() is not None or time.monotonic() > deadline:
                        self.fail("sink setup did not produce readiness receipt")
                    time.sleep(0.01)
                endpoint = json.loads(receipt.read_text())["endpoint"]
                self.assertTrue(endpoint.startswith("http://127.0.0.1:"))
                request = urllib.request.Request(endpoint, data=wire().SerializeToString(), headers={"Content-Type": "application/x-protobuf"})
                with urllib.request.urlopen(request, timeout=3) as response:
                    self.assertEqual(response.status, 200)
                index = json.loads((output / "capture.json").read_text())
                view, _ = sink.retrieve(index, SERVICE, OPEN)
                self.assertEqual(view["trace_id"], (b"t" * 16).hex())
                request = urllib.request.Request(endpoint, data=b"{}", headers={"Content-Type": "application/json"})
                with self.assertRaises(urllib.error.HTTPError) as error:
                    urllib.request.urlopen(request, timeout=3)
                self.assertEqual(error.exception.code, 400)
                self.assertTrue(json.loads((output / "capture.json").read_text())["errors"])
            finally:
                process.terminate()
                process.communicate(timeout=3)

    def test_optional_lifetime_validation_precedes_receiver_creation(self):
        with patch.object(sink, "make_server") as create:
            for invalid in (True, False, 0, -1, 651, 300.0, "650", None):
                with self.subTest(lifetime=invalid), self.assertRaises(ValueError):
                    with sink.telemetry_sink("unused", "run", lifetime_seconds=invalid):
                        self.fail("invalid lifetime admitted")
            create.assert_not_called()

    def test_receiver_lifetime_liveness_and_expiration_without_network(self):
        for lifetime in (None, 650):
            for expired in (False, True):
                with self.subTest(lifetime=lifetime, expired=expired), tempfile.TemporaryDirectory() as directory:
                    alive = [False]
                    server = Mock(server_port=12345)
                    server.shutdown.side_effect = lambda: alive.__setitem__(0, False)
                    thread = Mock()
                    thread.start.side_effect = lambda: alive.__setitem__(0, True)
                    thread.is_alive.side_effect = lambda: alive[0]
                    timer = Mock()
                    timer.is_alive.return_value = False
                    with patch.object(sink, "make_server", return_value=server), patch.object(sink.threading, "Thread", return_value=thread), patch.object(sink.threading, "Timer", return_value=timer) as clock:
                        kwargs = {} if lifetime is None else {"lifetime_seconds": lifetime}
                        with sink.telemetry_sink(directory, "lifetime", **kwargs) as handle:
                            self.assertEqual(clock.call_args.args[0], 300 if lifetime is None else lifetime)
                            self.assertTrue(handle.is_serving())
                            self.assertFalse(handle.expired)
                            if expired:
                                clock.call_args.args[1]()
                                self.assertFalse(handle.is_serving())
                                self.assertTrue(handle.expired)
                                self.assertIn("receiver_lifetime_expired", json.loads((handle.output / "capture.json").read_text())["errors"])
                                with self.assertRaises(ValueError):
                                    sink.final_index(handle.output)
                            else:
                                # Caller observes this after normal app stop, before receiver exit.
                                self.assertTrue(handle.is_serving())
                        self.assertFalse(handle.is_serving())
                        self.assertEqual(handle.expired, expired)
                        timer.cancel.assert_called_once()
                        server.server_close.assert_called_once()
                        thread.join.assert_called_once()
                        timer.join.assert_called_once()
                        self.assertTrue(0 <= thread.join.call_args.kwargs["timeout"] <= 4)
                        self.assertTrue(0 <= timer.join.call_args.kwargs["timeout"] <= 4)

    def test_expiry_callback_is_drained_before_final_persistence_without_network(self):
        real_thread = sink.threading.Thread
        entered = sink.threading.Event()
        release = sink.threading.Event()
        joining = sink.threading.Event()
        exited = sink.threading.Event()
        failures = []
        writes = []
        original_finish = sink.Capture.finish

        def finish(capture):
            if sink.threading.current_thread().name == "controlled-expiry":
                entered.set()
                if not release.wait(3):
                    raise RuntimeError("test expiry release deadline")
            original_finish(capture)
            writes.append(sink.threading.current_thread().name)

        class Timer:
            daemon = True
            def __init__(self, seconds, callback):
                self.worker = real_thread(target=callback, name="controlled-expiry")
            def start(self):
                self.worker.start()
            def cancel(self):
                pass  # Cancellation cannot stop an already-running callback.
            def join(self, timeout):
                joining.set()
                self.worker.join(timeout)
            def is_alive(self):
                return self.worker.is_alive()

        with tempfile.TemporaryDirectory() as directory:
            serving = Mock()
            serving.is_alive.return_value = False
            server = Mock(server_port=12345)
            def own_context():
                try:
                    with sink.telemetry_sink(directory, "callback-drain"):
                        if not entered.wait(3):
                            raise RuntimeError("test callback admission deadline")
                except Exception as error:
                    failures.append(error)
                finally:
                    exited.set()
            with patch.object(sink, "make_server", return_value=server), patch.object(sink.threading, "Thread", return_value=serving), patch.object(sink.threading, "Timer", Timer), patch.object(sink.Capture, "finish", finish):
                owner = real_thread(target=own_context, name="controlled-owner")
                owner.start()
                try:
                    self.assertTrue(joining.wait(3))
                    self.assertFalse(exited.is_set(), "owner must wait for active expiry write")
                finally:
                    release.set()
                    owner.join(3)
                self.assertFalse(owner.is_alive())
                self.assertEqual(failures, [])
                self.assertEqual(writes, ["controlled-expiry", "controlled-owner"])
                index = json.loads((Path(directory) / "view-telemetry-callback-drain" / "capture.json").read_text())
                self.assertEqual(index["errors"], ["receiver_lifetime_expired"])

    def test_transport_rejection_serializes_with_expiry_persistence_without_network(self):
        entered = sink.threading.Event()
        lock = sink.threading.Lock()
        class ObservedLock:
            def __enter__(self):
                entered.set()
                lock.acquire()
            def __exit__(self, *_):
                lock.release()
        with tempfile.TemporaryDirectory() as directory:
            capture = sink.Capture(Path(directory) / "capture")
            capture.lock = ObservedLock()
            with patch.object(sink, "HTTPServer") as create:
                sink.make_server(capture)
            handler_type = create.call_args.args[1]
            handler = object.__new__(handler_type)
            handler.connection = Mock()
            handler.headers = {"Content-Length": str(sink.MAX_BODY + 1)}
            handler.send_error = Mock()
            lock.acquire()  # Simulate expiry retaining the shared persistence lock.
            worker = sink.threading.Thread(target=handler.do_POST)
            worker.start()
            try:
                self.assertTrue(entered.wait(3))
                self.assertEqual(capture.errors, [])
                handler.send_error.assert_not_called()
                capture.errors.append("receiver_lifetime_expired")
                capture.finish()
            finally:
                lock.release()
                worker.join(3)
            self.assertFalse(worker.is_alive())
            handler.send_error.assert_called_once_with(400, "bounded synthetic export rejected")
            self.assertEqual(capture.errors, ["receiver_lifetime_expired"])
            self.assertEqual(json.loads((capture.output / "capture.json").read_text())["errors"], capture.errors)

    def test_context_contract_isolated_env_semantic_receipt_and_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            with sink.telemetry_sink(directory, "unit-run") as handle:
                self.assertEqual(set(handle.env), {"PHOENIX_TRACE_EXPORTER", "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "OTEL_SERVICE_NAME"})
                self.assertEqual(handle.env["PHOENIX_TRACE_EXPORTER"], "otlp")
                request = wire(0)
                request.resource_spans[0].resource.attributes[0].value.string_value = handle.service
                view = request.resource_spans[0].scope_spans[0].spans[0]
                for key, value in {"browser.frontend_source": "b" * 40, "browser.route": "ordinary", "browser.trigger": "navigation"}.items():
                    view.attributes.add(key=key).value.string_value = value
                expected = {"server.backend_source": "a" * 40, "server.instance_id": "candidate-instance", "browser.frontend_source": "b" * 40, "browser.outcome": "ready", "browser.route": "ordinary", "browser.trigger": "navigation"}
                result = {"journeys": [{"open_id": OPEN, "expected": expected, "correlations": []}], "forbidden": ["private transcript"]}
                with self.assertRaises(FileNotFoundError):
                    handle.assert_receipt(result, timeout=0.01)
                with urllib.request.urlopen(urllib.request.Request(handle.endpoint, data=request.SerializeToString(), headers={"Content-Type": "application/x-protobuf"}), timeout=3):
                    pass
                receipt = handle.assert_receipt(result, timeout=1)
                self.assertEqual(receipt["retrieved"][0]["trace_id"], (b"t" * 16).hex())
                with self.assertRaises(ValueError):
                    handle.assert_receipt({"journeys": []}, timeout=1)
            self.assertEqual(sink.final_index(handle.output)["spans"], 1)
            with self.assertRaises(urllib.error.URLError):
                urllib.request.urlopen(handle.endpoint, timeout=1)
            with self.assertRaises(ValueError):
                with sink.telemetry_sink(directory, "../invalid"):
                    pass

    def test_final_phoenix_helper_rejects_unexpected_export_and_browser_failure(self):
        sys.path.insert(0, str(Path(__file__).parent))
        from assert_view_telemetry import assertPhoenixTelemetry
        with tempfile.TemporaryDirectory() as directory:
            capture = sink.Capture(Path(directory) / "capture")
            request = wire(0)
            service = "history-qualification-final"
            request.resource_spans[0].resource.attributes[0].value.string_value = service
            span = request.resource_spans[0].scope_spans[0].spans[0]
            for key, value in {"browser.frontend_source": "b" * 40, "browser.route": "ordinary", "browser.trigger": "navigation"}.items():
                span.attributes.add(key=key).value.string_value = value
            capture.accept(request.SerializeToString())
            expected = {"server.backend_source": "a" * 40, "server.instance_id": "candidate-instance", "browser.frontend_source": "b" * 40, "browser.outcome": "ready", "browser.route": "ordinary", "browser.trigger": "navigation"}
            cases = [{"open_id": OPEN, "expected": expected, "correlations": []}]
            drain = {"run_id": "final", "service_name": service, "app_stopped": True, "receiver_stopped": True, "errors": []}
            with self.assertRaises(ValueError):
                assertPhoenixTelemetry(capture.output, service, "final", cases, drain_receipt={})
            self.assertEqual(len(assertPhoenixTelemetry(capture.output, service, "final", cases, drain_receipt=drain)["retrieved"]), 1)
            self.assertFalse(assertPhoenixTelemetry(capture.output, service, "final", cases, drain_receipt=drain)["scope"]["full_acceptance"])
            partial = Path(directory) / "partial.json"
            partial.write_text(json.dumps({"runId": "final", "serviceName": service, "journeys": cases,
                "trials": [{"reports": [{"open_id": OPEN}]}],
                "identity": {"appSource": "a" * 40, "frontendSource": "b" * 40, "serverInstanceId": "candidate-instance"},
                "acceptance": "partial_initial_open_retrieval_and_raw_overhead_only",
                "capabilities": {"bfcache_resume": "not_exercised"}, "comparison": {"overhead_clearance": False}}))
            semantic = assertPhoenixTelemetry(capture.output, service, "final", partial, drain_receipt=drain)
            self.assertEqual(semantic["scope"]["capabilities"]["bfcache_resume"], "not_exercised")
            self.assertFalse(semantic["scope"]["full_acceptance"])
            self.assertFalse(semantic["scope"]["comparison"]["overhead_clearance"])
            path = Path(directory) / "browser.json"
            path.write_text(json.dumps({"runId": "final", "serviceName": service, "journeys": cases, "failure": "oracle failed"}))
            with self.assertRaises(ValueError):
                assertPhoenixTelemetry(capture.output, service, "final", path, drain_receipt=drain)
            base_receipt = json.loads(partial.read_text())
            for extra in ({"failure": ""}, {"failure": None}, {"trials": [{"failure": "failed later"}]},
                          {"terminal": {"trials": [{"failure": ""}]}}, {"lifecycle": {"trials": [{"failure": ""}]}},
                          {"acceptance": ""}, {"comparison": {"overhead_clearance": True}}, {"comparison": {}}, {"identity": {}}, {"trials": []},
                          {"trials": [{"reports": [{"open_id": OPEN}, {"open_id": OPEN}]}]},
                          {"lifecycle": {"trials": [{"reports": [{"open_id": "unaccounted"}]}]}}):
                with self.subTest(extra=extra):
                    path.write_text(json.dumps({**base_receipt, **extra}))
                    with self.assertRaises(ValueError):
                        assertPhoenixTelemetry(capture.output, service, "final", path, drain_receipt=drain)
            request.resource_spans[0].scope_spans[0].spans[0].attributes[0].value.string_value = "different-journey"
            capture.accept(request.SerializeToString())
            with self.assertRaises(ValueError):
                assertPhoenixTelemetry(capture.output, service, "final", cases, drain_receipt=drain)

    def test_final_index_reconciles_decoded_content_not_only_byte_counts(self):
        for corrupt in ("index", "raw"):
            with self.subTest(corrupt=corrupt), tempfile.TemporaryDirectory() as directory:
                capture = sink.Capture(Path(directory) / "capture")
                capture.accept(wire().SerializeToString())
                if corrupt == "index":
                    path = capture.output / "capture.json"
                    index = json.loads(path.read_text())
                    index["records"][0]["attributes"]["browser.outcome"] = "failed"
                    path.write_text(json.dumps(index))
                else:
                    replacement = wire()
                    replacement.resource_spans[0].scope_spans[0].spans[0].trace_id = b"x" * 16
                    path = capture.output / "batch-000.pb"
                    self.assertEqual(len(replacement.SerializeToString()), path.stat().st_size)
                    path.write_bytes(replacement.SerializeToString())
                with self.assertRaises(ValueError):
                    sink.final_index(capture.output)

    def test_retrieval_rejects_missing_or_untyped_journey_identity(self):
        records = sink.decode(wire(0).SerializeToString())
        del records[0]["attributes"]["open.id"]
        for identity in (None, "", "not-a-uuid", 12):
            with self.subTest(identity=identity), self.assertRaises(ValueError):
                sink.retrieve({"errors": [], "records": records}, SERVICE, identity)

    def test_rejects_excess_correlation_children(self):
        index = {"errors": [], "records": sink.decode(wire(17).SerializeToString())}
        with self.assertRaises(ValueError):
            sink.retrieve(index, SERVICE, OPEN)


if __name__ == "__main__":
    unittest.main()
