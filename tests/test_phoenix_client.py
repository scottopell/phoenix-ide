import importlib.util
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from click.testing import CliRunner


SPEC = importlib.util.spec_from_file_location(
    "phoenix_client", Path(__file__).parents[1] / "phoenix-client.py"
)
phoenix_client = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(phoenix_client)


class PhoenixClientStateTests(unittest.TestCase):
    def test_poll_raises_nested_recoverable_continuation_failure_message(self):
        client = phoenix_client.PhoenixClient("http://localhost:1")
        client.get_messages = Mock(
            return_value={
                "conversation": {
                    "state": {
                        "type": "recoverable_continuation_failure",
                        "failure": {"message": "summary request failed"},
                    }
                },
                "messages": [],
            }
        )
        self.addCleanup(client.http.close)

        with self.assertRaisesRegex(phoenix_client.PhoenixError, "summary request failed"):
            client.poll_until_complete("conversation", timeout=1, interval=0)

    def test_stream_init_raises_recoverable_continuation_failure(self):
        client = phoenix_client.PhoenixClient("http://localhost:1")
        self.addCleanup(client.http.close)
        source = Mock()
        source.iter_sse.return_value = [
            Mock(
                event="init",
                data='{"conversation":{"state":{"type":"recoverable_continuation_failure","failure":{"message":"failed before reconnect"}}},"messages":[]}',
            )
        ]
        context = Mock()
        context.__enter__ = Mock(return_value=source)
        context.__exit__ = Mock(return_value=False)

        with unittest.mock.patch.object(phoenix_client, "connect_sse", return_value=context):
            with self.assertRaisesRegex(phoenix_client.PhoenixError, "failed before reconnect"):
                client.stream_until_complete("conversation", timeout=1)

    def test_state_helpers_parse_tagged_state_payloads(self):
        state = {
            "type": "recoverable_continuation_failure",
            "failure": {"message": "provider unavailable"},
        }
        self.assertEqual(
            phoenix_client._state_kind(state),
            "recoverable_continuation_failure",
        )
        self.assertEqual(
            phoenix_client._state_error_message(state, "fallback"),
            "provider unavailable",
        )


class PhoenixClientQuestionIdentityTests(unittest.TestCase):
    def setUp(self):
        self.client = phoenix_client.PhoenixClient("http://localhost:1")
        self.addCleanup(self.client.http.close)
        self.client.http.post = Mock()
        self.request_id = "9177feef-09df-4a32-9130-4f282e1a7bd1"
        self.answers = {"Which option?": "First"}

    def test_answer_sends_request_identity(self):
        self.client.respond_to_question("conversation", self.answers, self.request_id)
        self.client.http.post.assert_called_once_with(
            "http://localhost:1/api/conversations/conversation/respond",
            json={"answers": self.answers, "request_id": self.request_id},
        )

    def test_dismiss_sends_request_identity(self):
        self.client.dismiss_question("conversation", self.request_id)
        self.client.http.post.assert_called_once_with(
            "http://localhost:1/api/conversations/conversation/dismiss-question",
            json={"request_id": self.request_id},
        )

    def test_legacy_answer_omits_request_identity(self):
        self.client.respond_to_question("conversation", self.answers)
        self.client.http.post.assert_called_once_with(
            "http://localhost:1/api/conversations/conversation/respond",
            json={"answers": self.answers},
        )

    def test_legacy_dismiss_preserves_bodyless_post(self):
        self.client.dismiss_question("conversation")
        self.client.http.post.assert_called_once_with(
            "http://localhost:1/api/conversations/conversation/dismiss-question"
        )

    def test_malformed_identity_is_rejected_before_post(self):
        for request_id in ("", " \t\n", False, 1, [], {}):
            for action in ("answer", "dismiss"):
                with self.subTest(request_id=request_id, action=action):
                    with self.assertRaisesRegex(
                        phoenix_client.click.UsageError, "request_id.*non-empty string"
                    ):
                        if action == "answer":
                            self.client.respond_to_question(
                                "conversation", self.answers, request_id
                            )
                        else:
                            self.client.dismiss_question("conversation", request_id)
                    self.client.http.post.assert_not_called()

    def invoke_interaction(self, state, args):
        self.client.ensure_authenticated = Mock()
        self.client.get_conversation = Mock(
            return_value={"id": "conversation", "state": state}
        )
        with patch.object(phoenix_client, "PhoenixClient", return_value=self.client):
            return CliRunner().invoke(
                phoenix_client.main,
                ["--api-url", "http://localhost:1", "-c", "conversation", *args],
            )

    def test_cli_threads_pending_identity_for_answers_and_dismissal(self):
        state = {
            "type": "awaiting_user_response",
            "request_id": self.request_id,
            "questions": [{"question": "Which option?"}],
        }
        for args, endpoint, payload in (
            (["--respond", "Which option?=First"], "respond", {"answers": self.answers}),
            (
                ["--respond-json", '{"Which option?": "First"}'],
                "respond",
                {"answers": self.answers},
            ),
            (["--dismiss-question"], "dismiss-question", {}),
        ):
            with self.subTest(args=args):
                self.client.http.post.reset_mock()
                result = self.invoke_interaction(state, args)
                self.assertEqual(result.exit_code, 0, result.output)
                self.client.http.post.assert_called_once_with(
                    f"http://localhost:1/api/conversations/conversation/{endpoint}",
                    json={**payload, "request_id": self.request_id},
                )

    def test_cli_preserves_legacy_absence(self):
        for identity in ({}, {"request_id": None}):
            state = {
                "type": "awaiting_user_response",
                "questions": [{"question": "Which option?"}],
                **identity,
            }
            for args, endpoint, kwargs in (
                (
                    ["--respond", "Which option?=First"],
                    "respond",
                    {"json": {"answers": self.answers}},
                ),
                (["--dismiss-question"], "dismiss-question", {}),
            ):
                with self.subTest(identity=identity, args=args):
                    self.client.http.post.reset_mock()
                    result = self.invoke_interaction(state, args)
                    self.assertEqual(result.exit_code, 0, result.output)
                    self.client.http.post.assert_called_once_with(
                        f"http://localhost:1/api/conversations/conversation/{endpoint}",
                        **kwargs,
                    )

    def test_cli_rejects_malformed_pending_identity(self):
        for request_id in ("", " \t\n", False, 1, [], {}):
            state = {
                "type": "awaiting_user_response",
                "request_id": request_id,
                "questions": [{"question": "Which option?"}],
            }
            for args in (
                ["--respond", "Which option?=First"],
                ["--dismiss-question"],
            ):
                with self.subTest(request_id=request_id, args=args):
                    result = self.invoke_interaction(state, args)
                    self.assertEqual(result.exit_code, 2, result.output)
                    self.assertIn("request_id must be a non-empty string", result.output)
                    self.client.http.post.assert_not_called()


if __name__ == "__main__":
    unittest.main()
