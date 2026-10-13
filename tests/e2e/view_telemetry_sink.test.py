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
from pathlib import Path
from unittest.mock import patch
from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest

spec = importlib.util.spec_from_file_location("sink", Path(__file__).with_name("view_telemetry_sink.py"))
sink = importlib.util.module_from_spec(spec)
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

    def test_rejects_excess_correlation_children(self):
        index = {"errors": [], "records": sink.decode(wire(17).SerializeToString())}
        with self.assertRaises(ValueError):
            sink.retrieve(index, SERVICE, OPEN)


if __name__ == "__main__":
    unittest.main()
