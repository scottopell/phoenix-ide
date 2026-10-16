#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["opentelemetry-proto==1.39.0"]
# ///
"""Disposable loopback OTLP receiver for existing candidate qualification only."""
import argparse
import json
import uuid
import math
import threading
import time
import re
from contextlib import contextmanager
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from opentelemetry.proto.collector.trace.v1.trace_service_pb2 import ExportTraceServiceRequest

MAX_BYTES = 8 * 1024 * 1024
MAX_BATCHES = 256
MAX_SPANS = 10_000
MAX_BODY = 1024 * 1024
VIEW_ATTRIBUTES = {
    "open.id", "product_conversation.id", "segment.id", "server.backend_source", "server.instance_id",
    *{f"browser.{key}" for key in (
        "reporter_id counters_saturated route trigger outcome platform standalone visible frontend_source "
        "frontend_source_dirty cache message_count_bucket chapter_count_bucket response_size_bucket reconnect_count "
        "anchor_result presentation_evidence total_ms background_ms background_interval_count background_open "
        "observer_inspection_count observer_scan_limited timeline_saturated shell_ms snapshot_received_ms "
        "hydration_ready_ms meaningful_transcript_ms chapter_data_ready_ms breadcrumb_presented_ms "
        "breadcrumb_enabled_ms anchor_settled_ms breadcrumb_ready_minus_meaningful_ms sample_rate admitted_count "
        "sampled_count dropped_correlations transport_dropped_count transport_failed_count"
    ).split()},
}


class CaptureRejected(ValueError):
    def __init__(self, code):
        self.code = code
        super().__init__(code)


def scalar(value):
    kind = value.WhichOneof("value")
    if kind not in {"string_value", "bool_value", "int_value", "double_value"}:
        raise CaptureRejected("attribute_non_scalar")
    result = getattr(value, kind)
    if isinstance(result, str) and len(result.encode()) > 4096:
        raise CaptureRejected("attribute_string_budget")
    if isinstance(result, float) and not math.isfinite(result):
        raise CaptureRejected("attribute_non_finite")
    return result


def decode(data):
    try:
        request = ExportTraceServiceRequest.FromString(data)
    except Exception:
        raise CaptureRejected("protobuf_invalid") from None
    records = []
    for resource in request.resource_spans:
        service = next((scalar(a.value) for a in resource.resource.attributes if a.key == "service.name"), None)
        for scope in resource.scope_spans:
            for span in scope.spans:
                if len(records) >= MAX_SPANS:
                    raise CaptureRejected("batch_span_budget")
                attributes = {}
                for attribute in span.attributes if span.name in {"browser.conversation_view", "browser.view_correlation", "coordinator.history_open"} else ():
                    if len(attributes) >= 128:
                        raise CaptureRejected("attribute_count_budget")
                    if attribute.key in attributes:
                        raise CaptureRejected("attribute_duplicate")
                    attributes[attribute.key] = scalar(attribute.value)
                allowed = VIEW_ATTRIBUTES if span.name == "browser.conversation_view" else ({"open.id", "correlation.kind", "correlation.id"} if span.name == "browser.view_correlation" else {"open.id", "product_conversation.id"})
                if set(attributes) - allowed:
                    raise CaptureRejected("attribute_unknown")
                if len(span.trace_id) != 16 or not any(span.trace_id) or len(span.span_id) != 8 or not any(span.span_id):
                    raise CaptureRejected("trace_identity_invalid")
                records.append({"service": service, "name": span.name, "trace_id": span.trace_id.hex(),
                                "span_id": span.span_id.hex(), "parent_span_id": span.parent_span_id.hex(),
                                "attributes": attributes})
    return records


