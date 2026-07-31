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
        cls.dev.build_rust(release=True)
        if not binary.exists():
            raise AssertionError(f"release phoenix_ide binary was not built: {binary}")

    def test_fresh_seed_and_fixture_repair_use_migrated_schema(self):
        with tempfile.TemporaryDirectory(prefix="phoenix-modern-seed-") as directory:
            db_path = Path(directory) / "seed.db"
            seed_worktree_root = Path(directory) / "seed-worktrees"
            with (
                mock.patch.object(self.dev, "get_db_path", return_value=db_path),
                mock.patch.object(self.dev, "get_pid", return_value=None),
                mock.patch.object(
                    self.dev,
                    "SEED_WORKTREE_ROOT",
                    seed_worktree_root,
                ),
            ):
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
                    direct_scope_groups = conn.execute(
                        "SELECT COUNT(*), COUNT(DISTINCT work_scope_id)"
                        " FROM conversations"
                        " WHERE slug GLOB 'refactor-the-database-connection-pool-current*'"
                    ).fetchone()
                    self.assertEqual(direct_scope_groups[0], direct_scope_groups[1])
                    fixture = conn.execute(
                        "SELECT id FROM conversations WHERE slug = 'fixture-diff-review'"
                    ).fetchone()
                    self.assertIsNotNone(fixture)
                    fixture_id = fixture[0]
                    fixture_scope = conn.execute(
                        "SELECT work_scope_id FROM conversations WHERE id = ?",
                        (fixture_id,),
                    ).fetchone()[0]
                    user_content = conn.execute(
                        "SELECT content FROM messages"
                        " WHERE conversation_id = ? AND message_type = 'user'",
                        (fixture_id,),
                    ).fetchone()[0]
                    self.assertNotIn('"images"', user_content)

                    columns = [
                        row[1] for row in conn.execute("PRAGMA table_info(conversations)")
                    ]
                    source = conn.execute(
                        f"SELECT {', '.join(columns)} FROM conversations WHERE id = ?",
                        (fixture_id,),
                    ).fetchone()
                    successor = dict(zip(columns, source, strict=True))
                    successor.update({
                        "id": "seed-repair-successor",
                        "slug": "seed-repair-successor",
                        "title": "Seed Repair Successor",
                        "parent_conversation_id": None,
                        "seed_label": None,
                        "continued_in_conv_id": None,
                    })
                    conn.execute(
                        f"INSERT INTO conversations ({', '.join(columns)})"
                        f" VALUES ({', '.join('?' for _ in columns)})",
                        tuple(successor[column] for column in columns),
                    )
                    conn.execute(
                        "DELETE FROM messages WHERE conversation_id = ?", fixture
                    )
                    conn.execute(
                        "UPDATE work_scopes SET worktree_path = ? WHERE id = ?",
                        ("/tmp/stale-seed-worktree", fixture_scope),
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
                            "SELECT work_scope_id FROM conversations WHERE id = ?",
                            ("seed-repair-successor",),
                        ).fetchone()[0],
                        fixture_scope,
                    )
                    self.assertEqual(
                        conn.execute(
                            "SELECT COUNT(*) FROM work_scopes WHERE id = ?",
                            (fixture_scope,),
                        ).fetchone()[0],
                        1,
                    )
                    repaired_scope = conn.execute(
                        "SELECT work_scope_id FROM conversations"
                        " WHERE slug = 'fixture-diff-review'"
                    ).fetchone()[0]
                    self.assertNotEqual(repaired_scope, fixture_scope)
                    self.assertEqual(
                        conn.execute(
                            "SELECT worktree_path FROM work_scope_environments"
                            " WHERE work_scope_id = ?",
                            (repaired_scope,),
                        ).fetchone()[0],
                        str(seed_worktree_root / "diff-review-fixture"),
                    )
                    self.assertEqual(
                        conn.execute(
                            "SELECT COUNT(*) FROM messages"
                            " WHERE message_type = 'user'"
                            " AND json_type(content, '$.images') IS NOT NULL"
                        ).fetchone()[0],
                        0,
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
