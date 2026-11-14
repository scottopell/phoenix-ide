import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("frozen_rc5_release_test", ROOT / "scripts/frozen_rc5_release.py")
release = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = release
SPEC.loader.exec_module(release)


def git(repo, *args, input=None):
    return subprocess.run(["git", "-C", str(repo), *args], input=input, check=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout.decode().strip()


class FrozenRc5GateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="frozen-rc5-test-")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        git(self.repo, "init", "-q")
        git(self.repo, "config", "user.name", "Test")
        git(self.repo, "config", "user.email", "test@example.invalid")
        (self.repo / "crates/phoenix-ide").mkdir(parents=True)
        (self.repo / "crates/phoenix-ide/Cargo.toml").write_bytes(
            b'[package]\nname = "phoenix_ide"\nversion = "0.13.2-rc.3"\n')
        (self.repo / "Cargo.lock").write_bytes(
            b'[[package]]\nname = "phoenix_ide"\nversion = "0.13.2-rc.3"\n')
        (self.repo / "stable.txt").write_text("unchanged\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "base")
        self.base = git(self.repo, "rev-parse", "HEAD")
        self.patch_base = mock.patch.object(release, "BASE", self.base)
        self.patch_base.start()
        self.addCleanup(self.patch_base.stop)

    def candidate(self, mutate=None, mode=None, parent=None):
        git(self.repo, "checkout", "-q", "--detach", parent or self.base)
        manifest = self.repo / "crates/phoenix-ide/Cargo.toml"
        lock = self.repo / "Cargo.lock"
        manifest.write_bytes(manifest.read_bytes().replace(b"0.13.2-rc.3", b"0.13.2-rc.5"))
        lock.write_bytes(lock.read_bytes().replace(b'phoenix_ide"\nversion = "0.13.2-rc.3',
                                                    b'phoenix_ide"\nversion = "0.13.2-rc.5'))
        if mutate:
            mutate(self.repo)
        if mode:
            os.chmod(manifest, mode)
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "candidate")
        candidate = git(self.repo, "rev-parse", "HEAD")
        with mock.patch.object(release, "CANDIDATE", candidate):
            return release.verify_candidate(self.repo)

    def test_accepts_exact_version_only_candidate(self):
        self.assertTrue(self.candidate())

    def test_rejects_extra_path(self):
        def add_extra(repo):
            (repo / "extra.txt").write_text("unexpected")
        with self.assertRaisesRegex(ValueError, "tracked paths"):
            self.candidate(mutate=add_extra)

    def test_rejects_deleted_path(self):
        def delete_stable(repo):
            (repo / "stable.txt").unlink()
        with self.assertRaisesRegex(ValueError, "tracked paths"):
            self.candidate(mutate=delete_stable)

    def test_rejects_version_mismatch(self):
        def wrong_version(repo):
            path = repo / "crates/phoenix-ide/Cargo.toml"
            path.write_bytes(path.read_bytes().replace(b"0.13.2-rc.5", b"0.13.2-rc.4"))
        with self.assertRaisesRegex(ValueError, "exact two-file/version-only"):
            self.candidate(mutate=wrong_version)

    def test_rejects_blob_change_outside_two_version_files(self):
        def edit(repo):
            (repo / "stable.txt").write_text("changed\n")
        with self.assertRaisesRegex(ValueError, "exact two-file/version-only"):
            self.candidate(mutate=edit)

    def test_rejects_mode_change(self):
        with self.assertRaisesRegex(ValueError, "exact two-file/version-only"):
            self.candidate(mode=0o755)

    def test_rejects_symlink_replacement(self):
        def replace(repo):
            (repo / "stable.txt").unlink()
            (repo / "stable.txt").symlink_to("Cargo.lock")
        with self.assertRaisesRegex(ValueError, "exact two-file/version-only"):
            self.candidate(mutate=replace)

    def test_rejects_empty_subtree(self):
        empty = subprocess.check_output(["git", "-C", str(self.repo), "mktree"], input=b"").decode().strip()
        listing = subprocess.check_output(["git", "-C", str(self.repo), "ls-tree", self.base])
        root = subprocess.check_output(["git", "-C", str(self.repo), "mktree"],
                                      input=listing + f"040000 tree {empty}\tempty\n".encode()).decode().strip()
        with self.assertRaisesRegex(ValueError, "empty subtree"):
            release.tree(self.repo, root)

    def test_rejects_wrong_parent(self):
        git(self.repo, "checkout", "-q", "--detach", self.base)
        (self.repo / "stable.txt").write_text("different parent\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "other parent")
        wrong_parent = git(self.repo, "rev-parse", "HEAD")
        with self.assertRaisesRegex(ValueError, "sole parent"):
            self.candidate(parent=wrong_parent)

    def test_ci_gate_requires_exact_completed_success(self):
        good = {"path": ".github/workflows/ci.yml", "head_sha": release.CANDIDATE,
                "event": "workflow_dispatch", "status": "completed", "conclusion": "success",
                "run_number": 12, "id": 77}
        with mock.patch.object(release.urllib.request, "urlopen") as open_url:
            for runs in ([], [dict(good, event="push")], [dict(good, head_sha="f" * 40)],
                         [dict(good, status="in_progress")], [dict(good, conclusion="failure")]):
                payload = __import__("io").BytesIO(__import__("json").dumps({"workflow_runs": runs}).encode())
                open_url.return_value = __import__("contextlib").nullcontext(payload)
                with self.assertRaises(ValueError):
                    release.workflow_dispatch_ci_run()
            payload = __import__("io").BytesIO(__import__("json").dumps({"workflow_runs": [good]}).encode())
            open_url.return_value = __import__("contextlib").nullcontext(payload)
            self.assertEqual(77, release.workflow_dispatch_ci_run()["id"])


if __name__ == "__main__":
    unittest.main()
