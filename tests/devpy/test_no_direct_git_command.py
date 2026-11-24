"""Regression coverage for the direct Git subprocess structural lint."""

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RULE = ROOT / "ast-grep-rules" / "rust-no-direct-git-command.yml"


@unittest.skipUnless(shutil.which("ast-grep"), "ast-grep is not installed")
class NoDirectGitCommandTests(unittest.TestCase):
    def findings(self, source):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.rs"
            path.write_text(source)
            result = subprocess.run(
                ["ast-grep", "scan", "--inline-rules", RULE.read_text(),
                 "--json=compact", str(path)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertIn(result.returncode, (0, 1), result.stderr)
            diagnostics = json.loads(result.stdout)
            self.assertTrue(all(item["ruleId"] == "no-direct-git-command"
                                for item in diagnostics))
            return [item["text"] for item in diagnostics]

    def test_flags_bare_and_qualified_subprocess_constructors(self):
        for constructor in ("Command", "std::process::Command", "tokio::process::Command"):
            with self.subTest(constructor=constructor):
                expression = f'{constructor}::new("git")'
                self.assertEqual(
                    [expression],
                    self.findings(f"fn fixture() {{ let command = {expression}; }}"),
                )

    def test_os_string_comparison_does_not_hide_direct_constructor(self):
        self.assertEqual(
            ['Command::new("git")'],
            self.findings('''fn fixture() {
                let command = Command::new("git");
                if command.get_program() != std::ffi::OsStr::new("git") {}
            }'''),
        )

    def test_allows_nonprocess_constructors_and_shared_factory(self):
        self.assertEqual([], self.findings('''fn fixture() {
            let name = OsStr::new("git");
            let name = std::ffi::OsStr::new("git");
            let path = std::path::Path::new("git");
            let other = Command::new("echo");
            let safe = phoenix_core::git::command();
            let asynchronous = tokio::process::Command::from(phoenix_core::git::command());
        }'''))

    def test_test_modules_are_not_exempt(self):
        self.assertEqual(
            ['Command::new("git")'],
            self.findings('''#[cfg(test)] mod tests {
                #[test] fn fixture() { let command = Command::new("git"); }
            }'''),
        )
