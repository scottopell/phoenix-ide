import importlib.util
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load_checker():
    spec = importlib.util.spec_from_file_location(
        "rust_test_timing_under_test", ROOT / "scripts" / "check_rust_test_timing.py"
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


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
                tokio::time::timeout(Duration::from_secs(1), rx.recv()).await.unwrap();
                tokio::time::timeout(Duration::from_secs(1), done.notified()).await.unwrap();
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

    def test_changed_line_filter_blocks_only_new_debt(self):
        source = """
            #[tokio::test]
            async fn two_smells() {
                tokio::time::sleep(Duration::from_millis(10)).await;
                rx.recv().await;
            }
        """
        with tempfile.TemporaryDirectory(dir=ROOT) as directory:
            path = Path(directory) / "fixture.rs"
            path.write_text(source)
            relative = str(path.relative_to(ROOT))
            diagnostics = self.checker.findings(
                [str(path)], changed_lines={relative: {5}}
            )
        self.assertEqual(1, len(diagnostics))
        self.assertIn("recv", diagnostics[0])


if __name__ == "__main__":
    unittest.main()