class Capture:
    def __init__(self, output):
        self.output = Path(output)
        self.output.mkdir(parents=True, exist_ok=False)
        self.lock = threading.Lock()
        self.bytes = self.batches = self.spans = 0
        self.errors = []
        self.records = []

    def accept(self, data):
        with self.lock:
            if self.errors:
                raise ValueError("capture already failed")
            try:
                if len(data) > MAX_BODY:
                    raise CaptureRejected("body_byte_budget")
                if self.bytes + len(data) > MAX_BYTES:
                    raise CaptureRejected("capture_byte_budget")
                if self.batches >= MAX_BATCHES:
                    raise CaptureRejected("batch_count_budget")
                records = decode(data)
                if self.spans + len(records) > MAX_SPANS:
                    raise CaptureRejected("capture_span_budget")
                if len(json.dumps(self.records + records).encode()) > MAX_BYTES:
                    raise CaptureRejected("decoded_byte_budget")
                (self.output / f"batch-{self.batches:03d}.pb").write_bytes(data)
                self.bytes += len(data)
                self.batches += 1
                self.spans += len(records)
                self.records.extend(records)
            except Exception as error:
                self.errors.append(error.code if isinstance(error, CaptureRejected) else "capture_internal_failure")
                self.finish()
                raise
            self.finish()

    def finish(self):
        # Synthetic-only raw bytes are retained separately; no production destination.
        path = self.output / "capture.json"
        temporary = self.output / "capture.json.tmp"
        temporary.write_text(json.dumps({"bytes": self.bytes, "batches": self.batches,
            "spans": self.spans, "errors": self.errors, "records": self.records}, allow_nan=False))
        temporary.replace(path)


def final_index(output):
    output = Path(output)
    index = json.loads((output / "capture.json").read_text())
    batches = sorted(output.glob("batch-*.pb"))
    if len(batches) != index["batches"] or sum(path.stat().st_size for path in batches) != index["bytes"]:
        raise ValueError("raw/index retention mismatch")
    if index["errors"]:
        raise ValueError("late export/drain failure")
    if len(batches) > MAX_BATCHES or index["bytes"] > MAX_BYTES or index["spans"] > MAX_SPANS:
        raise ValueError("final retention budget exceeded")
    records = []
    for ordinal, path in enumerate(batches):
        if path.name != f"batch-{ordinal:03d}.pb" or path.stat().st_size > MAX_BODY:
            raise ValueError("invalid retained batch inventory")
        records.extend(decode(path.read_bytes()))
        if len(records) > MAX_SPANS:
            raise ValueError("final span budget exceeded")
    if records != index["records"] or len(records) != index["spans"]:
        raise ValueError("raw/decoded content mismatch")
    return index


def retrieve(capture, service, open_id):
    try:
        if not isinstance(open_id, str) or str(uuid.UUID(open_id)) != open_id:
            raise ValueError("invalid journey identity")
    except (ValueError, AttributeError, TypeError):
        raise ValueError("invalid journey identity") from None
    if capture["errors"]:
        raise ValueError("export capture failed")
    records = [r for r in capture["records"] if r["service"] == service and r["attributes"].get("open.id") == open_id]
    views = [r for r in records if r["name"] == "browser.conversation_view"]
    if len(views) != 1:
        raise ValueError("missing or duplicate journey export")
    view = views[0]
    children = [r for r in capture["records"] if r["service"] == service and r["name"] == "browser.view_correlation"
                and r["trace_id"] == view["trace_id"] and r["parent_span_id"] == view["span_id"]]
    if len(children) > 16:
        raise ValueError("correlation export budget exceeded")
    return view, children


def validate_correlations(capture, view, children, expected):
    actual = []
    for child in children:
        attributes = child["attributes"]
        if set(attributes) != {"open.id", "correlation.kind", "correlation.id"} or attributes["open.id"] != view["attributes"]["open.id"]:
            raise ValueError("invalid correlation child")
        actual.append((attributes["correlation.kind"], attributes["correlation.id"]))
    wanted = [(entry["kind"], entry["id"]) for entry in expected]
    if sorted(actual) != sorted(wanted) or len(set(actual)) != len(actual):
        raise ValueError("missing, duplicate or mismatched correlation export")
    trace_ids = {record["trace_id"] for record in capture["records"] if record["service"] == view["service"]
                 and not record["name"].startswith("browser.") and record["trace_id"] != view["trace_id"]}
    if any(identifier not in trace_ids for kind, identifier in actual if kind == "trace"):
        raise ValueError("correlated server trace not retrieved")


