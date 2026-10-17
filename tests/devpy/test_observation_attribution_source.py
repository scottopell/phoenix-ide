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
    DB: ["ReadStage::name", "ReadCompletion::new", "ReadCompletion::complete", "ReadCompletion::emit", "ReadCompletion::drop", "complete_read", "get_ordinary_product_conversation", "read_ordinary_product_conversation_snapshot", "get_product_conversation_on"],
    AUTH: ["observation_request_span", "auth_middleware"],
    ROUTES: ["resolve_reference", "require_member"],
    "crates/phoenix-ide/src/api/product_conversations.rs": ["read_product_conversation"],
    STARTUP: ["Stage::name", "StageCompletion::new", "StageCompletion::complete", "StageCompletion::emit", "StageCompletion::drop", "complete_stage", "RequestCompletion::new", "RequestCompletion::response_ready", "RequestCompletion::emit", "RequestCompletion::drop", "attribution_subscriber", "AttributionRequest::poll", "AttributionRequest::drop", "with_attribution_dispatch", "run", "run_with_attribution", "serve"],
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
    if path == DB:
        start = source.index("#[derive(Clone, Copy)]\nenum ReadStage")
        end = source.index("#[derive(Debug, Clone, PartialEq, Eq)]", start)
        source = source[:start] + source[end:]
        source = source[:source.index("#[cfg(test)]\nmod completion_tests {")]
    items = tokens(source)

    def remove_exact(fragment, count, replacement=""):
        nonlocal items
        sequence = tokens(fragment)
        result, index, found = [], 0, 0
        while index < len(items):
            if items[index:index + len(sequence)] == sequence:
                found += 1
                result.extend(tokens(replacement))
                index += len(sequence)
            else:
                result.append(items[index])
                index += 1
        if found != count:
            raise AssertionError(f"{fragment}: expected {count} exact wrappers, got {found}")
        items = result

    if path in WRAPPERS:
        for name, count in WRAPPERS[path].items():
            remove_exact(f'.instrument(tracing::info_span!("{name}"))', count)
    if path == ROUTES:
        remove_exact('#[tracing::instrument(name = "observation.require_member", skip_all)]', 1)
        remove_exact("use tracing::Instrument;", 1)
        remove_exact("use crate::observation_startup::{complete_stage, Stage};", 1)
        stages = {name: 1 for name in ["Admission", "ResolveOrdinary", "GetAggregate", "RequireMember", "MemberResolveOrdinary", "MemberGetAggregate"]}
        callee, enum = "complete_stage", "Stage"
    elif path == DB:
        remove_exact("Self::read_continuation_opening_on(&mut tx, &aggregate.latest_transcript_row_id,)", 1,
                     "Self::read_continuation_opening_on(&mut tx, &aggregate.latest_transcript_row_id)")
        stages = {name: 2 for name in ["Product", "Topology", "Member", "Handoff", "Source"]}
        stages.update({name: 1 for name in ["PoolWait", "BeginRead", "Resolve", "Aggregate", "Close", "MessagePage", "SnapshotRead", "RollbackRead"]})
        callee, enum = "complete_read", "ReadStage"
    else:
        stages = {"Detail": 1, "Projection": 1}
        callee, enum = "crate::observation_startup::complete_stage", "crate::observation_startup::Stage"
    prefix = tokens(callee + "(" + enum + "::")
    found = dict.fromkeys(stages, 0)

    def unwrap(sequence):
        result, index = [], 0
        while index < len(sequence):
            if sequence[index:index + len(prefix)] != prefix:
                result.append(sequence[index])
                index += 1
                continue
            stage_index = index + len(prefix)
            stage = sequence[stage_index]
            if stage not in stages or sequence[stage_index + 1] != ",":
                raise AssertionError("unreviewed stage wrapper")
            found[stage] += 1
            inner_start = stage_index + 2
            depth, end = 1, inner_start
            while depth:
                depth += (sequence[end] == "(") - (sequence[end] == ")")
                end += 1
            inner = sequence[inner_start:end - 1]
            if inner[-1:] == [","]:
                inner = inner[:-1]
            if stage in ["Admission", "RequireMember"]:
                if inner[:2] != ["async", "{"] or inner[-1:] != ["}"] or sequence[end:end+2] != [".", "await"]:
                    raise AssertionError("expected exact whole-method await wrapper")
                inner, end = inner[2:-1], end + 2
            result.extend(unwrap(inner))
            index = end
        return result

    items = unwrap(items)
    if found != stages:
        raise AssertionError(f"stage inventory mismatch: {found} != {stages}")
    return items


