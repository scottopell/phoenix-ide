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

    def test_question_response_posts_pending_tool_use_id(self):
        client = phoenix_client.PhoenixClient("http://localhost:1")
        self.addCleanup(client.http.close)
        response = Mock()
        response.raise_for_status = Mock()
        response.json.return_value = {"success": True}
        client.http.post = Mock(return_value=response)

        client.respond_to_question("conv-1", "tool-1", {"Question?": "Answer"})

        client.http.post.assert_called_once_with(
            "http://localhost:1/api/conversations/conv-1/respond",
            json={"tool_use_id": "tool-1", "answers": {"Question?": "Answer"}},
        )

    def test_question_dismissal_posts_pending_tool_use_id(self):
        client = phoenix_client.PhoenixClient("http://localhost:1")
        self.addCleanup(client.http.close)
        response = Mock()
        response.raise_for_status = Mock()
        response.json.return_value = {"success": True}
        client.http.post = Mock(return_value=response)

        client.dismiss_question("conv-1", "tool-1")

        client.http.post.assert_called_once_with(
            "http://localhost:1/api/conversations/conv-1/dismiss-question",
            json={"tool_use_id": "tool-1"},
        )

    def test_pending_question_tool_use_id_requires_awaiting_state_identity(self):
        self.assertEqual(
            phoenix_client._pending_question_tool_use_id(
                {"type": "awaiting_user_response", "tool_use_id": "tool-1"}
            ),
            "tool-1",
        )
        with self.assertRaisesRegex(phoenix_client.click.UsageError, "missing tool_use_id"):
            phoenix_client._pending_question_tool_use_id(
                {"type": "awaiting_user_response", "questions": []}
            )
        with self.assertRaisesRegex(phoenix_client.click.UsageError, "not awaiting"):
            phoenix_client._pending_question_tool_use_id({"type": "idle"})


if __name__ == "__main__":
    unittest.main()
