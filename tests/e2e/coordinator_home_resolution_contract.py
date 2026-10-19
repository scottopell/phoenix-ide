"""Supplemental local-home assertions for an existing caller-owned fixture.

No server, session, database, or filesystem is opened by this module. Paths in
responses belong to the responding server: compare them as opaque strings.
These assertions do not authorize execution or establish federation acceptance.
"""

from collections.abc import Mapping
from typing import Any, Protocol
import unittest


class _Response(Protocol):
    def raise_for_status(self) -> None: ...

    def json(self) -> Any: ...


class _Client(Protocol):
    def post(
        self, url: str, *, json: Mapping[str, str], timeout: float
    ) -> _Response: ...


def assert_home_resolution(
    payload: Any, *, expected_transcript_id: str, expected_scope_id: str, expected_home: str
) -> None:
    """Assert the local resolver's home projection, not a remote envelope.

    The caller supplies the independently observed scope ID and canonical path
    from its isolated server fixture. This function never canonicalizes a path
    on the machine running the assertion. Unexpected extra payload fields are
    allowed; the listed fields must all be present with the specified values.
    """
    if not isinstance(expected_transcript_id, str) or not expected_transcript_id:
        raise ValueError("expected_transcript_id must be a nonempty exact transcript ID")
    if not isinstance(expected_scope_id, str) or not expected_scope_id:
        raise ValueError("expected_scope_id must be a nonempty string")
    if not isinstance(expected_home, str) or not expected_home:
        raise ValueError("expected_home must be a nonempty server path string")
    if not isinstance(payload, Mapping):
        raise AssertionError("resolver response must be an object")
    if payload.get("kind") != "conversation" or payload.get("id") != expected_transcript_id:
        raise AssertionError("resolver response must identify the requested exact transcript")
    scope = payload.get("work_scope")
    if not isinstance(scope, Mapping):
        raise AssertionError("resolver response must contain work_scope object")
    expected = {
        "status": "available",
        "work_scope_id": expected_scope_id,
        "lifecycle": "active",
        "environment_kind": "coordinator_home",
        "cwd": expected_home,
        "worktree_path": None,
        "effective_path": expected_home,
        "path_semantics": "server_filesystem",
    }
    for field, value in expected.items():
        if field not in scope or scope[field] != value:
            raise AssertionError(f"home resolution field {field!r} is missing or incorrect")


def check_home_resolution(
    client: _Client,
    base_url: str,
    coordinator_id: str,
    *,
    expected_scope_id: str,
    expected_home: str,
    timeout: float = 10.0,
) -> Any:
    """Use the supplied authenticated client; return its checked JSON payload.

    The caller owns transport, TLS trust, authentication, endpoint selection,
    fixture lifecycle and timeout policy. No client/session is constructed here.
    For a continued Coordinator, pass the exact selected transcript ID, not its
    ProductConversation ID or a WorkScope ID. The local home projection is not
    automatically valid as a federation envelope or local execution target.
    """
    if not isinstance(coordinator_id, str) or not coordinator_id:
        raise ValueError("coordinator_id must be a nonempty exact transcript ID")
    if any(character.isspace() or character in "/#?@:" for character in coordinator_id):
        raise ValueError("coordinator_id must be an unadorned exact transcript ID")
    response = client.post(
        f"{base_url.rstrip('/')}/api/global/resolve",
        json={"reference": f"@transcript:{coordinator_id}"},
        timeout=timeout,
    )
    response.raise_for_status()
    payload = response.json()
    assert_home_resolution(
        payload, expected_transcript_id=coordinator_id,
        expected_scope_id=expected_scope_id, expected_home=expected_home
    )
    return payload


