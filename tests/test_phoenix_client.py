import importlib.util
import io
import json
import unittest
from copy import deepcopy
from pathlib import Path
from unittest.mock import Mock, patch

from click.testing import CliRunner


SPEC = importlib.util.spec_from_file_location(
    "phoenix_client", Path(__file__).parents[1] / "phoenix-client.py"
)
phoenix_client = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(phoenix_client)


class PhoenixClientRemovedWakeTests(unittest.TestCase):
    def test_client_has_no_wake_methods(self):
        self.assertFalse(hasattr(phoenix_client.PhoenixClient, "get_wake_status"))
        self.assertFalse(hasattr(phoenix_client.PhoenixClient, "cancel_wake"))

    def test_help_does_not_advertise_removed_wake_options(self):
        result = CliRunner().invoke(phoenix_client.main, ["--help"])
        self.assertEqual(result.exit_code, 0, result.output)
        self.assertNotIn("--wake-status", result.output)
        self.assertNotIn("--wake-cancel", result.output)

    def test_removed_wake_options_are_rejected_before_client_creation(self):
        for args in (["--wake-status"], ["--wake-cancel", "contract"]):
            with self.subTest(args=args):
                with patch.object(phoenix_client, "PhoenixClient") as client:
                    result = CliRunner().invoke(
                        phoenix_client.main, ["-c", "conversation", *args]
                    )
                    self.assertEqual(result.exit_code, 2, result.output)
                    self.assertIn("No such option", result.output)
                    self.assertIn(args[0], result.output)
                    client.assert_not_called()


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


