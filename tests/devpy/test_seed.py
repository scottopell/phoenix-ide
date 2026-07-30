import importlib.util
import sqlite3
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load_devpy():
    spec = importlib.util.spec_from_file_location("devpy_seed_under_test", ROOT / "dev.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ModernSeedTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dev = load_devpy()
        binary = ROOT / "target" / "release" / "phoenix_ide"
        if not binary.exists():
            raise unittest.SkipTest("release phoenix_ide binary is required")

    def test_fresh_seed_and_fixture_repair_use_migrated_schema(self):
        with tempfile.TemporaryDirectory(prefix="phoenix-modern-seed-") as directory:
            db_path = Path(directory) / "seed.db"
            with mock.patch.object(self.dev, "get_db_path", return_value=db_path):
                self.dev.cmd_seed(build=False)
                with sqlite3.connect(db_path) as conn:
                    columns = {
                        row[1] for row in conn.execute("PRAGMA table_info(conversations)")
                    }
                    self.assertNotIn("cwd", columns)
                    self.assertNotIn("conv_mode", columns)
                    self.assertEqual(
                        conn.execute(
                            "SELECT COUNT(*) FROM conversations WHERE work_scope_id IS NULL"
                        ).fetchone()[0],
                        0,
                    )
                    fixture = conn.execute(
                        "SELECT id FROM conversations WHERE slug = 'fixture-diff-review'"
                    ).fetchone()
                    self.assertIsNotNone(fixture)
                    conn.execute(
                        "DELETE FROM messages WHERE conversation_id = ?", fixture
                    )
                    conn.commit()

                self.dev.cmd_seed(build=False)

                with sqlite3.connect(db_path) as conn:
                    self.assertEqual(
                        conn.execute(
                            "SELECT COUNT(*) FROM messages m"
                            " JOIN conversations c ON c.id = m.conversation_id"
                            " WHERE c.slug = 'fixture-diff-review'"
                        ).fetchone()[0],
                        1,
                    )
                    self.assertEqual(
                        conn.execute(
                            "SELECT s.environment_kind FROM conversations c"
                            " JOIN work_scopes s ON s.id = c.work_scope_id"
                            " WHERE c.slug = 'fixture-diff-review'"
                        ).fetchone()[0],
                        "allocated_worktree",
                    )


if __name__ == "__main__":
    unittest.main()