class HomeResolutionContractTests(unittest.TestCase):
    """Pure fixture tests; no real transport or server-side state is exercised."""

    def payload(self, home: str = "/foreign/server/data/coordinator") -> dict[str, Any]:
        return {
            "kind": "conversation",
            "id": "exact-transcript",
            "work_scope": {
                "status": "available",
                "work_scope_id": "owned-home-scope",
                "lifecycle": "active",
                "environment_kind": "coordinator_home",
                "cwd": home,
                "worktree_path": None,
                "effective_path": home,
                "path_semantics": "server_filesystem",
            },
        }

    def check(self, payload: Any, home: str = "/foreign/server/data/coordinator") -> None:
        assert_home_resolution(
            payload, expected_transcript_id="exact-transcript",
            expected_scope_id="owned-home-scope", expected_home=home
        )

    def test_complete_home_projection(self) -> None:
        self.check(self.payload())

    def test_foreign_paths_are_opaque_not_resolved_on_assertion_host(self) -> None:
        for home in ("/nonexistent/foreign host/coordinator", "Z:\\remote\\coordinator"):
            with self.subTest(home=home):
                self.check(self.payload(home), home)

    def test_rejects_wrong_scope_environment_lifecycle_and_path(self) -> None:
        changes = {
            "status": "unavailable",
            "work_scope_id": "worker-scope",
            "lifecycle": "retired",
            "environment_kind": "unowned_cwd",
            "cwd": "/worker",
            "worktree_path": "/worker/worktree",
            "effective_path": "/another/server/coordinator",
            "path_semantics": "client_filesystem",
        }
        for field, value in changes.items():
            with self.subTest(field=field):
                payload = self.payload()
                payload["work_scope"][field] = value
                with self.assertRaises(AssertionError):
                    self.check(payload)

    def test_all_projection_fields_must_be_present_even_null_worktree(self) -> None:
        for field in self.payload()["work_scope"]:
            with self.subTest(field=field):
                payload = self.payload()
                del payload["work_scope"][field]
                with self.assertRaises(AssertionError):
                    self.check(payload)

    def test_rejects_missing_unavailable_and_malformed_scope(self) -> None:
        for scope in (None, [], {"status": "missing"}, {"status": "unavailable"}):
            with self.subTest(scope=scope), self.assertRaises(AssertionError):
                payload = self.payload()
                payload["work_scope"] = scope
                self.check(payload)
        for payload in ({}, [], None):
            with self.subTest(payload=payload), self.assertRaises(AssertionError):
                self.check(payload)

    def test_caller_owned_client_receives_only_exact_reference(self) -> None:
        payload = self.payload()
        calls: list[Any] = []

        class Response:
            def raise_for_status(self) -> None:
                calls.append("status_checked")

            def json(self) -> Any:
                return payload

        class Client:
            def post(self, url: str, **kwargs: Any) -> Response:
                calls.append((url, kwargs))
                return Response()

        result = check_home_resolution(
            Client(), "https://existing-fixture/", "exact-transcript",
            expected_scope_id="owned-home-scope",
            expected_home="/foreign/server/data/coordinator", timeout=3.0,
        )
        self.assertIs(result, payload)
        self.assertEqual(calls, [
            ("https://existing-fixture/api/global/resolve", {
                "json": {"reference": "@transcript:exact-transcript"}, "timeout": 3.0,
            }),
            "status_checked",
        ])

    def test_rejects_wrong_or_missing_response_identity_with_shared_home(self) -> None:
        for field, value in (("kind", "product_conversation"), ("kind", None),
                             ("id", "another-transcript"), ("id", None)):
            with self.subTest(field=field, value=value):
                payload = self.payload()
                if value is None:
                    del payload[field]
                else:
                    payload[field] = value

                class Response:
                    def raise_for_status(self) -> None:
                        pass

                    def json(self) -> Any:
                        return payload

                class Client:
                    def post(self, *_args: Any, **_kwargs: Any) -> Response:
                        return Response()

                with self.assertRaises(AssertionError):
                    check_home_resolution(
                        Client(), "https://existing-fixture", "exact-transcript",
                        expected_scope_id="owned-home-scope",
                        expected_home="/foreign/server/data/coordinator",
                    )

    def test_adorned_or_empty_transcript_id_is_rejected_before_transport(self) -> None:
        class NoTransport:
            def post(self, *args: Any, **kwargs: Any) -> Any:
                raise AssertionError("transport must not be called")

        for transcript in ("", "@transcript:id", "id#message-x", "id/child", "id space"):
            with self.subTest(transcript=transcript), self.assertRaises(ValueError):
                check_home_resolution(
                    NoTransport(), "https://existing-fixture", transcript,
                    expected_scope_id="owned-home-scope", expected_home="/server/home",
                )
