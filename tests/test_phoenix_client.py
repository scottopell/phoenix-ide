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


if __name__ == "__main__":
    unittest.main()
