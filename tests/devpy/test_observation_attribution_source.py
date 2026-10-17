"""Source-only attribution proof: no SQLite, executables, services or network."""
import hashlib
import json
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BASE = "2702d8389c2fc78ac2e528c876c35a1a7040d692"
PACKET = "docs/conversation-observation-attribution.json"
DB = "crates/phoenix-db/src/product_conversation_read.rs"
ROUTES = "crates/phoenix-ide/src/api/observation.rs"
AUTH = "crates/phoenix-ide/src/api/auth.rs"
STARTUP = "crates/phoenix-ide/src/observation_startup.rs"
FUNCTIONS = {
    DB: ["get_ordinary_product_conversation", "get_product_conversation_on"],
    AUTH: ["observation_request_span", "auth_middleware"],
    ROUTES: ["resolve_reference", "require_member"],
    STARTUP: ["attribution_subscriber", "with_attribution_dispatch", "run", "run_with_attribution", "serve"],
    "crates/phoenix-ide/src/observation_startup/attribution_tests.rs": [],
    "docs/conversation-observation-attribution.md": [],
    "tests/devpy/test_observation_attribution_source.py": [],
    "ui/tests/observationSourceParity.test.ts": [],
}
WRAPPERS = {
    DB: {f"observation.db.{name}": 2 for name in
         ["product", "topology", "read_member", "handoff", "source"]},
    ROUTES: {f"observation.{name}": 1 for name in
             ["resolve_ordinary", "get_aggregate", "member.resolve_ordinary", "member.get_aggregate"]},
}
TOKEN = re.compile(r'//[^\n]*|/\*.*?\*/|r(?P<hashes>#{0,255})".*?"(?P=hashes)|"(?:\\.|[^"\\])*"|[A-Za-z_]\w*|\d+|[^\s]', re.S)


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True)


def pinned(path):
    return git("show", f"{BASE}:{path}")


def tokens(source):
    # String tokens retain their exact whitespace, including every SQL literal.
    return [match.group() for match in TOKEN.finditer(source)]


def without_wrappers(source, path):
    items = tokens(source)

    def remove_exact(fragment, count):
        nonlocal items
        sequence = tokens(fragment)
        result, index, found = [], 0, 0
        while index < len(items):
            if items[index:index + len(sequence)] == sequence:
                found += 1
                index += len(sequence)
            else:
                result.append(items[index])
                index += 1
        if found != count:
            raise AssertionError(f"{fragment}: expected {count} exact wrappers, got {found}")
        items = result

    for name, count in WRAPPERS[path].items():
        remove_exact(f'.instrument(tracing::info_span!("{name}"))', count)
    if path == ROUTES:
        remove_exact('#[tracing::instrument(name = "observation.require_member", skip_all)]', 1)
        remove_exact("use tracing::Instrument;", 1)
    return items


def without_auth_branch_instrumentation(source):
    expected = '''ApplicationState::Observation(application) => {
        use tracing::Instrument;
        let span = observation_request_span(&req);
        return async {
            let admitted = tracing::info_span!("observation.authorization")
                .in_scope(|| application.admit_peer(peer_from_extensions(&req), req.headers()));
            let response = match admitted {
                Ok(()) => next.run(req).await,
                Err(error) => error.into_response(),
            };
            tracing::Span::current().record("status", response.status().as_u16());
            tracing::Span::current().record("response_ready", true);
            response
        }
        .instrument(span)
        .await;
    }'''
    if tokens(source) != tokens(expected):
        raise AssertionError("auth branch differs from exact synchronous admission/root future wrapper")
    return tokens('''ApplicationState::Observation(application) => {
        if let Err(error) = application.admit_peer(peer_from_extensions(&req), req.headers()) {
            return error.into_response();
        }
        return next.run(req).await;
    }''')


def exact_diff(path, blob):
    with tempfile.TemporaryDirectory(prefix="observation-attribution-source-") as directory:
        before, after = Path(directory) / "product", Path(directory) / "actual"
        before.write_bytes(subprocess.check_output(["git", "cat-file", "blob", blob], cwd=ROOT) if blob else b"")
        after.write_bytes((ROOT / path).read_bytes())
        diff = subprocess.run([
            "git", "diff", "--no-index", "--no-color", "--no-ext-diff", "--full-index",
            "--src-prefix=a/", "--dst-prefix=b/", "--", str(before), str(after),
        ], cwd=ROOT, text=True, capture_output=True, check=False)
        if diff.returncode != 1:
            raise AssertionError(f"expected an exact exception diff: {path}: {diff.stderr}")
        return diff.stdout.replace(str(before)[1:], path).replace(str(after)[1:], path)