class PhoenixClientFederationTests(unittest.TestCase):
    PEER = '11111111-1111-4111-8111-111111111111'
    OWNER = '22222222-2222-4222-8222-222222222222'
    MESSAGE = '33333333-3333-4333-8333-333333333333'
    TARGET = {'kind': 'transcript', 'transcript_id': 'opaque/transcript'}
    SECRET = 'phx_peer_do-not-print-private-credential'

    def setUp(self):
        self.client = phoenix_client.PhoenixClient('https://owner.example', password='owner-password')
        self.client.http.close()
        self.requests = []
        self.responses = {}
        self.client.http = phoenix_client.httpx.Client(
            transport=phoenix_client.httpx.MockTransport(self.handle),
            headers=self.client._auth_headers(),
        )
        self.addCleanup(self.client.http.close)
        self.summary = {'peer_instance_id': self.PEER, 'peer_display_name': 'laptop',
                        'base_url': 'https://peer.example', 'created_at': '2026-10-19T00:00:00Z'}
        self.row = {'reference': {'instance_id': self.PEER, 'product_conversation_id': 'product'},
                    'subscription_id': self.MESSAGE, 'state': 'active', 'admitted_cursor': 0,
                    'enrolled': True, 'pending_unsubscribe': False}
        self.responses['/api/federation/peers'] = [self.summary]
        self.responses['/api/federation/subscriptions'] = [self.row]

    def handle(self, request):
        self.requests.append(request)
        self.assertEqual(request.url.host, 'owner.example')
        self.assertEqual(request.headers['authorization'], 'Bearer owner-password')
        self.assertNotIn('/api/federation/peer/', request.url.path)
        value = self.responses[request.url.path]
        if isinstance(value, Exception):
            raise value
        if isinstance(value, phoenix_client.httpx.Response):
            return value
        if isinstance(value, tuple):
            status, value = value
        else:
            status = 200
        return phoenix_client.httpx.Response(status, json=deepcopy(value))

    def ref(self, target=None, instance=None):
        return {'instance_id': instance or self.PEER, 'target': deepcopy(target or self.TARGET)}

    def body(self):
        return json.loads(self.requests[-1].content)

    def assert_route(self, method, suffix, body=None):
        request = self.requests[-1]
        self.assertEqual(request.method, method)
        self.assertEqual(request.url.path, suffix)
        if body is not None:
            self.assertEqual(self.body(), body)

    def query_result(self):
        cells = [{'type': 'null'}, {'type': 'integer', 'value': 2}, {'type': 'text', 'value': '雪'},
                 {'type': 'blob', 'value': {'bytes': 10}},
                 {'type': 'real', 'value': {'kind': 'finite', 'value': 1.5}},
                 {'type': 'real', 'value': {'kind': 'positive_infinity'}},
                 {'type': 'real', 'value': {'kind': 'negative_infinity'}},
                 {'type': 'real', 'value': {'kind': 'nan'}}]
        return {'destination_instance_id': self.PEER, 'caller_instance_id': self.OWNER,
                'result': {'columns': [str(i) for i in range(len(cells))], 'rows': [cells],
                           'truncated': False, 'row_limit': 200, 'byte_limit': 10000, 'elapsed_ms': 1}}

    def read_result(self):
        return {'requested': self.ref(), 'resolved': self.ref(), 'stable_conversation': None,
                'evidence': [self.ref()], 'content': 'text', 'next_cursor': 4,
                'source_navigation': {'kind': 'peer', 'url': 'https://peer.example/t/exact'},
                'stable_navigation': None}

    def resolution(self):
        return {'reference': self.ref(), 'href': 'https://peer.example/t/exact', 'title': None,
                'work_scope': {'status': 'available', 'work_scope_id': 'scope', 'lifecycle': 'active',
                               'environment_kind': 'allocated_worktree', 'cwd': '~/server-only',
                               'worktree_path': 'Z:\\remote\\work', 'effective_path': '~/server-only',
                               'path_semantics': 'server_filesystem'},
                'path_owner_instance_id': self.PEER, 'related_references': [], 'summary': 'resolved'}

    def send_result(self, outcome='accepted'):
        return {'destination_instance_id': self.PEER, 'caller_instance_id': self.OWNER,
                'target': deepcopy(self.TARGET), 'message_id': self.MESSAGE,
                'receipt': {'outcome': outcome, 'acceptance': {
                    'accepting_transcript': self.TARGET['transcript_id'],
                    'stable_destination': deepcopy(self.row['reference']),
                    'stable_navigation': {'kind': 'peer', 'url': 'https://peer.example/c/stable'},
                    'accepting_transcript_navigation': {'kind': 'peer', 'url': 'https://peer.example/t/exact'},
                    'accepting_message_navigation': {'kind': 'peer', 'url': 'https://peer.example/t/exact#msg'},
                    'disposition': 'queued'}}}

    def openapi(self, instance=None):
        return {'openapi': '3.1.0', 'x-phoenix-instance-id': instance or self.PEER,
                'info': {'title': 'Federation API', 'description': 'Not the complete API', 'version': '1'},
                'paths': {'/openapi.json': {}}, 'components': {'schemas': {}, 'securitySchemes': {}}}

    def test_peer_summaries_and_alias_selection_use_owner_only(self):
        self.assertEqual(self.client.list_peer_connections(), [self.summary])
        self.assert_route('GET', '/api/federation/peers')
        self.responses['/api/federation/search-conversations'] = {'destination_instance_id': self.PEER, 'hits': []}
        result = self.client.remote_search_messages('query', 'laptop')
        self.assertEqual(result['hits'], [])
        self.assert_route('POST', '/api/federation/search-conversations', {'instance': self.PEER, 'query': 'query'})
        self.responses['/api/federation/peers'] = [self.summary, {**self.summary, 'peer_instance_id': self.OWNER}]
        with self.assertRaisesRegex(phoenix_client.PhoenixError, 'ambiguous'):
            self.client.remote_search_messages('query', 'laptop')
        self.assert_route('GET', '/api/federation/peers')

    def test_notes_preserve_whole_text_and_only_allow_notes(self):
        path = f'/api/federation/peers/{self.PEER}/notes'
        for notes in ('  line\r\n雪\0  ', '', '雪' * 10922 + 'ab'):
            self.responses[path] = {'notes': notes}
            self.assertEqual(self.client.set_peer_notes(notes, self.PEER), {'notes': notes})
            self.assert_route('PUT', path, {'notes': notes})
            self.assertEqual(self.client.get_peer_notes(self.PEER), {'notes': notes})
            self.assert_route('GET', path)
        with self.assertRaises(phoenix_client.PhoenixError):
            self.client.set_peer_notes('雪' * 10923, self.PEER)
        self.responses[path] = {'notes': 'x', 'bearer_credential': self.SECRET}
        with self.assertRaises(phoenix_client.PhoenixError) as error:
            self.client.get_peer_notes(self.PEER)
        self.assertNotIn(self.SECRET, str(error.exception))

    def test_local_search_and_send_keep_distinct_routes_and_payloads(self):
        self.responses['/api/conversations/search'] = {'hits': [{'slug': 'local', 'snippet': 'text', 'score': 1}]}
        self.assertEqual(self.client.search_conversations('local', 3)[0]['slug'], 'local')
        self.assert_route('GET', '/api/conversations/search')
        self.assertEqual(dict(self.requests[-1].url.params), {'q': 'local', 'limit': '3'})
        self.responses['/api/conversations/local/chat'] = {}
        self.client.send_message('local', 'hello', [])
        self.assert_route('POST', '/api/conversations/local/chat')
        self.assertEqual(self.body()['text'], 'hello')
        self.assertEqual(self.body()['images'], [])
        self.assertEqual(set(self.body()), {'text', 'images', 'message_id'})

    def test_typed_query_cells_are_preserved(self):
        value = self.query_result()
        self.responses['/api/federation/query-database'] = value
        self.assertEqual(self.client.remote_query_database('SELECT * FROM conversations', self.PEER), value)
        self.assert_route('POST', '/api/federation/query-database',
                          {'instance': self.PEER, 'sql': 'SELECT * FROM conversations'})
        for cell in ({'type': 'unknown'}, {'type': 'integer', 'value': True},
                     {'type': 'real', 'value': {'kind': 'finite', 'value': float('inf')}},
                     {'type': 'null', 'value': None}):
            with self.subTest(cell=cell):
                bad = self.query_result()
                bad['result']['rows'][0][0] = cell
                self.responses['/api/federation/query-database'] = bad
                with self.assertRaises(phoenix_client.PhoenixError):
                    self.client.remote_query_database('SELECT 1', self.PEER)

    def test_qualified_search_read_and_server_paths(self):
        hit = {'source': self.ref({'kind': 'message', 'transcript_id': 't', 'message_id': 'm'}),
               'source_navigation': None, 'stable_conversation': None, 'stable_navigation': None,
               'role': 'user', 'sender': None, 'created_at': '2026-10-19T00:00:00Z', 'snippet': 'hit'}
        self.responses['/api/federation/search-conversations'] = {'destination_instance_id': self.PEER, 'hits': [hit]}
        self.assertEqual(self.client.remote_search_messages('q', self.PEER)['hits'], [hit])
        self.responses['/api/federation/read-conversation'] = self.read_result()
        self.assertEqual(self.client.remote_read_conversation(self.TARGET, 2, self.PEER), self.read_result())
        self.assert_route('POST', '/api/federation/read-conversation', {'instance': self.PEER, 'target': self.TARGET, 'cursor': 2})
        self.responses['/api/federation/resolve-reference'] = self.resolution()
        with patch.object(Path, 'exists', side_effect=AssertionError('must not inspect remote paths')), \
             patch.object(Path, 'expanduser', side_effect=AssertionError('must not expand remote paths')):
            self.assertEqual(self.client.remote_resolve_reference('@transcript:opaque/transcript', self.PEER), self.resolution())
        self.assert_route('POST', '/api/federation/resolve-reference',
                          {'instance': self.PEER, 'reference': '@transcript:opaque/transcript'})

    def test_send_preserves_uuid_and_receipt_variants(self):
        for outcome in ('accepted', 'duplicate'):
            value = self.send_result(outcome)
            self.responses['/api/federation/send-conversation-message'] = value
            supplied = self.MESSAGE.upper()
            self.assertEqual(self.client.remote_send_conversation_message(self.TARGET, 'hello', supplied, self.PEER), value)
            self.assert_route('POST', '/api/federation/send-conversation-message',
                              {'instance': self.PEER, 'target': self.TARGET, 'message': 'hello', 'message_id': supplied})
        rejected = self.send_result()
        rejected['receipt'] = {'outcome': 'rejected', 'code': 'busy', 'message': self.SECRET}
        self.responses['/api/federation/send-conversation-message'] = rejected
        result = self.client.remote_send_conversation_message(self.TARGET, 'hello', self.MESSAGE, self.PEER)
        self.assertEqual(result['receipt']['message'], 'destination rejected message')
        self.assertNotIn(self.SECRET, json.dumps(result))

    def test_uncertain_send_never_retries_or_generates_uuid(self):
        self.responses['/api/federation/send-conversation-message'] = phoenix_client.httpx.ReadTimeout(self.SECRET)
        with patch.object(phoenix_client, 'uuid4', side_effect=AssertionError('must not generate send UUID')):
            for _ in range(2):
                with self.assertRaisesRegex(phoenix_client.PhoenixError, 'same message_id') as error:
                    self.client.remote_send_conversation_message(self.TARGET, 'hello', self.MESSAGE, self.PEER)
                self.assertNotIn(self.SECRET, str(error.exception))
        self.assertEqual(len(self.requests), 2)
        self.assertEqual(self.requests[0].content, self.requests[1].content)
        self.assertEqual(self.body()['message_id'], self.MESSAGE)

    def test_watch_reuses_saved_identity_and_subscriptions_use_qualified_bodies(self):
        watch = {**self.row, 'snapshot': {'transcript_id': 't', 'state': {'kind': 'nonterminal'}}}
        self.responses['/api/federation/subscriptions/create'] = watch
        self.assertEqual(self.client.remote_watch_conversation('product', self.PEER), watch)
        body = {'instance': self.PEER, 'product_conversation_id': 'product', 'subscription_id': self.MESSAGE}
        self.assert_route('POST', '/api/federation/subscriptions/create', body)
        self.responses['/api/federation/subscriptions/sync-once'] = {'synced_once': True}
        self.assertEqual(self.client.sync_remote_subscription(self.MESSAGE), {'synced_once': True})
        self.assert_route('POST', '/api/federation/subscriptions/sync-once', body)
        receipt = {'reference': self.row['reference'], 'subscription_id': self.MESSAGE,
                   'local_stopped': True, 'remote_confirmed': False}
        self.responses['/api/federation/subscriptions/unsubscribe'] = receipt
        self.assertEqual(self.client.remote_unwatch_subscription(self.MESSAGE), receipt)
        self.assert_route('POST', '/api/federation/subscriptions/unsubscribe', body)

    def test_subscription_missing_or_ambiguous_id_fails_before_mutation(self):
        for rows in ([], [self.row, {**self.row, 'reference': {**self.row['reference'], 'instance_id': self.OWNER}}]):
            self.responses['/api/federation/subscriptions'] = rows
            for operation in (self.client.sync_remote_subscription, self.client.remote_unwatch_subscription):
                with self.assertRaises(phoenix_client.PhoenixError):
                    operation(self.MESSAGE)
                self.assert_route('GET', '/api/federation/subscriptions')
        self.responses['/api/federation/subscriptions'] = [{**self.row, 'pending_unsubscribe': True}]
        with self.assertRaises(phoenix_client.PhoenixError):
            self.client.remote_watch_conversation('product', self.PEER)
        self.assert_route('GET', '/api/federation/subscriptions')

    def test_openapi_local_and_selected_peer_never_direct_peer_fetch(self):
        local = self.openapi(self.OWNER)
        remote = {'destination_instance_id': self.PEER, 'document': self.openapi()}
        self.responses['/openapi.json'] = local
        self.responses['/api/federation/openapi'] = remote
        self.assertEqual(self.client.get_openapi(), local)
        self.assert_route('GET', '/openapi.json')
        self.assertEqual(self.client.get_openapi(self.PEER), remote)
        self.assert_route('POST', '/api/federation/openapi', {'instance': self.PEER})
        for bad in ({**remote, 'destination_instance_id': self.OWNER},
                    {**remote, 'document': self.openapi(self.OWNER)},
                    {**remote, 'document': {**self.openapi(), 'openapi': '2.0'}}):
            self.responses['/api/federation/openapi'] = bad
            with self.assertRaises(phoenix_client.PhoenixError):
                self.client.get_openapi(self.PEER)

    def test_malformed_discriminants_identities_and_unknown_secret_fields_fail_closed(self):
        cases = [
            ('query-database', self.query_result(), lambda: self.client.remote_query_database('SELECT 1', self.PEER)),
            ('search-conversations', {'destination_instance_id': self.PEER, 'hits': []}, lambda: self.client.remote_search_messages('q', self.PEER)),
            ('read-conversation', self.read_result(), lambda: self.client.remote_read_conversation(self.TARGET, 0, self.PEER)),
            ('resolve-reference', self.resolution(), lambda: self.client.remote_resolve_reference('@transcript:opaque/transcript', self.PEER)),
            ('send-conversation-message', self.send_result(), lambda: self.client.remote_send_conversation_message(self.TARGET, 'hello', self.MESSAGE, self.PEER)),
        ]
        for path, value, operation in cases:
            for bad in (None, [], {}, {**value, 'credential': self.SECRET}):
                with self.subTest(path=path, bad=bad):
                    self.responses['/api/federation/' + path] = bad
                    with self.assertRaises(phoenix_client.PhoenixError) as error:
                        operation()
                    self.assertNotIn(self.SECRET, str(error.exception))
            bad = deepcopy(value)
            if 'destination_instance_id' in bad:
                bad['destination_instance_id'] = self.OWNER
            elif 'requested' in bad:
                bad['resolved']['instance_id'] = self.OWNER
            else:
                bad['work_scope']['status'] = 'unknown'
            self.responses['/api/federation/' + path] = bad
            with self.assertRaises(phoenix_client.PhoenixError):
                operation()

    def test_http_failures_do_not_expose_body_url_or_fall_back(self):
        for status in (401, 404, 409, 422, 502):
            self.responses['/api/federation/query-database'] = (status, {'error': self.SECRET})
            with self.assertRaises(phoenix_client.PhoenixError) as error:
                self.client.remote_query_database('SELECT 1', self.PEER)
            self.assertNotIn(self.SECRET, str(error.exception))
            self.assert_route('POST', '/api/federation/query-database')
        self.assertEqual(len(self.requests), 5)

    def test_nested_identity_and_discriminant_mismatches_are_rejected(self):
        cases = [
            ('read-conversation', self.read_result(), ('resolved', 'target', 'transcript_id'), 'wrong',
             lambda: self.client.remote_read_conversation(self.TARGET, 0, self.PEER)),
            ('read-conversation', self.read_result(), ('source_navigation', 'kind'), 'unknown',
             lambda: self.client.remote_read_conversation(self.TARGET, 0, self.PEER)),
            ('resolve-reference', self.resolution(), ('path_owner_instance_id',), self.OWNER,
             lambda: self.client.remote_resolve_reference('@transcript:opaque/transcript', self.PEER)),
            ('resolve-reference', self.resolution(), ('reference', 'target', 'transcript_id'), 'wrong',
             lambda: self.client.remote_resolve_reference('@transcript:opaque/transcript', self.PEER)),
            ('send-conversation-message', self.send_result(), ('message_id',), self.OWNER,
             lambda: self.client.remote_send_conversation_message(self.TARGET, 'hello', self.MESSAGE, self.PEER)),
            ('send-conversation-message', self.send_result(), ('receipt', 'outcome'), 'unknown',
             lambda: self.client.remote_send_conversation_message(self.TARGET, 'hello', self.MESSAGE, self.PEER)),
            ('send-conversation-message', self.send_result(), ('receipt', 'acceptance', 'stable_destination', 'instance_id'), self.OWNER,
             lambda: self.client.remote_send_conversation_message(self.TARGET, 'hello', self.MESSAGE, self.PEER)),
        ]
        for path, value, keys, bad, operation in cases:
            with self.subTest(path=path, keys=keys):
                current = value
                for key in keys[:-1]:
                    current = current[key]
                current[keys[-1]] = bad
                self.responses['/api/federation/' + path] = value
                with self.assertRaises(phoenix_client.PhoenixError):
                    operation()
        self.responses['/api/federation/subscriptions'] = [{**self.row, 'state': 'unknown'}]
        with self.assertRaises(phoenix_client.PhoenixError):
            self.client.list_remote_subscriptions()
        self.responses['/api/federation/peers'] = [{**self.summary, 'base_url': f'https://user:{self.SECRET}@peer.example'}]
        with self.assertRaises(phoenix_client.PhoenixError) as error:
            self.client.list_peer_connections()
        self.assertNotIn(self.SECRET, str(error.exception))

    def test_related_sender_references_keep_their_own_instance_identity(self):
        value = self.resolution()
        value['related_references'] = [self.ref(instance=self.OWNER)]
        self.responses['/api/federation/resolve-reference'] = value
        self.assertEqual(self.client.remote_resolve_reference('@transcript:opaque/transcript', self.PEER), value)

    def test_new_watch_has_one_stable_create_uuid(self):
        self.responses['/api/federation/subscriptions'] = []
        watch = {**self.row, 'snapshot': {'transcript_id': 't', 'state': {'kind': 'terminal', 'outcome': 'completed'}}}
        self.responses['/api/federation/subscriptions/create'] = watch
        with patch.object(phoenix_client, 'uuid4', return_value=phoenix_client.UUID(self.MESSAGE)) as generate:
            self.assertEqual(self.client.remote_watch_conversation('product', self.PEER), watch)
            generate.assert_called_once_with()
        self.assertEqual(self.body()['subscription_id'], self.MESSAGE)
        self.assertEqual(sum(r.method == 'POST' for r in self.requests), 1)

    def test_malformed_json_and_cli_rendered_errors_never_expose_secrets(self):
        self.responses['/api/federation/query-database'] = phoenix_client.httpx.Response(200, content=self.SECRET)
        with self.assertRaises(phoenix_client.PhoenixError) as error:
            self.client.remote_query_database('SELECT 1', self.PEER)
        self.assertNotIn(self.SECRET, str(error.exception))
        for value in ((502, {'error': self.SECRET}), phoenix_client.httpx.ConnectError(self.SECRET)):
            self.responses['/api/federation/query-database'] = value
            self.client.ensure_authenticated = Mock()
            with patch.object(phoenix_client, 'PhoenixClient', return_value=self.client), \
                 patch.object(phoenix_client.sys, 'argv', ['phoenix-client.py', '--api-url', 'https://owner.example',
                                                        '--query-database', 'SELECT 1', '--instance', self.PEER]), \
                 patch.object(phoenix_client.sys, 'stderr', new_callable=io.StringIO) as stderr, \
                 patch.object(phoenix_client.sys, 'stdout', new_callable=io.StringIO) as stdout, \
                 self.assertRaises(SystemExit) as exit_error:
                phoenix_client.main_with_error_handling()
            self.assertEqual(exit_error.exception.code, 1)
            self.assertIn('no local fallback', stderr.getvalue())
            self.assertNotIn(self.SECRET, stderr.getvalue() + stdout.getvalue())
            self.assertEqual(stdout.getvalue(), '')

    def test_remote_methods_require_instance_before_any_request(self):
        operations = [lambda i: self.client.remote_query_database('SELECT 1', i),
                      lambda i: self.client.remote_search_messages('q', i),
                      lambda i: self.client.remote_read_conversation(self.TARGET, 0, i),
                      lambda i: self.client.remote_resolve_reference('r', i),
                      lambda i: self.client.remote_send_conversation_message(self.TARGET, 'hello', self.MESSAGE, i),
                      lambda i: self.client.remote_watch_conversation('product', i),
                      self.client.get_peer_notes, lambda i: self.client.set_peer_notes('', i)]
        for operation in operations:
            for instance in ('', ' ', None):
                with self.assertRaises(phoenix_client.PhoenixError):
                    operation(instance)
        self.assertEqual(self.requests, [])


