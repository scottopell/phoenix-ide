import importlib.util
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load_checker():
    spec = importlib.util.spec_from_file_location(
        "rust_test_timing_under_test", ROOT / "scripts" / "check_rust_test_timing.py"
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


@unittest.skipUnless(shutil.which("ast-grep"), "ast-grep is not installed")
class RustTestTimingTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.checker = load_checker()

    def check(self, source: str):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.rs"
            path.write_text(source)
            return self.checker.findings([str(path)])

    def test_flags_real_sleep_and_unbounded_waits_in_annotated_test(self):
        diagnostics = self.check(
            """
            #[tokio::test]
            #[serial]
            async fn races() {
                tokio::time::sleep(Duration::from_millis(10)).await;
                rx.recv().await;
                done.notified().await;
            }
            """
        )
        self.assertEqual(3, len(diagnostics))

    def test_flags_helper_in_cfg_test_module_after_unicode(self):
        diagnostics = self.check(
            """
            const LABEL: &str = "✓";
            #[cfg(test)]
            mod tests {
                async fn helper() { rx.recv().await; }
            }
            """
        )
        self.assertEqual(1, len(diagnostics))

    def test_ignores_production_and_timeout_bounded_waits(self):
        diagnostics = self.check(
            """
            async fn production() {
                tokio::time::sleep(Duration::from_secs(1)).await;
                rx.recv().await;
            }
            #[tokio::test]
            async fn bounded() {
                tokio::time::timeout(
                    Duration::from_secs(1),
                    async {
                        rx.recv().await;
                        done.notified().await;
                    },
                )
                .await
                .unwrap();
            }
            """
        )
        self.assertEqual([], diagnostics)

    def test_requires_nonempty_local_exemption_reason(self):
        diagnostics = self.check(
            """
            #[tokio::test]
            async fn behavior_driver() {
                // test-timing-allow: virtual protocol fixture intentionally delays response
                tokio::time::sleep(Duration::from_millis(10)).await;
                // test-timing-allow:
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            """
        )
        self.assertEqual(1, len(diagnostics))

    def test_semantic_diff_detects_context_only_changes(self):
        finding = self.checker.Finding(
            key=("fixture.rs", "races", "event:rx.recv().await"),
            diagnostic="current",
        )
        self.assertEqual([finding], self.checker._introduced([finding], []))
        self.assertEqual([], self.checker._introduced([finding], [finding]))

    def test_semantic_diff_detects_removed_timeout_wrapper(self):
        unbounded = self.checker.Finding(
            key=("fixture.rs", "races", "event:rx.recv().await"),
            diagnostic="timeout removed",
        )
        self.assertEqual([unbounded], self.checker._introduced([unbounded], []))

    def test_semantic_diff_does_not_reflag_unchanged_legacy_finding(self):
        finding = self.checker.Finding(
            key=("fixture.rs", "races", "event:rx.recv().await"),
            diagnostic="line number may differ",
        )
        baseline = self.checker.Finding(key=finding.key, diagnostic="old location")
        self.assertEqual([], self.checker._introduced([finding], [baseline]))

    def test_imported_sleep_and_test_only_helpers_are_flagged(self):
        diagnostics = self.check(
            """
            use tokio::time::{sleep, Duration};
            #[cfg(test)]
            async fn helper() { sleep(Duration::from_millis(10)).await; }
            """
        )
        self.assertEqual(1, len(diagnostics))
        self.assertIn("sleep", diagnostics[0].diagnostic)

    def test_cfg_any_test_scope_is_flagged(self):
        diagnostics = self.check(
            """
            #[cfg(any(test, feature = "test-support"))]
            async fn helper() { done.notified().await; }
            """
        )
        self.assertEqual(1, len(diagnostics))

    def test_std_thread_imported_sleep_is_flagged(self):
        diagnostics = self.check(
            """
            use std::thread::sleep;
            #[test]
            fn helper() { sleep(Duration::from_millis(10)); }
            """
        )
        self.assertEqual(1, len(diagnostics))

    def test_imported_timeout_bounds_event_wait(self):
        diagnostics = self.check(
            """
            use tokio::time::timeout;
            #[tokio::test]
            async fn helper() {
                timeout(Duration::from_secs(1), rx.recv()).await.unwrap();
            }
            """
        )
        self.assertEqual([], diagnostics)

    def test_not_test_cfg_does_not_create_test_scope(self):
        diagnostics = self.check(
            """
            #[cfg(not(test))]
            async fn production() { done.notified().await; }
            """
        )
        self.assertEqual([], diagnostics)

    def test_unrelated_receiver_method_name_is_still_an_event_wait(self):
        diagnostics = self.check(
            """
            #[tokio::test]
            async fn helper() { sleepy.recv().await; }
            """
        )
        self.assertEqual(1, len(diagnostics))
        self.assertIn("timeout", diagnostics[0].diagnostic)

    def test_semantic_identity_keeps_identical_waits_in_distinct_functions(self):
        first = self.checker.Finding(
            key=("fixture.rs", "first", "event:rx.recv().await"),
            diagnostic="first",
        )
        second = self.checker.Finding(
            key=("fixture.rs", "second", "event:rx.recv().await"),
            diagnostic="second",
        )
        self.assertEqual([second], self.checker._introduced([first, second], [first]))

    def test_exemption_does_not_suppress_unbounded_event_wait(self):
        diagnostics = self.check(
            """
            #[tokio::test]
            async fn event_wait() {
                // test-timing-allow: event delivery is intentionally delayed
                rx.recv().await;
            }
            """
        )
        self.assertEqual(1, len(diagnostics))
        self.assertIn("tokio::time::timeout", diagnostics[0].diagnostic)


if __name__ == "__main__":
    unittest.main()