def validate_view(record, expected, forbidden):
    attributes = record["attributes"]
    if set(attributes) - VIEW_ATTRIBUTES:
        raise ValueError("unknown browser-view attribute")
    if attributes.get("browser.outcome") == "ready":
        required = {"browser.total_ms", "browser.meaningful_transcript_ms", "browser.breadcrumb_presented_ms",
                    "browser.breadcrumb_enabled_ms", "browser.anchor_settled_ms", "browser.anchor_result"}
        if not required.issubset(attributes) or attributes["browser.anchor_result"] not in {"applied", "tail_pinned", "empty"}:
            raise ValueError("missing ready evidence")
        if not attributes.get("browser.timeline_saturated") and "browser.breadcrumb_ready_minus_meaningful_ms" not in attributes:
            raise ValueError("missing ready difference")
        if any(attributes[key] > attributes["browser.total_ms"] for key in required if key.endswith("_ms")):
            raise ValueError("ready mark exceeds total")
    for key, value in expected.items():
        if attributes.get(key) != value:
            raise ValueError(f"trace attribute mismatch: {key}")
    if attributes.get("browser.presentation_evidence") != "commit_frame_estimate":
        raise ValueError("unqualified presentation evidence")
    if not attributes.get("server.instance_id") or not attributes.get("server.backend_source"):
        raise ValueError("missing authoritative identity")
    encoded = json.dumps(attributes)
    if any(secret in encoded for secret in forbidden):
        raise ValueError("private sentinel exported")
    for key, value in attributes.items():
        if key.startswith("browser.") and key.endswith("_ms") and key != "browser.breadcrumb_ready_minus_meaningful_ms" and (not isinstance(value, (float, int)) or isinstance(value, bool) or not 0 <= value <= 300_000):
            raise ValueError("invalid exported timing")
    difference = attributes.get("browser.breadcrumb_ready_minus_meaningful_ms")
    if difference is not None:
        marks = [attributes.get(key) for key in ("browser.meaningful_transcript_ms", "browser.breadcrumb_presented_ms", "browser.breadcrumb_enabled_ms")]
        if any(value is None for value in marks) or difference != max(marks[1:]) - marks[0] or attributes.get("browser.timeline_saturated"):
            raise ValueError("invalid signed readiness difference")


def make_server(capture):
    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            super().setup()
            self.connection.settimeout(3)
        def log_message(self, *_):
            pass
        def do_POST(self):
            self.connection.settimeout(3)
            try:
                length = self.headers.get("Content-Length", "")
                if self.path != "/v1/traces" or self.headers.get_content_type() != "application/x-protobuf" or not length.isascii() or not length.isdecimal() or not 0 < int(length) <= MAX_BODY:
                    raise ValueError("invalid export transport")
                data = self.rfile.read(int(length))
                if len(data) != int(length):
                    raise ValueError("incomplete export")
                capture.accept(data)
                self.send_response(200)
                self.send_header("Content-Type", "application/x-protobuf")
                self.send_header("Content-Length", "0")
                self.end_headers()
            except Exception:
                with capture.lock:
                    if not capture.errors:
                        capture.errors.append("transport_rejected")
                        capture.finish()
                self.send_error(400, "bounded synthetic export rejected")
    return HTTPServer(("127.0.0.1", 0), Handler)


def serve(output, receipt):
    capture = Capture(output)
    server = make_server(capture)
    stop = threading.Timer(300, server.shutdown)
    stop.daemon = True
    stop.start()
    Path(receipt).write_text(json.dumps({"endpoint": f"http://127.0.0.1:{server.server_port}/v1/traces", "output": str(capture.output)}))
    try:
        server.serve_forever()
    finally:
        server.server_close()
        stop.cancel()
        capture.finish()


