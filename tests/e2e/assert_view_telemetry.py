#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["opentelemetry-proto==1.39.0"]
# ///
"""Final semantic assertions after the existing harness drains its synthetic app."""
import argparse
import json
import re
from pathlib import Path
from view_telemetry_sink import final_index, retrieve, validate_view, validate_correlations


def assertPhoenixTelemetry(output, service_name, run_id, expected_cases, forbidden=(), *, drain_receipt):
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,80}", run_id) or not service_name.endswith(run_id):
        raise ValueError("mismatched synthetic run identity")
    if isinstance(drain_receipt, (str, Path)):
        drain_receipt = json.loads(Path(drain_receipt).read_text())
    if drain_receipt != {"run_id": run_id, "service_name": service_name, "app_stopped": True, "receiver_stopped": True, "errors": []}:
        raise ValueError("missing owned app/export teardown receipt")
    if isinstance(expected_cases, (str, Path)):
        receipt = json.loads(Path(expected_cases).read_text())
        if receipt["runId"] != run_id or receipt["serviceName"] != service_name:
            raise ValueError("browser/harness identity mismatch")
        expected_cases = receipt["journeys"]
        forbidden = tuple(forbidden) + tuple(receipt.get("forbidden", []))
        if receipt.get("failure"):
            raise ValueError("browser checks failed")
    if not 0 < len(expected_cases) <= 64 or len({case["open_id"] for case in expected_cases}) != len(expected_cases):
        raise ValueError("invalid journey acceptance inventory")
    index = final_index(output)
    retrieved = []
    for case in expected_cases:
        mandatory = {"server.backend_source", "server.instance_id", "browser.frontend_source", "browser.outcome", "browser.route", "browser.trigger"}
        if not mandatory.issubset(case["expected"]):
            raise ValueError("missing authoritative candidate expectation")
        view, children = retrieve(index, service_name, case["open_id"])
        validate_view(view, {**case["expected"], "open.id": case["open_id"]}, forbidden)
        validate_correlations(index, view, children, case["correlations"])
        retrieved.append({"open_id": case["open_id"], "trace_id": view["trace_id"], "outcome": view["attributes"]["browser.outcome"]})
    service_views = [record for record in index["records"] if record["service"] == service_name and record["name"] == "browser.conversation_view"]
    if {record["attributes"].get("open.id") for record in service_views} != {case["open_id"] for case in expected_cases} or len(service_views) != len(expected_cases):
        raise ValueError("unexpected exported journey/event volume")
    return {"run_id": run_id, "service_name": service_name, "retrieved": retrieved,
            "bytes": index["bytes"], "spans": index["spans"], "batches": index["batches"]}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("--service-name", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--expected-cases", required=True)
    parser.add_argument("--receipt", required=True)
    parser.add_argument("--drain-receipt", required=True)
    args = parser.parse_args()
    result = assertPhoenixTelemetry(args.output, args.service_name, args.run_id, args.expected_cases, drain_receipt=args.drain_receipt)
    Path(args.receipt).write_text(json.dumps(result, allow_nan=False))