def without_auth_branch_instrumentation(source):
    expected = '''ApplicationState::Observation(application) => {
            use crate::observation_startup::{RequestCompletion, Stage, StageCompletion};
            use tracing::Instrument;
            let (request_id, span) = observation_request_span(&req);
            return async {
                let completion = RequestCompletion::new(request_id);
                let admitted = tracing::info_span!("observation.authorization").in_scope(|| {
                    let completion = StageCompletion::new(Stage::Authorization);
                    let result = application.admit_peer(peer_from_extensions(&req), req.headers());
                    completion.complete(&result);
                    result
                });
                let response = match admitted {
                    Ok(()) => next.run(req).await,
                    Err(error) => error.into_response(),
                };
                tracing::Span::current().record("status", response.status().as_u16());
                tracing::Span::current().record("response_ready", true);
                completion.response_ready(response.status());
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
        for path in [DB, ROUTES, "crates/phoenix-ide/src/api/product_conversations.rs"]:
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
            without_wrappers(source.replace('.instrument(tracing::info_span!("observation.db.read_member"))',
                                            '.instrument(tracing::info_span!("unreviewed"))', 1), DB)
        with self.assertRaises(AssertionError):
            without_wrappers(source.replace("ReadStage::Member,", "ReadStage::Unreviewed,", 1), DB)

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
        synchronous_admission = "let result = application.admit_peer(peer_from_extensions(&req), req.headers());"
        self.assertIn(synchronous_admission, observation_branch)
        for changed in [observation_branch.replace(synchronous_admission, "let result = async { application.admit_peer(peer_from_extensions(&req), req.headers()) }.await;", 1),
                        observation_branch.replace("peer_from_extensions(&req)", "None", 1),
                        observation_branch.replace("req.headers()", "&Default::default()", 1),
                        observation_branch.replace("Err(error) => error.into_response()", "Err(error) => next.run(req).await", 1),
                        observation_branch.replace("completion.response_ready(response.status());", "", 1)]:
            with self.assertRaises(AssertionError):
                without_auth_branch_instrumentation(changed)
        actual = actual[:new_start] + baseline[old_start:old_end] + actual[new_end:]
        self.assertEqual(actual, baseline)

    def test_finite_events_have_owner_dispatch_monotonic_elapsed_and_no_payload_fields(self):
        startup = (ROOT / STARTUP).read_text()
        db = (ROOT / DB).read_text()
        self.assertIn("FmtSpan::CLOSE", startup)  # diagnostic only
        self.assertIn('metadata.target() == "phoenix::observation::completion"', startup)
        self.assertIn('"observation.stage_complete" | "observation.request_complete"', startup)
        for source in [startup, db]:
            self.assertIn("std::time::Instant::now()", source)
            self.assertIn("self.started.elapsed().as_micros()", source)
            self.assertIn("tracing::dispatcher::get_default(Clone::clone)", source)
            self.assertIn("tracing::dispatcher::with_default(&self.dispatch", source)
            self.assertIn("parent: &self.parent", source)
            self.assertIn('self.emit("cancel")', source)
            self.assertIn("completion.complete(&result);", source)
            self.assertIn("self.finished = true;", source)
            for forbidden in ["set_global_default", "task_local!", "on_event(", "Subscriber for", "std::time::SystemTime"]:
                self.assertNotIn(forbidden, source)
        self.assertIn('self.emit("cancel", None)', startup)
        self.assertIn('request_id = self.request_id', startup)
        self.assertIn('response_ready = status.is_some()', startup)
        self.assertIn("with_default(&self.dispatch, || drop(self.future.take()))", startup)
        self.assertIn("this.future.as_mut().unwrap().as_mut().poll(cx)", startup)
        self.assertIn(".with_subscriber(dispatch)", startup)
        stage_event = '''tracing::event!(name: "observation.stage_complete", target: "phoenix::observation::completion",
                    parent: &self.parent, tracing::Level::INFO,
                    stage = self.stage.name(), outcome, elapsed_us);'''
        request_event = '''tracing::event!(name: "observation.request_complete", target: "phoenix::observation::completion",
                    parent: &self.parent, tracing::Level::INFO,
                    stage = "observation.request", request_id = self.request_id,
                    outcome, elapsed_us, status, response_ready = status.is_some());'''
        for source, expected in [(startup, [stage_event, request_event]), (db.split("#[cfg(test)]", 1)[0], [stage_event])]:
            actual = re.findall(r"tracing::event!\(.*?;", source, re.S)
            self.assertEqual([tokens(event) for event in actual], [tokens(event) for event in expected])
        for forbidden in [".init()", "try_init", "set_global_default", "EnvFilter", "init_logging", ".enter()"]:
            self.assertNotIn(forbidden, startup)
        self.assertEqual(git("diff", BASE, "--", "crates/phoenix-ide/src/main.rs", "crates/phoenix-ide/src/logging.rs"), "")

    def test_authored_router_and_cancellation_cases_assert_events_not_root_close(self):
        source = (ROOT / "crates/phoenix-ide/src/observation_startup/attribution_tests.rs").read_text()
        for expected in ["actual_router_completes_success_error_and_denial_before_response_handoff", "crate::api::create_router(application.clone())", "pending_drop_emits_one_cancel_per_started_stage_under_owner_not_ambient", "abort_is_observed_at_await_drop_not_a_sqlite_worker_completion", "tracing::Dispatch::none(), existing.dispatch()", "std::task::Waker::noop()", "drop(future);", "task.abort();", "task.await.unwrap_err().is_cancelled()", "fn assert_send<T: Send>", "assert_send(&future)", '"elapsed_us"', '"response_ready"', '"request_id"', '"cancel"', '"product_conversation.snapshot_read"', '"product_conversation.aggregate"', '"product_conversation.post_snapshot_projection"', "assert_stage(&events", "terminal(&events)", "completions(&capture.records()).len(),", "diagnostic CLOSE is not a terminal event", "actual_router_admission_error_completes_only_started_stages", "request_terminal_is_available_with_root_span_still_retained"]:
            self.assertIn(expected, source)
        for forbidden in ["sleep(", "yield_now(", "TcpListener", "open_read_only", "standalone_path", "set_global_default", 'record["span"]["name"] == "observation.request"']:
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
