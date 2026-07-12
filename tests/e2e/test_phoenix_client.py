#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "click",
#     "httpx",
#     "httpx-sse",
# ]
# ///
"""Focused tests for phoenix-client.py command surfaces."""

import contextlib
import importlib.util
import io
import json
import sys
import unittest
from pathlib import Path
from unittest import mock

import httpx
from click.testing import CliRunner

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("phoenix_client", ROOT / "phoenix-client.py")
phoenix_client = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(phoenix_client)


class FakeWakeClient:
    snapshot = {}

    def __init__(self, base_url, password=None):
        self.base_url = base_url
        self.password = password

    def ensure_authenticated(self):
        return None

    def get_wake_status(self, conversation_id):
        return self.snapshot


class WakeStatusTests(unittest.TestCase):
    def invoke(self, snapshot, *args):
        FakeWakeClient.snapshot = snapshot
        with mock.patch.object(phoenix_client, "PhoenixClient", FakeWakeClient):
            return CliRunner().invoke(
                phoenix_client.wake_status,
                [*args, "--api-url", "http://phoenix.test"],
            )

    def test_empty_status(self):
        result = self.invoke(
            {
                "conversation_id": "conv-empty",
                "pending_count": 0,
                "soonest_expiry": None,
                "lifecycle_blocked": False,
                "contracts": [],
            },
            "conv-empty",
        )

        self.assertEqual(result.exit_code, 0, result.output)
        self.assertIn("Conversation: conv-empty", result.output)
        self.assertIn("Pending: 0 | Soonest expiry: -", result.output)
        self.assertIn("No wake contracts.", result.output)

    def test_pending_bash_and_tmux_contracts(self):
        result = self.invoke(
            {
                "conversation_id": "conv-pending",
                "pending_count": 2,
                "soonest_expiry": "2026-07-10T12:01:00Z",
                "lifecycle_blocked": True,
                "contracts": [
                    {
                        "id": "wake-bash",
                        "handle": {"kind": "bash", "id": "b-17"},
                        "registered_at": "2026-07-10T12:00:00Z",
                        "expires_at": "2026-07-10T12:01:00Z",
                        "status": "pending",
                        "cause": None,
                        "forgotten_reason": None,
                    },
                    {
                        "id": "wake-tmux",
                        "handle": {"kind": "tmux_window", "id": "@9"},
                        "registered_at": "2026-07-10T12:00:00Z",
                        "expires_at": "2026-07-10T12:02:00Z",
                        "status": "pending",
                        "cause": None,
                        "forgotten_reason": None,
                    },
                ],
            },
            "conv-pending",
        )

        self.assertEqual(result.exit_code, 0, result.output)
        self.assertIn("Pending: 2 | Soonest expiry: 2026-07-10T12:01:00Z", result.output)
        self.assertIn("- wake-bash | handle=bash:b-17", result.output)
        self.assertIn("- wake-tmux | handle=tmux_window:@9", result.output)

    def test_terminal_causes_and_forgotten_reason(self):
        result = self.invoke(
            {
                "conversation_id": "conv-terminal",
                "pending_count": 0,
                "soonest_expiry": None,
                "lifecycle_blocked": False,
                "contracts": [
                    {
                        "id": "wake-fired",
                        "handle": {"kind": "bash", "id": "b-1"},
                        "expires_at": "2026-07-10T12:01:00Z",
                        "status": "fired",
                        "cause": "fired",
                        "forgotten_reason": None,
                    },
                    {
                        "id": "wake-forgotten",
                        "handle": {"kind": "tmux_window", "id": "@2"},
                        "expires_at": "2026-07-10T12:02:00Z",
                        "status": "forgotten",
                        "cause": "forgotten",
                        "forgotten_reason": "handle_missing",
                    },
                ],
            },
            "conv-terminal",
        )

        self.assertEqual(result.exit_code, 0, result.output)
        self.assertIn(
            "status=fired | terminal_cause=fired | forgotten=-",
            result.output,
        )
        self.assertIn(
            "status=forgotten | terminal_cause=forgotten | forgotten=handle_missing",
            result.output,
        )

    def test_json_output_preserves_api_snapshot(self):
        snapshot = {
            "conversation_id": "conv-json",
            "pending_count": 0,
            "soonest_expiry": None,
            "lifecycle_blocked": False,
            "contracts": [],
        }
        result = self.invoke(snapshot, "conv-json", "--json")

        self.assertEqual(result.exit_code, 0, result.output)
        self.assertEqual(json.loads(result.output), snapshot)

    def test_conversation_id_is_url_encoded_as_one_segment(self):
        seen = []

        def respond(request):
            seen.append(request.url.raw_path)
            return httpx.Response(200, json={"contracts": []})

        client = phoenix_client.PhoenixClient("http://phoenix.test")
        client.http = httpx.Client(transport=httpx.MockTransport(respond))
        client.get_wake_status("team/alpha ?#")

        self.assertEqual(
            seen,
            [b"/api/conversations/team%2Falpha%20%3F%23/wakes"],
        )

    def test_non_2xx_error_is_actionable(self):
        request = httpx.Request(
            "GET", "http://phoenix.test/api/conversations/missing/wakes"
        )
        response = httpx.Response(404, text='{"error":"conversation missing"}', request=request)

        class ErrorClient(FakeWakeClient):
            def get_wake_status(self, conversation_id):
                raise httpx.HTTPStatusError(
                    "not found", request=request, response=response
                )

        stderr = io.StringIO()
        with (
            mock.patch.object(phoenix_client, "PhoenixClient", ErrorClient),
            mock.patch.object(
                sys,
                "argv",
                [
                    "phoenix-client.py",
                    "wake-status",
                    "missing",
                    "--api-url",
                    "http://phoenix.test",
                ],
            ),
            contextlib.redirect_stderr(stderr),
            self.assertRaises(SystemExit) as exited,
        ):
            phoenix_client.main_with_error_handling()

        self.assertEqual(exited.exception.code, 1)
        self.assertIn(
            'API error: 404 - {"error":"conversation missing"}',
            stderr.getvalue(),
        )

    def test_help_names_command_and_json_option(self):
        result = CliRunner().invoke(phoenix_client.wake_status, ["--help"])

        self.assertEqual(result.exit_code, 0, result.output)
        self.assertIn("Usage: wake-status [OPTIONS] CONVERSATION_ID", result.output)
        self.assertIn("--json-output, --json", result.output)


if __name__ == "__main__":
    unittest.main()