@dataclass(frozen=True)
class TelemetrySink:
    endpoint: str
    service: str
    output: Path
    _thread: threading.Thread
    _expired: threading.Event
    _closed: threading.Event

    @property
    def expired(self):
        return self._expired.is_set()

    def is_serving(self):
        return self._thread.is_alive() and not self.expired and not self._closed.is_set()

    @property
    def env(self):
        return {
            "PHOENIX_TRACE_EXPORTER": "otlp",
            "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL": "http/protobuf",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": self.endpoint,
            "OTEL_SERVICE_NAME": self.service,
        }

    def assert_receipt(self, result, *, timeout=10):
        """Await expected semantic exports; caller stops app before final_index drain check."""
        if not 0 < timeout <= 15:
            raise ValueError("invalid export deadline")
        if isinstance(result, (str, Path)):
            result = json.loads(Path(result).read_text())
        journeys = result.get("journeys", [])
        if not 0 < len(journeys) <= 64 or len({j["open_id"] for j in journeys}) != len(journeys):
            raise ValueError("invalid expected journey inventory")
        for journey in journeys:
            mandatory = {"server.backend_source", "server.instance_id", "browser.frontend_source", "browser.outcome", "browser.route", "browser.trigger"}
            if not mandatory.issubset(journey["expected"]):
                raise ValueError("missing candidate expectation identity")
        deadline = time.monotonic() + timeout
        while True:
            if not self.is_serving():
                raise ValueError("receiver not serving through app drain")
            try:
                index = final_index(self.output)
                retrieved = []
                for journey in journeys:
                    view, children = retrieve(index, self.service, journey["open_id"])
                    validate_view(view, {**journey["expected"], "open.id": journey["open_id"]}, result.get("forbidden", []))
                    validate_correlations(index, view, children, journey["correlations"])
                    retrieved.append({"open_id": journey["open_id"], "trace_id": view["trace_id"], "correlations": len(children)})
                return {"retrieved": retrieved, "bytes": index["bytes"], "spans": index["spans"]}
            except (FileNotFoundError, ValueError):
                if time.monotonic() >= deadline:
                    raise
                time.sleep(min(0.05, max(0, deadline - time.monotonic())))


@contextmanager
def telemetry_sink(output_dir, run_id, *, lifetime_seconds=300):
    """Existing harness owns app/export drain; this context owns only loopback receiver."""
    if type(lifetime_seconds) is not int or not 1 <= lifetime_seconds <= 650:
        raise ValueError("invalid receiver lifetime")
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,80}", run_id):
        raise ValueError("invalid synthetic run identity")
    capture = Capture(Path(output_dir) / f"view-telemetry-{run_id}")
    server = make_server(capture)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    expired = threading.Event()
    closed = threading.Event()

    def expire():
        expired.set()
        try:
            with capture.lock:
                if "receiver_lifetime_expired" not in capture.errors:
                    capture.errors.append("receiver_lifetime_expired")
                capture.finish()
        finally:
            server.shutdown()

    stop = threading.Timer(lifetime_seconds, expire)
    stop.daemon = True
    stop.start()
    handle = TelemetrySink(f"http://127.0.0.1:{server.server_port}/v1/traces", f"phoenix-view-qualification-{run_id}", capture.output, thread, expired, closed)
    try:
        yield handle
    finally:
        closed.set()
        stop.cancel()
        server.shutdown()
        deadline = time.monotonic() + 4
        stop.join(timeout=max(0, deadline - time.monotonic()))
        thread.join(timeout=max(0, deadline - time.monotonic()))
        server.server_close()
        if stop.is_alive() or thread.is_alive():
            raise RuntimeError("sink teardown deadline exceeded")
        with capture.lock:
            capture.finish()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("--receipt", required=True)
    args = parser.parse_args()
    serve(args.output, args.receipt)