class AttributionSourceTests(unittest.TestCase):
    def test_bounded_exact_diffs_and_changed_function_inventory(self):
        packet = json.loads((ROOT / PACKET).read_text())
        self.assertEqual(packet["base"], BASE)
        self.assertEqual([entry["path"] for entry in packet["files"]], sorted(FUNCTIONS))
        for entry in packet["files"]:
            path = entry["path"]
            with self.subTest(path=path):
                self.assertEqual(entry["changed_functions"], FUNCTIONS[path])
                tree = git("ls-tree", BASE, "--", path).strip()
                self.assertEqual(entry["base_blob"], tree.split()[2] if tree else None)
                body = (ROOT / path).read_bytes()
                blob = hashlib.sha1(f"blob {len(body)}\0".encode() + body).hexdigest()
                self.assertEqual(entry["actual_blob"], blob)
                self.assertEqual(entry["diff"], exact_diff(path, entry["base_blob"]))
        changed = set(git("diff", "--name-only", BASE).splitlines())
        changed.update(git("ls-files", "--others", "--exclude-standard").splitlines())
        self.assertEqual(changed, set(FUNCTIONS) | {PACKET})

    def test_only_exact_instrument_wrappers_change_db_and_admission_tokens(self):
        for path in [DB, ROUTES]:
            with self.subTest(path=path):
                self.assertEqual(tokens(pinned(path)), without_wrappers((ROOT / path).read_text(), path))

    def test_proof_does_not_erase_sql_whitespace_or_helper_changes(self):
        literal = 'r#"SELECT .instrument(tracing::info_span!("observation.db.product"))"#'
        self.assertEqual(tokens(literal), [literal])
        source = (ROOT / DB).read_text()
        for changed in [source.replace("SELECT kind, ordinary_lifecycle", "SELECT  kind, ordinary_lifecycle", 1),
                        source.replace(".get_conversation(&id)", ".get_conversation_by_slug(&id)", 1)]:
            self.assertNotEqual(tokens(pinned(DB)), without_wrappers(changed, DB))
        with self.assertRaises(AssertionError):
            without_wrappers(source.replace('"observation.db.read_member"', '"unreviewed"', 1), DB)

    def test_live_auth_and_response_path_outside_observation_branch_are_exact(self):
        baseline, actual = pinned(AUTH), (ROOT / AUTH).read_text()
        helper_start = actual.index("pub(crate) fn observation_request_span(")
        helper_end = actual.index("/// Axum middleware", helper_start)
        actual = actual[:helper_start] + actual[helper_end:]
        branch = "        ApplicationState::Observation(application) => {"
        end = "    };\n    // No password configured"
        old_start, new_start = baseline.index(branch), actual.index(branch)
        old_end, new_end = baseline.index(end, old_start), actual.index(end, new_start)
        observation_branch = actual[new_start:new_end]
        self.assertEqual(tokens(baseline[old_start:old_end]),
                         without_auth_branch_instrumentation(observation_branch))
        synchronous_admission = '''tracing::info_span!("observation.authorization")
                    .in_scope(|| application.admit_peer(peer_from_extensions(&req), req.headers()))'''
        borrowed_async_admission = '''async { application.admit_peer(peer_from_extensions(&req), req.headers()) }
                    .instrument(tracing::info_span!("observation.authorization")).await'''
        self.assertIn(synchronous_admission, observation_branch)
        for changed in [observation_branch.replace(synchronous_admission, borrowed_async_admission, 1),
                        observation_branch.replace("peer_from_extensions(&req)", "None", 1),
                        observation_branch.replace("req.headers()", "&Default::default()", 1),
                        observation_branch.replace("Err(error) => error.into_response()",
                                                   "Err(error) => next.run(req).await", 1)]:
            with self.assertRaises(AssertionError):
                without_auth_branch_instrumentation(changed)
        actual = actual[:new_start] + baseline[old_start:old_end] + actual[new_end:]
        self.assertEqual(actual, baseline)

    def test_private_subscriber_uses_scoped_dispatch_not_global_or_live_logging(self):
        startup = (ROOT / STARTUP).read_text()
        self.assertIn("FmtSpan::CLOSE", startup)
        self.assertIn("metadata.is_span()", startup)
        self.assertIn(".with_subscriber(dispatch)", startup)
        helper = '''fn with_attribution_dispatch(app: axum::Router, dispatch: tracing::Dispatch) -> axum::Router {
    app.layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            next.run(request).with_subscriber(dispatch.clone())
        },
    ))
}'''
        self.assertIn(helper, startup)
        self.assertIn('''let app = with_attribution_dispatch(
        create_router(ApplicationState::Observation(application))
            .layer(CompressionLayer::new().gzip(true).br(true)),
        dispatch,
    );''', startup)
        inventory = startup.split('metadata.name(),', 1)[1].split(')', 1)[0]
        self.assertEqual(re.findall(r'"([^"]+)"', inventory), [
            "observation.request", "observation.authorization", "replay.admission",
            "observation.resolve_ordinary", "observation.get_aggregate", "observation.require_member",
            "observation.member.resolve_ordinary", "observation.member.get_aggregate",
            "observation.db.product", "observation.db.topology", "observation.db.read_member",
            "observation.db.handoff", "observation.db.source", "product_conversation.detail_get",
            "product_conversation.post_snapshot_projection", "product_conversation.pool_wait",
            "product_conversation.begin_read", "product_conversation.resolve", "product_conversation.aggregate",
            "product_conversation.close", "product_conversation.message_page",
            "product_conversation.snapshot_read", "product_conversation.rollback_read",
        ])
        for forbidden in [".init()", "try_init", "set_global_default", "EnvFilter", "init_logging", ".enter()"]:
            self.assertNotIn(forbidden, startup)
        self.assertEqual(git("diff", BASE, "--", "crates/phoenix-ide/src/main.rs", "crates/phoenix-ide/src/logging.rs"), "")

    def test_authored_actual_router_case_uses_startup_helper_and_another_dispatch(self):
        source = (ROOT / "crates/phoenix-ide/src/observation_startup/attribution_tests.rs").read_text()
        self.assertIn("async fn startup_dispatch_captures_actual_product_detail_without_inheriting_default()", source)
        self.assertIn('''for (index, outer) in [tracing::Dispatch::none(), existing.dispatch()]
        .into_iter()
        .enumerate()''', source)
        self.assertIn("let emit_sample = index == 0;", source)
        self.assertNotIn("outer.is_none()", source)
        self.assertIn("fn assert_send<T: Send>(_: &T) {}", source)
        self.assertIn('''|state: State<ApplicationState>, req: Request<Body>, next: Next| async move {
                let future = auth_middleware(state, req, next);
                assert_send(&future);
                future.await
            }''', source)
        self.assertIn("let app = with_attribution_dispatch(", source)
        self.assertIn("crate::api::create_router(", source)
        self.assertIn("crate::api::application_state::ApplicationState::Observation(application.clone())", source)
        self.assertIn('"/api/product-conversations/{}"', source)
        self.assertIn("ObservationApplication::for_target(db, &target.id)", source)
        self.assertIn("Database::open_in_memory()", source)
        self.assertEqual(re.findall(r"\.with_subscriber\(([^)]*)\)", source), ["outer"])
        for forbidden in ["TcpListener", "open_read_only", "standalone_path", "set_global_default"]:
            self.assertNotIn(forbidden, source)

    def test_all_ui_bytes_except_exactly_manifested_guard_are_pinned(self):
        tree = git("ls-tree", "-r", BASE, "--", "ui").splitlines()
        expected = set()
        for line in tree:
            metadata, path = line.split("\t")
            expected.add(path)
            if path == "ui/tests/observationSourceParity.test.ts":
                continue
            body = (ROOT / path).read_bytes()
            self.assertEqual(hashlib.sha1(f"blob {len(body)}\0".encode() + body).hexdigest(), metadata.split()[2], path)
        self.assertEqual(set(git("ls-files", "--cached", "--others", "--exclude-standard", "--", "ui").splitlines()), expected)


if __name__ == "__main__":
    unittest.main()