class PhoenixClientFederationCliTests(unittest.TestCase):
    def test_conflicts_and_unsupported_instance_fail_before_client_creation(self):
        cases = [
            ['--instance', 'peer'], ['--instance', 'peer', '--search-conversations', 'local'],
            ['--instance', 'peer', '--list-models'], ['--instance', 'peer', '--version'],
            ['--instance', 'peer', 'hello'], ['--query-database', 'SELECT 1'],
            ['--query-database', 'SELECT 1', '--instance', ' '],
            ['--instance', 'peer', '--search-messages', 'q', '--list-conversations'],
            ['--instance', 'peer', '--get-peer-notes', '--set-peer-notes', 'x'],
            ['--list-peer-connections', '--instance', 'peer'],
            ['--sync-remote-subscription', 'id', '--instance', 'peer'],
            ['--list-remote-subscriptions', '--cursor', '1'],
            ['--openapi', '--instance', ''], ['--openapi', '--message-id', 'id'],
            ['--instance', 'peer', '--read-conversation', '{"kind":"unknown"}'],
            ['--instance', 'peer', '--send-conversation-message', '{"kind":"coordinator"}', 'hello'],
            ['--instance', 'peer', '--send-conversation-message', '{"kind":"coordinator"}', '--message-id', 'bad', 'hello'],
        ]
        for args in cases:
            with self.subTest(args=args), patch.object(phoenix_client, 'PhoenixClient') as client:
                result = CliRunner().invoke(phoenix_client.main, args, env={'PHOENIX_CONVERSATION': ''})
                self.assertEqual(result.exit_code, 2, result.output)
                client.assert_not_called()

    def test_cli_dispatches_typed_json_and_env_password_without_local_send(self):
        peer = PhoenixClientFederationTests.PEER
        identity = PhoenixClientFederationTests.MESSAGE
        target = {'kind': 'coordinator'}
        with patch.object(phoenix_client, 'PhoenixClient') as factory:
            client = factory.return_value
            client.remote_send_conversation_message.return_value = {'receipt': {'outcome': 'duplicate'}}
            result = CliRunner().invoke(phoenix_client.main, [
                '--api-url', 'https://owner.example', '--instance', peer,
                '--send-conversation-message', json.dumps(target), '--message-id', identity,
            ], input='hello', env={'PHOENIX_PASSWORD': 'hidden-owner-password', 'PHOENIX_CONVERSATION': ''})
            self.assertEqual(result.exit_code, 0, result.output)
            self.assertEqual(result.output, '{"receipt": {"outcome": "duplicate"}}\n')
            self.assertEqual(json.loads(result.output), {'receipt': {'outcome': 'duplicate'}})
            factory.assert_called_once_with('https://owner.example', password='hidden-owner-password')
            client.remote_send_conversation_message.assert_called_once_with(target, 'hello', identity, peer)
            client.send_message.assert_not_called()
            client.get_conversation.assert_not_called()
            self.assertNotIn('hidden-owner-password', result.output)

    def test_cli_actions_have_individual_methods_and_json_output(self):
        peer = PhoenixClientFederationTests.PEER
        identity = PhoenixClientFederationTests.MESSAGE
        cases = [
            (['--list-peer-connections'], 'list_peer_connections', ()),
            (['--get-peer-notes', '--instance', peer], 'get_peer_notes', (peer,)),
            (['--set-peer-notes', '', '--instance', peer], 'set_peer_notes', ('', peer)),
            (['--query-database', 'SELECT 1', '--instance', peer], 'remote_query_database', ('SELECT 1', peer)),
            (['--search-messages', 'q', '--instance', peer], 'remote_search_messages', ('q', peer)),
            (['--read-conversation', '{"kind":"transcript","transcript_id":"t"}', '--cursor', '4', '--instance', peer],
             'remote_read_conversation', ({'kind': 'transcript', 'transcript_id': 't'}, 4, peer)),
            (['--resolve-reference', '@conv:p', '--instance', peer], 'remote_resolve_reference', ('@conv:p', peer)),
            (['--watch-conversation', 'p', '--instance', peer], 'remote_watch_conversation', ('p', peer)),
            (['--list-remote-subscriptions'], 'list_remote_subscriptions', ()),
            (['--sync-remote-subscription', identity], 'sync_remote_subscription', (identity,)),
            (['--unwatch-remote-subscription', identity], 'remote_unwatch_subscription', (identity,)),
            (['--openapi'], 'get_openapi', (None,)),
            (['--openapi', '--instance', peer], 'get_openapi', (peer,)),
        ]
        for args, method, expected in cases:
            with self.subTest(method=method), patch.object(phoenix_client, 'PhoenixClient') as factory:
                getattr(factory.return_value, method).return_value = {'typed': True}
                result = CliRunner().invoke(phoenix_client.main, ['--api-url', 'https://owner.example', *args],
                                            env={'PHOENIX_CONVERSATION': ''})
                self.assertEqual(result.exit_code, 0, result.output)
                self.assertEqual(json.loads(result.output), {'typed': True})
                getattr(factory.return_value, method).assert_called_once_with(*expected)

    def test_ambient_local_conversation_default_cannot_redirect_peer_action(self):
        with patch.object(phoenix_client, 'PhoenixClient') as factory:
            factory.return_value.remote_search_messages.return_value = {'hits': []}
            result = CliRunner().invoke(phoenix_client.main, [
                '--api-url', 'https://owner.example', '--search-messages', 'q', '--instance', 'peer',
            ], env={'PHOENIX_CONVERSATION': 'ambient-local-id'})
            self.assertEqual(result.exit_code, 0, result.output)
            factory.return_value.remote_search_messages.assert_called_once_with('q', 'peer')
            factory.return_value.get_conversation.assert_not_called()
        with patch.object(phoenix_client, 'PhoenixClient') as factory:
            result = CliRunner().invoke(phoenix_client.main, [
                '--api-url', 'https://owner.example', '--search-messages', 'q', '--instance', 'peer',
                '--conversation', 'explicit-local-id',
            ])
            self.assertEqual(result.exit_code, 2, result.output)
            factory.assert_not_called()

    def test_no_password_or_peer_token_cli_options(self):
        help_result = CliRunner().invoke(phoenix_client.main, ['--help'])
        self.assertNotIn('--password', help_result.output)
        self.assertNotIn('--peer-token', help_result.output)
        self.assertNotIn('--private-ca', help_result.output)


if __name__ == "__main__":
    unittest.main()
