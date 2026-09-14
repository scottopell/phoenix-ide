import importlib.util
import unittest
from pathlib import Path
from unittest.mock import Mock


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


class QuestionIdentityTests(unittest.TestCase):
    def test_mutations_carry_the_observed_identity(self):
        client = phoenix_client.PhoenixClient("http://localhost:1")
        self.addCleanup(client.http.close)
        client.http = Mock()
        client.respond_to_question("c1", "original", {"Question?": "yes"})
        self.assertEqual(client.http.post.call_args.kwargs["json"],
                         {"tool_use_id": "original", "answers": {"Question?": "yes"}})
        client.dismiss_question("c1", "original")
        self.assertEqual(client.http.post.call_args.kwargs["json"], {"tool_use_id": "original"})

    def test_missing_or_consumed_identity_is_not_guessed(self):
        for state in [{"type": "idle"}, {"type": "awaiting_user_response"},
                      {"type": "awaiting_user_response", "tool_use_id": " "}]:
            with self.subTest(state=state), self.assertRaises(phoenix_client.click.UsageError):
                phoenix_client.pending_question_identity({"state": state})

    def test_identical_questions_preserve_distinct_request_identity(self):
        for identity in ["first", "second"]:
            self.assertEqual(phoenix_client.pending_question_identity({"state": {
                "type": "awaiting_user_response", "tool_use_id": identity,
                "questions": [{"question": "Same?"}]}}), identity)


if __name__ == "__main__":
    unittest.main()
